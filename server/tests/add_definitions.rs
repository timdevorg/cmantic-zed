//! Fixture tests for "Add all missing definitions" and "Generate Constructor (initialize all
//! members)". `@` marks the cursor; in expected texts it marks the cursor after the edit.

use cmantic_ls::actions::add_definition::{
    BulkEdit, BulkTarget, FileInput, Initializers, Insertion, Target, add_definition_with,
    add_definitions, available_bulk_targets, available_targets, offers_constructor_initializers,
};
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

fn source(text: &str) -> FileInput<'_> {
    FileInput {
        text,
        is_header: false,
        is_c: false,
    }
}

/// Applies LSP edits (non-overlapping, against `text`) and marks `cursor` with `@`.
fn apply_bulk(text: &str, bulk: &BulkEdit) -> String {
    let mut result = text.to_string();
    for edit in bulk.edits.iter().rev() {
        let start = position_to_offset(text, edit.range.start);
        let end = position_to_offset(text, edit.range.end);
        result.replace_range(start..end, &edit.new_text);
    }
    let cursor = position_to_offset(&result, bulk.cursor);
    result.insert(cursor, '@');
    result
}

fn apply(text: &str, insertion: &Insertion) -> String {
    let start = position_to_offset(text, insertion.range.start);
    let end = position_to_offset(text, insertion.range.end);
    let mut result = format!("{}{}{}", &text[..start], insertion.new_text, &text[end..]);
    let cursor = position_to_offset(&result, insertion.cursor);
    result.insert(cursor, '@');
    result
}

fn bulk_targets(header_text: &str, source_text: Option<&str>) -> Vec<BulkTarget> {
    let (text, offset) = split_cursor(header_text);
    let src = source_text.map(source);
    available_bulk_targets(
        &header(&text),
        offset_to_position(&text, offset),
        src.as_ref(),
    )
}

fn run_bulk(header_text: &str, source_text: Option<&str>, target: BulkTarget) -> String {
    run_bulk_with(header_text, source_text, target, &Config::default())
}

fn run_bulk_with(
    header_text: &str,
    source_text: Option<&str>,
    target: BulkTarget,
    config: &Config,
) -> String {
    let src = source_text.map(source);
    let bulk = add_definitions(&header(header_text), target, src.as_ref(), config).expect("edits");
    match target {
        BulkTarget::MatchingSource => apply_bulk(source_text.unwrap(), &bulk),
        BulkTarget::CurrentFile => apply_bulk(header_text, &bulk),
    }
}

const CLASS: &str = "#pragma once\n\nnamespace ns {\n\nclass Fo@o {\npublic:\n    Foo(int x);\n    void a();\n    int b() const;\n    void c() {}\n    inline void d();\n    virtual void e() = 0;\n};\n\nvoid free(int y);\n\n}\n";

#[test]
fn bulk_offered_on_class_head() {
    assert_eq!(
        bulk_targets(CLASS, Some("")),
        vec![BulkTarget::MatchingSource, BulkTarget::CurrentFile]
    );
    assert_eq!(bulk_targets(CLASS, None), vec![BulkTarget::CurrentFile]);
}

#[test]
fn bulk_to_matching_source_in_declaration_order() {
    let text = CLASS.replace('@', "");
    let src = "#include \"foo.h\"\n";
    assert_eq!(
        run_bulk(&text, Some(src), BulkTarget::MatchingSource),
        "#include \"foo.h\"\n\nns::Foo::Foo(int x)\n{\n    @\n}\n\nvoid ns::Foo::a() {\n    \n}\n\nint ns::Foo::b() const {\n    \n}\n\nvoid ns::free(int y) {\n    \n}\n"
    );
}

#[test]
fn bulk_to_matching_source_skips_existing_and_header_only() {
    let text =
        "struct S {\n    void a();\n    void b();\n    constexpr int c();\n    void d();\n};\n";
    let src = "#include \"s.h\"\n\nvoid S::b() {\n}\n";
    assert_eq!(
        run_bulk(text, Some(src), BulkTarget::MatchingSource),
        "#include \"s.h\"\n\nvoid S::a() {\n    @\n}\n\nvoid S::b() {\n}\n\nvoid S::d() {\n    \n}\n"
    );
}

#[test]
fn bulk_in_this_file() {
    let text = CLASS.replace('@', "");
    assert_eq!(
        run_bulk(&text, None, BulkTarget::CurrentFile),
        "#pragma once\n\nnamespace ns {\n\nclass Foo {\npublic:\n    Foo(int x);\n    void a();\n    int b() const;\n    void c() {}\n    inline void d();\n    virtual void e() = 0;\n};\n\nvoid free(int y);\n\ninline Foo::Foo(int x)\n{\n    @\n}\n\ninline void Foo::a() {\n    \n}\n\ninline int Foo::b() const {\n    \n}\n\ninline void Foo::d() {\n    \n}\n\ninline void free(int y) {\n    \n}\n\n}\n"
    );
}

#[test]
fn bulk_crlf() {
    let text = "struct S {\r\n    void a();\r\n    void b();\r\n};\r\n";
    let src = "#include \"s.h\"\r\n";
    assert_eq!(
        run_bulk(text, Some(src), BulkTarget::MatchingSource),
        "#include \"s.h\"\r\n\r\nvoid S::a() {\r\n    @\r\n}\r\n\r\nvoid S::b() {\r\n    \r\n}\r\n"
    );
}

#[test]
fn bulk_constructors_initialize_required_members() {
    let text = "class A {\npublic:\n    A();\n    void f();\nprivate:\n    int& r;\n    const int c;\n    const int d = 1;\n    int x;\n};\n";
    assert_eq!(
        run_bulk(text, Some(""), BulkTarget::MatchingSource),
        "A::A()\n    : r(@),\n      c()\n{\n    \n}\n\nvoid A::f() {\n    \n}\n"
    );
}

#[test]
fn bulk_not_offered() {
    // Inside a class body but not on a declaration.
    assert!(
        bulk_targets(
            "struct S {\n    int @x;\n    void a();\n    void b();\n};\n",
            None
        )
        .is_empty()
    );
    // On the class head, but everything is defined.
    let src = "void S::a() {}\n";
    assert!(bulk_targets("struct @S {\n    void a();\n};\n", Some(src)).is_empty());
    // Only header-only functions: no matching-source variant.
    assert_eq!(
        bulk_targets("struct @S {\n    inline void a();\n};\n", Some("")),
        vec![BulkTarget::CurrentFile]
    );
    let none = add_definitions(
        &header("struct S {\n    void a() {}\n};\n"),
        BulkTarget::CurrentFile,
        None,
        &Config::default(),
    );
    assert!(none.is_none());
}

#[test]
fn bulk_not_offered_on_declarations() {
    // Only the class head offers "Add all missing definitions", to keep the menu short.
    assert!(bulk_targets("struct S {\n    void a@();\n    void b();\n};\n", None).is_empty());
    assert_eq!(
        bulk_targets("struct S@ {\n    void a();\n    void b();\n};\n", None),
        vec![BulkTarget::CurrentFile]
    );
}

fn run_ctor(
    header_text: &str,
    source_text: Option<&str>,
    target: Target,
    config: &Config,
) -> String {
    let (text, offset) = split_cursor(header_text);
    let decl = header(&text);
    let position = offset_to_position(&text, offset);
    let src = source_text.map(source);
    assert!(available_targets(&decl, position, src.as_ref()).contains(&target));
    assert!(offers_constructor_initializers(&decl, position));
    let insertion = add_definition_with(
        &decl,
        position,
        target,
        src.as_ref(),
        Initializers::All,
        config,
    )
    .expect("an edit");
    match target {
        Target::MatchingSource => apply(source_text.unwrap(), &insertion),
        _ => apply(&text, &insertion),
    }
}

const CTOR: &str = "class Foo : public ns::Base<int>, private Other {\npublic:\n    Fo@o(int x = 0);\n    static int s;\n    int a;\n    std::string name{\"x\"};\n    const int& r;\n};\n";

#[test]
fn constructor_initializes_bases_and_members() {
    assert_eq!(
        run_ctor(CTOR, Some(""), Target::MatchingSource, &Config::default()),
        "Foo::Foo(int x)\n    : Base(@),\n      Other(),\n      a(),\n      name(),\n      r()\n{\n    \n}\n"
    );
}

#[test]
fn constructor_braced_initialization() {
    let config = Config::from_value(&json!({ "cpp": { "bracedInitialization": true } }));
    assert_eq!(
        run_ctor(CTOR, Some(""), Target::MatchingSource, &config),
        "Foo::Foo(int x)\n    : Base{@},\n      Other{},\n      a{},\n      name{},\n      r{}\n{\n    \n}\n"
    );
}

#[test]
fn constructor_in_this_file_and_inline() {
    let text = "struct P {\n    P@();\n    int x;\n};\n";
    assert_eq!(
        run_ctor(text, None, Target::CurrentFile, &Config::default()),
        "struct P {\n    P();\n    int x;\n};\n\ninline P::P()\n    : x(@)\n{\n    \n}\n"
    );
    assert_eq!(
        run_ctor(text, None, Target::Inline, &Config::default()),
        "struct P {\n    P()\n        : x(@)\n    {\n        \n    }\n    int x;\n};\n"
    );
}

#[test]
fn constructor_initializers_not_offered() {
    let offered = |text: &str| {
        let (text, offset) = split_cursor(text);
        offers_constructor_initializers(&header(&text), offset_to_position(&text, offset))
    };
    // No bases or non-static members.
    assert!(!offered("struct E {\n    E@();\n    static int s;\n};\n"));
    // Not a constructor.
    assert!(!offered("struct E {\n    void f@();\n    int x;\n};\n"));
    // Destructor.
    assert!(!offered("struct E {\n    ~E@();\n    int x;\n};\n"));
    // Already defined.
    assert!(!offered("struct E {\n    E@() {}\n    int x;\n};\n"));
    // Plain Add Definition keeps an empty body.
    let (text, offset) = split_cursor("struct E {\n    E@();\n    int& r;\n};\n");
    let insertion = add_definition_with(
        &header(&text),
        offset_to_position(&text, offset),
        Target::MatchingSource,
        Some(&source("")),
        Initializers::None,
        &Config::default(),
    )
    .unwrap();
    assert_eq!(insertion.new_text, "E::E()\n{\n    \n}\n");
}

/// Code actions must never panic, wherever the cursor is.
#[test]
fn every_offset_is_safe() {
    let texts = [
        CLASS.replace('@', ""),
        CTOR.replace('@', ""),
        "#ifndef A_H\n#define A_H\n#include <string>\ntemplate <typename T, int N = 3>\nclass Box : public Base<T> {\npublic:\n    Box();\n    explicit Box(const T& t, int n = N);\n    ~Box();\n    template <typename U> U as() const;\n    friend bool operator==(const Box& a, const Box& b);\n    operator bool() const;\n    T& get() &;\nprivate:\n    T value_;\n    int (*fp)(int);\n    union { int i; float f; };\n};\nextern \"C\" int c_api(void);\nnamespace a::b { struct S { S(int); int x, y; }; }\n#endif\n".to_string(),
        "struct Broken : {\n    Broken(\n    int x\n".to_string(),
    ];
    let config = Config::default();
    for text in &texts {
        let decl = header(text);
        let src = source("#include \"a.h\"\nnamespace a::b {\n}\n");
        for (offset, _) in text.char_indices() {
            let position = offset_to_position(text, offset);
            for target in available_targets(&decl, position, Some(&src)) {
                for initializers in [Initializers::None, Initializers::All] {
                    let _ = add_definition_with(
                        &decl,
                        position,
                        target,
                        Some(&src),
                        initializers,
                        &config,
                    );
                }
            }
            let _ = available_bulk_targets(&decl, position, Some(&src));
            let _ = cmantic_ls::actions::may_offer_actions_at(&decl, position);
        }
        // The bulk result doesn't depend on the cursor.
        for target in [BulkTarget::MatchingSource, BulkTarget::CurrentFile] {
            let _ = add_definitions(&decl, target, Some(&src), &config);
        }
    }
}
