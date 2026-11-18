//! Where to put a new definition (ports of `SourceDocument.findSmartPositionForFunctionDefinition`
//! and `ProposedPosition.formatTextToInsert`).

use crate::symbol::{ScopeKind, Symbol, SymbolKind, SymbolPath, SymbolTree};
use crate::text::{leading_whitespace, line_count, line_end, line_of, line_start, line_text};

/// A byte offset in the target file plus hints for how to format text inserted there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProposedPosition {
    pub offset: usize,
    /// Start of the symbol the position is relative to; its line's indentation is used.
    pub relative_to: Option<usize>,
    pub before: bool,
    pub after: bool,
    /// Don't separate the new text from its neighbour with a blank line.
    pub next_to: bool,
    /// Indent the new text one level relative to the position's line.
    pub indent: bool,
}

impl ProposedPosition {
    fn after(offset: usize, relative_to: Option<usize>) -> Self {
        Self {
            offset,
            relative_to,
            after: true,
            ..Default::default()
        }
    }

    fn before(offset: usize, relative_to: usize) -> Self {
        Self {
            offset,
            relative_to: Some(relative_to),
            before: true,
            ..Default::default()
        }
    }
}

/// End of a symbol including a comment that trails it on the same line
/// (`CSymbol.trailingCommentEnd`).
pub fn trailing_comment_end(text: &str, symbol: &Symbol) -> usize {
    let end = symbol.range.end;
    let eol = line_end(text, end);
    let rest = text[end..eol].trim_start();
    if rest.is_empty() || rest.starts_with("//") {
        return eol;
    }
    if rest.starts_with("/*") {
        let comment_start = eol - rest.len();
        if let Some(i) = text[comment_start + 2..].find("*/") {
            return comment_start + 2 + i + 2;
        }
    }
    end
}

/// `util.positionAfterLastNonEmptyLine`.
pub fn position_after_last_non_empty_line(text: &str) -> ProposedPosition {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return ProposedPosition::default();
    }
    ProposedPosition::after(line_end(text, trimmed.len()), None)
}

fn position_after_last_symbol(text: &str, tree: &SymbolTree) -> ProposedPosition {
    match tree.symbols.last() {
        Some(last) => {
            ProposedPosition::after(trailing_comment_end(text, last), Some(last.range.start))
        }
        None => position_after_last_non_empty_line(text),
    }
}

/// Finds the best position in `target` for the definition of the declaration at `decl`.
///
/// Prefers a position next to the definition of a sibling declaration, then the end of the
/// matching namespace block, then after the last top-level symbol.
pub fn find_position_for_definition(
    decl_text: &str,
    decl: SymbolPath,
    target_text: &str,
    target: &SymbolTree,
    same_file: bool,
) -> ProposedPosition {
    if target.symbols.is_empty() {
        return position_after_last_non_empty_line(target_text);
    }
    let symbol = decl.symbol();

    let functions: Vec<&Symbol> = decl
        .siblings
        .iter()
        .filter(|s| s.kind == SymbolKind::Function)
        .collect();
    let index = functions
        .iter()
        .position(|s| std::ptr::eq(*s, symbol))
        .unwrap_or(0);
    let before = functions[..index].iter().rev();
    let after = functions.get(index + 1..).unwrap_or_default().iter();

    let linked_definition = |sibling: &Symbol| -> Option<&Symbol> {
        let definition = target.find_definition(sibling)?;
        let inside_parent = same_file
            && decl.parent.is_some_and(|p| {
                let start = definition
                    .function
                    .as_ref()
                    .map_or(0, |f| f.name_range.start);
                p.range.contains(&start)
            });
        (!inside_parent).then_some(definition)
    };

    for sibling in before.filter(|s| s.is_function_declaration()).take(6) {
        if let Some(definition) = linked_definition(sibling) {
            return ProposedPosition::after(
                trailing_comment_end(target_text, definition),
                Some(definition.range.start),
            );
        }
    }
    for sibling in after.filter(|s| s.is_function_declaration()).take(6) {
        if let Some(definition) = linked_definition(sibling) {
            return ProposedPosition::before(definition.comment_start, definition.range.start);
        }
    }

    if let Some(position) = position_in_parent_namespace(decl_text, symbol, target_text, target) {
        return position;
    }

    position_after_last_symbol(target_text, target)
}

/// `SourceDocument.findPositionInParentNamespace`.
fn position_in_parent_namespace(
    decl_text: &str,
    symbol: &Symbol,
    target_text: &str,
    target: &SymbolTree,
) -> Option<ProposedPosition> {
    let column = |offset: usize| offset - line_start(decl_text, offset);
    let mut previous_start = symbol.range.start;
    for (i, scope) in symbol.scopes.iter().enumerate().rev() {
        if scope.kind == ScopeKind::Namespace {
            let path: Vec<String> = symbol.scopes[..=i].iter().map(|s| s.name.clone()).collect();
            if let Some(namespace) = target.find_namespace(&path) {
                let Some(last) = namespace.children.last() else {
                    let body = namespace.body.clone().unwrap_or(namespace.range.clone());
                    return Some(ProposedPosition {
                        offset: body.start + 1,
                        after: true,
                        indent: column(previous_start) > column(scope.range.start),
                        ..Default::default()
                    });
                };
                return Some(ProposedPosition::after(
                    trailing_comment_end(target_text, last),
                    Some(last.range.start),
                ));
            }
        }
        if !scope.qualified_namespace {
            previous_start = scope.range.start;
        }
    }
    None
}

/// Indents and pads `insert_text` (which uses `\n` line endings) for insertion at `position`
/// (`ProposedPosition.formatTextToInsert`). Returns text with `\n` line endings.
pub fn format_text_to_insert(
    insert_text: &str,
    position: &ProposedPosition,
    text: &str,
    indent_unit: &str,
) -> String {
    let prefix_lines =
        |s: &str, prefix: &str| format!("{prefix}{}", s.replace('\n', &format!("\n{prefix}")));

    let mut insert = insert_text.to_string();
    if position.indent {
        insert = prefix_lines(&insert, indent_unit);
    }

    let indentation_line = line_of(text, position.relative_to.unwrap_or(position.offset));
    let indentation = leading_whitespace(line_text(text, indentation_line)).to_string();
    if !position.before {
        insert = prefix_lines(&insert, &indentation);
    } else {
        insert = insert.replace('\n', &format!("\n{indentation}"));
    }

    let line = line_of(text, position.offset);
    let count = line_count(text);
    let next_line = if position.after && line + 1 < count {
        Some(line_text(text, line + 1))
    } else if position.before && line > 0 {
        Some(line_text(text, line - 1))
    } else {
        None
    };
    let single = match next_line {
        None => true,
        Some(_) if position.next_to => true,
        Some(next) => {
            let blank = next.trim().is_empty();
            (!blank && position.after && !next.trim_start().starts_with('}'))
                || (!blank && position.before && !next.trim_end().ends_with('{'))
        }
    };
    let new_lines = if single { "\n" } else { "\n\n" };

    if position.after {
        insert = format!("{new_lines}{insert}");
    } else if position.before {
        insert.push_str(new_lines);
    }
    if line + 1 == count {
        insert.push('\n');
    }
    if position.before {
        insert.push_str(&indentation);
    }
    insert
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_with_blank_line() {
        let text = "void a()\n{\n}\n\nint x;\n";
        let pos = ProposedPosition::after(12, Some(0));
        assert_eq!(
            format_text_to_insert("void b() {}", &pos, text, "    "),
            "\n\nvoid b() {}"
        );
    }

    #[test]
    fn after_last_line_adds_newline() {
        let text = "#include \"a.h\"";
        let pos = position_after_last_non_empty_line(text);
        assert_eq!(format_text_to_insert("x", &pos, text, "    "), "\nx\n");
    }

    #[test]
    fn empty_file() {
        let pos = position_after_last_non_empty_line("");
        assert_eq!(format_text_to_insert("x", &pos, "", "    "), "x\n");
    }

    #[test]
    fn before_definition() {
        let text = "int x;\n\n  void b() {}\n";
        let pos = ProposedPosition::before(10, 10);
        assert_eq!(
            format_text_to_insert("void a()\n{\n}", &pos, text, "    "),
            "void a()\n  {\n  }\n\n  "
        );
    }
}
