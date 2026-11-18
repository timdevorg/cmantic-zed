//! Generate Getters and Setters (port of `commands/generateGetterSetter.ts`, `Accessor.ts` and
//! the naming helpers of `CSymbol.ts`).
//!
//! Types are judged by syntax only (no clangd): a setter takes its parameter by value for
//! pointers and members whose type contains a primitive type keyword, by `const T&` otherwise
//! (C-Mantic's behaviour with `resolveTypes` off, its default).

use tower_lsp_server::ls_types::Position;

use crate::actions::add_definition::FileInput;
use crate::actions::member::{ClassEditor, GeneratedEdit, NewMember};
use crate::config::{CaseStyle, Config, DefinitionLocation};
use crate::placement::{Side, declares_name};
use crate::symbol::{ScopeKind, Symbol, SymbolTree, VariableInfo};
use crate::text::{contains_word, is_word_char, position_to_offset};

pub const TITLE_GETTER_SETTER: &str = "Generate Getter and Setter";
pub const TITLE_GETTER: &str = "Generate Getter";
pub const TITLE_SETTER: &str = "Generate Setter";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorKind {
    GetterAndSetter,
    Getter,
    Setter,
}

impl AccessorKind {
    fn has_getter(self) -> bool {
        self != Self::Setter
    }

    fn has_setter(self) -> bool {
        self != Self::Getter
    }

    fn title_prefix(self) -> &'static str {
        match self {
            Self::GetterAndSetter => TITLE_GETTER_SETTER,
            Self::Getter => TITLE_GETTER,
            Self::Setter => TITLE_SETTER,
        }
    }
}

/// An accessor action available at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessorAction {
    pub kind: AccessorKind,
    /// `Generate Getter for "x"`.
    pub title: String,
    /// A definition goes into the matching source file (by the location settings), so the
    /// action edits two files.
    pub uses_matching_source: bool,
}

/// A member variable under the cursor.
struct Member<'t> {
    class: &'t Symbol,
    var: &'t VariableInfo,
}

fn member_at<'t>(tree: &'t SymbolTree, text: &str, offset: usize) -> Option<Member<'t>> {
    let path = tree.symbol_at(offset)?;
    let symbol = path.symbol();
    let class = path
        .parent
        .filter(|p| p.class.is_some() && !p.name.is_empty())?;
    if symbol
        .parent_scope()
        .is_none_or(|s| s.kind != ScopeKind::Class)
    {
        return None;
    }
    // The declarator under (or after) the cursor: `int a, b;` has two.
    let var = symbol
        .variables
        .iter()
        .find(|v| offset <= v.name_range.end)
        .or(symbol.variables.last())?;
    let template = text[symbol.range.start..].starts_with("template");
    if var.is_array || var.is_function || template || base_name(&var.name).is_empty() {
        return None;
    }
    Some(Member { class, var })
}

/// The getter and setter that would be generated for a member variable.
struct Accessors {
    getter: NewMember,
    setter: Option<NewMember>,
}

fn accessors(member: &Member, config: &Config) -> Accessors {
    let var = member.var;
    let style = config.case_style;
    let settings = &config.cpp;
    let prefix = if var.is_static {
        format!("{}::", member.class.name)
    } else if settings.use_explicit_this_pointer {
        "this->".to_string()
    } else {
        String::new()
    };
    let static_specifier = if var.is_static { "static " } else { "" };
    let getter_name = getter_name(var, style, settings.accessor.bool_getter_is_prefix);
    let setter_name = setter_name(&var.name, style);

    let declaration = format!(
        "{static_specifier}{}{getter_name}(){}",
        getter_return_type(var),
        if var.is_static { "" } else { " const" }
    );
    let body = format!("return {prefix}{};", var.name);
    let getter = NewMember {
        inline_text: format!("{declaration} {{ {body} }}"),
        name: getter_name.clone(),
        declaration,
        body,
        location: settings.accessor.getter_definition_location,
        relative: Some((setter_name.clone(), Side::Before)),
    };

    let setter = can_have_setter(var).then(|| {
        let parameter_name = setter_parameter_name(&var.name, style, !prefix.is_empty());
        let declaration = format!(
            "{static_specifier}void {setter_name}({}{parameter_name})",
            setter_parameter_type(var)
        );
        let body = format!("{prefix}{} = {parameter_name};", var.name);
        NewMember {
            inline_text: format!("{declaration} {{ {body} }}"),
            name: setter_name.clone(),
            declaration,
            body,
            location: settings.accessor.setter_definition_location,
            relative: Some((getter_name, Side::After)),
        }
    });
    Accessors { getter, setter }
}

/// Const, constexpr and reference members can't be assigned.
fn can_have_setter(var: &VariableInfo) -> bool {
    !var.is_const && !var.is_reference && !contains_word(&var.leading_text, "constexpr")
}

/// The accessor actions for the member variable at `position`, skipping accessors that
/// already exist (a class member with the accessor's name) and setters of members that can't
/// be assigned. `has_matching_source`: whether the file has a matching source file.
pub fn available_accessors(
    file: &FileInput,
    position: Position,
    has_matching_source: bool,
    config: &Config,
) -> Vec<AccessorAction> {
    if file.is_c {
        return Vec::new();
    }
    let tree = SymbolTree::new(file.text);
    let offset = position_to_offset(file.text, position);
    let Some(member) = member_at(&tree, file.text, offset) else {
        return Vec::new();
    };
    let accessors = accessors(&member, config);
    let exists = |name: &str| member.class.children.iter().any(|c| declares_name(c, name));
    let getter = !exists(&accessors.getter.name);
    let setter = accessors.setter.as_ref().is_some_and(|s| !exists(&s.name));
    let kinds = [
        (AccessorKind::GetterAndSetter, getter && setter),
        (AccessorKind::Getter, getter),
        (AccessorKind::Setter, setter),
    ];
    let to_source = |location: DefinitionLocation| {
        location == DefinitionLocation::MatchingSourceFile
            && file.is_header
            && has_matching_source
            && !member
                .class
                .scopes
                .iter()
                .chain(&member.class.own_scopes)
                .any(|s| s.template.is_some())
    };
    let accessor = &config.cpp.accessor;
    kinds
        .into_iter()
        .filter(|(_, offered)| *offered)
        .map(|(kind, _)| AccessorAction {
            kind,
            title: format!("{} for \"{}\"", kind.title_prefix(), member.var.name),
            uses_matching_source: kind.has_getter()
                && to_source(accessor.getter_definition_location)
                || kind.has_setter() && to_source(accessor.setter_definition_location),
        })
        .collect()
}

/// Generates the accessors of `kind` for the member variable at `position`. Definitions whose
/// location is the matching source file go into `matching_source` (below the class if `None`).
pub fn generate_accessors(
    file: &FileInput,
    position: Position,
    kind: AccessorKind,
    matching_source: Option<&FileInput>,
    config: &Config,
) -> Option<GeneratedEdit> {
    let available = available_accessors(file, position, matching_source.is_some(), config);
    if !available.iter().any(|a| a.kind == kind) {
        return None;
    }
    let tree = SymbolTree::new(file.text);
    let member = member_at(&tree, file.text, position_to_offset(file.text, position))?;
    let Accessors { mut getter, setter } = accessors(&member, config);
    let mut editor = ClassEditor::new(
        *file,
        matching_source.copied(),
        member.class.range.start,
        config,
    );
    if kind == AccessorKind::GetterAndSetter {
        // The setter goes right after the new getter.
        getter.relative = None;
    }
    if kind.has_getter() {
        editor.add(&getter)?;
    }
    if kind.has_setter() {
        editor.add(setter.as_ref()?)?;
    }
    Some(editor.finish())
}

/// Strips common member prefixes and suffixes (`SourceSymbol.baseName`): `_x`, `x_`, `m_x`,
/// `s_x` -> `x`.
pub fn base_name(name: &str) -> &str {
    if name.starts_with('_') || name.ends_with('_') {
        return name.trim_matches('_');
    }
    for prefix in ["m_", "s_"] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return rest;
        }
    }
    name
}

/// `util.make_snake_case`: `_` before each uppercase letter not at the start or after `_`,
/// then lowercase.
fn snake_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    let mut previous: Option<char> = None;
    for c in text.chars() {
        if c.is_ascii_uppercase() && previous.is_some_and(|p| p != '_') {
            out.push('_');
        }
        out.push(c);
        previous = Some(c);
    }
    out.to_lowercase()
}

/// `util.makeCamelCase` / `util.MakePascalCase`: `_x` -> `X`, then the first remaining `_` is
/// removed and the first letter lowered or raised.
fn camel_case(text: &str, pascal: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_'
            && let Some(&next) = chars.peek()
            && next.is_ascii_lowercase()
        {
            out.push(next.to_ascii_uppercase());
            chars.next();
            continue;
        }
        out.push(c);
    }
    if let Some(index) = out.find('_') {
        out.remove(index);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) if pascal => first.to_uppercase().chain(chars).collect(),
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => out,
    }
}

/// `cfg.formatToCaseStyle`.
pub fn format_to_case_style(text: &str, style: CaseStyle) -> String {
    match style {
        CaseStyle::SnakeCase => snake_case(text),
        CaseStyle::CamelCase => camel_case(text, false),
        CaseStyle::PascalCase => camel_case(text, true),
    }
}

/// Whether `text` contains `word` outside `<...>` and `(...)`.
fn contains_word_at_depth_zero(text: &str, word: &str) -> bool {
    let mut masked = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        masked.push(if depth > 0 { ' ' } else { c });
    }
    contains_word(&masked, word)
}

/// `CSymbol.getterName`.
fn getter_name(var: &VariableInfo, style: CaseStyle, bool_is_prefix: bool) -> String {
    let formatted = format_to_case_style(base_name(&var.name), style);
    if bool_is_prefix && contains_word_at_depth_zero(&var.leading_text, "bool") {
        let mut chars = formatted.chars();
        let has_is_prefix = matches!(chars.next(), Some('i' | 'I'))
            && chars.next() == Some('s')
            && chars
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_uppercase());
        if !has_is_prefix {
            return format_to_case_style(&format!("is_{formatted}"), style);
        }
    }
    if formatted != var.name {
        return formatted;
    }
    format_to_case_style(&format!("get_{formatted}"), style)
}

/// `CSymbol.setterName`.
fn setter_name(name: &str, style: CaseStyle) -> String {
    format_to_case_style(&format!("set_{}", base_name(name)), style)
}

/// The setter's parameter name (`Setter` constructor): the base name if it differs from the
/// member's name, else the case-styled base name, else the name with a trailing `_`.
fn setter_parameter_name(name: &str, style: CaseStyle, has_member_prefix: bool) -> String {
    let base = base_name(name);
    if base != name {
        return base.to_string();
    }
    let formatted = format_to_case_style(base, style);
    if formatted != name || has_member_prefix {
        formatted
    } else {
        format!("{base}_")
    }
}

/// Removes the whole words in `words` that are outside `<...>`, `(...)` and start at or
/// after byte `from`, then collapses whitespace (keeping a trailing space) and trims the start.
fn remove_words(text: &str, words: &[&str], from: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        if is_word_char(c) {
            let end = text[i..]
                .find(|c: char| !is_word_char(c))
                .map_or(text.len(), |e| i + e);
            let word = &text[i..end];
            if !(depth == 0 && i >= from && words.contains(&word)) {
                out.push_str(word);
            }
            i = end;
            continue;
        }
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        out.push(c);
        i += c.len_utf8();
    }
    let mut collapsed = String::with_capacity(out.len());
    for c in out.chars() {
        if c.is_whitespace() {
            if !collapsed.ends_with(' ') {
                collapsed.push(' ');
            }
        } else {
            collapsed.push(c);
        }
    }
    collapsed.trim_start().to_string()
}

/// The getter's return type with a trailing separator (`int `, `char* `). Like C-Mantic it
/// returns by value without `static`/`mutable`; unlike C-Mantic only top-level `const` and
/// `volatile` are removed, so `const char*` and `const T&` members stay valid.
fn getter_return_type(var: &VariableInfo) -> String {
    let stripped = remove_words(&var.leading_text, &["static", "mutable"], 0);
    if var.is_reference {
        return stripped;
    }
    let from = if var.is_pointer {
        match last_star_at_depth_zero(&stripped) {
            Some(star) => star,
            None => return stripped,
        }
    } else {
        0
    };
    remove_words(&stripped, &["const", "volatile"], from)
}

fn last_star_at_depth_zero(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut last = None;
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            '*' if depth == 0 => last = Some(i),
            _ => {}
        }
    }
    last
}

const PRIMITIVE_TYPES: [&str; 14] = [
    "void", "bool", "char", "wchar_t", "char8_t", "char16_t", "char32_t", "int", "short", "long",
    "signed", "unsigned", "float", "double",
];

/// `parse.matchesPrimitiveType`: a primitive type keyword, and no template arguments.
fn is_primitive(text: &str) -> bool {
    !(text.contains('<') && text.contains('>'))
        && PRIMITIVE_TYPES.iter().any(|t| contains_word(text, t))
}

/// The setter's parameter type with a trailing separator (`int `, `const std::string &`).
fn setter_parameter_type(var: &VariableInfo) -> String {
    let ty = remove_words(&var.leading_text, &["static", "mutable", "inline"], 0);
    if !var.is_pointer && !is_primitive(&var.leading_text) {
        format!("const {ty}&")
    } else {
        ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_styles() {
        assert_eq!(snake_case("get_myValue"), "get_my_value");
        assert_eq!(snake_case("Value"), "value");
        assert_eq!(camel_case("get_my_value", false), "getMyValue");
        assert_eq!(camel_case("set_myValue", false), "setMyValue");
        assert_eq!(camel_case("get_x", true), "GetX");
        assert_eq!(camel_case("is_Ready", false), "isReady");
    }

    #[test]
    fn base_names() {
        assert_eq!(base_name("m_count"), "count");
        assert_eq!(base_name("s_count"), "count");
        assert_eq!(base_name("_count"), "count");
        assert_eq!(base_name("count_"), "count");
        assert_eq!(base_name("m_"), "m");
        assert_eq!(base_name("count"), "count");
    }

    #[test]
    fn word_removal() {
        assert_eq!(
            remove_words(
                "static  const std::map<const int, x>  ",
                &["static", "const"],
                0
            ),
            "std::map<const int, x> "
        );
    }
}
