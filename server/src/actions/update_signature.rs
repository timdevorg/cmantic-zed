//! Update Function Signature (`updateSignature.ts`): after the head of a function declaration
//! or definition was edited, rewrite the head of its counterpart (the definition of a
//! declaration, the declaration of a definition) to match.
//!
//! Unlike C-Mantic, which re-finds the changed function by its (possibly changed) name, the
//! [`Tracker`] follows each edited head by position: every full-text change is reduced to one
//! changed byte range (common prefix/suffix) and tracked ranges are mapped through it. The
//! original signature and the counterpart's location and text are recorded when an edit first
//! touches the head, so any combination of changes (name, parameters, types, return type,
//! qualifiers) can be propagated, and broken syntax while typing doesn't lose the function.
//!
//! What is copied to the counterpart: the return type and leading specifiers (except
//! `virtual`, `static`, `explicit`, `friend` and `inline`, which stay as the counterpart has
//! them), the name (its scope qualifiers stay), the parameters and the trailing qualifiers
//! (`const`, `volatile`, `&`/`&&`, `noexcept`, trailing return type; `override`/`final` stay
//! on the declaration). Template prefixes and scope qualifiers aren't changed.
//!
//! Parameter names: a definition keeps its own names for parameters that correspond to one of
//! its existing parameters, so its body never breaks; new parameters get the edited side's
//! names. A declaration takes the edited side's names. Default arguments stay on the
//! declaration (mapped to the corresponding parameters) and are never copied to a definition.

use std::collections::HashMap;
use std::ops::Range;

use tower_lsp_server::ls_types::{Position, TextEdit, Uri};
use tree_sitter::{Node, Tree};

use crate::edit_buffer::EditBuffer;
use crate::symbol::{self, Symbol, SymbolTree, strip_comments};
use crate::text::{
    contains_word, detect_eol, find_word, is_word_char, leading_whitespace, line_start,
    normalize_whitespace, remove_word_and_leading_space, remove_word_and_trailing_space,
};

pub const TITLE: &str = "Update Function Signature";

/// Leading specifiers that belong to one side only and are never copied.
const OWN_LEADING: [&str; 5] = ["virtual", "static", "explicit", "friend", "inline"];
/// Trailing specifiers that are only legal on a declaration.
const OWN_TRAILING: [&str; 2] = ["override", "final"];

/// The single changed range between two texts: `old[start..old_end]` became
/// `new[start..new_end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextChange {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

/// The change between `old` and `new` as common prefix/suffix diff, on char boundaries.
pub fn diff(old: &str, new: &str) -> Option<TextChange> {
    if old == new {
        return None;
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut prefix = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = (ob.len() - prefix).min(nb.len() - prefix);
    let mut suffix = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(ob.len() - suffix) || !new.is_char_boundary(nb.len() - suffix) {
        suffix -= 1;
    }
    Some(TextChange {
        start: prefix,
        old_end: ob.len() - suffix,
        new_end: nb.len() - suffix,
    })
}

impl TextChange {
    /// Maps a range of the old text into the new text. A range overlapping the change grows to
    /// include the inserted text. `None` if the range was deleted entirely.
    pub fn map_range(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let shift = |o: usize| o - self.old_end + self.new_end;
        let start = if range.start < self.start {
            range.start
        } else if range.start >= self.old_end {
            shift(range.start)
        } else {
            self.start
        };
        let end = if range.end <= self.start {
            range.end
        } else if range.end >= self.old_end {
            shift(range.end)
        } else {
            self.new_end
        };
        (start < end || range.is_empty()).then_some(start..end)
    }

    /// Whether the change touches `range` (boundaries included).
    pub fn touches(&self, range: &Range<usize>) -> bool {
        self.start <= range.end && self.old_end >= range.start
    }
}

/// `decl_start..decl_end` of a function: from the return type to the end of the declarator.
fn head(symbol: &Symbol) -> Option<Range<usize>> {
    symbol.function.as_ref().map(|f| f.decl_start..f.decl_end)
}

fn is_head(symbol: &Symbol) -> bool {
    symbol
        .function
        .as_ref()
        .is_some_and(|f| f.is_declaration() || f.is_definition())
}

fn norm(text: &str) -> String {
    normalize_whitespace(&strip_comments(text))
}

#[derive(Debug, Clone)]
struct Param {
    /// The declaration without its default argument.
    text: String,
    name: String,
    /// ` = value`, or empty.
    default: String,
}

/// The parts of a function head that are propagated.
#[derive(Debug, Clone)]
pub struct Signature {
    symbol: Symbol,
    is_declaration: bool,
    /// Text before the qualifiers/name without the side-specific specifiers, trimmed.
    leading: String,
    name: String,
    params: Vec<Param>,
    /// Text between `)` and the end of the declarator without `override`/`final`, trimmed.
    trailing: String,
}

impl Signature {
    fn of(text: &str, symbol: &Symbol) -> Option<Self> {
        let func = symbol.function.as_ref()?;
        let mut leading = text[func.decl_start..func.scope_start].to_string();
        for word in OWN_LEADING {
            leading = remove_word_and_trailing_space(&leading, word, false);
        }
        let mut trailing = text[func.params.end..func.decl_end].to_string();
        for word in OWN_TRAILING {
            trailing = remove_word_and_leading_space(&trailing, word);
        }
        let mut params: Vec<Param> = func
            .param_ranges
            .iter()
            .zip(&func.param_names)
            .map(|(range, name)| Param {
                text: text[range.clone()].trim().to_string(),
                name: name.clone(),
                default: String::new(),
            })
            .collect();
        for default in &func.default_args {
            if let Some(i) = func
                .param_ranges
                .iter()
                .rposition(|r| r.end <= default.start)
            {
                params[i].default = text[default.clone()].to_string();
            }
        }
        Some(Self {
            symbol: symbol.clone(),
            is_declaration: func.is_declaration(),
            leading: leading.trim().to_string(),
            name: text[func.name_range.clone()].to_string(),
            params,
            trailing: trailing.trim().to_string(),
        })
    }

    fn params_key(&self) -> Vec<String> {
        self.params.iter().map(|p| norm(&p.text)).collect()
    }

    /// Equal keys mean nothing that would be propagated has changed.
    fn key(&self) -> String {
        format!(
            "{}\0{}\0{}\0{}",
            norm(&self.leading),
            self.name,
            self.params_key().join(","),
            norm(&self.trailing)
        )
    }
}

/// Whether there is a syntax error (or missing node) intersecting `range`.
fn has_error_in(node: Node, range: &Range<usize>) -> bool {
    if !node.has_error() || node.end_byte() < range.start || node.start_byte() > range.end {
        return false;
    }
    if node.is_error() || node.is_missing() {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| has_error_in(c, range))
}

/// A parsed file: syntax tree (for error checks) plus symbols.
struct Parsed {
    tree: Option<Tree>,
    symbols: SymbolTree,
}

impl Parsed {
    fn new(text: &str) -> Self {
        let tree = symbol::parse(text);
        let symbols = match &tree {
            Some(tree) => SymbolTree::from_tree(text, tree),
            None => SymbolTree::default(),
        };
        Self { tree, symbols }
    }

    fn parses(&self, range: &Range<usize>) -> bool {
        self.tree
            .as_ref()
            .is_some_and(|t| !has_error_in(t.root_node(), range))
    }

    /// The function head overlapping `range` the most (containing it, if it is empty).
    fn head_at(&self, range: &Range<usize>) -> Option<&Symbol> {
        self.symbols
            .all()
            .filter(|s| is_head(s))
            .filter_map(|s| {
                let h = head(s)?;
                let overlap = h.end.min(range.end) as isize - h.start.max(range.start) as isize;
                let hit = if range.is_empty() {
                    h.start <= range.start && range.start <= h.end
                } else {
                    overlap > 0
                };
                hit.then_some((overlap, s))
            })
            .max_by_key(|(overlap, _)| *overlap)
            .map(|(_, s)| s)
    }
}

/// Where the counterpart is, and its head text when it was last seen.
#[derive(Debug, Clone)]
pub struct Counterpart {
    pub uri: Uri,
    pub head: Range<usize>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub id: u64,
    /// Tracked head range in the edited file.
    pub range: Range<usize>,
    original: Signature,
    pub counterpart: Counterpart,
}

/// A computed update of the counterpart.
#[derive(Debug, Clone)]
pub struct SignatureUpdate {
    pub id: u64,
    pub title: String,
    pub counterpart_uri: Uri,
    /// Edits against the counterpart file.
    pub edits: Vec<TextEdit>,
    /// The counterpart's name after the edits.
    pub cursor: Position,
}

/// Pending signature changes of all files.
#[derive(Debug, Default)]
pub struct Tracker {
    files: HashMap<Uri, Vec<Pending>>,
    next_id: u64,
}

/// Finds the counterpart of `old` again in `text`: by its recorded head text (nearest to the
/// recorded position), else by the original signature.
fn locate<'a>(
    parsed: &'a Parsed,
    text: &str,
    counterpart: &Counterpart,
    original: &Signature,
) -> Option<&'a Symbol> {
    let opposite = |s: &&Symbol| {
        s.function
            .as_ref()
            .is_some_and(|f| f.is_declaration() != original.is_declaration && is_head(s))
    };
    parsed
        .symbols
        .all()
        .filter(opposite)
        .filter(|s| head(s).and_then(|h| text.get(h)) == Some(counterpart.text.as_str()))
        .min_by_key(|s| s.range.start.abs_diff(counterpart.head.start))
        .or_else(|| {
            parsed
                .symbols
                .all()
                .filter(opposite)
                .find(|s| original.symbol.matches_function(s))
        })
}

impl Tracker {
    pub fn pending(&self, uri: &Uri) -> &[Pending] {
        self.files.get(uri).map_or(&[], Vec::as_slice)
    }

    pub fn close(&mut self, uri: &Uri) {
        self.files.remove(uri);
    }

    pub fn remove(&mut self, uri: &Uri, id: u64) {
        if let Some(list) = self.files.get_mut(uri) {
            list.retain(|p| p.id != id);
        }
    }

    /// URIs of the counterparts of the pending changes in `uri`.
    pub fn counterpart_uris(&self, uri: &Uri) -> Vec<Uri> {
        let mut uris: Vec<Uri> = Vec::new();
        for p in self.pending(uri) {
            if !uris.contains(&p.counterpart.uri) {
                uris.push(p.counterpart.uri.clone());
            }
        }
        uris
    }

    /// Handles a change of `uri` from `old` to `new`. `other` gives the matching header/source
    /// file (only called if an edit touches a function head that isn't tracked yet).
    pub fn did_change(
        &mut self,
        uri: &Uri,
        old: &str,
        new: &str,
        other: impl FnOnce() -> Option<(Uri, String)>,
    ) {
        let Some(change) = diff(old, new) else {
            return;
        };
        let old_parsed = Parsed::new(old);
        let mut new_parsed: Option<Parsed> = None;

        // New pendings for touched heads that aren't tracked (or counterparts of tracked ones).
        let mut created = Vec::new();
        let mut other = Some(other);
        let mut other_file: Option<Option<(Uri, String, Parsed)>> = None;
        for symbol in old_parsed.symbols.all().filter(|s| is_head(s)) {
            let Some(range) = head(symbol) else { continue };
            if !change.touches(&range) {
                continue;
            }
            let overlaps = |r: &Range<usize>| r.start <= range.end && range.start <= r.end;
            let tracked = self.pending(uri).iter().any(|p| overlaps(&p.range))
                || self
                    .files
                    .values()
                    .flatten()
                    .any(|p| p.counterpart.uri == *uri && overlaps(&p.counterpart.head));
            if tracked {
                continue;
            }
            let Some(original) = Signature::of(old, symbol) else {
                continue;
            };
            let find = |tree: &SymbolTree| -> Option<Range<usize>> {
                if original.is_declaration {
                    tree.find_definition(symbol).and_then(head)
                } else {
                    tree.find_declaration(symbol).and_then(|p| head(p.symbol()))
                }
            };
            let counterpart = match find(&old_parsed.symbols) {
                Some(cp) if cp != range => change.map_range(&cp).map(|mapped| Counterpart {
                    uri: uri.clone(),
                    head: mapped,
                    text: old[cp].to_string(),
                }),
                _ => {
                    let file = other_file.get_or_insert_with(|| {
                        let (u, t) = (other.take()?)()?;
                        let p = Parsed::new(&t);
                        Some((u, t, p))
                    });
                    file.as_ref().and_then(|(u, t, p)| {
                        find(&p.symbols).map(|cp| Counterpart {
                            uri: u.clone(),
                            head: cp.clone(),
                            text: t[cp].to_string(),
                        })
                    })
                }
            };
            let (Some(counterpart), Some(mapped)) = (counterpart, change.map_range(&range)) else {
                continue;
            };
            self.next_id += 1;
            created.push(Pending {
                id: self.next_id,
                range: mapped,
                original,
                counterpart,
            });
        }

        // Map the tracked ranges; validate counterparts in this file that the change touched.
        for (file_uri, list) in self.files.iter_mut() {
            list.retain_mut(|p| {
                if file_uri == uri {
                    match change.map_range(&p.range) {
                        Some(r) => p.range = r,
                        None => return false,
                    }
                }
                if p.counterpart.uri != *uri {
                    return true;
                }
                let touched = change.touches(&p.counterpart.head);
                p.counterpart.head = change
                    .map_range(&p.counterpart.head)
                    .unwrap_or(change.start..change.start);
                if !touched {
                    return true;
                }
                let parsed = new_parsed.get_or_insert_with(|| Parsed::new(new));
                let Some(cp) = locate(parsed, new, &p.counterpart, &p.original) else {
                    return false;
                };
                let Some(h) = head(cp) else { return false };
                p.counterpart.text = new[h.clone()].to_string();
                p.counterpart.head = h;
                // In the same file: drop it once the counterpart is in sync (applied or edited
                // by hand), so that nothing is offered in either direction.
                if file_uri == uri
                    && let Some(cur) = parsed.head_at(&p.range)
                    && let Some(current) = Signature::of(new, cur)
                {
                    return rewrite(new, cur, &current, &p.original, new, cp).is_some();
                }
                true
            });
        }
        self.files.entry(uri.clone()).or_default().extend(created);

        // Snap to the current head; drop pendings edited back to the original.
        let parsed = new_parsed.get_or_insert_with(|| Parsed::new(new));
        let list = self.files.get_mut(uri).expect("inserted above");
        let mut seen: Vec<Range<usize>> = Vec::new();
        list.retain_mut(|p| {
            let Some(symbol) = parsed.head_at(&p.range) else {
                return true;
            };
            let Some(h) = head(symbol) else { return true };
            if !parsed.parses(&h) {
                return true;
            }
            if seen.contains(&h) {
                return false;
            }
            seen.push(h.clone());
            p.range = h;
            Signature::of(new, symbol).is_none_or(|s| s.key() != p.original.key())
        });
        if list.is_empty() {
            self.files.remove(uri);
        }
    }

    /// The update offered at `offset` in `uri` (current text `text`). `files` holds the texts of
    /// the counterpart files (see [`Self::counterpart_uris`]). Drops pendings whose counterpart
    /// can't be found anymore.
    pub fn action_at(
        &mut self,
        uri: &Uri,
        text: &str,
        offset: usize,
        files: &HashMap<Uri, String>,
    ) -> Option<SignatureUpdate> {
        let parsed = Parsed::new(text);
        let list = self.files.get_mut(uri)?;
        let mut result = None;
        let mut dropped = Vec::new();
        for p in list.iter_mut() {
            let Some(symbol) = parsed.head_at(&p.range) else {
                continue;
            };
            let (Some(h), Some(func)) = (head(symbol), symbol.function.as_ref()) else {
                continue;
            };
            if !(symbol.range.start <= offset && offset <= symbol.range.end)
                || !parsed.parses(&h)
                || func.is_declaration() != p.original.is_declaration
            {
                continue;
            }
            let Some(current) = Signature::of(text, symbol) else {
                continue;
            };
            if current.key() == p.original.key() {
                continue;
            }
            let same_file = p.counterpart.uri == *uri;
            let Some(cp_text) = (if same_file {
                Some(text)
            } else {
                files.get(&p.counterpart.uri).map(String::as_str)
            }) else {
                continue;
            };
            let owned;
            let cp_parsed = if same_file {
                &parsed
            } else {
                owned = Parsed::new(cp_text);
                &owned
            };
            let Some(cp) = locate(cp_parsed, cp_text, &p.counterpart, &p.original) else {
                dropped.push(p.id);
                continue;
            };
            let cp_head = head(cp)?;
            p.counterpart.text = cp_text[cp_head.clone()].to_string();
            p.counterpart.head = cp_head;
            let Some((edits, cursor)) = rewrite(text, symbol, &current, &p.original, cp_text, cp)
            else {
                // Already in sync: a later edit starts a new pending change.
                dropped.push(p.id);
                continue;
            };
            let title = if same_file {
                TITLE.to_string()
            } else {
                let name = p
                    .counterpart
                    .uri
                    .to_file_path()
                    .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "matching file".into());
                format!("{TITLE} in \"{name}\"")
            };
            result = Some(SignatureUpdate {
                id: p.id,
                title,
                counterpart_uri: p.counterpart.uri.clone(),
                edits,
                cursor,
            });
            break;
        }
        list.retain(|p| !dropped.contains(&p.id));
        result
    }
}

/// Column of `offset` and the indentation width of its line.
fn columns(text: &str, offset: usize) -> (usize, usize) {
    let start = line_start(text, offset);
    (offset - start, leading_whitespace(&text[start..]).len())
}

/// Re-indents the continuation lines of a separator copied from the edited head (parameter
/// list starting at column `col`, line indented by `indent`) for the counterpart.
fn reindent(sep: &str, from: (usize, usize), to: (usize, usize), eol: &str) -> String {
    let sep = sep.replace("\r\n", "\n");
    let mut lines = sep.split('\n');
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        let ws = leading_whitespace(line);
        let rest = &line[ws.len()..];
        out.push_str(eol);
        if ws.contains('\t') {
            out.push_str(line);
            continue;
        }
        // Aligned with the parameter list, or indented relative to the line.
        let width = if ws.len() > from.0 {
            (ws.len() + to.0).saturating_sub(from.0)
        } else {
            (ws.len() + to.1).saturating_sub(from.1)
        };
        out.push_str(&" ".repeat(width));
        out.push_str(rest);
    }
    out
}

/// Gives the parameter declaration `text` (named `old`, maybe empty) the name `new`.
fn with_name(text: &str, old: &str, new: &str) -> String {
    if old.is_empty() {
        if text.contains(['(', '[']) {
            return text.to_string();
        }
        return format!("{text} {new}");
    }
    let mut last = None;
    let mut from = 0;
    while let Some(i) = find_word(&text[from..], old) {
        last = Some(from + i);
        from += i + old.len();
    }
    match last {
        Some(i) => format!("{}{new}{}", &text[..i], &text[i + old.len()..]),
        None => text.to_string(),
    }
}

/// The new text inside the counterpart's parentheses.
fn new_params(
    text: &str,
    cur: &Symbol,
    current: &Signature,
    original: &Signature,
    cp_text: &str,
    cp: &Symbol,
    counterpart: &Signature,
) -> String {
    let cf = cur.function.as_ref().expect("function");
    let pf = cp.function.as_ref().expect("function");
    if current.params.is_empty() {
        return text[cf.params.start + 1..cf.params.end - 1]
            .trim()
            .to_string();
    }
    // Each current parameter's original parameter: same name, else same position.
    let mut used = vec![false; original.params.len()];
    let mut map: Vec<Option<usize>> = vec![None; current.params.len()];
    for (j, p) in current.params.iter().enumerate() {
        if p.name.is_empty() {
            continue;
        }
        if let Some(i) =
            (0..original.params.len()).find(|&i| !used[i] && original.params[i].name == p.name)
        {
            map[j] = Some(i);
            used[i] = true;
        }
    }
    for j in 0..current.params.len() {
        if map[j].is_none() && j < used.len() && !used[j] {
            map[j] = Some(j);
            used[j] = true;
        }
    }
    let target_is_definition = !counterpart.is_declaration;
    let texts: Vec<String> = current
        .params
        .iter()
        .zip(&map)
        .map(|(p, i)| {
            let Some(cp_param) = i.and_then(|i| counterpart.params.get(i)) else {
                return p.text.clone();
            };
            if target_is_definition {
                if !cp_param.name.is_empty() && cp_param.name != p.name {
                    return with_name(&p.text, &p.name, &cp_param.name);
                }
                p.text.clone()
            } else {
                format!("{}{}", p.text, cp_param.default)
            }
        })
        .collect();

    let eol = detect_eol(cp_text);
    let from = columns(text, cf.params.start);
    let to = columns(cp_text, pf.params.start);
    let param_end = |func: &symbol::FunctionInfo, i: usize| {
        let r = &func.param_ranges[i];
        func.default_args
            .iter()
            .filter(|d| {
                d.start >= r.end
                    && func
                        .param_ranges
                        .get(i + 1)
                        .is_none_or(|n| d.end <= n.start)
            })
            .map(|d| d.end)
            .max()
            .unwrap_or(r.end)
    };
    let edges = |t: &str, func: &symbol::FunctionInfo| {
        let n = func.param_ranges.len();
        (
            t[func.params.start + 1..func.param_ranges[0].start].to_string(),
            t[param_end(func, n - 1)..func.params.end - 1].to_string(),
        )
    };
    let seps = |t: &str, func: &symbol::FunctionInfo| -> Vec<String> {
        (0..func.param_ranges.len().saturating_sub(1))
            .map(|i| t[param_end(func, i)..func.param_ranges[i + 1].start].to_string())
            .collect()
    };
    let (prefix, suffix) = if pf.param_ranges.is_empty() {
        let (p, s) = edges(text, cf);
        (reindent(&p, from, to, eol), reindent(&s, from, to, eol))
    } else {
        edges(cp_text, pf)
    };
    let separators: Vec<String> = if pf.param_ranges.len() >= 2 {
        seps(cp_text, pf)
    } else {
        seps(text, cf)
            .iter()
            .map(|s| reindent(s, from, to, eol))
            .collect()
    };
    let mut out = prefix;
    for (i, t) in texts.iter().enumerate() {
        if i > 0 {
            let sep = separators
                .get(i - 1)
                .or(separators.last())
                .map_or(", ", String::as_str);
            out.push_str(sep);
        }
        out.push_str(t);
    }
    out.push_str(&suffix);
    out
}

/// Edits of the counterpart `cp` (in `cp_text`) for the change of `cur` from `original` to
/// `current`, and the offset of its name afterwards. `None` if nothing changes.
fn rewrite(
    text: &str,
    cur: &Symbol,
    current: &Signature,
    original: &Signature,
    cp_text: &str,
    cp: &Symbol,
) -> Option<(Vec<TextEdit>, Position)> {
    let pf = cp.function.as_ref()?;
    let counterpart = Signature::of(cp_text, cp)?;
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();

    if norm(&current.leading) != norm(&original.leading) {
        let old = &cp_text[pf.decl_start..pf.scope_start];
        let mut new: String = old
            .split(|c: char| !is_word_char(c))
            .filter(|w| OWN_LEADING.contains(w))
            .map(|w| format!("{w} "))
            .collect();
        let core = current.leading.as_str();
        if !core.is_empty() {
            let ws = &old[old.trim_end().len()..];
            new.push_str(core);
            new.push_str(if ws.is_empty() && core.ends_with(is_word_char) {
                " "
            } else {
                ws
            });
        }
        edits.push((pf.decl_start..pf.scope_start, new));
    }
    if current.name != original.name {
        edits.push((pf.name_range.clone(), current.name.clone()));
    }
    if current.params_key() != original.params_key() {
        let inner = pf.params.start + 1..pf.params.end - 1;
        let new = new_params(text, cur, current, original, cp_text, cp, &counterpart);
        edits.push((inner, new));
    }
    if norm(&current.trailing) != norm(&original.trailing) {
        let range = pf.params.end..pf.decl_end;
        let old = &cp_text[range.clone()];
        let mut new = if current.trailing.is_empty() {
            String::new()
        } else {
            format!(" {}", current.trailing)
        };
        if counterpart.is_declaration {
            for word in OWN_TRAILING.iter().filter(|w| contains_word(old, w)) {
                new.push(' ');
                new.push_str(word);
            }
        }
        edits.push((range, new));
    }

    edits.retain(|(range, new)| cp_text[range.clone()] != *new);
    if edits.is_empty() {
        return None;
    }
    let mut buffer = EditBuffer::new(cp_text);
    let mut cursor = pf.name_range.start;
    for (range, new) in edits.iter().rev() {
        if range.end <= pf.name_range.start {
            cursor = cursor + new.len() - (range.end - range.start);
        }
        buffer.replace(range.clone(), new);
    }
    Some((buffer.edits(), buffer.position(cursor)))
}
