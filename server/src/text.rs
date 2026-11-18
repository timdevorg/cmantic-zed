//! Plain-text helpers: LSP (UTF-16) positions, lines, end-of-line and indentation detection.

use tower_lsp_server::ls_types::Position;

/// Converts a byte offset into an LSP position (UTF-16 code units).
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let line_start = line_start(text, offset);
    let line = text[..line_start].matches('\n').count();
    let character: usize = text[line_start..offset].chars().map(char::len_utf16).sum();
    Position::new(line as u32, character as u32)
}

/// Converts an LSP position (UTF-16 code units) into a byte offset, clamped to the line end.
pub fn position_to_offset(text: &str, position: Position) -> usize {
    let mut offset = 0;
    for _ in 0..position.line {
        match text[offset..].find('\n') {
            Some(i) => offset += i + 1,
            None => return text.len(),
        }
    }
    let end = line_end(text, offset);
    let mut units = 0u32;
    for (i, ch) in text[offset..end].char_indices() {
        if units >= position.character {
            return offset + i;
        }
        units += ch.len_utf16() as u32;
    }
    end
}

/// Byte offset of the start of the line containing `offset`.
pub fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// Byte offset of the end of the line containing `offset`, excluding `\r\n` / `\n`.
pub fn line_end(text: &str, offset: usize) -> usize {
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    if end > 0 && text.as_bytes()[end - 1] == b'\r' && end > offset {
        end - 1
    } else {
        end
    }
}

/// Zero-based line number of `offset`.
pub fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count()
}

/// Number of lines, counted like VS Code does (a trailing newline starts an empty last line).
pub fn line_count(text: &str) -> usize {
    text.matches('\n').count() + 1
}

/// Text of the given zero-based line, without its line ending.
pub fn line_text(text: &str, line: usize) -> &str {
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return "",
        }
    }
    &text[start..line_end(text, start)]
}

/// Leading whitespace of a line.
pub fn leading_whitespace(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// The end-of-line sequence used by `text` (`\r\n` if its first line ending is CRLF).
pub fn detect_eol(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// Guesses one level of indentation from the file's contents.
///
/// Returns `"\t"` if indented lines mostly start with tabs. Otherwise returns the most common
/// increase in leading spaces after a line that opens a block (ends with `{`) or is an access
/// specifier or case label. Other increases (aligned continuation lines such as member
/// initializer lists or multi-line parameter lists) are only used when there are no such
/// lines at all.
pub fn detect_indentation(text: &str) -> Option<String> {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    // Increases after a block opener, and between any two consecutive lines (fallback).
    let mut block_deltas = [0usize; 9];
    let mut deltas = [0usize; 9];
    let mut previous = 0usize;
    // Indentation of the previous code line, if it opens a block.
    let mut opener: Option<usize> = None;
    // Whether the previous code line is continued on the next one (ends with `,` or `(`).
    let mut continues = false;
    let mut in_block_comment = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        // Skip continuation lines of block comments (` * foo`), which are indented by one space.
        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        let comment_only = trimmed.starts_with("//") || trimmed.starts_with("/*");
        if trimmed.starts_with("/*") && !trimmed.contains("*/") {
            in_block_comment = true;
        }
        let indent = leading_whitespace(line);
        if indent.starts_with('\t') {
            tabs += 1;
            if !comment_only {
                opener = None;
                continues = false;
            }
            continue;
        }
        let width = indent.len();
        if !indent.is_empty() {
            spaces += 1;
        }
        let continuation = continues
            || trimmed.starts_with(':') && !trimmed.starts_with("::")
            || trimmed.starts_with(',');
        if width > previous && width - previous <= 8 && !continuation {
            deltas[width - previous] += 1;
        }
        if let Some(base) = opener
            && width > base
            && width - base <= 8
        {
            block_deltas[width - base] += 1;
        }
        previous = width;
        if !comment_only {
            opener = opens_block(trimmed).then_some(width);
            let code = strip_trailing_comment(trimmed);
            continues = code.ends_with([',', '(']) || code.ends_with("&&") || code.ends_with("||");
        }
    }
    if tabs == 0 && spaces == 0 {
        return None;
    }
    if tabs > spaces {
        return Some("\t".into());
    }
    let most_common = |deltas: &[usize; 9]| {
        deltas
            .iter()
            .enumerate()
            .skip(1)
            .max_by_key(|&(width, count)| (*count, std::cmp::Reverse(width)))
            .filter(|(_, count)| **count > 0)
            .map(|(width, _)| " ".repeat(width))
    };
    most_common(&block_deltas).or_else(|| most_common(&deltas))
}

/// A code line without a trailing `//` or `/* */` comment and trailing whitespace.
fn strip_trailing_comment(line: &str) -> &str {
    let code = line.find("//").map_or(line, |i| &line[..i]);
    let mut code = code.trim_end();
    while code.ends_with("*/") {
        match code.rfind("/*") {
            Some(i) => code = code[..i].trim_end(),
            None => break,
        }
    }
    code
}

/// Whether a (left-trimmed) code line is followed by indented content: it ends with `{`, or
/// is an access specifier or a case label. Trailing comments are ignored.
fn opens_block(line: &str) -> bool {
    let code = strip_trailing_comment(line);
    if code.ends_with('{') {
        return true;
    }
    let Some(label) = code.strip_suffix(':') else {
        return false;
    };
    if label.ends_with(':') {
        return false;
    }
    let label = label.trim_end();
    matches!(label, "public" | "protected" | "private" | "default")
        || label.starts_with("case ")
        || label.starts_with("case(")
}

/// Whether `c` can be part of a C++ identifier.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Removes every whole-word occurrence of `word` together with the whitespace following it.
pub fn remove_word_and_trailing_space(text: &str, word: &str, only_first: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut removed = false;
    while let Some(i) = find_word(rest, word) {
        if removed && only_first {
            break;
        }
        out.push_str(&rest[..i]);
        rest = rest[i + word.len()..].trim_start();
        removed = true;
    }
    out.push_str(rest);
    out
}

/// Removes every whole-word occurrence of `word` together with the whitespace preceding it.
pub fn remove_word_and_leading_space(text: &str, word: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = find_word(rest, word) {
        out.push_str(rest[..i].trim_end());
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    out
}

/// Byte index of the first whole-word occurrence of `word` in `text`.
pub fn find_word(text: &str, word: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before_ok = !text[..start].chars().next_back().is_some_and(is_word_char);
        let after_ok = !text[end..].chars().next().is_some_and(is_word_char);
        if before_ok && after_ok {
            return Some(start);
        }
        from = end;
    }
    None
}

/// Whether `text` contains `word` as a whole word.
pub fn contains_word(text: &str, word: &str) -> bool {
    find_word(text, word).is_some()
}

/// Collapses whitespace the way C-Mantic's `normalizeWhitespace` does: whitespace between two
/// word characters becomes a single space, all other whitespace is removed.
pub fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && is_word_char(c) && out.chars().next_back().is_some_and(is_word_char) {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_positions_round_trip() {
        let text = "a\n😀b\r\nc";
        let offset = text.find('b').unwrap();
        let pos = offset_to_position(text, offset);
        assert_eq!(pos, Position::new(1, 2));
        assert_eq!(position_to_offset(text, pos), offset);
        assert_eq!(line_end(text, 2), text.find('\r').unwrap());
        assert_eq!(
            position_to_offset(text, Position::new(1, 99)),
            text.find('\r').unwrap()
        );
    }

    #[test]
    fn detects_indentation() {
        assert_eq!(
            detect_indentation("a {\n  b {\n    c\n  }\n}\n").as_deref(),
            Some("  ")
        );
        assert_eq!(detect_indentation("a {\n\tb\n}\n").as_deref(), Some("\t"));
        assert_eq!(detect_indentation("a\nb\n"), None);
    }

    #[test]
    fn aligned_continuation_lines_do_not_count() {
        // Only empty bodies plus an aligned member initializer list: nothing to go by.
        let empty = "Foo::Foo()\n    : Base(),\n      m_width()\n{\n}\n\nvoid Foo::f()\n{\n}\n";
        assert_eq!(detect_indentation(empty), None);
        let ctor = "Foo::Foo()\n    : Base(),\n      m_width(),\n      m_height()\n{\n}\n\nvoid Foo::f()\n{\n}\n\nvoid Foo::g() {\n    x();\n}\n";
        assert_eq!(detect_indentation(ctor).as_deref(), Some("    "));
        // Aligned multi-line parameter lists.
        let params = "void f(int a,\n       int b,\n       int c);\nvoid g(int a,\n       int b);\nstruct S {\n    int x;\n};\n";
        assert_eq!(detect_indentation(params).as_deref(), Some("    "));
        // Access specifiers and case labels open a level too.
        let labels = "class A {\npublic:\n  int x;\n};\nswitch (x) {\ncase 1:\n  break;\ndefault: // d\n  break;\n}\n";
        assert_eq!(detect_indentation(labels).as_deref(), Some("  "));
        // Trailing comments after `{` are ignored.
        let comment = "void f() { // c\n  a(b,\n    c);\n}\nvoid g() { /* c */\n  x;\n}\n";
        assert_eq!(detect_indentation(comment).as_deref(), Some("  "));
    }

    #[test]
    fn indentation_fallbacks() {
        // 2-space projects are still 2.
        let two = "namespace n {\nclass A {\n  void f() {\n    if (x) {\n      y(a,\n        b);\n    }\n  }\n};\n}\n";
        assert_eq!(detect_indentation(two).as_deref(), Some("  "));
        // Tabs, even with aligned continuation lines.
        let tabs = "void f()\n{\n\tg(a,\n\t  b);\n\th();\n}\n";
        assert_eq!(detect_indentation(tabs).as_deref(), Some("\t"));
        // No block openers at all: other increases count, except continuation lines.
        assert_eq!(
            detect_indentation("int a =\n   1;\nint b =\n   2;\n").as_deref(),
            Some("   ")
        );
        assert_eq!(detect_indentation("void f(int a,\n       int b);\n"), None);
        // A comment line between `{` and the body doesn't hide the opener.
        let commented = "void f() {\n    // c\n    x;\n}\n";
        assert_eq!(detect_indentation(commented).as_deref(), Some("    "));
    }

    #[test]
    fn word_helpers() {
        assert_eq!(
            remove_word_and_trailing_space("virtual  int virtualx", "virtual", false),
            "int virtualx"
        );
        assert_eq!(
            remove_word_and_leading_space("() const override", "override"),
            "() const"
        );
        assert_eq!(normalize_whitespace("const  Foo &  x"), "const Foo&x");
    }
}
