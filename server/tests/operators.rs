//! Fixture tests for Generate Equality / Relational / Stream Output Operators. `@` marks the
//! cursor.

use cmantic_ls::actions::add_definition::FileInput;
use cmantic_ls::actions::operators::{OperatorKind, available_operators, generate_operators};
use cmantic_ls::config::Config;
use cmantic_ls::text::{offset_to_position, position_to_offset};
use serde_json::json;

fn split_cursor(text: &str) -> (String, usize) {
    let offset = text.find('@').expect("fixture has a cursor marker");
    (text.replacen('@', "", 1), offset)
}

fn header(text: &str) -> FileInput<'_> {
    FileInput {
        text,
        is_header: true,
        is_c: false,
    }
}

fn titles(text: &str) -> Vec<String> {
    let (text, offset) = split_cursor(text);
    available_operators(&header(&text), offset_to_position(&text, offset))
        .into_iter()
        .map(|a| a.title)
        .collect()
}

fn run_with(text: &str, kind: OperatorKind, config: &Config) -> String {
    let (text, offset) = split_cursor(text);
    let generated = generate_operators(
        &header(&text),
        offset_to_position(&text, offset),
        kind,
        config,
    )
    .expect("an edit");
    assert!(generated.source_edits.is_empty());
    let mut result = text.clone();
    for edit in generated.edits.iter().rev() {
        let start = position_to_offset(&text, edit.range.start);
        let end = position_to_offset(&text, edit.range.end);
        result.replace_range(start..end, &edit.new_text);
    }
    result
}

fn run(text: &str, kind: OperatorKind) -> String {
    run_with(text, kind, &Config::default())
}

const CLASS: &str = "class Fo@o : public ns::Base<int> {\npublic:\n    Foo();\n\nprivate:\n    int a;\n    std::string b;\n};\n";

#[test]
fn titles_on_class_head() {
    assert_eq!(
        titles(CLASS),
        vec![
            "Generate Equality Operators for \"Foo\"",
            "Generate Relational Operators for \"Foo\"",
            "Generate Stream Output Operator for \"Foo\"",
        ]
    );
}

#[test]
fn equality_operators() {
    assert_eq!(
        run(CLASS, OperatorKind::Equality),
        "class Foo : public ns::Base<int> {\npublic:\n    Foo();\n\n    bool operator==(const Foo &other) const {\n        return static_cast<const ns::Base<int> &>(*this) == static_cast<const ns::Base<int> &>(other)\n            && a == other.a\n            && b == other.b;\n    }\n    bool operator!=(const Foo &other) const { return !(*this == other); }\n\nprivate:\n    int a;\n    std::string b;\n};\n"
    );
}

#[test]
fn relational_operators() {
    let text = "struct P@ {\n    int x;\n    int y;\n};\n";
    assert_eq!(
        run(text, OperatorKind::Relational),
        "struct P {\n    int x;\n    int y;\n\n    bool operator<(const P &other) const {\n        if (x < other.x)\n            return true;\n        if (other.x < x)\n            return false;\n        return y < other.y;\n    }\n    bool operator>(const P &other) const { return other < *this; }\n    bool operator<=(const P &other) const { return !(other < *this); }\n    bool operator>=(const P &other) const { return !(*this < other); }\n};\n"
    );
}

#[test]
fn stream_output_operator() {
    assert_eq!(
        run(CLASS, OperatorKind::StreamOutput),
        "class Foo : public ns::Base<int> {\npublic:\n    Foo();\n\n    friend std::ostream &operator<<(std::ostream &os, const Foo &rhs) {\n        os << static_cast<const ns::Base<int> &>(rhs)\n           << \" a: \" << rhs.a\n           << \" b: \" << rhs.b;\n        return os;\n    }\n\nprivate:\n    int a;\n    std::string b;\n};\n"
    );
}

#[test]
fn single_operand_stays_on_one_line() {
    let text = "struct P@ {\n    int x;\n};\n";
    assert_eq!(
        run(text, OperatorKind::Equality),
        "struct P {\n    int x;\n\n    bool operator==(const P &other) const { return x == other.x; }\n    bool operator!=(const P &other) const { return !(*this == other); }\n};\n"
    );
    assert_eq!(
        run(text, OperatorKind::StreamOutput),
        "struct P {\n    int x;\n\n    friend std::ostream &operator<<(std::ostream &os, const P &rhs) {\n        os << \"x: \" << rhs.x;\n        return os;\n    }\n};\n"
    );
}

#[test]
fn friend_comparison_operators() {
    let config = Config::from_value(&json!({ "cpp": { "friendComparisonOperators": true } }));
    let text = "struct P@ : Base {\n    int x;\n};\n";
    assert_eq!(
        run_with(text, OperatorKind::Equality, &config),
        "struct P : Base {\n    int x;\n\n    friend bool operator==(const P &lhs, const P &rhs) {\n        return static_cast<const Base &>(lhs) == static_cast<const Base &>(rhs)\n            && lhs.x == rhs.x;\n    }\n    friend bool operator!=(const P &lhs, const P &rhs) { return !(lhs == rhs); }\n};\n"
    );
    let text = "struct P@ {\n    int x;\n};\n";
    assert_eq!(
        run_with(text, OperatorKind::Relational, &config),
        "struct P {\n    int x;\n\n    friend bool operator<(const P &lhs, const P &rhs) { return lhs.x < rhs.x; }\n    friend bool operator>(const P &lhs, const P &rhs) { return rhs < lhs; }\n    friend bool operator<=(const P &lhs, const P &rhs) { return !(rhs < lhs); }\n    friend bool operator>=(const P &lhs, const P &rhs) { return !(lhs < rhs); }\n};\n"
    );
}

#[test]
fn explicit_this_pointer_and_new_line_braces() {
    let config = Config::from_value(&json!({ "cpp": {
        "useExplicitThisPointer": true,
        "curlyBraceFormat": { "function": "newLine" }
    } }));
    let text = "struct P@ {\n    int x;\n    int y;\n};\n";
    assert_eq!(
        run_with(text, OperatorKind::Equality, &config),
        "struct P {\n    int x;\n    int y;\n\n    bool operator==(const P &other) const\n    {\n        return this->x == other.x\n            && this->y == other.y;\n    }\n    bool operator!=(const P &other) const { return !(*this == other); }\n};\n"
    );
}

#[test]
fn class_without_public_section_gets_one() {
    let text = "class P@ {\n    int x;\n};\n";
    assert_eq!(
        run(text, OperatorKind::Equality),
        "class P {\n    int x;\n\npublic:\n    bool operator==(const P &other) const { return x == other.x; }\n    bool operator!=(const P &other) const { return !(*this == other); }\n};\n"
    );
}

#[test]
fn templates_use_the_templated_name() {
    let text = "template <typename T, int N = 1>\nclass Bo@x {\n    T v;\n};\n";
    assert_eq!(
        run(text, OperatorKind::Equality),
        "template <typename T, int N = 1>\nclass Box {\n    T v;\n\npublic:\n    bool operator==(const Box<T, N> &other) const { return v == other.v; }\n    bool operator!=(const Box<T, N> &other) const { return !(*this == other); }\n};\n"
    );
    let text = "template <>\nclass Bo@x<int> {\n    int v;\n};\n";
    assert_eq!(
        run(text, OperatorKind::StreamOutput),
        "template <>\nclass Box<int> {\n    int v;\n\npublic:\n    friend std::ostream &operator<<(std::ostream &os, const Box<int> &rhs) {\n        os << \"v: \" << rhs.v;\n        return os;\n    }\n};\n"
    );
}

#[test]
fn empty_class() {
    let text = "struct E@ {};\n";
    assert_eq!(
        run(text, OperatorKind::Equality),
        "struct E {\n    bool operator==(const E &other) const { return true; }\n    bool operator!=(const E &other) const { return !(*this == other); }\n};\n"
    );
    assert_eq!(
        run(text, OperatorKind::StreamOutput),
        "struct E {\n    friend std::ostream &operator<<(std::ostream &os, const E &rhs) { return os; }\n};\n"
    );
}

#[test]
fn existing_operators_are_skipped() {
    let text = "struct P@ {\n    bool operator==(const P &o) const;\n    friend bool operator<(const P &a, const P &b);\n    friend std::ostream &operator<<(std::ostream &os, const P &p);\n    int x;\n};\n";
    assert_eq!(
        titles(text),
        vec![
            "Generate Equality Operators for \"P\"",
            "Generate Relational Operators for \"P\""
        ]
    );
    // Only the missing ones, next to the existing one.
    assert_eq!(
        run(text, OperatorKind::Equality),
        "struct P {\n    bool operator==(const P &o) const;\n    bool operator!=(const P &other) const { return !(*this == other); }\n    friend bool operator<(const P &a, const P &b);\n    friend std::ostream &operator<<(std::ostream &os, const P &p);\n    int x;\n};\n"
    );
    assert_eq!(
        run(text, OperatorKind::Relational),
        "struct P {\n    bool operator==(const P &o) const;\n    friend bool operator<(const P &a, const P &b);\n    bool operator>(const P &other) const { return other < *this; }\n    bool operator<=(const P &other) const { return !(other < *this); }\n    bool operator>=(const P &other) const { return !(*this < other); }\n    friend std::ostream &operator<<(std::ostream &os, const P &p);\n    int x;\n};\n"
    );
    let all = "struct P@ {\n    bool operator==(const P&) const;\n    bool operator!=(const P&) const;\n    bool operator<(const P&) const;\n    bool operator>(const P&) const;\n    bool operator<=(const P&) const;\n    bool operator>=(const P&) const;\n    friend std::ostream& operator<<(std::ostream&, const P&);\n};\n";
    assert!(titles(all).is_empty());
}

#[test]
fn nested_class() {
    let text = "class Outer {\npublic:\n    struct In@ner {\n        int v;\n    };\n};\n";
    assert_eq!(
        run(text, OperatorKind::Equality),
        "class Outer {\npublic:\n    struct Inner {\n        int v;\n\n        bool operator==(const Inner &other) const { return v == other.v; }\n        bool operator!=(const Inner &other) const { return !(*this == other); }\n    };\n};\n"
    );
}

#[test]
fn not_offered() {
    // Inside the body.
    assert!(titles("struct P {\n    int x@;\n};\n").is_empty());
    assert!(titles("struct P {\n  @  int x;\n};\n").is_empty());
    // Unions, anonymous structs, forward declarations, functions.
    assert!(titles("union U@ {\n    int i;\n};\n").is_empty());
    assert!(titles("struct@ {\n    int i;\n} s;\n").is_empty());
    assert!(titles("struct P@;\n").is_empty());
    assert!(titles("void f@();\n").is_empty());
    // C files.
    let (text, offset) = split_cursor("struct P@ {\n    int x;\n};\n");
    let c = FileInput {
        text: &text,
        is_header: true,
        is_c: true,
    };
    assert!(available_operators(&c, offset_to_position(&text, offset)).is_empty());
}

/// Operator actions must never panic, wherever the cursor is.
#[test]
fn every_offset_is_safe() {
    let texts = [
        CLASS.replace('@', ""),
        "#pragma once\ntemplate <typename T>\nstruct Box : Base<T>, private Other {\n    T value_; int *a, b = 2, c[3];\n    static int s;\n    union { int i; float f; };\n    struct Inner { int n; } inner;\n    bool operator==(const Box&) const;\nprivate:\n    int (*fp)(int);\n};\nclass E {};\nnamespace n { class C { int x; }; }\n".to_string(),
        "class Broken : public {\n    int x\n".to_string(),
        "struct S {\r\n\tint x;\r\n};".to_string(),
    ];
    let configs = [
        Config::default(),
        Config::from_value(&json!({ "cpp": {
            "friendComparisonOperators": true,
            "useExplicitThisPointer": true,
            "curlyBraceFormat": { "function": "newLine" }
        } })),
    ];
    for text in &texts {
        let file = header(text);
        let mut previous = Vec::new();
        for (offset, _) in text.char_indices() {
            let position = offset_to_position(text, offset);
            let offered = available_operators(&file, position);
            if offered != previous {
                for action in &offered {
                    for config in &configs {
                        assert!(
                            generate_operators(&file, position, action.kind, config).is_some(),
                            "{} at {offset}",
                            action.title
                        );
                    }
                }
            }
            previous = offered;
        }
    }
}
