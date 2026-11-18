//! Fixture tests for Add Declaration. `@` marks the cursor in the file with the definition.

use cmantic_ls::actions::add_declaration::{
    DeclarationTarget, add_declaration, available_declarations,
};
use cmantic_ls::actions::add_definition::FileInput;
use cmantic_ls::config::Config;
use cmantic_ls::text::{offset_to_position, position_to_offset};
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
    file_is_header: bool,
    header: Option<&'a str>,
}

impl<'a> Case<'a> {
    /// A source file (`.cpp`) with the cursor.
    fn source(file: &'a str) -> Self {
        Self {
            file,
            file_is_header: false,
            header: None,
        }
    }

    fn in_header(file: &'a str) -> Self {
        Self {
            file,
            file_is_header: true,
            header: None,
        }
    }

    fn header(mut self, header: &'a str) -> Self {
        self.header = Some(header);
        self
    }

    fn inputs(&self) -> (String, usize) {
        let offset = self.file.find('@').expect("cursor");
        (self.file.replacen('@', "", 1), offset)
    }

    fn titles(&self) -> Vec<String> {
        let (text, offset) = self.inputs();
        let file = FileInput {
            text: &text,
            is_header: self.file_is_header,
            is_c: false,
        };
        let header = self.header.map(|text| FileInput {
            text,
            is_header: true,
            is_c: false,
        });
        available_declarations(&file, offset_to_position(&text, offset), header.as_ref())
            .into_iter()
            .map(|a| a.title)
            .collect()
    }

    /// Runs `target`; returns the new text of the current file and of the header.
    fn run(&self, target: DeclarationTarget) -> (String, Option<String>) {
        let (text, offset) = self.inputs();
        let file = FileInput {
            text: &text,
            is_header: self.file_is_header,
            is_c: false,
        };
        let header = self.header.map(|text| FileInput {
            text,
            is_header: true,
            is_c: false,
        });
        let edit = add_declaration(
            &file,
            offset_to_position(&text, offset),
            target,
            header.as_ref(),
            &Config::default(),
        )
        .expect("an edit");
        let header = self
            .header
            .filter(|_| !edit.other_edits.is_empty())
            .map(|h| apply(h, &edit.other_edits));
        (apply(&text, &edit.edits), header)
    }

    fn header_result(&self, target: DeclarationTarget) -> String {
        let (file, header) = self.run(target);
        assert_eq!(file, self.inputs().0, "the current file is unchanged");
        header.expect("the header changed")
    }

    fn file_result(&self, target: DeclarationTarget) -> String {
        let (file, header) = self.run(target);
        assert!(header.is_none());
        file
    }
}

const HEADER: &str = "#pragma once\n\nnamespace ns {\n\nclass Foo {\npublic:\n    Foo();\n    void a();\n    void c();\n\nprivate:\n    int m_x;\n};\n\nvoid free_a();\n\n}\n";

#[test]
fn member_goes_next_to_neighbour_declaration() {
    let source = "#include \"foo.h\"\n\nnamespace ns {\n\nvoid Foo::a() {}\n\nint Foo::b@(int x) const\n{\n    return x;\n}\n\n}\n";
    let case = Case::source(source).header(HEADER);
    assert_eq!(case.titles(), vec!["Add Declaration in class \"Foo\""]);
    assert_eq!(
        case.header_result(DeclarationTarget::Class),
        HEADER.replace(
            "    void a();\n",
            "    void a();\n    int b(int x) const;\n"
        )
    );
}

#[test]
fn member_before_following_neighbour_and_qualified_definition() {
    let source = "#include \"foo.h\"\n\nvoid ns::Foo::b@() {}\nvoid ns::Foo::c() {}\n";
    let case = Case::source(source).header(HEADER);
    assert_eq!(
        case.header_result(DeclarationTarget::Class),
        HEADER.replace("    void c();\n", "    void b();\n    void c();\n")
    );
}

#[test]
fn member_without_neighbours_goes_after_last_public_member() {
    let source = "#include \"foo.h\"\n\nstatic int helper() { return 1; }\n\nconst std::string &ns::Foo::name@() const { return n; }\n";
    let case = Case::source(source).header(HEADER);
    assert_eq!(
        case.header_result(DeclarationTarget::Class),
        HEADER.replace(
            "    void c();\n",
            "    void c();\n    const std::string &name() const;\n"
        )
    );
}

#[test]
fn member_of_class_without_public_section_gets_label() {
    let header = "#pragma once\nclass Foo {\n    int m_x;\n};\n";
    let source = "#include \"foo.h\"\nvoid Foo::f@(int a,\n            int b) {}\n";
    let case = Case::source(source).header(header);
    assert_eq!(
        case.header_result(DeclarationTarget::Class),
        "#pragma once\nclass Foo {\n    int m_x;\npublic:\n    void f(int a,\n           int b);\n};\n"
    );
}

#[test]
fn member_of_class_in_same_file_and_struct_title() {
    let file = "struct S {\n    int x;\n};\n\ninline void S::f@() {}\n";
    let case = Case::in_header(file);
    assert_eq!(case.titles(), vec!["Add Declaration in struct \"S\""]);
    assert_eq!(
        case.file_result(DeclarationTarget::Class),
        "struct S {\n    int x;\n    void f();\n};\n\ninline void S::f() {}\n"
    );
}

#[test]
fn member_of_class_template_and_nested_class() {
    let file = "template <typename T>\nclass Box {\npublic:\n    struct Inner {\n    };\n};\n\ntemplate <typename T>\ntemplate <typename U>\nU Box<T>::as@() const { return U(); }\n\ntemplate <typename T>\nvoid Box<T>::Inner::g() {}\n";
    let case = Case::in_header(file);
    assert_eq!(case.titles(), vec!["Add Declaration in class \"Box\""]);
    let result = case.file_result(DeclarationTarget::Class);
    assert!(
        result.starts_with("template <typename T>\nclass Box {\npublic:\n    struct Inner {\n    };\n    template <typename U>\n    U as() const;\n};"),
        "{result}"
    );
    let nested = file.replace("as@()", "as()").replace("::g()", "::g@()");
    let case = Case::in_header(&nested);
    assert_eq!(case.titles(), vec!["Add Declaration in struct \"Inner\""]);
    assert!(
        case.file_result(DeclarationTarget::Class)
            .contains("    struct Inner {\n        void g();\n    };")
    );
}

#[test]
fn not_offered_when_declared_inside_class_or_unknown_class() {
    let source = "#include \"foo.h\"\nvoid ns::Foo::a@() {}\n";
    assert!(Case::source(source).header(HEADER).titles().is_empty());
    // In-class definitions.
    assert!(
        Case::in_header("class A {\n    void f@() {}\n};\n")
            .titles()
            .is_empty()
    );
    // The class can't be found.
    assert!(
        Case::source("void Unknown::f@() {}\n")
            .header(HEADER)
            .titles()
            .is_empty()
    );
    // A declaration in the header.
    let source = "#include \"foo.h\"\nnamespace ns {\nvoid free_a@() {}\n}\n";
    assert!(Case::source(source).header(HEADER).titles().is_empty());
    // Not on the body.
    assert!(
        Case::source("void f() {\n    @x();\n}\n")
            .header(HEADER)
            .titles()
            .is_empty()
    );
    // Not for main.
    assert!(Case::source("int ma@in() {}\n").titles().is_empty());
}

#[test]
fn free_function_into_matching_namespace_next_to_sibling() {
    let source = "#include \"foo.h\"\n\nnamespace ns {\n\nvoid free_a() {}\n\n// Doc.\nint free_b@(int x = 2)\n{\n    return x;\n}\n\n}\n";
    let case = Case::source(source).header(HEADER);
    assert_eq!(
        case.titles(),
        vec![
            "Add Declaration in matching header file",
            "Add Declaration in this file"
        ]
    );
    assert_eq!(
        case.header_result(DeclarationTarget::MatchingHeader),
        HEADER.replace("void free_a();\n", "void free_a();\nint free_b(int x);\n")
    );
}

#[test]
fn free_function_into_namespace_block() {
    let header = "#pragma once\nnamespace ns {\nint x();\nclass Foo {\n    void a();\n};\n}\n";
    let source = "namespace ns {\nvoid Foo::a() {}\nvoid g@() {}\n}\n";
    assert_eq!(
        Case::source(source)
            .header(header)
            .header_result(DeclarationTarget::MatchingHeader),
        "#pragma once\nnamespace ns {\nint x();\nclass Foo {\n    void a();\n};\n\nvoid g();\n}\n"
    );
}

#[test]
fn free_function_with_missing_namespace_is_wrapped() {
    let header = "#ifndef FOO_H\n#define FOO_H\n\nnamespace a {\nint x;\n}\n\n#endif\n";
    let source = "namespace a::b {\nvoid g@() {}\n}\n";
    assert_eq!(
        Case::source(source)
            .header(header)
            .header_result(DeclarationTarget::MatchingHeader),
        "#ifndef FOO_H\n#define FOO_H\n\nnamespace a {\nint x;\n\nnamespace b {\nvoid g();\n}\n}\n\n#endif\n"
    );
    let source = "namespace c {\nvoid g@() {}\n}\n";
    assert_eq!(
        Case::source(source)
            .header(header)
            .header_result(DeclarationTarget::MatchingHeader),
        "#ifndef FOO_H\n#define FOO_H\n\nnamespace a {\nint x;\n}\n\nnamespace c {\nvoid g();\n}\n\n#endif\n"
    );
}

#[test]
fn free_function_empty_header_and_crlf() {
    let source = "void g@(int a) {}\r\n";
    assert_eq!(
        Case::source(source)
            .header("")
            .header_result(DeclarationTarget::MatchingHeader),
        "void g(int a);\n"
    );
    let header = "#pragma once\r\nint x();\r\n";
    assert_eq!(
        Case::source(source)
            .header(header)
            .header_result(DeclarationTarget::MatchingHeader),
        "#pragma once\r\nint x();\r\n\r\nvoid g(int a);\r\n"
    );
}

#[test]
fn internal_functions_only_in_this_file() {
    let source = "static int helper@() { return 1; }\n";
    let case = Case::source(source).header(HEADER);
    assert_eq!(case.titles(), vec!["Add Declaration in this file"]);
    // `static` is kept for the forward declaration.
    assert_eq!(
        case.file_result(DeclarationTarget::CurrentFile),
        "static int helper();\nstatic int helper() { return 1; }\n"
    );
    let source = "namespace {\nint helper@() { return 1; }\n}\n";
    assert_eq!(
        Case::source(source).header(HEADER).titles(),
        vec!["Add Declaration in this file"]
    );
}

#[test]
fn this_file_before_first_use() {
    let source = "#include <cstdio>\n\nvoid a()\n{\n    b(); // uses b\n}\n\n// Says hi.\ntemplate <typename T>\nT b@(T x = T())\n{\n    return x;\n}\n";
    let case = Case::source(source);
    assert_eq!(case.titles(), vec!["Add Declaration in this file"]);
    assert_eq!(
        case.file_result(DeclarationTarget::CurrentFile),
        "#include <cstdio>\n\ntemplate <typename T>\nT b(T x);\n\nvoid a()\n{\n    b(); // uses b\n}\n\n// Says hi.\ntemplate <typename T>\nT b(T x = T())\n{\n    return x;\n}\n"
    );
}

#[test]
fn this_file_next_to_sibling_declaration_or_before_definition() {
    let source = "void a();\n\nvoid a() {}\nvoid b@() {}\n";
    assert_eq!(
        Case::source(source).file_result(DeclarationTarget::CurrentFile),
        "void a();\nvoid b();\n\nvoid a() {}\nvoid b() {}\n"
    );
    let source = "namespace n {\n    int x;\n\n    inline void f@() {}\n}\n";
    assert_eq!(
        Case::in_header(source).file_result(DeclarationTarget::CurrentFile),
        "namespace n {\n    int x;\n\n    void f();\n\n    inline void f() {}\n}\n"
    );
    // Already declared in this file.
    assert!(
        Case::source("void a();\nvoid a@() {}\n")
            .titles()
            .is_empty()
    );
}

#[test]
fn every_offset_is_safe() {
    let header = FileInput {
        text: HEADER,
        is_header: true,
        is_c: false,
    };
    let texts = [
        "#include \"foo.h\"\nnamespace ns {\nvoid Foo::a() {}\nint Foo::b(int x) const { return x; }\nvoid free_b() {}\n}\nstatic void s() {}\ntemplate <typename T> void Box<T>::f() {}\nns::Foo::Foo()\n    : m_x(1)\n{\n}\nvoid broken(\n",
        "struct S { void f() {} int g(); };\nint S::g() { return 0; }\nvoid S::h() const & {}\nnamespace { void anon() {} }\r\nauto t() -> int { return 1; }\r\n",
    ];
    for text in texts {
        for is_header in [false, true] {
            let file = FileInput {
                text,
                is_header,
                is_c: false,
            };
            for (offset, _) in text.char_indices() {
                let position = offset_to_position(text, offset);
                for action in available_declarations(&file, position, Some(&header)) {
                    assert!(
                        add_declaration(
                            &file,
                            position,
                            action.target,
                            Some(&header),
                            &Config::default()
                        )
                        .is_some(),
                        "{} at {offset}",
                        action.title
                    );
                }
            }
        }
    }
}
