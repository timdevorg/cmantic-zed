//! Add Definition (port of `commands/addDefinition.ts` and `CSymbol.formatDeclaration`).
//!
//! Pure text-in, edit-out logic; the LSP wiring lives in `server.rs`.

use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::config::Config;
use crate::edit_buffer::EditBuffer;
use crate::placement::{ProposedPosition, find_position_for_definition, format_text_to_insert};
use crate::symbol::{Scope, ScopeKind, Symbol, SymbolPath, SymbolTree, dedent, leading_text_has};
use crate::text::{
    detect_eol, detect_indentation, leading_whitespace, line_start, offset_to_position,
    position_to_offset, remove_word_and_leading_space, remove_word_and_trailing_space,
};

pub const TITLE_MATCHING_SOURCE: &str = "Add Definition in matching source file";
pub const TITLE_CURRENT_FILE: &str = "Add Definition in this file";
pub const TITLE_INLINE: &str = "Add Definition inline";
pub const TITLE_ALL_MATCHING_SOURCE: &str = "Add all missing definitions in matching source file";
pub const TITLE_ALL_CURRENT_FILE: &str = "Add all missing definitions in this file";
pub const TITLE_CTOR_MATCHING_SOURCE: &str =
    "Generate Constructor in matching source file (initialize all members)";
pub const TITLE_CTOR_CURRENT_FILE: &str =
    "Generate Constructor in this file (initialize all members)";

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

    /// Title of the "Generate Constructor (initialize all members)" variant. Not offered
    /// inline (the declaration is right there to edit; it would only add menu clutter).
    pub fn constructor_title(self) -> Option<&'static str> {
        match self {
            Self::MatchingSource => Some(TITLE_CTOR_MATCHING_SOURCE),
            Self::CurrentFile => Some(TITLE_CTOR_CURRENT_FILE),
            Self::Inline => None,
        }
    }
}

/// Which member initializers a new constructor definition gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Initializers {
    /// Empty body, no initializer list.
    None,
    /// Only members that must be initialized (references and const members without a
    /// default member initializer).
    Required,
    /// Base classes and all non-static data members, in declaration order.
    All,
}

/// Where "Add all missing definitions" puts the definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkTarget {
    MatchingSource,
    CurrentFile,
}

impl BulkTarget {
    pub fn title(self) -> &'static str {
        match self {
            Self::MatchingSource => TITLE_ALL_MATCHING_SOURCE,
            Self::CurrentFile => TITLE_ALL_CURRENT_FILE,
        }
    }
}

/// Edits for several definitions in one file, plus the cursor inside the first new body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkEdit {
    /// Non-overlapping edits against the target file's original text.
    pub edits: Vec<TextEdit>,
    /// Cursor in the target file after the edits are applied.
    pub cursor: Position,
    /// Number of definitions added.
    pub count: usize,
}

/// The file a definition goes into.
#[derive(Clone, Copy)]
pub(crate) struct Destination<'a> {
    pub file: &'a FileInput<'a>,
    pub tree: &'a SymbolTree,
    pub same_file: bool,
}

/// A definition to insert, in byte offsets.
pub(crate) struct DefinitionText {
    pub offset: usize,
    pub text: String,
    /// Offset of the cursor inside `text`.
    pub cursor: usize,
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
    add_definition_with(
        decl,
        position,
        target,
        matching_source,
        Initializers::None,
        config,
    )
}

/// Whether the declaration at `position` is a constructor of a class with base classes or
/// non-static data members, so "Generate Constructor (initialize all members)" makes sense.
pub fn offers_constructor_initializers(decl: &FileInput, position: Position) -> bool {
    let tree = SymbolTree::new(decl.text);
    let Some(path) = declaration_at(&tree, position_to_offset(decl.text, position)) else {
        return false;
    };
    !initializer_names(path, Initializers::All).is_empty()
}

/// Like [`add_definition`], with member initializers for constructors.
pub fn add_definition_with(
    decl: &FileInput,
    position: Position,
    target: Target,
    matching_source: Option<&FileInput>,
    initializers: Initializers,
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
            Some(inline_definition(decl, path, initializers, config))
        }
        Target::CurrentFile => Some(out_of_line_definition(
            decl,
            path,
            decl,
            &tree,
            true,
            initializers,
            config,
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
                initializers,
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

pub(crate) fn indent_unit(target: &str, decl: &str) -> String {
    detect_indentation(target)
        .or_else(|| detect_indentation(decl))
        .unwrap_or_else(|| "    ".into())
}

/// A function body. `content` (may be empty) goes on the line after `{`, indented by `unit`;
/// its continuation lines keep their own indentation relative to the function.
fn body(on_new_line: bool, indentation: &str, unit: &str, content: &str) -> String {
    let content = content.replace('\n', &format!("\n{indentation}"));
    if on_new_line {
        format!("\n{indentation}{{\n{indentation}{unit}{content}\n{indentation}}}")
    } else {
        format!(" {{\n{indentation}{unit}{content}\n{indentation}}}")
    }
}

pub(crate) fn with_eol(text: String, eol: &str) -> String {
    if eol == "\n" {
        text
    } else {
        text.replace('\n', eol)
    }
}

fn utf16_len(s: &str) -> u32 {
    s.chars().map(char::len_utf16).sum::<usize>() as u32
}

fn inline_definition(
    decl: &FileInput,
    path: SymbolPath,
    initializers: Initializers,
    config: &Config,
) -> Insertion {
    let symbol = path.symbol();
    let func = symbol.function.as_ref().expect("declaration is a function");
    let text = decl.text;
    let indentation = leading_whitespace(&text[line_start(text, func.decl_start)..]);
    let unit = indent_unit(text, text);
    let new_line = config
        .function_brace_format(decl.is_c)
        .brace_on_new_line(func.is_constructor || func.is_destructor);
    let init_list = initializer_list(path, initializers, &unit, config)
        .replace('\n', &format!("\n{indentation}"));
    let new_text = init_list + &body(new_line, indentation, &unit, "");

    let start = func.decl_end;
    let end = symbol.range.end;
    let cursor = cursor_in(&new_text);
    let mut after = text[..start].to_string();
    after.push_str(&new_text[..cursor]);
    Insertion {
        range: Range::new(
            offset_to_position(text, start),
            offset_to_position(text, end),
        ),
        new_text: with_eol(new_text, detect_eol(text)),
        cursor: offset_to_position(&after, after.len()),
    }
}

/// Names to initialize in a constructor's member initializer list.
fn initializer_names(path: SymbolPath, initializers: Initializers) -> Vec<String> {
    let symbol = path.symbol();
    let is_constructor = symbol.function.as_ref().is_some_and(|f| f.is_constructor);
    let Some(class) = path.parent.filter(|p| p.class.is_some()) else {
        return Vec::new();
    };
    if !is_constructor || initializers == Initializers::None {
        return Vec::new();
    }
    let members = class
        .non_static_member_variables()
        .filter(|v| initializers == Initializers::All || v.requires_initialization())
        .map(|v| v.name.clone());
    match initializers {
        Initializers::All => class
            .class
            .iter()
            .flat_map(|c| &c.base_classes)
            .map(|b| b.name.clone())
            .chain(members)
            .collect(),
        _ => members.collect(),
    }
}

/// `\n<unit>: a(),\n<unit>  b()` (`constructInitializerList`), or empty.
fn initializer_list(
    path: SymbolPath,
    initializers: Initializers,
    unit: &str,
    config: &Config,
) -> String {
    let names = initializer_names(path, initializers);
    if names.is_empty() {
        return String::new();
    }
    let parens = if config.cpp.braced_initialization {
        "{}"
    } else {
        "()"
    };
    let separator = format!(",\n{unit}  ");
    let list: Vec<String> = names.iter().map(|n| format!("{n}{parens}")).collect();
    format!("\n{unit}: {}", list.join(&separator))
}

/// Byte offset of the cursor in a formatted definition (`getPositionForCursor`): inside the
/// first member initializer if there is one, else at the end of the line after `{`.
fn cursor_in(text: &str) -> usize {
    let mut offset = 0;
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(':') && !trimmed.starts_with("::") {
            if let Some(index) = line.rfind([')', '}']) {
                return offset + index;
            }
            return 0;
        }
        if line.trim_end().ends_with('{')
            && let Some(next) = lines.get(i + 1)
        {
            return offset + line.len() + 1 + next.len();
        }
        offset += line.len() + 1;
    }
    0
}

fn out_of_line_definition(
    decl: &FileInput,
    path: SymbolPath,
    target: &FileInput,
    target_tree: &SymbolTree,
    same_file: bool,
    initializers: Initializers,
    config: &Config,
) -> Insertion {
    let unit = indent_unit(target.text, decl.text);
    let destination = Destination {
        file: target,
        tree: target_tree,
        same_file,
    };
    let definition = definition_text(decl, path, destination, initializers, "", &unit, config);
    let start = offset_to_position(target.text, definition.offset);
    let before_cursor = &definition.text[..definition.cursor];
    let lines = before_cursor.matches('\n').count() as u32;
    let last_line = before_cursor.rsplit('\n').next().unwrap_or("");
    let cursor = if lines == 0 {
        Position::new(start.line, start.character + utf16_len(last_line))
    } else {
        Position::new(start.line + lines, utf16_len(last_line))
    };
    Insertion {
        range: Range::new(start, start),
        new_text: with_eol(definition.text, detect_eol(target.text)),
        cursor,
    }
}

/// The formatted out-of-line definition of the declaration at `path` and where it goes
/// (`\n` line endings). `body` is the body's content (see [`body`]), usually empty.
pub(crate) fn definition_text(
    decl: &FileInput,
    path: SymbolPath,
    destination: Destination,
    initializers: Initializers,
    body_content: &str,
    unit: &str,
    config: &Config,
) -> DefinitionText {
    let Destination {
        file: target,
        tree: target_tree,
        same_file,
    } = destination;
    let symbol = path.symbol();
    let func = symbol.function.as_ref().expect("declaration is a function");
    let position =
        find_position_for_definition(decl.text, path, target.text, target_tree, same_file);

    let signature = format_definition(decl, path, target, target_tree, &position, same_file);
    let new_line = config
        .function_brace_format(target.is_c)
        .brace_on_new_line(func.is_constructor || func.is_destructor);
    let skeleton = signature
        + &initializer_list(path, initializers, unit, config)
        + &body(new_line, "", unit, body_content);
    let text = format_text_to_insert(&skeleton, &position, target.text, unit);
    DefinitionText {
        offset: position.offset,
        cursor: cursor_in(&text),
        text,
    }
}

/// Whether "Add all missing definitions" can be offered at `position`, and for which targets.
///
/// Offered when the cursor is on a class/struct head (outside its body) and the file has at
/// least one undefined function declaration. Not offered on individual declarations, to keep
/// the menu short there.
pub fn available_bulk_targets(
    decl: &FileInput,
    position: Position,
    matching_source: Option<&FileInput>,
) -> Vec<BulkTarget> {
    let tree = SymbolTree::new(decl.text);
    let offset = position_to_offset(decl.text, position);
    let Some(at) = tree.symbol_at(offset) else {
        return Vec::new();
    };
    if !is_on_class_head(at.symbol(), offset) {
        return Vec::new();
    }
    let source_tree = matching_source.map(|s| SymbolTree::new(s.text));
    let undefined: Vec<SymbolPath> = declaration_paths(&tree)
        .into_iter()
        .filter(|p| !has_definition(p.symbol(), &tree, source_tree.as_ref()))
        .collect();
    if undefined.is_empty() {
        return Vec::new();
    }
    let mut targets = Vec::new();
    if decl.is_header
        && matching_source.is_some()
        && undefined
            .iter()
            .any(|p| can_define_in_source_file(decl.text, p.symbol()))
    {
        targets.push(BulkTarget::MatchingSource);
    }
    targets.push(BulkTarget::CurrentFile);
    targets
}

/// Whether `offset` is on the head of class `symbol` (its name, base list, ... but not its body).
pub fn is_on_class_head(symbol: &Symbol, offset: usize) -> bool {
    symbol.class.is_some() && symbol.body.as_ref().is_some_and(|b| offset <= b.start)
}

/// Adds definitions for all function declarations of `decl` that have none, in declaration
/// order. For [`BulkTarget::MatchingSource`], declarations that must stay in the header
/// (inline, constexpr, consteval, templates) are skipped. Constructors get initializers for
/// the members that require them. Returns `None` if there is nothing to add.
pub fn add_definitions(
    decl: &FileInput,
    target: BulkTarget,
    matching_source: Option<&FileInput>,
    config: &Config,
) -> Option<BulkEdit> {
    let tree = SymbolTree::new(decl.text);
    let source_tree = matching_source.map(|s| SymbolTree::new(s.text));
    let pending: Vec<usize> = declaration_paths(&tree)
        .iter()
        .enumerate()
        .filter(|(_, p)| !has_definition(p.symbol(), &tree, source_tree.as_ref()))
        .filter(|(_, p)| {
            target == BulkTarget::CurrentFile || can_define_in_source_file(decl.text, p.symbol())
        })
        .map(|(i, _)| i)
        .collect();
    if pending.is_empty() {
        return None;
    }
    let (target_file, same_file) = match target {
        BulkTarget::CurrentFile => (*decl, true),
        BulkTarget::MatchingSource => {
            if !decl.is_header {
                return None;
            }
            (*matching_source?, false)
        }
    };

    // Detected once: generated initializer lists would skew detection on later iterations.
    let unit = indent_unit(target_file.text, decl.text);
    let mut buffer = EditBuffer::new(target_file.text);
    let mut cursor: Option<usize> = None;
    let mut count = 0;
    for index in pending {
        // In the same file, declarations keep their order (only definitions are inserted),
        // so the index still identifies the declaration after earlier insertions.
        let decl_text = if same_file {
            buffer.text().to_string()
        } else {
            decl.text.to_string()
        };
        let decl_tree = SymbolTree::new(&decl_text);
        let target_tree = SymbolTree::new(buffer.text());
        let paths = declaration_paths(&decl_tree);
        let Some(&path) = paths.get(index) else {
            continue;
        };
        let current_decl = FileInput {
            text: &decl_text,
            ..*decl
        };
        if has_definition(path.symbol(), &decl_tree, Some(&target_tree)) {
            continue;
        }
        let current_target = FileInput {
            text: buffer.text(),
            ..target_file
        };
        let destination = Destination {
            file: &current_target,
            tree: &target_tree,
            same_file,
        };
        let definition = definition_text(
            &current_decl,
            path,
            destination,
            Initializers::Required,
            "",
            &unit,
            config,
        );
        let eol = detect_eol(target_file.text);
        // Each `\n` before the cursor becomes `\r\n` in a CRLF file.
        let cursor_offset = definition.cursor
            + (eol.len() - 1) * definition.text[..definition.cursor].matches('\n').count();
        let text = with_eol(definition.text, eol);
        match cursor.as_mut() {
            Some(c) if definition.offset <= *c => *c += text.len(),
            Some(_) => {}
            None => cursor = Some(definition.offset + cursor_offset),
        }
        buffer.insert(definition.offset, &text);
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some(BulkEdit {
        edits: buffer.edits(),
        cursor: buffer.position(cursor.unwrap_or(0)),
        count,
    })
}

/// Every function declaration (without a body) in the file, in document order.
fn declaration_paths(tree: &SymbolTree) -> Vec<SymbolPath<'_>> {
    fn walk<'a>(parent: Option<&'a Symbol>, siblings: &'a [Symbol], out: &mut Vec<SymbolPath<'a>>) {
        for (index, symbol) in siblings.iter().enumerate() {
            if symbol.is_function_declaration() {
                out.push(SymbolPath {
                    parent,
                    siblings,
                    index,
                });
            }
            walk(Some(symbol), &symbol.children, out);
        }
    }
    let mut out = Vec::new();
    walk(None, &tree.symbols, &mut out);
    out
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
    format_head(
        decl.text,
        path,
        target,
        &target_tree.scopes_at(position.offset),
        position.offset,
        same_file,
        HeadKind::Definition,
    )
}

/// What [`format_head`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeadKind {
    /// A definition head: `inline` is added where needed (`checkForInline`).
    Definition,
    /// A declaration head (without `;`): `inline` is removed. `static` is removed unless
    /// `keep_static` (forward declarations of internal functions in the same file).
    Declaration { keep_static: bool },
}

/// A namespace or class qualifying a function: an enclosing scope or an explicit qualifier
/// (`Foo<T>::` in `void Foo<T>::f()`).
struct ScopeEntry {
    name: String,
    /// What to write before `::`.
    text: String,
    /// `None` for explicit qualifiers (it isn't known whether they name a class or namespace).
    kind: Option<ScopeKind>,
    /// Template statement that the qualified function needs for this scope.
    template: Option<String>,
    anonymous: bool,
}

/// Splits explicit qualifiers (`ns::Foo<A::B>::`, whitespace removed) at top-level `::`.
fn split_qualifiers(scope: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    let mut chars = scope.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && chars.peek() == Some(&':') => {
                chars.next();
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// The scopes qualifying `symbol` (enclosing ones, then explicit qualifiers) and the
/// function's own template statements (those not consumed by templated explicit qualifiers).
fn scope_entries(text: &str, symbol: &Symbol) -> (Vec<ScopeEntry>, Vec<String>) {
    let func = symbol.function.as_ref().expect("symbol is a function");
    let mut entries: Vec<ScopeEntry> = symbol
        .scopes
        .iter()
        .filter(|s| !func.is_friend || s.kind == ScopeKind::Namespace)
        .map(|s| ScopeEntry {
            name: s.name.clone(),
            text: s.qualifier.clone(),
            kind: Some(s.kind),
            template: s.template.clone().filter(|_| s.kind == ScopeKind::Class),
            anonymous: s.is_anonymous(),
        })
        .collect();
    let old_scope: String = text[func.scope_start..func.name_range.start]
        .split_whitespace()
        .collect();
    let segments = split_qualifiers(&old_scope);
    // Template statements of an out-of-line member definition (`template <class T> void
    // Foo<T>::f()`) belong to the templated qualifiers first.
    let templated = segments.iter().filter(|s| s.contains('<')).count();
    let mut own = func.templates.clone();
    let mut class_templates = Vec::new();
    if templated > 0 && templated <= own.len() {
        class_templates = own.drain(..templated).collect();
    }
    let mut class_templates = class_templates.into_iter();
    for segment in segments {
        let template = if segment.contains('<') {
            class_templates.next()
        } else {
            None
        };
        let name = segment.split('<').next().unwrap_or("").to_string();
        entries.push(ScopeEntry {
            name,
            text: segment,
            kind: None,
            template,
            anonymous: false,
        });
    }
    (entries, own)
}

/// Formats the function at `path` (a declaration or a definition, in `text`) as a head for
/// `offset` in `target`, whose enclosing scopes are `target_scopes`: scope qualifiers are
/// added for the scopes the position isn't inside, default arguments and specifiers that are
/// only valid in a declaration are removed and multi-line declarations are re-aligned.
/// No body or `;`. Uses `\n` line endings.
pub(crate) fn format_head(
    text: &str,
    path: SymbolPath,
    target: &FileInput,
    target_scopes: &[Scope],
    offset: usize,
    same_file: bool,
    kind: HeadKind,
) -> String {
    let symbol = path.symbol();
    let func = symbol.function.as_ref().expect("symbol is a function");

    // Scope qualifiers, omitting the scopes that the target position is already inside.
    let (entries, own_templates) = scope_entries(text, symbol);
    let common = entries
        .iter()
        .zip(target_scopes)
        .take_while(|(a, b)| {
            a.kind.is_none_or(|k| k == b.kind)
                && a.name == b.name
                && !(a.kind.is_none() && a.name.is_empty())
        })
        .count();
    let scope_string: String = entries[common..]
        .iter()
        .filter(|s| !s.anonymous)
        .map(|s| format!("{}::", s.text))
        .collect();

    let target_in_class = target_scopes
        .last()
        .is_some_and(|s| s.kind == ScopeKind::Class);
    let already_inline = ["inline", "constexpr", "consteval"]
        .iter()
        .any(|w| leading_text_has(text, symbol, w));
    let inline_specifier = match kind {
        HeadKind::Definition if !target_in_class && !already_inline => {
            let inside_parent = same_file
                && path
                    .parent
                    .is_some_and(|p| p.range.start < offset && offset < p.range.end);
            // In a header, only a definition inside the class body is implicitly inline. A
            // definition inside a namespace block still needs `inline` to avoid
            // multiple-definition link errors (C-Mantic skipped it there).
            let inside_class = inside_parent
                && symbol
                    .parent_scope()
                    .is_some_and(|s| s.kind == ScopeKind::Class);
            let needs_inline = if target.is_header {
                !inside_class
            } else {
                same_file && !inside_parent
            };
            if needs_inline { "inline " } else { "" }
        }
        _ => "",
    };

    // Text before the name (return type and specifiers). Continuation lines of the declaration
    // are assumed to be aligned relative to the column of the name.
    let new_line_alignment = func.name_range.start - line_start(text, func.name_range.start);

    let mut leading = text[func.decl_start..func.scope_start].replace("\r\n", "\n");
    let keep_static = matches!(kind, HeadKind::Declaration { keep_static: true });
    for word in ["virtual", "static", "explicit", "friend"] {
        if !(word == "static" && keep_static) {
            leading = remove_word_and_trailing_space(&leading, word, false);
        }
    }
    if !target.is_header || matches!(kind, HeadKind::Declaration { .. }) {
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

    // Template statements of enclosing class templates the position isn't inside, then the
    // function's own.
    let mut templates = String::new();
    for template in entries[common..]
        .iter()
        .filter_map(|e| e.template.as_ref())
        .chain(&own_templates)
    {
        templates.push_str(template);
        templates.push('\n');
    }

    let mut result = format!("{templates}{inline_specifier}{leading}{scope_string}{definition}");
    for word in ["override", "final"] {
        result = remove_word_and_leading_space(&result, word);
    }
    result
}
