//! Add Definition (port of `commands/addDefinition.ts` and `CSymbol.formatDeclaration`).
//!
//! Pure text-in, edit-out logic; the LSP wiring lives in `server.rs`.

use tower_lsp_server::ls_types::{Position, Range};

use crate::config::Config;
use crate::placement::{ProposedPosition, find_position_for_definition, format_text_to_insert};
use crate::symbol::{ScopeKind, Symbol, SymbolPath, SymbolTree, dedent, leading_text_has};
use crate::text::{
    detect_eol, detect_indentation, leading_whitespace, line_of, line_start, offset_to_position,
    position_to_offset, remove_word_and_leading_space, remove_word_and_trailing_space,
};

pub const TITLE_MATCHING_SOURCE: &str = "Add Definition in matching source file";
pub const TITLE_CURRENT_FILE: &str = "Add Definition in this file";
pub const TITLE_INLINE: &str = "Add Definition inline";

/// A file taking part in the refactoring.
#[derive(Debug, Clone, Copy)]
pub struct FileInput<'a> {
    pub text: &'a str,
    pub is_header: bool,
    /// Whether the file is C (as opposed to C++); selects the curly brace setting.
    pub is_c: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Out-of-line definition in the matching source file.
    MatchingSource,
    /// Out-of-line definition in the file of the declaration.
    CurrentFile,
    /// Replace the declaration's `;` with a body (class members only).
    Inline,
}

impl Target {
    pub fn title(self) -> &'static str {
        match self {
            Self::MatchingSource => TITLE_MATCHING_SOURCE,
            Self::CurrentFile => TITLE_CURRENT_FILE,
            Self::Inline => TITLE_INLINE,
        }
    }
}

/// A single text edit plus where to put the cursor after applying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    /// Range to replace, in the target file before the edit.
    pub range: Range,
    pub new_text: String,
    /// Empty position inside the new body, in the target file after the edit.
    pub cursor: Position,
}

/// Whether there is a function declaration (without a body) at `position`. Cheap pre-check
/// before looking for the matching source file.
pub fn has_declaration_at(decl: &FileInput, position: Position) -> bool {
    let tree = SymbolTree::new(decl.text);
    declaration_at(&tree, position_to_offset(decl.text, position)).is_some()
}

/// Returns the Add Definition actions available for the function declaration at `position`.
///
/// `matching_source` is the matching source file of `decl` (only relevant for headers).
pub fn available_targets(
    decl: &FileInput,
    position: Position,
    matching_source: Option<&FileInput>,
) -> Vec<Target> {
    let tree = SymbolTree::new(decl.text);
    let offset = position_to_offset(decl.text, position);
    let Some(path) = declaration_at(&tree, offset) else {
        return Vec::new();
    };
    let symbol = path.symbol();
    let source_tree = matching_source.map(|s| SymbolTree::new(s.text));
    if has_definition(symbol, &tree, source_tree.as_ref()) {
        return Vec::new();
    }

    let mut targets = Vec::new();
    if decl.is_header && matching_source.is_some() && can_define_in_source_file(decl.text, symbol) {
        targets.push(Target::MatchingSource);
    }
    targets.push(Target::CurrentFile);
    if symbol
        .parent_scope()
        .is_some_and(|s| s.kind == ScopeKind::Class)
    {
        targets.push(Target::Inline);
    }
    targets
}

/// Builds the edit for `target`. For [`Target::MatchingSource`], `matching_source` must be set.
pub fn add_definition(
    decl: &FileInput,
    position: Position,
    target: Target,
    matching_source: Option<&FileInput>,
    config: &Config,
) -> Option<Insertion> {
    let tree = SymbolTree::new(decl.text);
    let offset = position_to_offset(decl.text, position);
    let path = declaration_at(&tree, offset)?;
    let symbol = path.symbol();
    let source_tree = matching_source.map(|s| SymbolTree::new(s.text));
    if has_definition(symbol, &tree, source_tree.as_ref()) {
        return None;
    }

    match target {
        Target::Inline => {
            if symbol
                .parent_scope()
                .is_none_or(|s| s.kind != ScopeKind::Class)
            {
                return None;
            }
            Some(inline_definition(decl, symbol, config))
        }
        Target::CurrentFile => Some(out_of_line_definition(
            decl, path, decl, &tree, true, config,
        )),
        Target::MatchingSource => {
            let source = matching_source?;
            if !decl.is_header || !can_define_in_source_file(decl.text, symbol) {
                return None;
            }
            Some(out_of_line_definition(
                decl,
                path,
                source,
                source_tree.as_ref()?,
                false,
                config,
            ))
        }
    }
}

fn declaration_at(tree: &SymbolTree, offset: usize) -> Option<SymbolPath<'_>> {
    tree.symbol_at(offset)
        .filter(|path| path.symbol().is_function_declaration())
}

fn has_definition(symbol: &Symbol, tree: &SymbolTree, source: Option<&SymbolTree>) -> bool {
    tree.find_definition(symbol).is_some()
        || source.is_some_and(|s| s.find_definition(symbol).is_some())
}

/// Inline, constexpr, consteval and template functions must be defined where they are declared.
fn can_define_in_source_file(text: &str, symbol: &Symbol) -> bool {
    !["inline", "constexpr", "consteval"]
        .iter()
        .any(|word| leading_text_has(text, symbol, word))
        && !symbol.has_unspecialized_template()
}

fn indent_unit(target: &str, decl: &str) -> String {
    detect_indentation(target)
        .or_else(|| detect_indentation(decl))
        .unwrap_or_else(|| "    ".into())
}

fn body(on_new_line: bool, indentation: &str, unit: &str) -> String {
    if on_new_line {
        format!("\n{indentation}{{\n{indentation}{unit}\n{indentation}}}")
    } else {
        format!(" {{\n{indentation}{unit}\n{indentation}}}")
    }
}

fn with_eol(text: String, eol: &str) -> String {
    if eol == "\n" {
        text
    } else {
        text.replace('\n', eol)
    }
}

fn utf16_len(s: &str) -> u32 {
    s.chars().map(char::len_utf16).sum::<usize>() as u32
}

fn inline_definition(decl: &FileInput, symbol: &Symbol, config: &Config) -> Insertion {
    let func = symbol.function.as_ref().expect("declaration is a function");
    let text = decl.text;
    let indentation = leading_whitespace(&text[line_start(text, func.decl_start)..]);
    let unit = indent_unit(text, text);
    let new_line = config
        .function_brace_format(decl.is_c)
        .brace_on_new_line(func.is_constructor || func.is_destructor);
    let new_text = body(new_line, indentation, &unit);

    let start = func.decl_end;
    let end = symbol.range.end;
    let cursor = Position::new(
        line_of(text, start) as u32 + if new_line { 2 } else { 1 },
        utf16_len(indentation) + utf16_len(&unit),
    );
    Insertion {
        range: Range::new(
            offset_to_position(text, start),
            offset_to_position(text, end),
        ),
        new_text: with_eol(new_text, detect_eol(text)),
        cursor,
    }
}

fn out_of_line_definition(
    decl: &FileInput,
    path: SymbolPath,
    target: &FileInput,
    target_tree: &SymbolTree,
    same_file: bool,
    config: &Config,
) -> Insertion {
    let symbol = path.symbol();
    let func = symbol.function.as_ref().expect("declaration is a function");
    let position =
        find_position_for_definition(decl.text, path, target.text, target_tree, same_file);

    let signature = format_definition(decl, path, target, target_tree, &position, same_file);
    let unit = indent_unit(target.text, decl.text);
    let new_line = config
        .function_brace_format(target.is_c)
        .brace_on_new_line(func.is_constructor || func.is_destructor);
    let skeleton = signature + &body(new_line, "", &unit);
    let insert = format_text_to_insert(&skeleton, &position, target.text, &unit);

    let start = offset_to_position(target.text, position.offset);
    let lines: Vec<&str> = insert.split('\n').collect();
    let cursor = lines
        .iter()
        .position(|line| line.trim_end().ends_with('{'))
        .and_then(|i| Some((i, lines.get(i + 1)?)))
        .map_or(start, |(i, line)| {
            Position::new(start.line + i as u32 + 1, utf16_len(line))
        });

    Insertion {
        range: Range::new(start, start),
        new_text: with_eol(insert, detect_eol(target.text)),
        cursor,
    }
}

/// Formats the declaration as a definition head without the body (`CSymbol.formatDeclaration`
/// with `checkForInline`). Uses `\n` line endings.
fn format_definition(
    decl: &FileInput,
    path: SymbolPath,
    target: &FileInput,
    target_tree: &SymbolTree,
    position: &ProposedPosition,
    same_file: bool,
) -> String {
    let text = decl.text;
    let symbol = path.symbol();
    let func = symbol.function.as_ref().expect("declaration is a function");

    // Scope qualifiers, omitting the scopes that the target position is already inside.
    let scopes: Vec<_> = symbol
        .scopes
        .iter()
        .filter(|s| !func.is_friend || s.kind == ScopeKind::Namespace)
        .collect();
    let target_scopes = target_tree.scopes_at(position.offset);
    let common = scopes
        .iter()
        .zip(&target_scopes)
        .take_while(|(a, b)| a.kind == b.kind && a.name == b.name)
        .count();
    let mut scope_string: String = scopes[common..]
        .iter()
        .filter(|s| !s.is_anonymous())
        .map(|s| format!("{}::", s.qualifier))
        .collect();
    let old_scope = &text[func.scope_start..func.name_range.start];
    scope_string.push_str(&old_scope.split_whitespace().collect::<String>());

    let inline_specifier = {
        let outside_parent = path
            .parent
            .is_none_or(|p| !(p.range.start < position.offset && position.offset < p.range.end))
            || !same_file;
        let already = ["inline", "constexpr", "consteval"]
            .iter()
            .any(|w| leading_text_has(text, symbol, w));
        if outside_parent && (same_file || target.is_header) && !already {
            "inline "
        } else {
            ""
        }
    };

    // Text before the name (return type and specifiers). Continuation lines of the declaration
    // are assumed to be aligned relative to the column of the name.
    let new_line_alignment = func.name_range.start - line_start(text, func.name_range.start);

    let mut leading = text[func.decl_start..func.scope_start].replace("\r\n", "\n");
    for word in ["virtual", "static", "explicit", "friend"] {
        leading = remove_word_and_trailing_space(&leading, word, false);
    }
    if !target.is_header {
        leading = remove_word_and_trailing_space(&leading, "inline", true);
    }
    let true_start_indent = leading_whitespace(&text[line_start(text, symbol.range.start)..]);
    leading = dedent(&leading, true_start_indent);

    // Name through the end of the declaration, without default arguments.
    let mut params = String::new();
    let mut pos = func.params.start;
    for range in &func.default_args {
        params.push_str(&text[pos..range.start]);
        pos = range.end;
    }
    params.push_str(&text[pos..func.params.end]);
    let mut definition = format!(
        "{}{}{}",
        &text[func.name_range.start..func.params.start],
        params,
        &text[func.params.end..func.decl_end],
    )
    .replace("\r\n", "\n");

    // Re-align continuation lines of a multi-line declaration.
    let new_align_length = leading.rsplit('\n').next().unwrap_or("").len();
    let inline_alignment = if leading.contains('\n') {
        0
    } else {
        inline_specifier.len()
    };
    if new_line_alignment > 0 {
        let old = format!("\n{}", " ".repeat(new_line_alignment));
        let new = format!(
            "\n{}",
            " ".repeat(new_align_length + inline_alignment + scope_string.len())
        );
        definition = definition.replace(&old, &new);
    }

    // Template statements of enclosing class templates, then the function's own.
    let mut templates = String::new();
    for scope in &scopes {
        if scope.kind == ScopeKind::Class
            && let Some(template) = &scope.template
        {
            templates.push_str(template);
            templates.push('\n');
        }
    }
    for template in &func.templates {
        templates.push_str(template);
        templates.push('\n');
    }

    let mut result = format!("{templates}{inline_specifier}{leading}{scope_string}{definition}");
    for word in ["override", "final"] {
        result = remove_word_and_leading_space(&result, word);
    }
    result
}
