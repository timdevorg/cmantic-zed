//! Fixture tests for Move Definition. `@` marks the cursor in the file with the definition.

use cmantic_ls::actions::add_definition::FileInput;
use cmantic_ls::actions::move_definition::{MoveTarget, available_moves, move_definition};
use cmantic_ls::config::Config;
use cmantic_ls::text::{offset_to_position, position_to_offset};
use serde_json::json;
use tower_lsp_server::ls_types::TextEdit;

fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_string();
    for edit in edits.iter().rev() {
        let start = position_to_offset(text, edit.range.start);
        let end = position_to_offset(text, edit.range.end);
        out.replace_range(start..end, &edit.new_text);
    }
    out
}

struct Case<'a> {
    file: &'a str,
    is_header: bool,
    is_c: bool,
    other: Option<&'a str>,
    config: Config,
}

impl<'a> Case<'a> {
    fn header(file: &'a str) -> Self {
        Self {
            file,
            is_header: true,
            is_c: false,
            other: None,
            config: Config::default(),
        }
    }

    fn source(file: &'a str) -> Self {
        Self {
            is_header: false,
            ..Self::header(file)
        }
    }

    fn other(mut self, other: &'a str) -> Self {
        self.other = Some(other);
        self
    }

    fn inputs(&self) -> (String, Position) {
        let offset = self.file.find('@').expect("cursor");
        let text = self.file.replacen('@', "", 1);
        let position = offset_to_position(&text, offset);
        (text, position)
    }

    fn titles(&self) -> Vec<String> {
        let (text, position) = self.inputs();
        let file = FileInput {
            text: &text,
            is_header: self.is_header,
            is_c: self.is_c,
        };
        let other = self.other.map(|text| FileInput {
            text,
            is_header: !self.is_header,
            is_c: false,
        });
        available_moves(&file, position, other.as_ref())
            .into_iter()
            .map(|a| a.title)
            .collect()
    }

    /// Runs `target`; returns the new texts of the current and the other file.
    fn run(&self, target: MoveTarget) -> (String, Option<String>) {
        let (text, position) = self.inputs();
        let file = FileInput {
            text: &text,
            is_header: self.is_header,
            is_c: self.is_c,
        };
        let other = self.other.map(|text| FileInput {
            text,
            is_header: !self.is_header,
            is_c: false,
        });
        let edit = move_definition(&file, position, target, other.as_ref(), &self.config)
            .expect("an edit");
        let other = self
            .other
            .filter(|_| !edit.other_edits.is_empty())
            .map(|o| apply(o, &edit.other_edits));
        (apply(&text, &edit.edits), other)
    }

    fn same_file(&self, target: MoveTarget) -> String {
        let (current, other) = self.run(target);
        assert!(other.is_none(), "{other:?}");
        current
    }

    fn both(&self, target: MoveTarget) -> (String, String) {
        let (current, other) = self.run(target);
        (current, other.expect("the other file changed"))
    }
}

use tower_lsp_server::ls_types::Position;

const HEADER: &str = "#pragma once\n\nnamespace ns {\n\nclass Foo {\npublic:\n    void a();\n    // Doc of b.\n    int b(int x = 1) const\n    {\n        if (x) {\n            return x;\n        }\n        return 0;\n    }\n    void c(int);\n};\n\n}\n";
const SOURCE: &str = "#include \"foo.h\"\n\nnamespace ns {\n\nvoid Foo::a() {}\n\n/// Doc of c.\nvoid Foo::c(int value)\n{\n    use(value);\n}\n\n}\n";

#[test]
fn in_class_definition_to_matching_source_file() {
    let file = HEADER.replace("int b(", "int b@(");
    let case = Case::header(&file).other(SOURCE);
    assert_eq!(
        case.titles(),
        vec![
            "Move Definition to matching source file",
            "Move Definition below class body"
        ]
    );
    let (header, source) = case.both(MoveTarget::MatchingSource);
    assert_eq!(
        header,
        HEADER.replace(
            "    // Doc of b.\n    int b(int x = 1) const\n    {\n        if (x) {\n            return x;\n        }\n        return 0;\n    }\n",
            "    int b(int x = 1) const;\n"
        )
    );
    assert_eq!(
        source,
        SOURCE.replace(
            "void Foo::a() {}\n",
            "void Foo::a() {}\n\n// Doc of b.\nint Foo::b(int x) const\n{\n    if (x) {\n        return x;\n    }\n    return 0;\n}\n"
        )
    );
}

#[test]
fn comments_stay_with_declaration_unless_always_move_comments() {
    let file = HEADER.replace("int b(", "int b@(");
    let mut case = Case::header(&file).other(SOURCE);
    case.config = Config::from_value(&json!({ "alwaysMoveComments": false }));
    let (header, source) = case.both(MoveTarget::MatchingSource);
    assert!(header.contains("    // Doc of b.\n    int b(int x = 1) const;\n"));
    assert!(source.contains("void Foo::a() {}\n\nint Foo::b(int x) const\n{"));
    let out = case.same_file(MoveTarget::OutOfClass);
    assert!(out.contains("    // Doc of b.\n    int b(int x = 1) const;\n"));
    assert!(out.contains("};\n\ninline int Foo::b(int x) const\n{"));
}

#[test]
fn in_class_definition_below_class() {
    let file = HEADER.replace("int b(", "int b@(");
    let case = Case::header(&file).other(SOURCE);
    assert_eq!(
        case.same_file(MoveTarget::OutOfClass),
        "#pragma once\n\nnamespace ns {\n\nclass Foo {\npublic:\n    void a();\n    int b(int x = 1) const;\n    void c(int);\n};\n\n// Doc of b.\ninline int Foo::b(int x) const\n{\n    if (x) {\n        return x;\n    }\n    return 0;\n}\n\n}\n"
    );
}

#[test]
fn source_definition_to_matching_header_and_into_class() {
    let file = SOURCE.replace("Foo::c(", "Foo::c@(");
    let case = Case::source(&file).other(HEADER);
    assert_eq!(
        case.titles(),
        vec![
            "Move Definition to matching header file",
            "Move Definition into class \"Foo\""
        ]
    );
    let removed = SOURCE.replace(
        "\n/// Doc of c.\nvoid Foo::c(int value)\n{\n    use(value);\n}\n",
        "",
    );
    let (source, header) = case.both(MoveTarget::MatchingHeader);
    assert_eq!(source, removed);
    assert_eq!(
        header,
        HEADER.replace(
            "};\n\n}\n",
            "};\n\n/// Doc of c.\ninline void Foo::c(int value)\n{\n    use(value);\n}\n\n}\n"
        )
    );
    // Into the class: the declaration's head with the definition's parameter names.
    let (source, header) = case.both(MoveTarget::IntoClass);
    assert_eq!(source, removed);
    assert_eq!(
        header,
        HEADER.replace(
            "    void c(int);\n",
            "    /// Doc of c.\n    void c(int value)\n    {\n        use(value);\n    }\n"
        )
    );
}

#[test]
fn header_definition_below_class_into_class_or_source() {
    let header = "#pragma once\n\nstruct S {\n    // Declared.\n    virtual int f(int a = 0) const;\n};\n\n// Defined.\ninline int S::f@(int b) const\n{\n    return b;\n}\n";
    let case = Case::header(header).other("#include \"s.h\"\n");
    assert_eq!(
        case.titles(),
        vec![
            "Move Definition to matching source file",
            "Move Definition into struct \"S\""
        ]
    );
    // Both comments are kept.
    assert_eq!(
        case.same_file(MoveTarget::IntoClass),
        "#pragma once\n\nstruct S {\n    // Declared.\n    // Defined.\n    virtual int f(int b = 0) const\n    {\n        return b;\n    }\n};\n"
    );
    // `inline` is dropped in the source file; the declaration stays.
    let (header, source) = case.both(MoveTarget::MatchingSource);
    assert_eq!(
        header,
        "#pragma once\n\nstruct S {\n    // Declared.\n    virtual int f(int a = 0) const;\n};\n"
    );
    assert_eq!(
        source,
        "#include \"s.h\"\n\n// Defined.\nint S::f(int b) const\n{\n    return b;\n}\n"
    );
}

#[test]
fn free_function_from_header_leaves_declaration() {
    let header = "#pragma once\nnamespace ns {\n\n// Adds.\nint ad@d(int a, int b = 2) { return a + b; }\n\n}\n";
    let source = "#include \"a.h\"\n";
    let case = Case::header(header).other(source);
    assert_eq!(
        case.titles(),
        vec!["Move Definition to matching source file"]
    );
    let (header, source) = case.both(MoveTarget::MatchingSource);
    assert_eq!(
        header,
        "#pragma once\nnamespace ns {\n\nint add(int a, int b = 2);\n\n}\n"
    );
    assert_eq!(
        source,
        "#include \"a.h\"\n\n// Adds.\nint ns::add(int a, int b) { return a + b; }\n"
    );
}

#[test]
fn free_function_from_source_to_header_gets_inline() {
    let file = "#include \"a.h\"\n\nint tw@ice(int x)\n{\n    return 2 * x;\n}\n\nint main() {}\n";
    let case = Case::source(file).other("#pragma once\n\nint other();\n");
    assert_eq!(
        case.titles(),
        vec!["Move Definition to matching header file"]
    );
    let (source, header) = case.both(MoveTarget::MatchingHeader);
    assert_eq!(source, "#include \"a.h\"\n\nint main() {}\n");
    assert_eq!(
        header,
        "#pragma once\n\nint other();\n\ninline int twice(int x)\n{\n    return 2 * x;\n}\n"
    );
    // Not for internal functions, or without a matching header.
    for internal in [
        "static int tw@ice() { return 2; }\n",
        "namespace {\nint tw@ice() { return 2; }\n}\n",
    ] {
        assert!(Case::source(internal).other("").titles().is_empty());
    }
    assert!(Case::source(file).titles().is_empty());
}

#[test]
fn not_offered_to_source_for_inline_constexpr_and_templates() {
    let source = "#include \"a.h\"\n";
    for header in [
        "inline int f@() { return 1; }\n",
        "constexpr int f@() { return 1; }\n",
        "consteval int f@() { return 1; }\n",
        "template <typename T>\nT f@(T t) { return t; }\n",
        "static int f@() { return 1; }\n",
        "void Unknown::f@() {}\n",
    ] {
        assert!(
            Case::header(header).other(source).titles().is_empty(),
            "{header}"
        );
    }
    // A class template: only below the class.
    let header =
        "template <typename T>\nclass Box {\n    T get@() const { return t; }\n    T t;\n};\n";
    let case = Case::header(header).other(source);
    assert_eq!(case.titles(), vec!["Move Definition below class body"]);
    assert_eq!(
        case.same_file(MoveTarget::OutOfClass),
        "template <typename T>\nclass Box {\n    T get() const;\n    T t;\n};\n\ntemplate <typename T>\ninline T Box<T>::get() const { return t; }\n"
    );
    // An inline definition with a non-inline declaration may move (dropping `inline`).
    let header = "struct S { void f(); };\ninline void S::f@() {}\n";
    assert_eq!(
        Case::header(header).other(source).titles()[0],
        "Move Definition to matching source file"
    );
}

#[test]
fn class_template_member_into_class() {
    let header = "template <typename T>\nclass Box {\npublic:\n    T get() const;\n};\n\ntemplate <typename T>\nT Box<T>::ge@t() const\n{\n    return T();\n}\n";
    let case = Case::header(header);
    assert_eq!(case.titles(), vec!["Move Definition into class \"Box\""]);
    assert_eq!(
        case.same_file(MoveTarget::IntoClass),
        "template <typename T>\nclass Box {\npublic:\n    T get() const\n    {\n        return T();\n    }\n};\n"
    );
}

#[test]
fn into_class_without_declaration() {
    let file = "class Foo {\npublic:\n    void a();\nprivate:\n    int m;\n};\n\nvoid Foo::a() {}\n\n// New.\nint Foo::b@() const\n{\n    return m;\n}\n";
    let case = Case::source(file);
    assert_eq!(case.titles(), vec!["Move Definition into class \"Foo\""]);
    assert_eq!(
        case.same_file(MoveTarget::IntoClass),
        "class Foo {\npublic:\n    void a();\n    // New.\n    int b() const\n    {\n        return m;\n    }\nprivate:\n    int m;\n};\n\nvoid Foo::a() {}\n"
    );
    // The class is in the matching header.
    let header = "#pragma once\nclass Foo {\npublic:\n    void a();\n};\n";
    let source = "#include \"foo.h\"\n\nvoid Foo::a() {}\n\nvoid Foo::b@(int x) { use(x); }\n";
    let (source, header) = Case::source(source)
        .other(header)
        .both(MoveTarget::IntoClass);
    assert_eq!(source, "#include \"foo.h\"\n\nvoid Foo::a() {}\n");
    assert_eq!(
        header,
        "#pragma once\nclass Foo {\npublic:\n    void a();\n    void b(int x) { use(x); }\n};\n"
    );
}

#[test]
fn namespace_blocks_and_qualified_names() {
    // Qualified definition in the source, namespace block in the header.
    let header = "#pragma once\nnamespace a::b {\nclass C {\n    void f();\n};\n}\n";
    let source = "#include \"c.h\"\nvoid a::b::C::f@() { x(); }\n";
    let (source, header) = Case::source(source)
        .other(header)
        .both(MoveTarget::MatchingHeader);
    assert_eq!(source, "#include \"c.h\"\n");
    assert_eq!(
        header,
        "#pragma once\nnamespace a::b {\nclass C {\n    void f();\n};\n\ninline void C::f() { x(); }\n}\n"
    );
    // Namespace block in the header, none in the source.
    let header = "#pragma once\nnamespace n {\nstruct S {\n    void f@() {}\n};\n}\n";
    let (_, source) = Case::header(header)
        .other("#include \"s.h\"\n")
        .both(MoveTarget::MatchingSource);
    assert_eq!(source, "#include \"s.h\"\n\nvoid n::S::f() {}\n");
}

#[test]
fn nested_class() {
    let header = "class Outer {\npublic:\n    struct Inner {\n        int g@() const { return 1; }\n    };\n};\n";
    let case = Case::header(header);
    assert_eq!(case.titles(), vec!["Move Definition below struct body"]);
    assert_eq!(
        case.same_file(MoveTarget::OutOfClass),
        "class Outer {\npublic:\n    struct Inner {\n        int g() const;\n    };\n};\n\ninline int Outer::Inner::g() const { return 1; }\n"
    );
    // And back in.
    let moved = "class Outer {\npublic:\n    struct Inner {\n        int g() const;\n    };\n};\n\ninline int Outer::Inner::g@() const { return 1; }\n";
    assert_eq!(
        Case::header(moved).same_file(MoveTarget::IntoClass),
        "class Outer {\npublic:\n    struct Inner {\n        int g() const { return 1; }\n    };\n};\n"
    );
}

#[test]
fn mixed_indentation_and_constructor_initializers() {
    // The body has a tab-indented line and a line indented less than the function.
    let header = "struct P {\n    P@(int x)\n        : m(x)\n    {\n\tif (x)\n            go();\n  // odd\n    }\n    int m;\n};\n";
    let (header, source) = Case::header(header)
        .other("#include \"p.h\"\n")
        .both(MoveTarget::MatchingSource);
    assert_eq!(header, "struct P {\n    P(int x);\n    int m;\n};\n");
    assert_eq!(
        source,
        "#include \"p.h\"\n\nP::P(int x)\n    : m(x)\n{\n\tif (x)\n        go();\n// odd\n}\n"
    );
}

#[test]
fn crlf_files() {
    let header =
        "#pragma once\r\nstruct S {\r\n    void f@()\r\n    {\r\n        x();\r\n    }\r\n};\r\n";
    let source = "#include \"s.h\"\r\n";
    let (header, source) = Case::header(header)
        .other(source)
        .both(MoveTarget::MatchingSource);
    assert_eq!(
        header,
        "#pragma once\r\nstruct S {\r\n    void f();\r\n};\r\n"
    );
    assert_eq!(
        source,
        "#include \"s.h\"\r\n\r\nvoid S::f()\r\n{\r\n    x();\r\n}\r\n"
    );
    let back = "#include \"s.h\"\r\n\r\nvoid S::f@()\r\n{\r\n    x();\r\n}\r\n";
    let (source, header) = Case::source(back)
        .other("#pragma once\r\nstruct S {\r\n    void f();\r\n};\r\n")
        .both(MoveTarget::IntoClass);
    assert_eq!(source, "#include \"s.h\"\r\n");
    assert_eq!(
        header,
        "#pragma once\r\nstruct S {\r\n    void f()\r\n    {\r\n        x();\r\n    }\r\n};\r\n"
    );
}

#[test]
fn not_offered() {
    // On the body.
    assert!(
        Case::header("struct S {\n    void f() {\n        @x();\n    }\n};\n")
            .titles()
            .is_empty()
    );
    // Anonymous class.
    assert!(
        Case::header("struct {\n    void f@() {}\n} s;\n")
            .titles()
            .is_empty()
    );
    // C has no classes.
    let mut case = Case::header("int f@(void) { return 0; }\n").other("#include \"a.h\"\n");
    case.is_c = true;
    assert_eq!(
        case.titles(),
        vec!["Move Definition to matching source file"]
    );
    // A source file whose declaration is in the same file.
    assert!(
        Case::source("void f();\nvoid f@() {}\n")
            .other("#pragma once\n")
            .titles()
            .is_empty()
    );
    // A hidden friend of a class template.
    assert!(
        Case::header("template <class T>\nstruct B {\n    friend bool operator=@=(B, B) { return true; }\n};\n")
            .other("")
            .titles()
            .is_empty()
    );
    // A hidden friend of a class moves out as a free function.
    assert_eq!(
        Case::header("struct B {\n    friend bool operator=@=(B, B) { return true; }\n};\n")
            .same_file(MoveTarget::OutOfClass),
        "struct B {\n    friend bool operator==(B, B);\n};\n\ninline bool operator==(B, B) { return true; }\n"
    );
    // Defaulted functions aren't definitions.
    assert!(
        Case::header("struct S {\n    S@() = default;\n};\n")
            .titles()
            .is_empty()
    );
}

#[test]
fn every_offset_is_safe() {
    let texts = [
        (HEADER, true, SOURCE),
        (SOURCE, false, HEADER),
        (
            "#ifndef A_H\n#define A_H\ntemplate <typename T>\nstruct Box : Base<T> {\n    Box() : Base<T>() {}\n    template <typename U> U as() const { return U(); }\n    friend bool operator==(const Box&, const Box&) { return true; }\n    operator bool() const { return true; }\n    T& get() & { return t; }\n    struct In { void f() {} };\n    T t;\n};\nvoid Box<int>::g() {}\ninline int free_f(int x = 1) { return x; }\nstruct { void h() {} } anon;\nnamespace { void hidden() {} }\nauto t() -> int { return 1; }\nvoid broken(\n#endif\n",
            true,
            "#include \"a.h\"\r\nnamespace a::b {\r\n}\r\n",
        ),
        (
            "#include \"a.h\"\r\nint Box<int>::g() { return 0; }\r\nvoid free_f() try {} catch (...) {}\r\nstatic void s() {}\r\n",
            false,
            "struct Box { int g(); };\n",
        ),
    ];
    let config = Config::default();
    for (text, is_header, other_text) in texts {
        let file = FileInput {
            text,
            is_header,
            is_c: false,
        };
        let other = FileInput {
            text: other_text,
            is_header: !is_header,
            is_c: false,
        };
        let mut previous = Vec::new();
        for (offset, _) in text.char_indices() {
            let position = offset_to_position(text, offset);
            for other in [None, Some(&other)] {
                let offered = available_moves(&file, position, other);
                if offered != previous || other.is_none() {
                    for action in &offered {
                        assert!(
                            move_definition(&file, position, action.target, other, &config)
                                .is_some(),
                            "{} at {offset} in {text:?}",
                            action.title
                        );
                    }
                }
                if other.is_some() {
                    previous = offered;
                }
            }
        }
    }
}
