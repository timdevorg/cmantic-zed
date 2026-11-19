//! End-to-end test driving the server through tower-lsp-server's `LspService`, with a fake
//! client that answers `workspace/applyEdit` and `window/showDocument`.

use std::fs;
use std::path::PathBuf;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tower::{Service, ServiceExt};
use tower_lsp_server::LspService;
use tower_lsp_server::jsonrpc::{Request, Response};
use tower_lsp_server::ls_types::Uri;

use cmantic_ls::server::{Backend, COMMAND_ADD_DEFINITION_IN_SOURCE, COMMAND_APPLY_ACTION};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn uri(path: &std::path::Path) -> String {
    Uri::from_file_path(path).unwrap().to_string()
}

async fn call(service: &mut LspService<Backend>, request: Request) -> Option<Value> {
    let response = service
        .ready()
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap()?;
    let (_, result) = response.into_parts();
    Some(result.expect("request succeeded"))
}

fn request(id: i64, method: &'static str, params: Value) -> Request {
    Request::build(method).id(id).params(params).finish()
}

fn notification(method: &'static str, params: Value) -> Request {
    Request::build(method).params(params).finish()
}

#[tokio::test]
async fn add_definition_in_matching_source_file() {
    let dir = std::env::temp_dir().join(format!("cmantic-ls-{}-e2e", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("include")).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    let dir = TempDir(dir.canonicalize().unwrap());
    let header_path = dir.0.join("include/foo.h");
    let source_path = dir.0.join("src/foo.cpp");
    let header = "#pragma once\n\nnamespace ns {\nclass Foo {\npublic:\n    int get(int x = 1) const;\n};\n}\n";
    fs::write(&header_path, header).unwrap();
    // The source file is not open in the editor; the server reads it from disk.
    fs::write(&source_path, "#include \"foo.h\"\n").unwrap();

    let (mut service, socket) = LspService::new(Backend::new);
    let (mut requests, mut responses) = socket.split();
    let (sent_tx, mut sent_rx) = mpsc::unbounded_channel::<(String, Value)>();
    tokio::spawn(async move {
        while let Some(message) = requests.next().await {
            let (method, id, params) = message.into_parts();
            let result = match method.as_ref() {
                "workspace/applyEdit" => json!({ "applied": true }),
                "window/showDocument" => json!({ "success": true }),
                _ => Value::Null,
            };
            sent_tx
                .send((method.to_string(), params.unwrap_or(Value::Null)))
                .unwrap();
            if let Some(id) = id {
                responses.send(Response::from_ok(id, result)).await.unwrap();
            }
        }
    });

    let init = call(
        &mut service,
        request(
            1,
            "initialize",
            json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": uri(&dir.0), "name": "project" }],
                "initializationOptions": { "cpp": { "curlyBraceFormat": { "function": "newLine" } } }
            }),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        init.pointer("/capabilities/executeCommandProvider/commands/0"),
        Some(&json!(COMMAND_ADD_DEFINITION_IN_SOURCE))
    );
    call(&mut service, notification("initialized", json!({}))).await;

    let header_uri = uri(&header_path);
    call(
        &mut service,
        notification(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": header_uri, "languageId": "cpp", "version": 1, "text": header } }),
        ),
    )
    .await;

    let actions = call(
        &mut service,
        request(
            2,
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": header_uri },
                "range": { "start": { "line": 5, "character": 9 }, "end": { "line": 5, "character": 9 } },
                "context": { "diagnostics": [] }
            }),
        ),
    )
    .await
    .unwrap();
    let titles: Vec<&str> = actions
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Add Definition in matching source file",
            "Add Definition in this file",
            "Add Definition inline"
        ]
    );

    // "This file" carries its edit directly.
    let this_file = &actions[1];
    assert_eq!(this_file["kind"], "refactor");
    let edits = &this_file["edit"]["changes"][&header_uri];
    assert_eq!(
        edits[0]["newText"],
        "\n\ninline int Foo::get(int x) const\n{\n    \n}"
    );

    // "Matching source file" is a command; executing it makes the server apply the edit.
    let source_action = &actions[0];
    assert!(source_action.get("edit").is_none());
    let command = &source_action["command"];
    assert_eq!(command["command"], COMMAND_APPLY_ACTION);
    assert_eq!(
        command["arguments"][0]["action"],
        "addDefinition.matchingSource"
    );
    call(
        &mut service,
        request(
            3,
            "workspace/executeCommand",
            json!({ "command": command["command"], "arguments": command["arguments"] }),
        ),
    )
    .await;

    let mut apply_edit = None;
    let mut show_document = None;
    while let Ok((method, params)) = sent_rx.try_recv() {
        match method.as_str() {
            "workspace/applyEdit" => apply_edit = Some(params),
            "window/showDocument" => show_document = Some(params),
            _ => {}
        }
    }
    let apply_edit = apply_edit.expect("server sent workspace/applyEdit");
    let source_uri = uri(&source_path);
    assert_eq!(
        apply_edit["edit"]["changes"][&source_uri],
        json!([{
            "range": { "start": { "line": 0, "character": 16 }, "end": { "line": 0, "character": 16 } },
            "newText": "\n\nint ns::Foo::get(int x) const\n{\n    \n}"
        }])
    );
    let show_document = show_document.expect("server sent window/showDocument");
    assert_eq!(show_document["uri"], json!(source_uri));
    assert_eq!(show_document["takeFocus"], json!(true));
    assert_eq!(
        show_document["selection"],
        json!({ "start": { "line": 4, "character": 4 }, "end": { "line": 4, "character": 4 } })
    );

    // With revealNewDefinition off, no showDocument is sent.
    call(
        &mut service,
        notification(
            "workspace/didChangeConfiguration",
            json!({ "settings": { "revealNewDefinition": false } }),
        ),
    )
    .await;
    call(
        &mut service,
        request(
            4,
            "workspace/executeCommand",
            json!({ "command": command["command"], "arguments": command["arguments"] }),
        ),
    )
    .await;
    let mut methods = Vec::new();
    while let Ok((method, _)) = sent_rx.try_recv() {
        methods.push(method);
    }
    assert!(methods.contains(&"workspace/applyEdit".to_string()));
    assert!(!methods.contains(&"window/showDocument".to_string()));

    // The older dedicated command still works.
    call(
        &mut service,
        request(
            5,
            "workspace/executeCommand",
            json!({
                "command": COMMAND_ADD_DEFINITION_IN_SOURCE,
                "arguments": [{ "uri": header_uri, "position": { "line": 5, "character": 9 } }]
            }),
        ),
    )
    .await;
    let mut methods = Vec::new();
    while let Ok((method, _)) = sent_rx.try_recv() {
        methods.push(method);
    }
    assert!(methods.contains(&"workspace/applyEdit".to_string()));
}

/// A server with a fake client that accepts every `workspace/applyEdit` and
/// `window/showDocument` and records all requests and notifications it receives.
struct Harness {
    service: LspService<Backend>,
    sent: mpsc::UnboundedReceiver<(String, Value)>,
    next_id: i64,
    dir: TempDir,
}

impl Harness {
    async fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("cmantic-ls-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let dir = TempDir(dir.canonicalize().unwrap());
        for (path, text) in files {
            let path = dir.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let (service, socket) = LspService::new(Backend::new);
        let (mut requests, mut responses) = socket.split();
        let (sent_tx, sent) = mpsc::unbounded_channel::<(String, Value)>();
        tokio::spawn(async move {
            while let Some(message) = requests.next().await {
                let (method, id, params) = message.into_parts();
                let result = match method.as_ref() {
                    "workspace/applyEdit" => json!({ "applied": true }),
                    "window/showDocument" => json!({ "success": true }),
                    _ => Value::Null,
                };
                let _ = sent_tx.send((method.to_string(), params.unwrap_or(Value::Null)));
                if let Some(id) = id {
                    let _ = responses.send(Response::from_ok(id, result)).await;
                }
            }
        });
        let mut harness = Self {
            service,
            sent,
            next_id: 1,
            dir,
        };
        let root = uri(&harness.dir.0);
        harness
            .request(
                "initialize",
                json!({ "capabilities": {}, "workspaceFolders": [{ "uri": root, "name": "p" }] }),
            )
            .await;
        call(&mut harness.service, notification("initialized", json!({}))).await;
        harness
    }

    fn uri(&self, path: &str) -> String {
        uri(&self.dir.0.join(path))
    }

    async fn request(&mut self, method: &'static str, params: Value) -> Option<Value> {
        self.next_id += 1;
        call(&mut self.service, request(self.next_id, method, params)).await
    }

    async fn configure(&mut self, settings: Value) {
        call(
            &mut self.service,
            notification(
                "workspace/didChangeConfiguration",
                json!({ "settings": settings }),
            ),
        )
        .await;
    }

    async fn open(&mut self, path: &str) {
        let text = fs::read_to_string(self.dir.0.join(path)).unwrap();
        let uri = self.uri(path);
        call(
            &mut self.service,
            notification(
                "textDocument/didOpen",
                json!({ "textDocument": { "uri": uri, "languageId": "cpp", "version": 1, "text": text } }),
            ),
        )
        .await;
    }

    async fn change(&mut self, path: &str, version: i32, text: &str) {
        let uri = self.uri(path);
        call(
            &mut self.service,
            notification(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }]
                }),
            ),
        )
        .await;
    }

    async fn actions(&mut self, path: &str, line: u32, character: u32) -> Vec<Value> {
        let uri = self.uri(path);
        let position = json!({ "line": line, "character": character });
        self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": { "start": position, "end": position },
                "context": { "diagnostics": [] }
            }),
        )
        .await
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
    }

    async fn run(&mut self, action: &Value) -> Vec<(String, Value)> {
        let command = &action["command"];
        assert_eq!(command["command"], COMMAND_APPLY_ACTION, "{action}");
        self.request(
            "workspace/executeCommand",
            json!({ "command": command["command"], "arguments": command["arguments"] }),
        )
        .await;
        let mut sent = Vec::new();
        while let Ok(message) = self.sent.try_recv() {
            if message.0 != "window/logMessage" {
                sent.push(message);
            }
        }
        sent
    }
}

fn find<'a>(actions: &'a [Value], title: &str) -> &'a Value {
    actions
        .iter()
        .find(|a| a["title"] == title)
        .unwrap_or_else(|| panic!("no action {title:?} in {actions:?}"))
}

fn titles(actions: &[Value]) -> Vec<&str> {
    actions
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect()
}

const BULK_HEADER: &str = "#pragma once\n\nclass Foo {\npublic:\n    Foo();\n    void a();\n    void b();\nprivate:\n    int& r;\n};\n";

#[tokio::test]
async fn add_all_missing_definitions_in_matching_source_file() {
    let mut h = Harness::new(
        "bulk-source",
        &[("foo.h", BULK_HEADER), ("foo.cpp", "#include \"foo.h\"\n")],
    )
    .await;
    h.open("foo.h").await;
    // On the class name.
    let actions = h.actions("foo.h", 2, 7).await;
    assert_eq!(
        titles(&actions)[..2],
        [
            "Add all missing definitions in matching source file",
            "Add all missing definitions in this file"
        ]
    );
    let sent = h
        .run(find(
            &actions,
            "Add all missing definitions in matching source file",
        ))
        .await;
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].0, "workspace/applyEdit");
    let source_uri = h.uri("foo.cpp");
    assert_eq!(
        sent[0].1["edit"]["changes"][&source_uri],
        json!([{
            "range": { "start": { "line": 0, "character": 16 }, "end": { "line": 0, "character": 16 } },
            "newText": "\n\nFoo::Foo()\n    : r()\n{\n    \n}\n\nvoid Foo::a() {\n    \n}\n\nvoid Foo::b() {\n    \n}"
        }])
    );
    assert_eq!(sent[1].0, "window/showDocument");
    assert_eq!(sent[1].1["uri"], json!(source_uri));
    assert_eq!(
        sent[1].1["selection"]["start"],
        json!({ "line": 3, "character": 8 })
    );
}

#[tokio::test]
async fn add_all_missing_definitions_in_this_file() {
    let mut h = Harness::new("bulk-this", &[("foo.h", BULK_HEADER)]).await;
    h.open("foo.h").await;
    // On a declaration: only the single Add Definition actions.
    let actions = h.actions("foo.h", 5, 10).await;
    assert_eq!(
        titles(&actions),
        vec!["Add Definition in this file", "Add Definition inline"]
    );
    // On the class head.
    let actions = h.actions("foo.h", 2, 7).await;
    assert_eq!(
        titles(&actions)[0],
        "Add all missing definitions in this file"
    );
    assert!(
        titles(&actions)[1..]
            .iter()
            .all(|t| t.starts_with("Generate "))
    );
    let sent = h
        .run(find(&actions, "Add all missing definitions in this file"))
        .await;
    let header_uri = h.uri("foo.h");
    assert_eq!(sent[0].0, "workspace/applyEdit");
    let edits = sent[0].1["edit"]["changes"][&header_uri]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0]["newText"],
        "\n\ninline Foo::Foo()\n    : r()\n{\n    \n}\n\ninline void Foo::a() {\n    \n}\n\ninline void Foo::b() {\n    \n}"
    );
    assert_eq!(sent[1].0, "window/showDocument");
    assert_eq!(sent[1].1["uri"], json!(header_uri));
}

#[tokio::test]
async fn generate_constructor_initializing_all_members() {
    let header =
        "#pragma once\n\nstruct P : Base {\n    P(int x);\n    int a;\n    double b;\n};\n";
    let mut h = Harness::new("ctor", &[("p.h", header), ("p.cpp", "#include \"p.h\"\n")]).await;
    h.open("p.h").await;
    let actions = h.actions("p.h", 3, 5).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Add Definition in matching source file",
            "Add Definition in this file",
            "Add Definition inline",
            "Generate Constructor in matching source file (initialize all members)",
            "Generate Constructor in this file (initialize all members)",
        ]
    );
    let this_file = find(
        &actions,
        "Generate Constructor in this file (initialize all members)",
    );
    assert_eq!(
        this_file["edit"]["changes"][&h.uri("p.h")][0]["newText"],
        "\n\ninline P::P(int x)\n    : Base(),\n      a(),\n      b()\n{\n    \n}"
    );
    let sent = h
        .run(find(
            &actions,
            "Generate Constructor in matching source file (initialize all members)",
        ))
        .await;
    let source_uri = h.uri("p.cpp");
    assert_eq!(
        sent[0].1["edit"]["changes"][&source_uri][0]["newText"],
        "\n\nP::P(int x)\n    : Base(),\n      a(),\n      b()\n{\n    \n}"
    );
    assert_eq!(
        sent[1].1["selection"]["start"],
        json!({ "line": 3, "character": 11 })
    );
}

const ACCESSOR_HEADER: &str =
    "#pragma once\n\nclass Foo {\npublic:\n    Foo();\nprivate:\n    int m_count;\n};\n";

#[tokio::test]
async fn generate_getter_and_setter_inline() {
    let mut h = Harness::new("accessors-inline", &[("foo.h", ACCESSOR_HEADER)]).await;
    h.open("foo.h").await;
    // Nothing else is offered on a member variable.
    let actions = h.actions("foo.h", 6, 9).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Generate Getter and Setter for \"m_count\"",
            "Generate Getter for \"m_count\"",
            "Generate Setter for \"m_count\""
        ]
    );
    // Inline (the default) is a plain edit.
    let both = find(&actions, "Generate Getter and Setter for \"m_count\"");
    assert!(both.get("command").is_none());
    assert_eq!(
        both["edit"]["changes"][&h.uri("foo.h")],
        json!([{
            "range": { "start": { "line": 4, "character": 10 }, "end": { "line": 4, "character": 10 } },
            "newText": "\n    int count() const { return m_count; }\n    void setCount(int count) { m_count = count; }"
        }])
    );
}

#[tokio::test]
async fn generate_getter_and_setter_in_matching_source_file() {
    let mut h = Harness::new(
        "accessors-source",
        &[
            ("foo.h", ACCESSOR_HEADER),
            ("foo.cpp", "#include \"foo.h\"\n"),
        ],
    )
    .await;
    h.configure(json!({ "cpp": { "accessor": {
        "getterDefinitionLocation": "matchingSourceFile",
        "setterDefinitionLocation": "matchingSourceFile"
    } } }))
    .await;
    h.open("foo.h").await;
    let actions = h.actions("foo.h", 6, 9).await;
    assert_eq!(actions.len(), 3);
    for action in &actions {
        assert!(action.get("edit").is_none());
        assert_eq!(action["kind"], "refactor");
    }
    let sent = h
        .run(find(&actions, "Generate Getter and Setter for \"m_count\""))
        .await;
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].0, "workspace/applyEdit");
    let changes = &sent[0].1["edit"]["changes"];
    assert_eq!(
        changes[&h.uri("foo.h")],
        json!([{
            "range": { "start": { "line": 4, "character": 10 }, "end": { "line": 4, "character": 10 } },
            "newText": "\n    int count() const;\n    void setCount(int count);"
        }])
    );
    let source_uri = h.uri("foo.cpp");
    assert_eq!(
        changes[&source_uri],
        json!([{
            "range": { "start": { "line": 0, "character": 16 }, "end": { "line": 0, "character": 16 } },
            "newText": "\n\nint Foo::count() const {\n    return m_count;\n}\n\nvoid Foo::setCount(int count) {\n    m_count = count;\n}"
        }])
    );
    assert_eq!(sent[1].0, "window/showDocument");
    assert_eq!(sent[1].1["uri"], json!(source_uri));
    assert_eq!(
        sent[1].1["selection"]["start"],
        json!({ "line": 3, "character": 19 })
    );

    // Getter and setter alone also go through the command.
    for title in [
        "Generate Getter for \"m_count\"",
        "Generate Setter for \"m_count\"",
    ] {
        let sent = h.run(find(&actions, title)).await;
        assert_eq!(sent[0].0, "workspace/applyEdit", "{title}");
        assert_eq!(
            sent[0].1["edit"]["changes"].as_object().unwrap().len(),
            2,
            "{title}"
        );
        assert_eq!(sent[1].0, "window/showDocument", "{title}");
    }
}

#[tokio::test]
async fn generate_accessors_falls_back_below_class_without_source_file() {
    let mut h = Harness::new("accessors-nosource", &[("foo.h", ACCESSOR_HEADER)]).await;
    h.configure(
        json!({ "cpp": { "accessor": { "getterDefinitionLocation": "matchingSourceFile" } } }),
    )
    .await;
    h.open("foo.h").await;
    let actions = h.actions("foo.h", 6, 9).await;
    let getter = find(&actions, "Generate Getter for \"m_count\"");
    assert_eq!(
        getter["edit"]["changes"][&h.uri("foo.h")][1]["newText"],
        "\n\ninline int Foo::count() const {\n    return m_count;\n}"
    );
}

#[tokio::test]
async fn class_head_offers_bulk_and_operator_actions() {
    let mut h = Harness::new("operators", &[("foo.h", ACCESSOR_HEADER)]).await;
    h.open("foo.h").await;
    let actions = h.actions("foo.h", 2, 7).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Add all missing definitions in this file",
            "Generate Equality Operators for \"Foo\"",
            "Generate Relational Operators for \"Foo\"",
            "Generate Stream Output Operator for \"Foo\""
        ]
    );
    // Operators are defined inline: plain edits.
    let equality = find(&actions, "Generate Equality Operators for \"Foo\"");
    assert!(equality.get("command").is_none());
    assert_eq!(
        equality["edit"]["changes"][&h.uri("foo.h")],
        json!([{
            "range": { "start": { "line": 4, "character": 10 }, "end": { "line": 4, "character": 10 } },
            "newText": "\n    bool operator==(const Foo &other) const { return m_count == other.m_count; }\n    bool operator!=(const Foo &other) const { return !(*this == other); }"
        }])
    );
    // A `refactor.extract` request gets nothing.
    let uri = h.uri("foo.h");
    let position = json!({ "line": 2, "character": 7 });
    let none = h
        .request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": { "start": position, "end": position },
                "context": { "diagnostics": [], "only": ["refactor.extract"] }
            }),
        )
        .await;
    assert_eq!(none, Some(Value::Null));
}

const DECL_HEADER: &str =
    "#pragma once\n\nclass Foo {\npublic:\n    void a();\n};\n\nvoid free_a();\n";
const DECL_SOURCE: &str = "#include \"foo.h\"\n\nvoid Foo::a() {}\n\nint Foo::b(int x) const { return x; }\n\nvoid free_a() {}\n\nvoid free_b() {}\n\nstatic void helper() {}\n";

#[tokio::test]
async fn add_declaration_in_class_and_matching_header() {
    let mut h = Harness::new(
        "add-declaration",
        &[("foo.h", DECL_HEADER), ("foo.cpp", DECL_SOURCE)],
    )
    .await;
    h.open("foo.cpp").await;
    // Member definition: into the class in the header (a command).
    let actions = h.actions("foo.cpp", 4, 9).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Add Declaration in class \"Foo\"",
            "Move Definition into class \"Foo\""
        ]
    );
    let sent = h.run(&actions[0]).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "workspace/applyEdit");
    let changes = sent[0].1["edit"]["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[&h.uri("foo.h")],
        json!([{
            "range": { "start": { "line": 4, "character": 13 }, "end": { "line": 4, "character": 13 } },
            "newText": "\n    int b(int x) const;"
        }])
    );
    // Free function: header or this file.
    let actions = h.actions("foo.cpp", 8, 6).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Add Declaration in matching header file",
            "Add Declaration in this file",
            "Move Definition to matching header file"
        ]
    );
    let sent = h
        .run(find(&actions, "Add Declaration in matching header file"))
        .await;
    assert_eq!(
        sent[0].1["edit"]["changes"][&h.uri("foo.h")][0]["newText"],
        "\nvoid free_b();"
    );
    let this_file = find(&actions, "Add Declaration in this file");
    assert!(this_file.get("command").is_none());
    assert_eq!(
        this_file["edit"]["changes"][&h.uri("foo.cpp")][0]["newText"],
        "void free_b();\n\n"
    );
    // Declared already, or internal: no Add Declaration / only this file.
    assert_eq!(
        titles(&h.actions("foo.cpp", 6, 6).await),
        vec!["Move Definition to matching header file"]
    );
    let actions = h.actions("foo.cpp", 10, 13).await;
    assert_eq!(titles(&actions), vec!["Add Declaration in this file"]);
    // Inside a body: nothing.
    assert!(h.actions("foo.cpp", 4, 30).await.is_empty());
}

const MOVE_HEADER: &str =
    "#pragma once\n\nclass Foo {\npublic:\n    void a();\n    int b() const { return 1; }\n};\n";
const MOVE_SOURCE: &str = "#include \"foo.h\"\n\nvoid Foo::a()\n{\n    x();\n}\n";

#[tokio::test]
async fn move_definition_between_files() {
    let mut h = Harness::new(
        "move-definition",
        &[("foo.h", MOVE_HEADER), ("foo.cpp", MOVE_SOURCE)],
    )
    .await;
    h.open("foo.h").await;
    h.open("foo.cpp").await;
    let header_uri = h.uri("foo.h");
    let source_uri = h.uri("foo.cpp");

    // In-class definition in the header.
    let actions = h.actions("foo.h", 5, 9).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Move Definition to matching source file",
            "Move Definition below class body"
        ]
    );
    let below = find(&actions, "Move Definition below class body");
    assert!(below.get("command").is_none());
    assert_eq!(
        below["edit"]["changes"][&header_uri],
        json!([
            {
                "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 31 } },
                "newText": "int b() const;"
            },
            {
                "range": { "start": { "line": 6, "character": 2 }, "end": { "line": 6, "character": 2 } },
                "newText": "\n\ninline int Foo::b() const { return 1; }"
            }
        ])
    );
    let sent = h
        .run(find(&actions, "Move Definition to matching source file"))
        .await;
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].0, "workspace/applyEdit");
    let changes = &sent[0].1["edit"]["changes"];
    assert_eq!(changes.as_object().unwrap().len(), 2);
    assert_eq!(
        changes[&header_uri],
        json!([{
            "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 31 } },
            "newText": "int b() const;"
        }])
    );
    assert_eq!(
        changes[&source_uri],
        json!([{
            "range": { "start": { "line": 5, "character": 1 }, "end": { "line": 5, "character": 1 } },
            "newText": "\n\nint Foo::b() const { return 1; }"
        }])
    );
    assert_eq!(sent[1].0, "window/showDocument");
    assert_eq!(sent[1].1["uri"], json!(source_uri));
    assert_eq!(
        sent[1].1["selection"]["start"],
        json!({ "line": 7, "character": 0 })
    );

    // Out-of-class definition in the source file.
    let actions = h.actions("foo.cpp", 2, 11).await;
    assert_eq!(
        titles(&actions),
        vec![
            "Move Definition to matching header file",
            "Move Definition into class \"Foo\""
        ]
    );
    let sent = h
        .run(find(&actions, "Move Definition into class \"Foo\""))
        .await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let changes = &sent[0].1["edit"]["changes"];
    assert_eq!(
        changes[&header_uri],
        json!([{
            "range": { "start": { "line": 4, "character": 4 }, "end": { "line": 4, "character": 13 } },
            "newText": "void a()\n    {\n        x();\n    }"
        }])
    );
    assert_eq!(
        changes[&source_uri],
        json!([{
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 6, "character": 0 } },
            "newText": ""
        }])
    );
    let sent = h
        .run(find(&actions, "Move Definition to matching header file"))
        .await;
    assert_eq!(
        sent[0].1["edit"]["changes"][&header_uri][0]["newText"],
        "\n\ninline void Foo::a()\n{\n    x();\n}"
    );
    assert_eq!(sent[1].0, "window/showDocument");
    assert_eq!(sent[1].1["uri"], json!(header_uri));
}

const SIG_HEADER: &str = "#pragma once\n\nclass Foo {\npublic:\n    virtual int get(int a, int b = 2) const;\n};\n\nvoid helper(int x);\n\ninline void helper(int x) {}\n";
const SIG_SOURCE: &str =
    "#include \"foo.h\"\n\nint Foo::get(int a, int b) const\n{\n    return a + b;\n}\n";

#[tokio::test]
async fn update_function_signature() {
    let mut h = Harness::new(
        "update-signature",
        &[("foo.h", SIG_HEADER), ("foo.cpp", SIG_SOURCE)],
    )
    .await;
    h.open("foo.h").await;
    let header_uri = h.uri("foo.h");
    let source_uri = h.uri("foo.cpp");

    // Several edits: rename, broken syntax while typing, new parameter, type change, an edit
    // elsewhere in the file and removed const.
    let steps = [
        SIG_HEADER.replace("int get(", "int fetch("),
        SIG_HEADER.replace(
            "int get(int a, int b = 2) const",
            "int fetch(int a, int b = 2, ",
        ),
        SIG_HEADER.replace(
            "int get(int a, int b = 2) const",
            "long fetch(float a, int b = 2, bool c)",
        ),
        format!(
            "// edited\n{}",
            SIG_HEADER.replace(
                "int get(int a, int b = 2) const",
                "long fetch(float a, int b = 2, bool c)"
            )
        ),
    ];
    for (i, text) in steps.iter().enumerate() {
        h.change("foo.h", i as i32 + 2, text).await;
    }
    // The cursor elsewhere: nothing to offer there.
    let actions = h.actions("foo.h", 0, 0).await;
    assert!(titles(&actions).iter().all(|t| !t.starts_with("Update")));

    let actions = h.actions("foo.h", 5, 20).await;
    let update = find(&actions, "Update Function Signature in \"foo.cpp\"");
    assert_eq!(
        titles(&actions)[0],
        "Update Function Signature in \"foo.cpp\""
    );
    let sent = h.run(update).await;
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].0, "workspace/applyEdit");
    let changes = &sent[0].1["edit"]["changes"];
    assert_eq!(changes.as_object().unwrap().len(), 1);
    let edits: Vec<String> = changes[&source_uri]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["newText"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(edits, vec!["long ", "fetch", "float a, int b, bool c", ""]);
    assert_eq!(sent[1].0, "window/showDocument");
    assert_eq!(sent[1].1["uri"], json!(source_uri));
    assert_eq!(
        sent[1].1["selection"]["start"],
        json!({ "line": 2, "character": 10 })
    );
    // Applied: no longer offered.
    let actions = h.actions("foo.h", 5, 20).await;
    assert!(titles(&actions).iter().all(|t| !t.starts_with("Update")));

    // Same file: a plain edit.
    let last = &steps[3];
    let edited = last.replace("void helper(int x);", "void helper(const int& x);");
    h.change("foo.h", 10, &edited).await;
    let actions = h.actions("foo.h", 8, 8).await;
    let update = find(&actions, "Update Function Signature");
    assert!(update.get("command").is_none());
    assert_eq!(
        update["edit"]["changes"][&header_uri],
        json!([{
            "range": { "start": { "line": 10, "character": 19 }, "end": { "line": 10, "character": 24 } },
            "newText": "const int& x"
        }])
    );
}
