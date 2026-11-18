//! Fixture tests for Generate Getters and Setters. `@` marks the cursor; in expected source
//! texts it marks the cursor after the edit.

use cmantic_ls::actions::accessors::{
    AccessorAction, AccessorKind, available_accessors, generate_accessors,
};
use cmantic_ls::actions::add_definition::FileInput;
use cmantic_ls::config::Config;
use cmantic_ls::text::{offset_to_position, position_to_offset};
use serde_json::{Value, json};
use tower_lsp_server::ls_types::{Position, TextEdit};

fn split_cursor(text: &str) -> (String, usize) {
    let offset = text.find('@').expect("fixture has a cursor marker");
    (text.replacen('@', "", 1), offset)
}

fn apply(text: &str, edits: &[TextEdit], cursor: Option<Position>) -> String {
    let mut result = text.to_string();
    for edit in edits.iter().rev() {
        let start = position_to_offset(text, edit.range.start);
        let end = position_to_offset(text, edit.range.end);
        result.replace_range(start..end, &edit.new_text);
    }
    if let Some(cursor) = cursor {
        let offset = position_to_offset(&result, cursor);
        result.insert(offset, '@');
    }
    result
}

fn header(text: &str) -> FileInput<'_> {
    FileInput {
        text,
        is_header: true,
        is_c: false,
    }
}

fn config(settings: Value) -> Config {
    Config::from_value(&settings)
}

fn actions(text: &str, has_source: bool, config: &Config) -> Vec<AccessorAction> {
    let (text, offset) = split_cursor(text);
    available_accessors(
        &header(&text),
        offset_to_position(&text, offset),
        has_source,
        config,
    )
}

fn titles(text: &str) -> Vec<String> {
    actions(text, false, &Config::default())
        .into_iter()
        .map(|a| a.title)
        .collect()
}

/// Runs `kind` and returns the new header text and the new source text (if changed).
fn run_with(
    text: &str,
    kind: AccessorKind,
    source: Option<&str>,
    config: &Config,
) -> (String, Option<String>) {
    let (text, offset) = split_cursor(text);
    let file = header(&text);
    let source_file = source.map(|text| FileInput {
        text,
        is_header: false,
        is_c: false,
    });
    let generated = generate_accessors(
        &file,
        offset_to_position(&text, offset),
        kind,
        source_file.as_ref(),
        config,
    )
    .expect("an edit");
    let new_source = (!generated.source_edits.is_empty()).then(|| {
        apply(
            source.unwrap(),
            &generated.source_edits,
            generated.source_cursor,
        )
    });
    (apply(&text, &generated.edits, None), new_source)
}

fn run(text: &str, kind: AccessorKind) -> String {
    run_with(text, kind, None, &Config::default()).0
}

const CLASS: &str =
    "class Foo {\npublic:\n    Foo();\n\nprivate:\n    int m_co@unt;\n    std::string name;\n};\n";

#[test]
fn titles_follow_c_mantic() {
    assert_eq!(
        titles(CLASS),
        vec![
            "Generate Getter and Setter for \"m_count\"",
            "Generate Getter for \"m_count\"",
            "Generate Setter for \"m_count\"",
        ]
    );
}

#[test]
fn getter_and_setter_inline_in_public_section() {
    assert_eq!(
        run(CLASS, AccessorKind::GetterAndSetter),
        "class Foo {\npublic:\n    Foo();\n\n    int count() const { return m_count; }\n    void setCount(int count) { m_count = count; }\n\nprivate:\n    int m_count;\n    std::string name;\n};\n"
    );
}

#[test]
fn getter_only_and_setter_only() {
    assert_eq!(
        run(CLASS, AccessorKind::Getter),
        "class Foo {\npublic:\n    Foo();\n\n    int count() const { return m_count; }\n\nprivate:\n    int m_count;\n    std::string name;\n};\n"
    );
    assert_eq!(
        run(CLASS, AccessorKind::Setter),
        "class Foo {\npublic:\n    Foo();\n\n    void setCount(int count) { m_count = count; }\n\nprivate:\n    int m_count;\n    std::string name;\n};\n"
    );
}

#[test]
fn class_types_are_set_by_const_reference() {
    let text = "class Foo {\npublic:\n    Foo();\nprivate:\n    std::string na@me;\n};\n";
    assert_eq!(
        run(text, AccessorKind::GetterAndSetter),
        "class Foo {\npublic:\n    Foo();\n    std::string getName() const { return name; }\n    void setName(const std::string &name_) { name = name_; }\nprivate:\n    std::string name;\n};\n"
    );
}

#[test]
fn pointers_and_primitives_are_set_by_value() {
    let text = "struct S {\n    const char* te@xt;\n    unsigned long n;\n};\n";
    assert_eq!(
        run(text, AccessorKind::GetterAndSetter),
        "struct S {\n    const char* text;\n    unsigned long n;\n\n    const char* getText() const { return text; }\n    void setText(const char* text_) { text = text_; }\n};\n"
    );
    let text = "struct S {\n    unsigned long n@;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Setter),
        "struct S {\n    unsigned long n;\n\n    void setN(unsigned long n_) { n = n_; }\n};\n"
    );
}

#[test]
fn getter_drops_top_level_qualifiers_only() {
    let text = "struct S {\n    mutable volatile int a@;\n    int* const p;\n    const std::vector<const int*>& r;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Getter),
        "struct S {\n    mutable volatile int a;\n    int* const p;\n    const std::vector<const int*>& r;\n\n    int getA() const { return a; }\n};\n"
    );
    let text = "struct S {\n    int* const p@;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Getter),
        "struct S {\n    int* const p;\n\n    int* getP() const { return p; }\n};\n"
    );
    let text = "struct S {\n    const std::vector<const int*>& r@;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Getter),
        "struct S {\n    const std::vector<const int*>& r;\n\n    const std::vector<const int*>& getR() const { return r; }\n};\n"
    );
}

#[test]
fn no_setter_for_const_constexpr_or_reference_members() {
    assert_eq!(
        titles("struct S {\n    const int c@ = 1;\n};\n"),
        vec!["Generate Getter for \"c\""]
    );
    assert_eq!(
        titles("struct S {\n    int& r@;\n};\n"),
        vec!["Generate Getter for \"r\""]
    );
    assert_eq!(
        titles("struct S {\n    static constexpr int k@ = 1;\n};\n"),
        vec!["Generate Getter for \"k\""]
    );
}

#[test]
fn static_members_get_static_accessors() {
    let text = "class Foo {\n    static int s_co@unt;\n};\n";
    assert_eq!(
        run(text, AccessorKind::GetterAndSetter),
        "class Foo {\n    static int s_count;\n\npublic:\n    static int count() { return Foo::s_count; }\n    static void setCount(int count) { Foo::s_count = count; }\n};\n"
    );
}

#[test]
fn existing_accessors_are_skipped() {
    let text = "class Foo {\n    int x@;\npublic:\n    int getX() const { return x; }\n};\n";
    assert_eq!(titles(text), vec!["Generate Setter for \"x\""]);
    let text = "class Foo {\n    int x@;\npublic:\n    void setX(int v);\n};\n";
    assert_eq!(titles(text), vec!["Generate Getter for \"x\""]);
    let text =
        "class Foo {\n    int x@;\npublic:\n    int getX() const;\n    void setX(int v);\n};\n";
    assert!(titles(text).is_empty());
}

#[test]
fn getter_goes_before_existing_setter_and_setter_after_existing_getter() {
    let text = "class Foo {\n    int x@;\npublic:\n    // Sets x.\n    void setX(int x_) { x = x_; }\n    void other();\n};\n";
    assert_eq!(
        run(text, AccessorKind::Getter),
        "class Foo {\n    int x;\npublic:\n    int getX() const { return x; }\n    // Sets x.\n    void setX(int x_) { x = x_; }\n    void other();\n};\n"
    );
    let text = "class Foo {\n  public:\n    int getX() const; // trailing\n    void other();\n  private:\n    int x@;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Setter),
        "class Foo {\n  public:\n    int getX() const; // trailing\n    void setX(int x_) { x = x_; }\n    void other();\n  private:\n    int x;\n};\n"
    );
}

#[test]
fn public_section_is_created_like_existing_labels() {
    let text = "namespace ns {\n    class Foo {\n      private:\n        int x@;\n    };\n}\n";
    assert_eq!(
        run(text, AccessorKind::GetterAndSetter),
        "namespace ns {\n    class Foo {\n      private:\n        int x;\n\n      public:\n        int getX() const { return x; }\n        void setX(int x_) { x = x_; }\n    };\n}\n"
    );
}

#[test]
fn struct_is_public_by_default() {
    let text = "struct P {\n    int x@;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Getter),
        "struct P {\n    int x;\n\n    int getX() const { return x; }\n};\n"
    );
}

#[test]
fn multi_declarator_uses_declarator_under_cursor() {
    let text = "class Foo {\n    int *x, y@;\n};\n";
    assert_eq!(
        titles(text),
        vec![
            "Generate Getter and Setter for \"y\"",
            "Generate Getter for \"y\"",
            "Generate Setter for \"y\""
        ]
    );
    assert_eq!(
        run(text, AccessorKind::Getter),
        "class Foo {\n    int *x, y;\n\npublic:\n    int getY() const { return y; }\n};\n"
    );
    // On the shared type: the first declarator, which here is a pointer.
    let text = "class Foo {\n    in@t *x, y;\n};\n";
    assert_eq!(
        run(text, AccessorKind::Setter),
        "class Foo {\n    int *x, y;\n\npublic:\n    void setX(int *x_) { x = x_; }\n};\n"
    );
}

#[test]
fn nested_class() {
    let text = "class Outer {\npublic:\n    struct Inner {\n        int v@;\n    };\n};\n";
    assert_eq!(
        run_with(
            text,
            AccessorKind::Getter,
            None,
            &config(json!({ "cpp": { "accessor": { "getterDefinitionLocation": "belowClass" } } }))
        )
        .0,
        "class Outer {\npublic:\n    struct Inner {\n        int v;\n\n        int getV() const;\n    };\n};\n\ninline int Outer::Inner::getV() const {\n    return v;\n}\n"
    );
}

#[test]
fn not_offered() {
    // Not a member variable.
    assert!(titles("int glo@bal;\n").is_empty());
    assert!(titles("struct S {\n    void f@();\n};\n").is_empty());
    // Arrays, function pointers, members of anonymous classes.
    assert!(titles("struct S {\n    int a@[3];\n};\n").is_empty());
    assert!(titles("struct S {\n    int (*f@p)(int);\n};\n").is_empty());
    assert!(titles("struct S {\n    union {\n        int i@;\n    };\n};\n").is_empty());
    // C files.
    let (text, offset) = split_cursor("struct S {\n    int x@;\n};\n");
    let c = FileInput {
        text: &text,
        is_header: true,
        is_c: true,
    };
    assert!(
        available_accessors(
            &c,
            offset_to_position(&text, offset),
            false,
            &Config::default()
        )
        .is_empty()
    );
}

#[test]
fn case_styles() {
    let text = "class Foo {\n    bool m_isReady@;\n    bool enabled_;\n};\n";
    let snake = config(json!({ "caseStyle": "snake_case" }));
    assert_eq!(
        run_with(text, AccessorKind::GetterAndSetter, None, &snake).0,
        "class Foo {\n    bool m_isReady;\n    bool enabled_;\n\npublic:\n    bool is_ready() const { return m_isReady; }\n    void set_is_ready(bool isReady) { m_isReady = isReady; }\n};\n"
    );
    let pascal = config(json!({ "caseStyle": "PascalCase" }));
    assert_eq!(
        run_with(text, AccessorKind::GetterAndSetter, None, &pascal).0,
        "class Foo {\n    bool m_isReady;\n    bool enabled_;\n\npublic:\n    bool IsReady() const { return m_isReady; }\n    void SetIsReady(bool isReady) { m_isReady = isReady; }\n};\n"
    );
    let camel = Config::default();
    assert_eq!(
        run_with(text, AccessorKind::GetterAndSetter, None, &camel).0,
        "class Foo {\n    bool m_isReady;\n    bool enabled_;\n\npublic:\n    bool isReady() const { return m_isReady; }\n    void setIsReady(bool isReady) { m_isReady = isReady; }\n};\n"
    );
    // A name without prefix/suffix gets `get`, and the setter parameter a trailing `_`.
    let text = "struct S {\n    int value@;\n};\n";
    assert_eq!(
        run_with(text, AccessorKind::GetterAndSetter, None, &snake).0,
        "struct S {\n    int value;\n\n    int get_value() const { return value; }\n    void set_value(int value_) { value = value_; }\n};\n"
    );
}

#[test]
fn bool_getter_is_prefix() {
    let settings = config(json!({ "cpp": { "accessor": { "boolGetterIsPrefix": true } } }));
    let text = "struct S {\n    bool enabled_@;\n};\n";
    assert_eq!(
        run_with(text, AccessorKind::Getter, None, &settings).0,
        "struct S {\n    bool enabled_;\n\n    bool isEnabled() const { return enabled_; }\n};\n"
    );
    // Not for `std::vector<bool>`.
    let text = "struct S {\n    std::vector<bool> flags@;\n};\n";
    assert_eq!(
        run_with(text, AccessorKind::Getter, None, &settings).0,
        "struct S {\n    std::vector<bool> flags;\n\n    std::vector<bool> getFlags() const { return flags; }\n};\n"
    );
}

#[test]
fn explicit_this_pointer() {
    let settings = config(json!({ "cpp": { "useExplicitThisPointer": true } }));
    let text = "struct S {\n    int value@;\n};\n";
    assert_eq!(
        run_with(text, AccessorKind::GetterAndSetter, None, &settings).0,
        "struct S {\n    int value;\n\n    int getValue() const { return this->value; }\n    void setValue(int value) { this->value = value; }\n};\n"
    );
}

const NS_CLASS: &str = "#pragma once\n\nnamespace ns {\nclass Foo {\npublic:\n    Foo();\nprivate:\n    std::string na@me;\n};\n}\n";

#[test]
fn definitions_below_class() {
    let settings = config(json!({ "cpp": { "accessor": {
        "getterDefinitionLocation": "belowClass",
        "setterDefinitionLocation": "Generate definition below class body"
    } } }));
    assert_eq!(
        run_with(NS_CLASS, AccessorKind::GetterAndSetter, None, &settings).0,
        "#pragma once\n\nnamespace ns {\nclass Foo {\npublic:\n    Foo();\n    std::string getName() const;\n    void setName(const std::string &name_);\nprivate:\n    std::string name;\n};\n\ninline std::string Foo::getName() const {\n    return name;\n}\n\ninline void Foo::setName(const std::string &name_) {\n    name = name_;\n}\n}\n"
    );
}

#[test]
fn definitions_in_matching_source_file() {
    let settings = config(json!({ "cpp": { "accessor": {
        "getterDefinitionLocation": "matchingSourceFile",
        "setterDefinitionLocation": "matchingSourceFile"
    } } }));
    let offered = actions(NS_CLASS, true, &settings);
    assert!(offered.iter().all(|a| a.uses_matching_source));
    // Without a matching source file, they are defined below the class.
    assert!(
        actions(NS_CLASS, false, &settings)
            .iter()
            .all(|a| !a.uses_matching_source)
    );
    let (new_header, new_source) = run_with(
        NS_CLASS,
        AccessorKind::GetterAndSetter,
        Some("#include \"foo.h\"\n"),
        &settings,
    );
    assert_eq!(
        new_header,
        "#pragma once\n\nnamespace ns {\nclass Foo {\npublic:\n    Foo();\n    std::string getName() const;\n    void setName(const std::string &name_);\nprivate:\n    std::string name;\n};\n}\n"
    );
    assert_eq!(
        new_source.unwrap(),
        "#include \"foo.h\"\n\nstd::string ns::Foo::getName() const {\n    return name;@\n}\n\nvoid ns::Foo::setName(const std::string &name_) {\n    name = name_;\n}\n"
    );
}

#[test]
fn mixed_locations_and_brace_format() {
    let settings = config(json!({ "cpp": {
        "accessor": { "setterDefinitionLocation": "matchingSourceFile" },
        "curlyBraceFormat": { "function": "newLine" }
    } }));
    let offered = actions(NS_CLASS, true, &settings);
    let flags: Vec<bool> = offered.iter().map(|a| a.uses_matching_source).collect();
    assert_eq!(flags, vec![true, false, true]);
    let (new_header, new_source) = run_with(
        NS_CLASS,
        AccessorKind::GetterAndSetter,
        Some("#include \"foo.h\"\n\nnamespace ns {\n\nFoo::Foo()\n{\n}\n\n}\n"),
        &settings,
    );
    assert_eq!(
        new_header,
        "#pragma once\n\nnamespace ns {\nclass Foo {\npublic:\n    Foo();\n    std::string getName() const { return name; }\n    void setName(const std::string &name_);\nprivate:\n    std::string name;\n};\n}\n"
    );
    assert_eq!(
        new_source.unwrap(),
        "#include \"foo.h\"\n\nnamespace ns {\n\nFoo::Foo()\n{\n}\n\nvoid Foo::setName(const std::string &name_)\n{\n    name = name_;@\n}\n\n}\n"
    );
}

#[test]
fn class_templates_never_use_the_source_file() {
    let settings = config(json!({ "cpp": { "accessor": {
        "getterDefinitionLocation": "matchingSourceFile"
    } } }));
    let text = "template <typename T>\nclass Box {\n    T val@ue_;\n};\n";
    assert!(
        actions(text, true, &settings)
            .iter()
            .all(|a| !a.uses_matching_source)
    );
    let (new_header, new_source) = run_with(text, AccessorKind::Getter, Some(""), &settings);
    assert!(new_source.is_none());
    assert_eq!(
        new_header,
        "template <typename T>\nclass Box {\n    T value_;\n\npublic:\n    T value() const;\n};\n\ntemplate <typename T>\ninline T Box<T>::value() const {\n    return value_;\n}\n"
    );
    // Inline setters in templates take unknown types by const reference.
    assert_eq!(
        run(text, AccessorKind::Setter),
        "template <typename T>\nclass Box {\n    T value_;\n\npublic:\n    void setValue(const T &value) { value_ = value; }\n};\n"
    );
}

#[test]
fn crlf_and_tabs() {
    let text = "struct S {\r\n\tint x@;\r\n};\r\n";
    assert_eq!(
        run(text, AccessorKind::GetterAndSetter),
        "struct S {\r\n\tint x;\r\n\r\n\tint getX() const { return x; }\r\n\tvoid setX(int x_) { x = x_; }\r\n};\r\n"
    );
}

#[test]
fn single_line_class() {
    let text = "struct S { int x@; };\n";
    assert_eq!(
        run(text, AccessorKind::Getter),
        "struct S { int x;\n\nint getX() const { return x; }\n};\n"
    );
}

/// Accessor actions must never panic, wherever the cursor is and whatever the settings.
#[test]
fn every_offset_is_safe() {
    let texts = [
        CLASS.replace('@', ""),
        NS_CLASS.replace('@', ""),
        "#ifndef A_H\n#define A_H\ntemplate <typename T, int N = 3>\nclass Box : public Base<T> {\n    T value_; int *a, b = 2, c[3];\n    static const std::map<int, T>* s_map;\n    [[no_unique_address]] mutable volatile bool m_flag; // flag\n    int (*fp)(int);\n    union { int i; float f; };\n    struct Inner { std::string name_; } inner;\n    int bits : 3;\n    const int& ref;\npublic:\n    T getValue() const;\n};\nstruct S { int x; };\nnamespace a::b { class C { int _ = 0, __; }; }\n#endif\n".to_string(),
        "class Broken {\n    int x\n    std::string\n".to_string(),
        "struct S {\r\n\tint x;\r\n};".to_string(),
    ];
    let configs = [
        Config::default(),
        config(json!({ "caseStyle": "snake_case", "cpp": {
            "accessor": { "getterDefinitionLocation": "belowClass", "setterDefinitionLocation": "matchingSourceFile", "boolGetterIsPrefix": true },
            "useExplicitThisPointer": true
        } })),
        config(json!({ "caseStyle": "PascalCase", "cpp": {
            "accessor": { "getterDefinitionLocation": "matchingSourceFile", "setterDefinitionLocation": "belowClass" }
        } })),
    ];
    let source = FileInput {
        text: "#include \"a.h\"\nnamespace a::b {\n}\n",
        is_header: false,
        is_c: false,
    };
    for text in &texts {
        let file = header(text);
        for config in &configs {
            let mut previous: Vec<AccessorAction> = Vec::new();
            for (offset, _) in text.char_indices() {
                let position = offset_to_position(text, offset);
                let offered = available_accessors(&file, position, true, config);
                // Generating is the expensive part; do it once per member.
                if offered != previous {
                    for action in &offered {
                        for source in [None, Some(&source)] {
                            assert!(
                                generate_accessors(&file, position, action.kind, source, config)
                                    .is_some(),
                                "{} at {offset} in {text:?}",
                                action.title
                            );
                        }
                    }
                }
                previous = offered;
            }
        }
    }
}
