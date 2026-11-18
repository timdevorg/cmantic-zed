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

use cmantic_ls::server::{Backend, COMMAND_ADD_DEFINITION_IN_SOURCE};

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
    assert_eq!(command["command"], COMMAND_ADD_DEFINITION_IN_SOURCE);
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
}
