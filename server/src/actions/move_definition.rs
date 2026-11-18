//! Move Definition (port of `commands/moveDefinition.ts`).
//!
//! Moves a function definition to the matching source/header file, into its class (replacing
//! the declaration) or out of its class (leaving a declaration behind). The body text is kept
//! as it is, re-indented for its new place.

use std::ops::Range;

use tower_lsp_server::ls_types::{Position, TextEdit};

use crate::actions::add_declaration::{
    class_title, definition_at, find_parent_class, has_internal_linkage, in_class_body,
    neighbour_declaration,
};
use crate::actions::add_definition::{FileInput, HeadKind, format_head, indent_unit, with_eol};
use crate::actions::member::{ClassEditor, GeneratedEdit, NewMember};
use crate::config::{Config, DefinitionLocation};
use crate::edit_buffer::EditBuffer;
use crate::placement::{
    ProposedPosition, find_position_for_definition, format_text_to_insert, trailing_comment_end,
};
use crate::symbol::{ClassKey, Symbol, SymbolPath, SymbolTree, leading_text_has};
use crate::text::{detect_eol, leading_whitespace, line_end, line_start, position_to_offset};

pub const TITLE_MATCHING_SOURCE: &str = "Move Definition to matching source file";
pub const TITLE_MATCHING_HEADER: &str = "Move Definition to matching header file";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTarget {
    /// From a header to its matching source file.
    MatchingSource,
    /// From a source file to its matching header.
    MatchingHeader,
    /// An out-of-class member definition into its class.
    IntoClass,
    /// An in-class member definition below the class.
    OutOfClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveAction {
    pub target: MoveTarget,
    pub title: String,
    /// The edit changes the matching header/source file.
    pub edits_other_file: bool,
}

/// The definition at the cursor and its declaration (with whether that is in the current file).
struct Found<'a> {
    def: SymbolPath<'a>,
    decl: Option<(SymbolPath<'a>, bool)>,
}

fn find<'a>(
    tree: &'a SymbolTree,
    other_tree: Option<&'a SymbolTree>,
    offset: usize,
) -> Option<Found<'a>> {
    let def = definition_at(tree, offset)?;
    let symbol = def.symbol();
    let decl = if in_class_body(symbol) {
        None
    } else {
        tree.find_declaration(symbol)
            .map(|d| (d, true))
            .or_else(|| other_tree?.find_declaration(symbol).map(|d| (d, false)))
    };
    Some(Found { def, decl })
}

/// Whether a class scope of `symbol` is anonymous (it can't be named in a qualifier).
fn in_anonymous_class(symbol: &Symbol) -> bool {
    symbol
        .scopes
        .iter()
        .any(|s| s.kind == crate::symbol::ScopeKind::Class && s.is_anonymous())
}

fn key_word(class: &Symbol) -> &'static str {
    match class.class.as_ref().map(|c| c.key) {
        Some(ClassKey::Struct) => "struct",
        _ => "class",
    }
}

/// The Move Definition actions for the definition at `position`. `other` is the matching
/// header/source file.
pub fn available_moves(
    file: &FileInput,
    position: Position,
    other: Option<&FileInput>,
) -> Vec<MoveAction> {
    let other = other.filter(|o| o.is_header != file.is_header);
    let tree = SymbolTree::new(file.text);
    let other_tree = other.map(|o| SymbolTree::new(o.text));
    let offset = position_to_offset(file.text, position);
    let Some(found) = find(&tree, other_tree.as_ref(), offset) else {
        return Vec::new();
    };
    let def = found.def.symbol();
    let func = def.function.as_ref().expect("definition is a function");
    let text = file.text;
    let in_class = in_class_body(def);
    let decl_text = |current: bool| {
        if current {
            text
        } else {
            other.map_or("", |o| o.text)
        }
    };
    let mut actions = Vec::new();

    if file.is_header && other.is_some() {
        let inline_blocked = leading_text_has(text, def, "inline")
            && found.decl.is_none_or(|(d, current)| {
                leading_text_has(decl_text(current), d.symbol(), "inline")
            });
        let allowed = !inline_blocked
            && !leading_text_has(text, def, "constexpr")
            && !leading_text_has(text, def, "consteval")
            && !def.has_unspecialized_template()
            && found
                .decl
                .is_none_or(|(d, _)| !d.symbol().has_unspecialized_template())
            && !in_anonymous_class(def)
            && (in_class || found.decl.is_some() || func.qualifiers.is_empty())
            && (in_class || !has_internal_linkage(text, def));
        if allowed {
            actions.push(MoveAction {
                target: MoveTarget::MatchingSource,
                title: TITLE_MATCHING_SOURCE.into(),
                edits_other_file: true,
            });
        }
    }
    if !file.is_header && other.is_some() && !in_class {
        let allowed = match found.decl {
            Some((_, current)) => !current,
            None => func.qualifiers.is_empty() && !has_internal_linkage(text, def),
        };
        if allowed {
            actions.push(MoveAction {
                target: MoveTarget::MatchingHeader,
                title: TITLE_MATCHING_HEADER.into(),
                edits_other_file: true,
            });
        }
    }
    if file.is_c {
        return actions;
    }
    if in_class {
        // A friend defined in a class template can't be defined outside it (it names `Box`
        // without template arguments).
        let template_friend = func.is_friend && def.scopes.iter().any(|s| s.template.is_some());
        if let Some(class) = found
            .def
            .parent
            .filter(|_| !in_anonymous_class(def) && !template_friend)
        {
            actions.push(MoveAction {
                target: MoveTarget::OutOfClass,
                title: format!("Move Definition below {} body", key_word(class)),
                edits_other_file: false,
            });
        }
    } else if !func.qualifiers.is_empty() {
        let class = match found.decl {
            Some((d, current)) => d
                .parent
                .filter(|p| p.class.is_some() && d.symbol().is_class_member())
                .map(|p| (p, current)),
            None => find_parent_class(def, &tree, other_tree.as_ref())
                .map(|(c, current)| (c.symbol(), current)),
        };
        if let Some((class, current)) = class {
            actions.push(MoveAction {
                target: MoveTarget::IntoClass,
                title: class_title("Move Definition into", class),
                edits_other_file: !current,
            });
        }
    }
    actions
}

/// Builds the edit for `target`. Edits to the matching file go into
/// [`GeneratedEdit::other_edits`]; for moves to the matching file, `other_cursor` is the start
/// of the moved definition.
pub fn move_definition(
    file: &FileInput,
    position: Position,
    target: MoveTarget,
    other: Option<&FileInput>,
    config: &Config,
) -> Option<GeneratedEdit> {
    if !available_moves(file, position, other)
        .iter()
        .any(|a| a.target == target)
    {
        return None;
    }
    let other = other.filter(|o| o.is_header != file.is_header);
    let tree = SymbolTree::new(file.text);
    let other_tree = other.map(|o| SymbolTree::new(o.text));
    let found = find(
        &tree,
        other_tree.as_ref(),
        position_to_offset(file.text, position),
    )?;
    match target {
        MoveTarget::MatchingSource | MoveTarget::MatchingHeader => {
            to_other_file(file, &found, other?, other_tree.as_ref()?, config)
        }
        MoveTarget::OutOfClass => out_of_class(file, &tree, &found, config),
        MoveTarget::IntoClass => match found.decl {
            Some(decl) => into_class(file, &found, decl, other),
            None => into_class_without_declaration(
                file,
                &tree,
                &found,
                other,
                other_tree.as_ref(),
                config,
            ),
        },
    }
}

/// Leading whitespace of the line containing `offset`.
fn indentation_at(text: &str, offset: usize) -> &str {
    leading_whitespace(&text[line_start(text, offset)..])
}

/// Removes the part of `prefix` that each line after the first starts with (so lines indented
/// less, or with other whitespace, lose what they have in common with it).
fn dedent_lines(text: &str, prefix: &str) -> String {
    let mut lines = text.split('\n');
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        let common = line
            .bytes()
            .zip(prefix.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        out.push_str(&line[common..]);
    }
    out
}

/// Everything after the head of the definition: initializer list and body, de-indented
/// (`\n` line endings).
fn body_text(text: &str, def: &Symbol) -> String {
    let func = def.function.as_ref().expect("definition is a function");
    dedent_lines(
        &text[func.decl_end..def.range.end].replace("\r\n", "\n"),
        indentation_at(text, def.range.start),
    )
}

/// The leading comment of `def`, de-indented, ending with a newline (empty if none).
fn comment_text(text: &str, def: &Symbol) -> String {
    if def.comment_start >= def.range.start {
        return String::new();
    }
    let end = line_start(text, def.range.start);
    let comment = if end > def.comment_start {
        text[def.comment_start..end].to_string()
    } else {
        format!("{}\n", text[def.comment_start..def.range.start].trim_end())
    };
    dedent_lines(
        &comment.replace("\r\n", "\n"),
        indentation_at(text, def.range.start),
    )
}

/// `CSymbol.newFunctionDeclaration`: the definition's head followed by `;`.
fn declaration_text(text: &str, def: &Symbol) -> String {
    let func = def.function.as_ref().expect("definition is a function");
    format!("{};", text[def.range.start..func.decl_end].trim_end())
}

fn next_line_start(text: &str, line_end: usize) -> usize {
    if text[line_end..].starts_with("\r\n") {
        line_end + 2
    } else if text[line_end..].starts_with('\n') {
        line_end + 1
    } else {
        line_end
    }
}

/// What to delete when a definition is moved away: its lines including comments and one
/// adjacent blank line (`getDeletionRange`), or only the definition if it shares its lines.
fn deletion_range(text: &str, def: &Symbol) -> Range<usize> {
    let start = def.comment_start;
    let end = trailing_comment_end(text, def);
    let first = line_start(text, start);
    let last = line_end(text, end);
    if !text[first..start].trim().is_empty() || !text[end..last].trim().is_empty() {
        return start..end;
    }
    let is_blank = |line: usize| text[line..line_end(text, line)].trim().is_empty();
    let mut range = first..next_line_start(text, last);
    if range.end < text.len() && is_blank(range.end) {
        range.end = next_line_start(text, line_end(text, range.end));
    } else if range.start > 0 && is_blank(line_start(text, range.start - 1)) {
        range.start = line_start(text, range.start - 1);
    }
    range
}

/// Formats `text` for `position` in `target` and converts it to the target's line endings.
fn format_for(text: &str, position: &ProposedPosition, target: &str, unit: &str) -> String {
    with_eol(
        format_text_to_insert(text, position, target, unit),
        detect_eol(target),
    )
}

/// Applies edits (byte ranges into the original `text`, non-overlapping) to a buffer.
fn edits_for(text: &str, mut edits: Vec<(Range<usize>, String)>) -> Vec<TextEdit> {
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut buffer = EditBuffer::new(text);
    for (range, new_text) in edits {
        buffer.replace(range, &new_text);
    }
    buffer.edits()
}

/// The new definition: comment (optional), head for `position` in `target`, original body.
fn moved_definition(
    file: &FileInput,
    def: SymbolPath,
    target: &FileInput,
    target_tree: &SymbolTree,
    position: &ProposedPosition,
    same_file: bool,
    with_comment: bool,
) -> String {
    let symbol = def.symbol();
    let head = format_head(
        file.text,
        def,
        target,
        &target_tree.scopes_at(position.offset),
        position.offset,
        same_file,
        HeadKind::Definition,
    );
    let comment = if with_comment {
        comment_text(file.text, symbol)
    } else {
        String::new()
    };
    format!("{comment}{head}{}", body_text(file.text, symbol))
}

fn to_other_file(
    file: &FileInput,
    found: &Found,
    other: &FileInput,
    other_tree: &SymbolTree,
    config: &Config,
) -> Option<GeneratedEdit> {
    let def = found.def.symbol();
    // A header definition without a declaration leaves one behind.
    let leave_declaration = found.decl.is_none() && file.is_header;
    let with_comment = !leave_declaration || config.always_move_comments;
    let position = match found.decl {
        Some((decl, current)) => {
            let decl_text = if current { file.text } else { other.text };
            find_position_for_definition(decl_text, decl, other.text, other_tree, !current)
        }
        None => find_position_for_definition(file.text, found.def, other.text, other_tree, false),
    };
    let text = moved_definition(
        file,
        found.def,
        other,
        other_tree,
        &position,
        false,
        with_comment,
    );
    let unit = indent_unit(other.text, file.text);
    let formatted = format_for(&text, &position, other.text, &unit);
    let mut other_buffer = EditBuffer::new(other.text);
    other_buffer.insert(position.offset, &formatted);
    let cursor = position.offset + (formatted.len() - formatted.trim_start().len());

    let edits = if leave_declaration {
        let start = if with_comment {
            def.comment_start
        } else {
            def.range.start
        };
        vec![(start..def.range.end, declaration_text(file.text, def))]
    } else {
        vec![(deletion_range(file.text, def), String::new())]
    };
    Some(GeneratedEdit {
        edits: edits_for(file.text, edits),
        other_edits: other_buffer.edits(),
        other_cursor: Some(other_buffer.position(cursor)),
    })
}

fn out_of_class(
    file: &FileInput,
    tree: &SymbolTree,
    found: &Found,
    config: &Config,
) -> Option<GeneratedEdit> {
    let def = found.def.symbol();
    let with_comment = config.always_move_comments;
    let position = find_position_for_definition(file.text, found.def, file.text, tree, true);
    let replaced_start = if with_comment {
        def.comment_start
    } else {
        def.range.start
    };
    let replaced = replaced_start..def.range.end;
    if replaced.contains(&position.offset) {
        return None;
    }
    let text = moved_definition(file, found.def, file, tree, &position, true, with_comment);
    let unit = indent_unit(file.text, file.text);
    let formatted = format_for(&text, &position, file.text, &unit);
    Some(GeneratedEdit {
        edits: edits_for(
            file.text,
            vec![
                (replaced, declaration_text(file.text, def)),
                (position.offset..position.offset, formatted),
            ],
        ),
        ..Default::default()
    })
}

/// The declaration's head with the parameter names of the definition (the body uses those).
fn head_with_definition_names(decl_text: &str, decl: &Symbol, text: &str, def: &Symbol) -> String {
    let df = decl.function.as_ref().expect("declaration is a function");
    let f = def.function.as_ref().expect("definition is a function");
    let start = decl.range.start;
    let mut head = decl_text[start..df.decl_end].to_string();
    if df.param_ranges.len() == f.param_ranges.len() {
        for i in (0..df.param_ranges.len()).rev() {
            let (name, def_name) = (&df.param_names[i], &f.param_names[i]);
            if !def_name.is_empty() && name != def_name {
                let range = &df.param_ranges[i];
                head.replace_range(
                    range.start - start..range.end - start,
                    &text[f.param_ranges[i].clone()],
                );
            }
        }
    }
    head
}

/// `CSymbol.combineDefinition`: replaces the declaration in the class with the declaration's
/// head plus the definition's body, and removes the definition.
fn into_class(
    file: &FileInput,
    found: &Found,
    (decl, current): (SymbolPath, bool),
    other: Option<&FileInput>,
) -> Option<GeneratedEdit> {
    let def = found.def.symbol();
    let decl_file = if current { *file } else { *other? };
    let decl = decl.symbol();
    let indentation = indentation_at(decl_file.text, decl.range.start);
    let reindent = |s: &str| {
        let mut out = String::new();
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
                if !line.is_empty() {
                    out.push_str(indentation);
                }
            }
            out.push_str(line);
        }
        out
    };
    let head = head_with_definition_names(decl_file.text, decl, file.text, def);
    // The definition's comment goes with it (after the declaration's comment, if any).
    let comment = comment_text(file.text, def);
    let comment = if comment.is_empty() {
        String::new()
    } else {
        format!(
            "{}{indentation}",
            reindent(comment.trim_end_matches('\n')) + "\n"
        )
    };
    let eol = detect_eol(decl_file.text);
    let combined = format!(
        "{}{head}{}",
        with_eol(comment, eol),
        with_eol(reindent(&body_text(file.text, def)), eol)
    );
    let replace = (decl.range.start..decl.range.end, combined);
    let delete = (deletion_range(file.text, def), String::new());
    Some(if current {
        GeneratedEdit {
            edits: edits_for(file.text, vec![replace, delete]),
            ..Default::default()
        }
    } else {
        GeneratedEdit {
            edits: edits_for(file.text, vec![delete]),
            other_edits: edits_for(decl_file.text, vec![replace]),
            other_cursor: None,
        }
    })
}

/// A member definition without a declaration: the whole definition goes into the class
/// (public section, next to a neighbour's declaration where possible).
fn into_class_without_declaration(
    file: &FileInput,
    tree: &SymbolTree,
    found: &Found,
    other: Option<&FileInput>,
    other_tree: Option<&SymbolTree>,
    config: &Config,
) -> Option<GeneratedEdit> {
    let def = found.def.symbol();
    let (class, current) = find_parent_class(def, tree, other_tree)?;
    let (class_file, class_tree) = if current {
        (*file, tree)
    } else {
        (*other?, other_tree?)
    };
    let class = class.symbol();
    let inside = class
        .body
        .as_ref()
        .map_or(class.range.start, |b| b.start + 1);
    let head = format_head(
        file.text,
        found.def,
        &class_file,
        &class_tree.scopes_at(inside),
        inside,
        false,
        HeadKind::Definition,
    );
    let inline_text = format!(
        "{}{head}{}",
        comment_text(file.text, def),
        body_text(file.text, def)
    );
    let relative = neighbour_declaration(found.def, class_tree, class);
    let mut editor = ClassEditor::new(class_file, None, class.range.start, config);
    editor.compact = true;
    let deletion = deletion_range(file.text, def);
    if current {
        editor.replace(deletion.clone(), "");
    }
    editor.add(&NewMember {
        name: def.name.clone(),
        declaration: String::new(),
        inline_text,
        body: String::new(),
        location: DefinitionLocation::Inline,
        relative,
    })?;
    let generated = editor.finish();
    Some(if current {
        generated
    } else {
        GeneratedEdit {
            edits: edits_for(file.text, vec![(deletion, String::new())]),
            other_edits: generated.edits,
            other_cursor: None,
        }
    })
}
