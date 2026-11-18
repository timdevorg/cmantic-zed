//! Fixture tests for Add Definition.
//!
//! Each case is a header (with `@` marking the cursor), an optional source file, and the
//! chosen action. Expected texts mark the cursor after the edit with `@`.

use cmantic_ls::actions::add_definition::{
    FileInput, Insertion, Target, add_definition, available_targets,
};
use cmantic_ls::config::Config;
use cmantic_ls::text::{offset_to_position, position_to_offset};
use serde_json::json;

const CURSOR: char = '@';

fn split_cursor(text: &str) -> (String, usize) {
    let offset = text.find(CURSOR).expect("fixture has a cursor marker");
    (text.replacen(CURSOR, "", 1), offset)
}

/// Applies the insertion and marks the resulting cursor with `@`.
fn apply(text: &str, insertion: &Insertion) -> String {
    let start = position_to_offset(text, insertion.range.start);
    let end = position_to_offset(text, insertion.range.end);
    let mut result = format!("{}{}{}", &text[..start], insertion.new_text, &text[end..]);
    let cursor = position_to_offset(&result, insertion.cursor);
    result.insert(cursor, CURSOR);
    result
}

struct Case<'a> {
    header: &'a str,
    source: Option<&'a str>,
    header_is_c: bool,
    config: Config,
}

impl<'a> Case<'a> {
    fn new(header: &'a str) -> Self {
        Self {
            header,
            source: None,
            header_is_c: false,
            config: Config::default(),
        }
    }

    fn source(mut self, source: &'a str) -> Self {
        self.source = Some(source);
        self
    }

    fn targets(&self) -> Vec<Target> {
        let (header, offset) = split_cursor(self.header);
        let decl = FileInput {
            text: &header,
            is_header: true,
            is_c: self.header_is_c,
        };
        let source = self.source.map(|text| FileInput {
            text,
            is_header: false,
            is_c: false,
        });
        available_targets(&decl, offset_to_position(&header, offset), source.as_ref())
    }

    /// Runs `target` and returns the resulting text of the changed file.
    fn run(&self, target: Target) -> String {
        let (header, offset) = split_cursor(self.header);
        let decl = FileInput {
            text: &header,
            is_header: true,
            is_c: self.header_is_c,
        };
        let source = self.source.map(|text| FileInput {
            text,
            is_header: false,
            is_c: false,
        });
        let position = offset_to_position(&header, offset);
        assert!(
            available_targets(&decl, position, source.as_ref()).contains(&target),
            "{target:?} is not offered"
        );
        let insertion = add_definition(&decl, position, target, source.as_ref(), &self.config)
            .expect("an edit");
        match target {
            Target::MatchingSource => apply(self.source.unwrap(), &insertion),
            _ => apply(&header, &insertion),
        }
    }
}

#[test]
fn free_function_in_namespace_to_empty_source() {
    let case = Case::new("#pragma once\n\nnamespace ns {\n\nvoid fo@o(int a, int b = 2);\n\n}\n")
        .source("#include \"foo.h\"\n");
    assert_eq!(
        case.targets(),
        vec![Target::MatchingSource, Target::CurrentFile]
    );
    assert_eq!(
        case.run(Target::MatchingSource),
        "#include \"foo.h\"\n\nvoid ns::foo(int a, int b) {\n    @\n}\n"
    );
}

#[test]
fn empty_source_file() {
    let case = Case::new("void fo@o();\n").source("");
    assert_eq!(case.run(Target::MatchingSource), "void foo() {\n    @\n}\n");
}

#[test]
fn member_placed_after_preceding_sibling_definition() {
    let header =
        "#pragma once\n\nclass Foo {\npublic:\n    void a();\n    void b@();\n    void c();\n};\n";
    let source = "#include \"foo.h\"\n\nvoid Foo::a() {\n}\n\nvoid Foo::c() {\n}\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "#include \"foo.h\"\n\nvoid Foo::a() {\n}\n\nvoid Foo::b() {\n    @\n}\n\nvoid Foo::c() {\n}\n"
    );
}

#[test]
fn member_placed_before_following_sibling_definition() {
    let header = "class Foo {\npublic:\n    void a@();\n    void b();\n    void c();\n};\n";
    let source = "#include \"foo.h\"\n\n// Does c.\nvoid Foo::c() {\n}\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "#include \"foo.h\"\n\nvoid Foo::a() {\n    @\n}\n\n// Does c.\nvoid Foo::c() {\n}\n"
    );
}

#[test]
fn compact_sibling_definitions_stay_compact() {
    let header = "struct S {\n    int a();\n    int b@();\n    int c();\n};\n";
    let source = "int S::a() { return 1; }\nint S::c() { return 3; }\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "int S::a() { return 1; }\nint S::b() {\n    @\n}\nint S::c() { return 3; }\n"
    );
}

#[test]
fn source_with_namespace_block() {
    let header = "namespace ns {\nclass Foo {\npublic:\n    int get@() const noexcept;\n};\n}\n";
    let source = "#include \"foo.h\"\n\nnamespace ns {\n\nint x = 0;\n\n}\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "#include \"foo.h\"\n\nnamespace ns {\n\nint x = 0;\n\nint Foo::get() const noexcept {\n    @\n}\n\n}\n"
    );
}

#[test]
fn source_with_empty_namespace_block() {
    let header = "namespace ns {\nclass Foo {\npublic:\n    int get@();\n};\n}\n";
    let source = "#include \"foo.h\"\n\nnamespace ns {\n}\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "#include \"foo.h\"\n\nnamespace ns {\n\nint Foo::get() {\n    @\n}\n}\n"
    );
}

#[test]
fn source_with_indented_namespace_block() {
    let header = "namespace ns {\n    void a();\n    void b@();\n}\n";
    let source = "namespace ns {\n  void a() {\n  }\n}\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "namespace ns {\n  void a() {\n  }\n\n  void b() {\n    @\n  }\n}\n"
    );
}

#[test]
fn source_with_fully_qualified_definitions() {
    let header =
        "namespace ns {\nclass Foo {\npublic:\n    void a();\n    void b@(int x);\n};\n}\n";
    let source = "#include \"foo.h\"\n\nvoid ns::Foo::a()\n{\n}\n";
    let config =
        Config::from_value(&json!({ "cpp": { "curlyBraceFormat": { "function": "newLine" } } }));
    let mut case = Case::new(header).source(source);
    case.config = config;
    assert_eq!(
        case.run(Target::MatchingSource),
        "#include \"foo.h\"\n\nvoid ns::Foo::a()\n{\n}\n\nvoid ns::Foo::b(int x)\n{\n    @\n}\n"
    );
}

#[test]
fn qualifiers_are_kept() {
    let header = "class Foo {\npublic:\n    int& ref@() &&;\n    const int& cref() const&;\n    void n() noexcept(true);\n};\n";
    let source = "";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "int& Foo::ref() && {\n    @\n}\n"
    );
    let header = "class Foo {\npublic:\n    const int& cre@f() const&;\n};\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "const int& Foo::cref() const& {\n    @\n}\n"
    );
    let header = "class Foo {\npublic:\n    void @n() const noexcept(true);\n};\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "void Foo::n() const noexcept(true) {\n    @\n}\n"
    );
}

#[test]
fn default_arguments_are_stripped() {
    let header = "#include <map>\nstruct S {\n    void f@(int a = 1, std::map<int, int> m = {}, int (*fp)(int) = nullptr, const char* s = \"a,b\");\n};\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "void S::f(int a, std::map<int, int> m, int (*fp)(int), const char* s) {\n    @\n}\n"
    );
}

#[test]
fn specifiers_are_stripped() {
    let source = "";
    let cases = [
        (
            "    virtual int g@() const override;",
            "int Base::g() const {\n    @\n}\n",
        ),
        (
            "    static Base* ma@ke();",
            "Base* Base::make() {\n    @\n}\n",
        ),
        ("    void fin@() final;", "void Base::fin() {\n    @\n}\n"),
        (
            "    explicit Ba@se(int x);",
            "Base::Base(int x)\n{\n    @\n}\n",
        ),
        (
            "    [[nodiscard]] virtual bool o@k() const;",
            "[[nodiscard]] bool Base::ok() const {\n    @\n}\n",
        ),
    ];
    for (member, expected) in cases {
        let header = format!("class Base : public Other {{\npublic:\n{member}\n}};\n");
        assert_eq!(
            Case::new(&header)
                .source(source)
                .run(Target::MatchingSource),
            expected,
            "{member}"
        );
    }
}

#[test]
fn class_template_member_only_in_header() {
    let header = "#pragma once\n\ntemplate <typename T, int N = 4>\nclass Foo {\npublic:\n    T ge@t(int i) const;\n};\n";
    let case = Case::new(header).source("#include \"foo.h\"\n");
    assert_eq!(case.targets(), vec![Target::CurrentFile, Target::Inline]);
    assert_eq!(
        case.run(Target::CurrentFile),
        "#pragma once\n\ntemplate <typename T, int N = 4>\nclass Foo {\npublic:\n    T get(int i) const;\n};\n\ntemplate <typename T, int N>\ninline T Foo<T, N>::get(int i) const {\n    @\n}\n"
    );
}

#[test]
fn member_function_template_of_class_template() {
    let header = "template <class T>\nstruct Box {\n    template <typename U>\n    U co@nvert() const;\n};\n";
    assert_eq!(
        Case::new(header).run(Target::CurrentFile),
        "template <class T>\nstruct Box {\n    template <typename U>\n    U convert() const;\n};\n\ntemplate <class T>\ntemplate <typename U>\ninline U Box<T>::convert() const {\n    @\n}\n"
    );
}

#[test]
fn inline_constexpr_and_templates_are_not_offered_for_source() {
    for member in [
        "    inline int f@();",
        "    constexpr int f@();",
        "    consteval int f@();",
        "    template <typename U> void f@(U u);",
    ] {
        let header = format!("struct S {{\n{member}\n}};\n");
        assert_eq!(
            Case::new(&header).source("").targets(),
            vec![Target::CurrentFile, Target::Inline],
            "{member}"
        );
    }
}

#[test]
fn nested_class() {
    let header = "namespace ns {\nclass Outer {\n    class Inner {\n        void f@(int x);\n    };\n};\n}\n";
    assert_eq!(
        Case::new(header)
            .source("#include \"a.h\"\n")
            .run(Target::MatchingSource),
        "#include \"a.h\"\n\nvoid ns::Outer::Inner::f(int x) {\n    @\n}\n"
    );
}

#[test]
fn operator_overloads() {
    let source = "";
    let cases = [
        (
            "    bool operator=@=(const Foo& other) const;",
            "bool Foo::operator==(const Foo& other) const {\n    @\n}\n",
        ),
        (
            "    explicit operator bo@ol() const;",
            "Foo::operator bool() const {\n    @\n}\n",
        ),
        (
            "    Foo& operator@()(int a);",
            "Foo& Foo::operator()(int a) {\n    @\n}\n",
        ),
        (
            "    Foo& op@erator<<(int a);",
            "Foo& Foo::operator<<(int a) {\n    @\n}\n",
        ),
    ];
    for (member, expected) in cases {
        let header = format!("class Foo {{\npublic:\n{member}\n}};\n");
        assert_eq!(
            Case::new(&header)
                .source(source)
                .run(Target::MatchingSource),
            expected,
            "{member}"
        );
    }
}

#[test]
fn constructor_and_destructor_brace_style() {
    let header = "class Foo {\npublic:\n    Fo@o(int x = 0);\n    virtual ~Foo();\n};\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "Foo::Foo(int x)\n{\n    @\n}\n"
    );
    let header = "class Foo {\npublic:\n    Foo(int x = 0);\n    virtual ~Fo@o();\n};\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "Foo::~Foo()\n{\n    @\n}\n"
    );
    let mut case = Case::new(header).source("");
    case.config =
        Config::from_value(&json!({ "cpp": { "curlyBraceFormat": { "function": "sameLine" } } }));
    assert_eq!(
        case.run(Target::MatchingSource),
        "Foo::~Foo() {\n    @\n}\n"
    );
}

#[test]
fn trailing_return_type() {
    let header = "struct S {\n    auto ma@ke() const -> int;\n};\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "auto S::make() const -> int {\n    @\n}\n"
    );
}

#[test]
fn multi_line_parameter_list_is_realigned() {
    let header = "class Foo {\npublic:\n    void configure(int width,\n                   int height = 2,\n                   bool fullscreen = false)@;\n};\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "void Foo::configure(int width,\n                    int height,\n                    bool fullscreen) {\n    @\n}\n"
    );
}

#[test]
fn inline_in_class() {
    let header = "class Foo {\npublic:\n    int ge@t() const;\n    Foo();\n};\n";
    assert_eq!(
        Case::new(header).run(Target::Inline),
        "class Foo {\npublic:\n    int get() const {\n        @\n    }\n    Foo();\n};\n"
    );
    let header = "class Foo {\npublic:\n    int get() const;\n    Fo@o();\n};\n";
    assert_eq!(
        Case::new(header).run(Target::Inline),
        "class Foo {\npublic:\n    int get() const;\n    Foo()\n    {\n        @\n    }\n};\n"
    );
}

#[test]
fn inline_not_offered_outside_class() {
    let case = Case::new("namespace ns {\nvoid f@();\n}\n");
    assert_eq!(case.targets(), vec![Target::CurrentFile]);
}

#[test]
fn existing_definition_means_no_action() {
    let header =
        "namespace ns {\nclass Foo {\npublic:\n    void f@(const std::string& s) const;\n};\n}\n";
    let source = "namespace ns {\nvoid Foo::f(const string &str) const {}\n}\n";
    assert!(Case::new(header).source(source).targets().is_empty());

    let header = "class Foo {\npublic:\n    void f@(int);\n};\ninline void Foo::f(int x) {}\n";
    assert!(Case::new(header).targets().is_empty());

    // An overload with different parameters or constness doesn't count.
    let header = "class Foo {\npublic:\n    void f@(int) const;\n};\n";
    let source = "void Foo::f(int) {}\nvoid Foo::f(long) const {}\n";
    assert_eq!(
        Case::new(header).source(source).targets(),
        vec![Target::MatchingSource, Target::CurrentFile, Target::Inline]
    );
}

#[test]
fn pure_virtual_defaulted_and_deleted_mean_no_action() {
    for member in [
        "    virtual void f@() = 0;",
        "    Fo@o() = default;",
        "    Foo(const Foo&) = del@ete;",
        "    void f@() {}",
        "    int (*f@p)(int);",
    ] {
        let header = format!("class Foo {{\npublic:\n{member}\n}};\n");
        assert!(
            Case::new(&header).source("").targets().is_empty(),
            "{member}"
        );
    }
}

#[test]
fn current_file_member_after_class_in_namespace() {
    let header = "#ifndef FOO_H\n#define FOO_H\n\nnamespace ns {\n\nclass Foo {\npublic:\n    void f@();\n};\n\n} // namespace ns\n\n#endif\n";
    assert_eq!(
        Case::new(header).run(Target::CurrentFile),
        "#ifndef FOO_H\n#define FOO_H\n\nnamespace ns {\n\nclass Foo {\npublic:\n    void f();\n};\n\ninline void Foo::f() {\n    @\n}\n\n} // namespace ns\n\n#endif\n"
    );
}

#[test]
fn current_file_inside_header_guard() {
    let header = "#ifndef FOO_H\n#define FOO_H\n\nvoid f@();\n\n#endif\n";
    assert_eq!(
        Case::new(header).run(Target::CurrentFile),
        "#ifndef FOO_H\n#define FOO_H\n\nvoid f();\n\ninline void f() {\n    @\n}\n\n#endif\n"
    );
}

#[test]
fn current_file_free_function_in_namespace_has_no_inline() {
    // Matches C-Mantic: the definition lands inside the parent namespace, so no `inline`.
    let header = "namespace ns {\nvoid f@();\n}\n";
    assert_eq!(
        Case::new(header).run(Target::CurrentFile),
        "namespace ns {\nvoid f();\n\nvoid f() {\n    @\n}\n}\n"
    );
}

#[test]
fn current_file_next_to_sibling_definition() {
    let header = "class Foo {\npublic:\n    void a();\n    void b@();\n};\n\ninline void Foo::a() {\n}\n\nint x;\n";
    assert_eq!(
        Case::new(header).run(Target::CurrentFile),
        "class Foo {\npublic:\n    void a();\n    void b();\n};\n\ninline void Foo::a() {\n}\n\ninline void Foo::b() {\n    @\n}\n\nint x;\n"
    );
}

#[test]
fn friend_function() {
    let header = "namespace ns {\nclass Foo {\n    friend bool operat@or==(const Foo& a, const Foo& b);\n};\n}\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "bool ns::operator==(const Foo& a, const Foo& b) {\n    @\n}\n"
    );
}

#[test]
fn anonymous_namespace_is_omitted() {
    let header = "namespace {\nstruct S {\n    void f@();\n};\n}\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "void S::f() {\n    @\n}\n"
    );
}

#[test]
fn indentation_and_crlf_follow_target_file() {
    let header = "struct S {\n  void a();\n  void b@();\n};\n";
    let source = "void S::a() {\r\n  int x;\r\n}\r\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "void S::a() {\r\n  int x;\r\n}\r\n\r\nvoid S::b() {\r\n  @\r\n}\r\n"
    );
}

#[test]
fn c_header_uses_c_brace_format() {
    let mut case =
        Case::new("#pragma once\nint ad@d(int a, int b);\n").source("#include \"a.h\"\n");
    case.header_is_c = true;
    // The target's language decides; this source is C++ here, so check the in-file action.
    assert_eq!(
        case.run(Target::CurrentFile),
        "#pragma once\nint add(int a, int b);\n\ninline int add(int a, int b)\n{\n    @\n}\n"
    );
}

#[test]
fn cursor_anywhere_in_declaration() {
    let header = "struct S {\n    @int f(int a,\n          int b);\n};\n";
    assert!(!Case::new(header).targets().is_empty());
    let header = "struct S {\n    int f(int a,\n          int b);@\n};\n";
    assert!(!Case::new(header).targets().is_empty());
    let header = "struct S {\n@\n    int f();\n};\n";
    assert!(Case::new(header).targets().is_empty());
}

#[test]
fn utf16_positions() {
    let header = "// 😀 émoji\nstruct S { void f@(); };\n";
    let source = "// 😀\nint x;\n";
    assert_eq!(
        Case::new(header).source(source).run(Target::MatchingSource),
        "// 😀\nint x;\n\nvoid S::f() {\n    @\n}\n"
    );
}

#[test]
fn multi_line_return_type_is_realigned() {
    let header = "struct S {\n    unsigned long long\n        comp@ute(int a,\n                int b) const;\n};\n";
    assert_eq!(
        Case::new(header).source("").run(Target::MatchingSource),
        "unsigned long long\n    S::compute(int a,\n               int b) const {\n    @\n}\n"
    );
}

#[test]
fn explicit_class_template_specialization_member() {
    let header = "template <typename T> struct W;\ntemplate <>\nstruct W<int> {\n    void spe@cial(int x = 3);\n};\n";
    let case = Case::new(header).source("");
    assert_eq!(
        case.targets(),
        vec![Target::MatchingSource, Target::CurrentFile, Target::Inline]
    );
    assert_eq!(
        case.run(Target::MatchingSource),
        "void W<int>::special(int x) {\n    @\n}\n"
    );
}
