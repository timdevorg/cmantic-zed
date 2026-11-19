//! Fixture tests for Update Function Signature. Each case is a sequence of texts of the edited
//! file (one didChange per step); `@` in the last one marks the cursor.

use std::collections::HashMap;

use cmantic_ls::actions::update_signature::{TITLE, TextChange, Tracker, diff};
use cmantic_ls::text::position_to_offset;
use tower_lsp_server::ls_types::{TextEdit, Uri};

fn uri(name: &str) -> Uri {
    format!("file:///project/{name}").parse().unwrap()
}

fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_string();
    for edit in edits.iter().rev() {
        let start = position_to_offset(text, edit.range.start);
        let end = position_to_offset(text, edit.range.end);
        out.replace_range(start..end, &edit.new_text);
    }
    out
}

fn cursor(text: &str) -> (String, usize) {
    let offset = text.find('@').expect("cursor");
    (text.replacen('@', "", 1), offset)
}

/// Runs the didChange steps on `a.h` (the first text is the opened one) with `other` as
/// `a.cpp`, and returns the action title and the updated counterpart file (`a.cpp`, or `a.h`
/// itself if `other` is `None`).
fn run_steps(steps: &[&str], other: Option<&str>) -> Option<(String, String)> {
    let (a, b) = (uri("a.h"), uri("a.cpp"));
    let mut tracker = Tracker::default();
    let mut prev = steps[0].to_string();
    let mut offset = 0;
    for step in &steps[1..] {
        let (text, o) = if step.contains('@') {
            cursor(step)
        } else {
            (step.to_string(), 0)
        };
        offset = o;
        tracker.did_change(&a, &prev, &text, || {
            other.map(|t| (b.clone(), t.to_string()))
        });
        prev = text;
    }
    let files: HashMap<Uri, String> = other
        .map(|t| (b.clone(), t.to_string()))
        .into_iter()
        .collect();
    let update = tracker.action_at(&a, &prev, offset, &files)?;
    let target = if update.counterpart_uri == a {
        prev.as_str()
    } else {
        other.unwrap()
    };
    Some((update.title, apply(target, &update.edits)))
}

fn run(before: &str, after: &str, other: Option<&str>) -> Option<String> {
    run_steps(&[before, after], other).map(|(_, text)| text)
}

// ---------------------------------------------------------------------------------------
// Tracking

#[test]
fn diff_is_common_prefix_and_suffix() {
    assert_eq!(diff("abc", "abc"), None);
    assert_eq!(
        diff("int f();", "int g();"),
        Some(TextChange {
            start: 4,
            old_end: 5,
            new_end: 5
        })
    );
    assert_eq!(
        diff("f(int)", "f(int, int)"),
        Some(TextChange {
            start: 5,
            old_end: 5,
            new_end: 10
        })
    );
    // Multi-byte characters stay on char boundaries.
    let change = diff("a é b", "a è b").unwrap();
    assert_eq!((change.start, change.old_end, change.new_end), (2, 4, 4));
}

#[test]
fn ranges_map_through_changes() {
    let change = TextChange {
        start: 10,
        old_end: 12,
        new_end: 15,
    };
    assert_eq!(change.map_range(&(0..5)), Some(0..5));
    assert_eq!(change.map_range(&(20..30)), Some(23..33));
    assert_eq!(change.map_range(&(5..11)), Some(5..15));
    assert_eq!(change.map_range(&(11..20)), Some(10..23));
    assert_eq!(change.map_range(&(10..12)), Some(10..15));
    let deletion = TextChange {
        start: 10,
        old_end: 20,
        new_end: 10,
    };
    assert_eq!(deletion.map_range(&(12..18)), None);
    assert_eq!(deletion.map_range(&(5..18)), Some(5..10));
}

const H: &str = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, int b) const;\n};\n";
const CPP: &str = "#include \"a.h\"\n\nint Foo::get(int a, int b) const\n{\n    return a + b;\n}\n";

#[test]
fn survives_broken_syntax_and_many_steps() {
    let steps = [
        H,
        "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, int b const;\n};\n",
        "#pragma once\n\nclass Foo {\npublic:\n    int fetch(int a, int b const;\n};\n",
        "#pragma once\n\nclass Foo {\npublic:\n    int fetch(int a, int b, \n};\n",
        "#pragma once\n\nclass Foo {\npublic:\n    long fetch(int a, int b, bool c) const;\n};\n",
        "#pragma once\n\nclass Foo {\npublic:\n    long fetch(int a, int b, bool c) const;@\n};\n",
    ];
    let (title, text) = run_steps(&steps, Some(CPP)).unwrap();
    assert_eq!(title, format!("{TITLE} in \"a.cpp\""));
    assert_eq!(
        text,
        "#include \"a.h\"\n\nlong Foo::fetch(int a, int b, bool c) const\n{\n    return a + b;\n}\n"
    );
}

#[test]
fn survives_edits_elsewhere_and_cursor_moves() {
    let steps = [
        H,
        "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, float b) const;\n};\n",
        "#pragma once\n#include <string>\n\nclass Foo {\npublic:\n    int get(int a, float b) const;\n};\n",
        "#pragma once\n#include <string>\n\nclass Foo {\npublic:\n    int get(int a, float b) const;\n    void other();\n};\n",
        "#pragma once\n#include <string>\n\nclass Foo {\npublic:\n    int g@et(int a, float b) const;\n    void other();\n};\n",
    ];
    let (_, text) = run_steps(&steps, Some(CPP)).unwrap();
    assert!(
        text.contains("int Foo::get(int a, float b) const\n{"),
        "{text}"
    );
}

#[test]
fn edited_back_to_original_drops_the_change() {
    let changed = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, float b) const;\n};\n";
    let back = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, int b) const;@\n};\n";
    assert!(run_steps(&[H, changed, back], Some(CPP)).is_none());
    // Only whitespace or a default argument changed: nothing to propagate.
    let spaces = "#pragma once\n\nclass Foo {\npublic:\n    int  get( int a,int b ) const;@\n};\n";
    assert!(run(H, spaces, Some(CPP)).is_none());
}

#[test]
fn deleted_head_drops_the_change() {
    let changed = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, float b) const;\n};\n";
    let deleted = "#pragma once\n\nclass Foo {\npublic:\n    ;\n};\n";
    let retyped = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, float b) const;@\n};\n";
    assert!(run_steps(&[H, changed, deleted, retyped], Some(CPP)).is_none());
}

#[test]
fn several_pending_functions_per_file() {
    let before = "void a(int x);\nvoid b(int y);\n\nvoid a(int x) {}\nvoid b(int y) {}\n";
    let s1 = "void a(long x);\nvoid b(int y);\n\nvoid a(int x) {}\nvoid b(int y) {}\n";
    let s2 = "void a(long x);\nvoid b(short y);\n\nvoid a(int x) {}\nvoid b(int y) {}\n";
    let at_a = "void a(long x)@;\nvoid b(short y);\n\nvoid a(int x) {}\nvoid b(int y) {}\n";
    let at_b = "void a(long x);\nvoid b(short y)@;\n\nvoid a(int x) {}\nvoid b(int y) {}\n";
    let (title, text) = run_steps(&[before, s1, s2, at_a], None).unwrap();
    assert_eq!(title, TITLE);
    assert_eq!(
        text,
        "void a(long x);\nvoid b(short y);\n\nvoid a(long x) {}\nvoid b(int y) {}\n"
    );
    let (_, text) = run_steps(&[before, s1, s2, at_b], None).unwrap();
    assert_eq!(
        text,
        "void a(long x);\nvoid b(short y);\n\nvoid a(int x) {}\nvoid b(short y) {}\n"
    );
}

#[test]
fn no_ping_pong_after_applying_in_the_same_file() {
    let (a, b) = (uri("a.cpp"), uri("b.h"));
    let before = "int f(int x);\n\nint f(int x) { return x; }\n";
    let edited = "long f(int x);\n\nint f(int x) { return x; }\n";
    let mut tracker = Tracker::default();
    tracker.did_change(&a, before, edited, || None);
    let files = HashMap::new();
    let update = tracker.action_at(&a, edited, 2, &files).unwrap();
    let applied = apply(edited, &update.edits);
    assert_eq!(applied, "long f(int x);\n\nlong f(int x) { return x; }\n");
    tracker.did_change(&a, edited, &applied, || None);
    assert!(tracker.pending(&a).is_empty());
    for (offset, _) in applied.char_indices() {
        assert!(tracker.action_at(&a, &applied, offset, &files).is_none());
    }
    for offset in 0..applied.len() {
        assert!(tracker.action_at(&a, &applied, offset, &files).is_none());
    }
    // The user changes the definition by hand to match: no reverse suggestion either.
    let mut tracker = Tracker::default();
    tracker.did_change(&a, before, edited, || None);
    tracker.did_change(&a, edited, &applied, || Some((b.clone(), String::new())));
    for offset in 0..applied.len() {
        assert!(tracker.action_at(&a, &applied, offset, &files).is_none());
    }
}

#[test]
fn counterpart_is_relocated_when_it_moved() {
    let (a, b) = (uri("a.h"), uri("a.cpp"));
    let edited = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, int b);\n};\n";
    let mut tracker = Tracker::default();
    tracker.did_change(&a, H, edited, || Some((b.clone(), CPP.to_string())));
    // Lines were added above the definition on disk.
    let moved = format!("// comment\n\n{CPP}");
    let files = HashMap::from([(b.clone(), moved.clone())]);
    let update = tracker.action_at(&a, edited, 45, &files).unwrap();
    assert_eq!(
        apply(&moved, &update.edits),
        "// comment\n\n#include \"a.h\"\n\nint Foo::get(int a, int b)\n{\n    return a + b;\n}\n"
    );
    // The definition was rewritten to something else: give up.
    let gone = "#include \"a.h\"\n\nint Foo::other() const\n{\n    return 1;\n}\n";
    let files = HashMap::from([(b.clone(), gone.to_string())]);
    assert!(tracker.action_at(&a, edited, 45, &files).is_none());
    assert!(tracker.pending(&a).is_empty());
}

#[test]
fn open_counterpart_edits_are_tracked() {
    let (a, b) = (uri("a.h"), uri("a.cpp"));
    let edited = "#pragma once\n\nclass Foo {\npublic:\n    int get(int a, int b);\n};\n";
    let mut tracker = Tracker::default();
    tracker.did_change(&a, H, edited, || Some((b.clone(), CPP.to_string())));
    // Edit the body and above the definition in the (open) source file.
    let cpp2 = CPP.replace("return a + b;", "return a * b;").replace(
        "#include \"a.h\"\n",
        "#include \"a.h\"\n#include <vector>\n",
    );
    tracker.did_change(&b, CPP, &cpp2, || None);
    assert_eq!(tracker.pending(&a).len(), 1);
    let files = HashMap::from([(b.clone(), cpp2.clone())]);
    let update = tracker.action_at(&a, edited, 45, &files).unwrap();
    assert!(
        apply(&cpp2, &update.edits).contains("int Foo::get(int a, int b)\n{\n    return a * b;")
    );
}

// ---------------------------------------------------------------------------------------
// Change types (header declaration -> source definition unless noted)

fn header(decl: &str) -> String {
    format!("#pragma once\n\nclass Foo {{\npublic:\n    {decl}\n}};\n")
}

fn source(def: &str) -> String {
    format!("#include \"a.h\"\n\n{def}\n{{\n    return a;\n}}\n")
}

/// Declaration `from` -> `to` in the class; the definition `def` becomes `expected`.
fn check_decl(from: &str, to: &str, def: &str, expected: &str) {
    let after = header(&format!("{to}@"));
    let result = run(&header(from), &after, Some(&source(def)));
    assert_eq!(
        result.as_deref(),
        Some(source(expected).as_str()),
        "{from} -> {to}"
    );
}

/// Definition `from` -> `to`; the declaration `decl` in the class becomes `expected`.
fn check_def(from: &str, to: &str, decl: &str, expected: &str) {
    let (a, b) = (uri("a.cpp"), uri("a.h"));
    let (before, after) = (source(from), source(&format!("{to}@")));
    let (after, offset) = cursor(&after);
    let mut tracker = Tracker::default();
    tracker.did_change(&a, &before, &after, || Some((b.clone(), header(decl))));
    let files = HashMap::from([(b.clone(), header(decl))]);
    let update = tracker
        .action_at(&a, &after, offset, &files)
        .unwrap_or_else(|| panic!("no action for {from} -> {to}"));
    assert_eq!(update.title, format!("{TITLE} in \"a.h\""));
    assert_eq!(apply(&header(decl), &update.edits), header(expected));
}

#[test]
fn const_ref_and_noexcept() {
    check_decl(
        "int get(int a);",
        "int get(int a) const;",
        "int Foo::get(int a)",
        "int Foo::get(int a) const",
    );
    check_decl(
        "int get(int a) const;",
        "int get(int a);",
        "int Foo::get(int a) const",
        "int Foo::get(int a)",
    );
    check_decl(
        "int get(int a) const;",
        "int get(int a) const &&;",
        "int Foo::get(int a) const",
        "int Foo::get(int a) const &&",
    );
    check_decl(
        "int get(int a);",
        "int get(int a) noexcept;",
        "int Foo::get(int a)",
        "int Foo::get(int a) noexcept",
    );
    check_def(
        "int Foo::get(int a) const noexcept",
        "int Foo::get(int a) &",
        "int get(int a) const noexcept;",
        "int get(int a) &;",
    );
}

#[test]
fn parameters_added_removed_reordered() {
    check_decl(
        "int get(int a);",
        "int get(int a, bool b);",
        "int Foo::get(int a)",
        "int Foo::get(int a, bool b)",
    );
    check_decl(
        "int get(int a, bool b);",
        "int get(int a);",
        "int Foo::get(int a, bool b)",
        "int Foo::get(int a)",
    );
    check_decl(
        "int get(int a, bool b);",
        "int get();",
        "int Foo::get(int a, bool b)",
        "int Foo::get()",
    );
    check_decl(
        "int get();",
        "int get(int a);",
        "int Foo::get()",
        "int Foo::get(int a)",
    );
    // Reordered: the definition keeps its own names, matched to the moved parameters.
    check_decl(
        "int get(int a, bool b);",
        "int get(bool b, int a);",
        "int Foo::get(int x, bool y)",
        "int Foo::get(bool y, int x)",
    );
    check_def(
        "int Foo::get(int a, bool b)",
        "int Foo::get(bool b, int a)",
        "int get(int, bool);",
        "int get(bool b, int a);",
    );
}

#[test]
fn parameter_renames() {
    // A definition keeps its parameter names, so its body stays valid.
    let after = header("int get(int count);@");
    assert!(
        run(
            &header("int get(int a);"),
            &after,
            Some(&source("int Foo::get(int a)"))
        )
        .is_none()
    );
    check_decl(
        "int get(int a);",
        "int get(long count);",
        "int Foo::get(int a)",
        "int Foo::get(long a)",
    );
    // A declaration takes the definition's names.
    check_def(
        "int Foo::get(int a)",
        "int Foo::get(int count)",
        "int get(int a);",
        "int get(int count);",
    );
    // Unnamed in the declaration: the definition's name is kept.
    check_decl(
        "int get(int);",
        "int get(long);",
        "int Foo::get(int a)",
        "int Foo::get(long a)",
    );
}

#[test]
fn parameter_and_return_types() {
    check_decl(
        "int get(int32_t a);",
        "int get(float a);",
        "int Foo::get(int32_t a)",
        "int Foo::get(float a)",
    );
    check_decl(
        "int get(int a);",
        "const std::string& get(int a);",
        "int Foo::get(int a)",
        "const std::string& Foo::get(int a)",
    );
    check_def(
        "int Foo::get(int a)",
        "std::vector<int> Foo::get(int a)",
        "int get(int a);",
        "std::vector<int> get(int a);",
    );
    check_decl(
        "int* get(int a);",
        "int get(int a);",
        "int* Foo::get(int a)",
        "int Foo::get(int a)",
    );
}

#[test]
fn renamed_function_keeps_qualifier() {
    check_decl(
        "int get(int a);",
        "int fetch(int a);",
        "int Foo::get(int a)",
        "int Foo::fetch(int a)",
    );
    check_def(
        "int Foo::get(int a)",
        "int Foo::fetch(int a)",
        "int get(int a);",
        "int fetch(int a);",
    );
}

#[test]
fn heavy_combined_edit() {
    // Rename + new parameter + type change + const, in one go: a name lookup would fail.
    check_decl(
        "int get(int32_t a);",
        "long fetch(float a, bool strict = false) const;",
        "int Foo::get(int32_t a)",
        "long Foo::fetch(float a, bool strict) const",
    );
    check_def(
        "int Foo::get(int32_t a)",
        "long Foo::fetch(float value, bool strict) const noexcept",
        "int get(int32_t a = 0);",
        "long fetch(float value = 0, bool strict) const noexcept;",
    );
}

#[test]
fn default_arguments_stay_on_the_declaration() {
    check_def(
        "int Foo::get(int a, int b)",
        "int Foo::get(long a, int b, int c)",
        "int get(int a, int b = 2);",
        "int get(long a, int b = 2, int c);",
    );
    check_decl(
        "int get(int a = 1);",
        "int get(int a = 1, int c = 3);",
        "int Foo::get(int a)",
        "int Foo::get(int a, int c)",
    );
}

#[test]
fn declaration_only_specifiers() {
    check_decl(
        "static int get(int a);",
        "static long get(int a);",
        "int Foo::get(int a)",
        "long Foo::get(int a)",
    );
    check_decl(
        "virtual int get(int a) override;",
        "virtual int get(int a, int b) override;",
        "int Foo::get(int a)",
        "int Foo::get(int a, int b)",
    );
    check_def(
        "int Foo::get(int a)",
        "long Foo::get(int a) const",
        "virtual int get(int a) override;",
        "virtual long get(int a) const override;",
    );
    check_def(
        "int Foo::get(int a)",
        "int Foo::get(short a)",
        "static int get(int a);",
        "static int get(short a);",
    );
    check_def(
        "int Foo::get(int a)",
        "int Foo::get(short a)",
        "explicit Foo(int a); int get(int a) final;",
        "explicit Foo(int a); int get(short a) final;",
    );
    // `inline` and `static` aren't copied from the edited side.
    check_decl(
        "int get(int a);",
        "inline static int get(int a, int b);",
        "int Foo::get(int a)",
        "int Foo::get(int a, int b)",
    );
}

#[test]
fn templates_and_same_file() {
    let before = "template <typename T>\nstruct Box {\n    T get(int i) const;\n};\n\ntemplate <typename T>\nT Box<T>::get(int i) const\n{\n    return T();\n}\n";
    let after = "template <typename T>\nstruct Box {\n    T& item(int i, int j)@;\n};\n\ntemplate <typename T>\nT Box<T>::get(int i) const\n{\n    return T();\n}\n";
    assert_eq!(
        run(before, after, None).as_deref(),
        Some(
            "template <typename T>\nstruct Box {\n    T& item(int i, int j);\n};\n\ntemplate <typename T>\nT& Box<T>::item(int i, int j)\n{\n    return T();\n}\n"
        )
    );
    // Function template, definition edited, declaration above in the same file.
    let before = "namespace n {\ntemplate <class T> T sum(T a, T b);\n}\n\ntemplate <class T> T n::sum(T a, T b) { return a + b; }\n";
    let after = "namespace n {\ntemplate <class T> T sum(T a, T b);\n}\n\ntemplate <class T> T n::sum(const T& a, const T& b)@ { return a + b; }\n";
    assert_eq!(
        run(before, after, None).as_deref(),
        Some(
            "namespace n {\ntemplate <class T> T sum(const T& a, const T& b);\n}\n\ntemplate <class T> T n::sum(const T& a, const T& b) { return a + b; }\n"
        )
    );
}

#[test]
fn multi_line_parameters_keep_the_counterpart_layout() {
    let h = header("int get(int a,\n            int b);");
    let after = header("int get(int a,\n            long b,\n            bool c);@");
    let cpp = "#include \"a.h\"\n\nint Foo::get(int a,\n             int b)\n{\n    return a;\n}\n";
    assert_eq!(
        run(&h, &after, Some(cpp)).as_deref(),
        Some(
            "#include \"a.h\"\n\nint Foo::get(int a,\n             long b,\n             bool c)\n{\n    return a;\n}\n"
        )
    );
    // A one-parameter counterpart takes the edited side's line breaks, re-aligned.
    let h = header("int get(int a);");
    let after = header("int get(int a,\n            int b);@");
    assert_eq!(
        run(&h, &after, Some(&source("int Foo::get(int a)"))).as_deref(),
        Some(source("int Foo::get(int a,\n             int b)").as_str())
    );
}

#[test]
fn nothing_without_counterpart_or_change() {
    let h = header("int get(int a);");
    assert!(run(&h, &header("int get(long a);@"), None).is_none());
    assert!(run(&h, &header("int get(long a);@"), Some("#include \"a.h\"\n")).is_none());
    // Cursor elsewhere in the file.
    let after = format!("@{}", header("int get(long a);"));
    assert!(run(&h, &after, Some(&source("int Foo::get(int a)"))).is_none());
}

#[test]
fn every_offset_is_safe() {
    let texts = [
        (H, Some(CPP)),
        (
            "template <typename T>\nstruct Box : Base<T> {\n    virtual T& get(int i = 0) const & noexcept override;\n    static Box make(const char* s, ...);\n    operator bool() const;\n};\ntemplate <typename T> T& Box<T>::get(int) const & noexcept { return t; }\nauto f() -> int;\nauto f() -> int { return 1; }\nvoid broken(\n",
            None,
        ),
        (
            "#include \"a.h\"\r\nint Foo::get(int a, int b) const\r\n{\r\n}\r\nvoid g(int (*cb)(int), int arr[3]);\r\nvoid g(int (*cb)(int), int arr[3]) {}\r\n",
            Some("class Foo { int get(int a, int b) const; };"),
        ),
    ];
    let (a, b) = (uri("a.h"), uri("a.cpp"));
    for (text, other) in texts {
        let files: HashMap<Uri, String> = other
            .map(|t| (b.clone(), t.to_string()))
            .into_iter()
            .collect();
        let boundaries: Vec<usize> = (0..=text.len())
            .filter(|&i| text.is_char_boundary(i))
            .collect();
        for &offset in &boundaries {
            for edit in ["x", "", ", int q", "(", ")"] {
                let end = boundaries
                    .iter()
                    .copied()
                    .find(|&e| e > offset)
                    .unwrap_or(offset);
                let end = if edit.is_empty() { end } else { offset };
                let mut changed = text.to_string();
                changed.replace_range(offset..end, edit);
                let mut tracker = Tracker::default();
                tracker.did_change(&a, text, &changed, || {
                    other.map(|t| (b.clone(), t.to_string()))
                });
                for probe in [offset, 0, changed.len() / 2, changed.len()] {
                    if let Some(update) = tracker.action_at(&a, &changed, probe, &files) {
                        let target = if update.counterpart_uri == a {
                            changed.as_str()
                        } else {
                            other.unwrap()
                        };
                        let _ = apply(target, &update.edits);
                    }
                }
            }
        }
    }
}
