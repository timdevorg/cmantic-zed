//! Add Declaration (port of `commands/addDeclaration.ts`).
//!
//! On a function definition without a declaration: declares a member function in its class
//! (public section, next to the declaration of a neighbouring definition where possible) or a
//! free function in the matching header file or in the current file.

use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::actions::add_definition::{FileInput, HeadKind, format_head, indent_unit, with_eol};
use crate::actions::member::{ClassEditor, GeneratedEdit, NewMember};
use crate::config::{Config, DefinitionLocation};
use crate::placement::{
    ProposedPosition, Side, format_text_to_insert, position_after_last_symbol, trailing_comment_end,
};
use crate::symbol::{
    Access, ClassKey, Scope, ScopeKind, Symbol, SymbolPath, SymbolTree, leading_text_has,
    strip_comments,
};
use crate::text::{contains_word, detect_eol, line_start, offset_to_position, position_to_offset};

pub const TITLE_MATCHING_HEADER: &str = "Add Declaration in matching header file";
pub const TITLE_CURRENT_FILE: &str = "Add Declaration in this file";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationTarget {
    /// A member function: into its class (in this file or the matching header).
    Class,
    /// A free function: into the matching header file.
    MatchingHeader,
    /// A free function: into the current file, before its first use.
    CurrentFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationAction {
    pub target: DeclarationTarget,
    pub title: String,
    /// The declaration goes into the matching header (not the current file).
    pub edits_other_file: bool,
}

/// The function definition whose head (not its body) contains `offset`.
pub(crate) fn definition_at(tree: &SymbolTree, offset: usize) -> Option<SymbolPath<'_>> {
    tree.symbol_at(offset).filter(|path| {
        let symbol = path.symbol();
        symbol.is_function_definition()
            && symbol
                .function
                .as_ref()
                .is_some_and(|f| offset <= f.decl_end)
    })
}

/// Whether a definition is inside a class body (an inline member definition).
pub(crate) fn in_class_body(symbol: &Symbol) -> bool {
    symbol
        .parent_scope()
        .is_some_and(|s| s.kind == ScopeKind::Class)
}

/// Name path of the class an out-of-class member definition (`void ns::Foo::f()`) belongs to.
pub(crate) fn class_path(symbol: &Symbol) -> Option<Vec<String>> {
    let func = symbol.function.as_ref()?;
    if func.qualifiers.is_empty() || in_class_body(symbol) {
        return None;
    }
    let mut path = symbol.qualified_name();
    path.pop();
    Some(path)
}

/// The class of an out-of-class member definition: in the current file, else in `other`.
/// The flag tells whether it was found in the current file.
pub(crate) fn find_parent_class<'a>(
    symbol: &Symbol,
    current: &'a SymbolTree,
    other: Option<&'a SymbolTree>,
) -> Option<(SymbolPath<'a>, bool)> {
    let path = class_path(symbol)?;
    if let Some(class) = current.find_class(&path) {
        return Some((class, true));
    }
    other?.find_class(&path).map(|class| (class, false))
}

/// `Add Declaration in class "Foo"` / `in struct "Foo"`.
pub(crate) fn class_title(prefix: &str, class: &Symbol) -> String {
    let key = match class.class.as_ref().map(|c| c.key) {
        Some(ClassKey::Struct) => "struct",
        _ => "class",
    };
    format!("{prefix} {key} \"{}\"", class.name)
}

/// Functions with internal linkage (`static`, anonymous namespace) stay out of headers.
pub(crate) fn has_internal_linkage(text: &str, symbol: &Symbol) -> bool {
    leading_text_has(text, symbol, "static") || symbol.scopes.iter().any(Scope::is_anonymous)
}

/// The Add Declaration actions for the function definition at `position`. `header` is the
/// matching header of a source file.
pub fn available_declarations(
    file: &FileInput,
    position: Position,
    header: Option<&FileInput>,
) -> Vec<DeclarationAction> {
    let tree = SymbolTree::new(file.text);
    let offset = position_to_offset(file.text, position);
    let Some(path) = definition_at(&tree, offset) else {
        return Vec::new();
    };
    let symbol = path.symbol();
    if in_class_body(symbol) || symbol.name == "main" {
        return Vec::new();
    }
    let header = header.filter(|_| !file.is_header);
    let header_tree = header.map(|h| SymbolTree::new(h.text));
    let func = symbol.function.as_ref().expect("definition is a function");

    if !func.qualifiers.is_empty() {
        let Some((class, in_current)) = find_parent_class(symbol, &tree, header_tree.as_ref())
        else {
            return Vec::new();
        };
        let class_tree = if in_current {
            &tree
        } else {
            header_tree.as_ref().expect("found in the header")
        };
        if class_tree.find_declaration(symbol).is_some() {
            return Vec::new();
        }
        return vec![DeclarationAction {
            target: DeclarationTarget::Class,
            title: class_title("Add Declaration in", class.symbol()),
            edits_other_file: !in_current,
        }];
    }

    if header_tree
        .as_ref()
        .is_some_and(|t| t.find_declaration(symbol).is_some())
    {
        return Vec::new();
    }
    let mut actions = Vec::new();
    if header.is_some() && !has_internal_linkage(file.text, symbol) {
        actions.push(DeclarationAction {
            target: DeclarationTarget::MatchingHeader,
            title: TITLE_MATCHING_HEADER.into(),
            edits_other_file: true,
        });
    }
    if tree.find_declaration(symbol).is_none() {
        actions.push(DeclarationAction {
            target: DeclarationTarget::CurrentFile,
            title: TITLE_CURRENT_FILE.into(),
            edits_other_file: false,
        });
    }
    actions
}

/// Builds the edit for `target`. Edits to the matching header go into
/// [`GeneratedEdit::other_edits`].
pub fn add_declaration(
    file: &FileInput,
    position: Position,
    target: DeclarationTarget,
    header: Option<&FileInput>,
    config: &Config,
) -> Option<GeneratedEdit> {
    let available = available_declarations(file, position, header);
    let action = available.iter().find(|a| a.target == target)?;
    let tree = SymbolTree::new(file.text);
    let path = definition_at(&tree, position_to_offset(file.text, position))?;
    let header = header.filter(|_| !file.is_header);
    let header_tree = header.map(|h| SymbolTree::new(h.text));

    let edits = match target {
        DeclarationTarget::Class => {
            let (class, in_current) =
                find_parent_class(path.symbol(), &tree, header_tree.as_ref())?;
            let class_file = if in_current { *file } else { *header? };
            let class_tree = if in_current {
                &tree
            } else {
                header_tree.as_ref()?
            };
            let declaration =
                member_declaration_text(file.text, path, &class_file, class_tree, class.symbol());
            let relative = neighbour_declaration(path, class_tree, class.symbol());
            let mut editor = ClassEditor::new(class_file, None, class.symbol().range.start, config);
            editor.compact = true;
            editor.add(&NewMember {
                name: path.symbol().name.clone(),
                declaration: String::new(),
                inline_text: declaration,
                body: String::new(),
                location: DefinitionLocation::Inline,
                relative,
            })?;
            editor.finish().edits
        }
        DeclarationTarget::MatchingHeader => {
            let header = header?;
            vec![free_declaration(
                file,
                path,
                header,
                header_tree.as_ref()?,
                false,
            )]
        }
        DeclarationTarget::CurrentFile => vec![free_declaration(file, path, file, &tree, true)],
    };
    Some(if action.edits_other_file {
        GeneratedEdit {
            other_edits: edits,
            ..Default::default()
        }
    } else {
        GeneratedEdit {
            edits,
            ..Default::default()
        }
    })
}

/// The declaration of member definition `path` for the body of `class` (no `Class::`).
pub(crate) fn member_declaration_text(
    text: &str,
    path: SymbolPath,
    class_file: &FileInput,
    class_tree: &SymbolTree,
    class: &Symbol,
) -> String {
    let inside = class
        .body
        .as_ref()
        .map_or(class.range.start, |b| b.start + 1);
    let head = format_head(
        text,
        path,
        class_file,
        &class_tree.scopes_at(inside),
        inside,
        false,
        HeadKind::Declaration { keep_static: false },
    );
    format!("{head};")
}

/// A public declaration in `class` of a definition next to `path` (up to six on each side),
/// to put the new declaration next to (`findPositionRelativeToSiblings`).
pub(crate) fn neighbour_declaration(
    path: SymbolPath,
    class_tree: &SymbolTree,
    class: &Symbol,
) -> Option<(String, Side)> {
    let index = path.index;
    let is_definition = |s: &&Symbol| s.is_function_definition();
    let before = path.siblings[..index]
        .iter()
        .rev()
        .filter(is_definition)
        .take(6);
    let after = path.siblings[index + 1..]
        .iter()
        .filter(is_definition)
        .take(6);
    let in_class = |sibling: &Symbol| {
        let decl = class_tree.find_declaration(sibling)?;
        let in_this_class = decl
            .parent
            .is_some_and(|p| p.range.start == class.range.start);
        (in_this_class && class.access_at(decl.symbol().range.start) == Access::Public)
            .then(|| decl.symbol().name.clone())
    };
    before
        .filter_map(|s| in_class(s).map(|n| (n, Side::After)))
        .chain(after.filter_map(|s| in_class(s).map(|n| (n, Side::Before))))
        .next()
}

/// Names of the namespaces enclosing a free function.
fn namespace_names(symbol: &Symbol) -> Vec<String> {
    symbol
        .scopes
        .iter()
        .filter(|s| s.kind == ScopeKind::Namespace)
        .map(|s| s.name.clone())
        .collect()
}

/// Inserts the declaration of the free function definition `path` into `target`.
fn free_declaration(
    file: &FileInput,
    path: SymbolPath,
    target: &FileInput,
    target_tree: &SymbolTree,
    same_file: bool,
) -> TextEdit {
    let symbol = path.symbol();
    let names = namespace_names(symbol);
    let (position, missing) = if same_file {
        (position_in_same_file(file.text, path, target_tree), 0)
    } else {
        position_in_header(file.text, target.text, path, target_tree, &names)
    };
    let mut target_scopes = target_tree.scopes_at(position.offset);
    for name in &names[names.len() - missing..] {
        target_scopes.push(Scope {
            kind: ScopeKind::Namespace,
            name: name.clone(),
            qualifier: name.clone(),
            template: None,
            range: 0..0,
            qualified_namespace: false,
        });
    }
    let head = format_head(
        file.text,
        path,
        target,
        &target_scopes,
        position.offset,
        same_file,
        HeadKind::Declaration {
            keep_static: same_file,
        },
    );
    let mut text = format!("{head};");
    for name in names[names.len() - missing..].iter().rev() {
        text = format!("namespace {name} {{\n{text}\n}}");
    }
    let unit = indent_unit(target.text, file.text);
    let formatted = format_text_to_insert(&text, &position, target.text, &unit);
    let start = offset_to_position(target.text, position.offset);
    TextEdit::new(
        Range::new(start, start),
        with_eol(formatted, detect_eol(target.text)),
    )
}

/// Sibling definitions of `path` (up to six before, nearest first, then up to six after)
/// whose declaration is found in `target` by `find`; returns the position next to it.
fn next_to_sibling_declaration<'t>(
    target_text: &str,
    path: SymbolPath,
    find: impl Fn(&Symbol) -> Option<SymbolPath<'t>>,
) -> Option<ProposedPosition> {
    let index = path.index;
    let is_definition = |s: &&Symbol| s.is_function_definition();
    for sibling in path.siblings[..index]
        .iter()
        .rev()
        .filter(is_definition)
        .take(6)
    {
        if let Some(decl) = find(sibling) {
            let decl = decl.symbol();
            return Some(ProposedPosition {
                next_to: true,
                ..ProposedPosition::after(
                    trailing_comment_end(target_text, decl),
                    Some(decl.range.start),
                )
            });
        }
    }
    for sibling in path.siblings[index + 1..]
        .iter()
        .filter(is_definition)
        .take(6)
    {
        if let Some(decl) = find(sibling) {
            let decl = decl.symbol();
            return Some(ProposedPosition {
                next_to: true,
                ..ProposedPosition::before(decl.comment_start, decl.range.start)
            });
        }
    }
    None
}

/// Whether `decl` is a free function declaration in exactly the namespaces `names`.
fn is_free_in(decl: &Symbol, names: &[String]) -> bool {
    decl.scopes.iter().all(|s| s.kind == ScopeKind::Namespace)
        && decl
            .function
            .as_ref()
            .is_some_and(|f| f.qualifiers.is_empty())
        && namespace_names(decl) == names
}

/// Position in the matching header for the declaration of a free function: next to the
/// declaration of a sibling definition, else at the end of the innermost matching namespace
/// block, else after the last symbol. Also returns how many of the innermost namespaces are
/// missing in the header (the declaration is wrapped in new namespace blocks for them).
fn position_in_header(
    def_text: &str,
    target_text: &str,
    path: SymbolPath,
    target: &SymbolTree,
    names: &[String],
) -> (ProposedPosition, usize) {
    if target.symbols.is_empty() {
        return (
            crate::placement::position_after_last_non_empty_line(target_text),
            names.len(),
        );
    }
    let find = |sibling: &Symbol| {
        target
            .find_declaration(sibling)
            .filter(|d| is_free_in(d.symbol(), names))
    };
    if let Some(position) = next_to_sibling_declaration(target_text, path, find) {
        return (position, 0);
    }
    for depth in (1..=names.len()).rev() {
        let Some(namespace) = target.find_namespace(&names[..depth]) else {
            continue;
        };
        let position = match namespace.children.last() {
            Some(last) => ProposedPosition::after(
                trailing_comment_end(target_text, last),
                Some(last.range.start),
            ),
            None => {
                let body = namespace.body.clone().unwrap_or(namespace.range.clone());
                // Indent like the definition is indented in its own namespace block.
                let column = |text: &str, offset: usize| offset - line_start(text, offset);
                let scope = path
                    .symbol()
                    .scopes
                    .iter()
                    .filter(|s| s.kind == ScopeKind::Namespace)
                    .nth(depth - 1);
                ProposedPosition {
                    offset: body.start + 1,
                    after: true,
                    indent: scope.is_some_and(|scope| {
                        column(def_text, path.symbol().range.start)
                            > column(def_text, scope.range.start)
                    }),
                    ..Default::default()
                }
            }
        };
        return (position, names.len() - depth);
    }
    (position_after_last_symbol(target_text, target), names.len())
}

/// Position in the definition's own file: next to the declaration of a sibling definition,
/// else before the first sibling that uses the function, else right before the definition.
fn position_in_same_file(text: &str, path: SymbolPath, tree: &SymbolTree) -> ProposedPosition {
    let symbol = path.symbol();
    let names = namespace_names(symbol);
    let find = |sibling: &Symbol| {
        tree.find_declaration(sibling).filter(|d| {
            is_free_in(d.symbol(), &names) && d.symbol().range.end <= symbol.comment_start
        })
    };
    if let Some(position) = next_to_sibling_declaration(text, path, find) {
        return position;
    }
    let first_use = path.siblings[..path.index]
        .iter()
        .find(|s| contains_word(&strip_comments(&text[s.range.clone()]), &symbol.name));
    let anchor = first_use.unwrap_or(symbol);
    ProposedPosition::before(anchor.comment_start, anchor.range.start)
}
