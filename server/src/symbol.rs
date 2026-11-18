//! Tree-sitter based C/C++ symbol model (stands in for clangd's document symbols plus the
//! text analysis in C-Mantic's `CSymbol.ts`).
//!
//! All ranges are byte offsets into the parsed text.

use std::ops::Range;

use tree_sitter::{Node, Parser, Tree};

use crate::text::{contains_word, leading_whitespace, line_start, normalize_whitespace};

pub fn parse(text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("tree-sitter-cpp grammar is compatible with the tree-sitter crate");
    parser.parse(text, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Namespace,
    Class,
    Function,
    /// Variables, enums, type aliases, forward declarations, ...
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Namespace,
    Class,
}

/// An enclosing namespace or class of a symbol.
#[derive(Debug, Clone)]
pub struct Scope {
    pub kind: ScopeKind,
    /// Plain name (no template arguments). Empty for anonymous namespaces/classes.
    pub name: String,
    /// What to write before `::` when qualifying a member: `ns`, `Foo`, `Foo<T>`, `Foo<int>`.
    pub qualifier: String,
    /// Template statement of an unspecialized class template, with default arguments removed.
    pub template: Option<String>,
    /// Range of the scope's symbol (`namespace ... { }` / `class ... { };`).
    pub range: Range<usize>,
    /// The second and later names of `namespace a::b { }` share the block of the first.
    pub qualified_namespace: bool,
}

impl Scope {
    pub fn is_anonymous(&self) -> bool {
        self.name.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionBody {
    /// Declaration only: `void f();`
    None,
    /// `{ ... }` or a function-try-block.
    Body,
    /// `= 0`
    PureVirtual,
    /// `= default` / `= delete`
    DefaultedOrDeleted,
}

#[derive(Debug, Clone)]
pub struct FunctionInfo {
    /// Normalized name: `foo`, `~Foo`, `operator==`, `operator bool`.
    pub name: String,
    pub name_range: Range<usize>,
    /// Start of explicit qualifiers before the name (`Foo::` in `void Foo::bar()`).
    pub scope_start: usize,
    /// Explicit qualifiers in the declarator, without template arguments.
    pub qualifiers: Vec<String>,
    /// Start of the declaration after any template statements.
    pub decl_start: usize,
    /// End of the declaration head: before `;`, `= 0`, an initializer list or the body.
    pub decl_end: usize,
    /// The parameter list including its parentheses.
    pub params: Range<usize>,
    /// Ranges (inside `params`) of ` = value` default arguments.
    pub default_args: Vec<Range<usize>>,
    /// Normalized parameter types, used to match declarations with definitions.
    pub param_types: Vec<String>,
    pub is_const: bool,
    pub ref_qualifier: String,
    pub body: FunctionBody,
    /// The function's own template statements, outermost first, defaults removed.
    pub templates: Vec<String>,
    pub is_friend: bool,
    pub is_constructor: bool,
    pub is_destructor: bool,
}

impl FunctionInfo {
    pub fn is_declaration(&self) -> bool {
        self.body == FunctionBody::None
    }

    pub fn is_definition(&self) -> bool {
        self.body == FunctionBody::Body
    }

    pub fn is_unspecialized_template(&self) -> bool {
        self.templates.iter().any(|t| !is_specialization(t))
    }
}

fn is_specialization(template_statement: &str) -> bool {
    template_statement
        .strip_prefix("template")
        .is_some_and(|rest| rest.trim().trim_start_matches('<').trim() == ">")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKey {
    Class,
    Struct,
    Union,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Public,
    Protected,
    Private,
}

/// A base class in a class head (`: public ns::Base<T>`).
#[derive(Debug, Clone)]
pub struct BaseClass {
    /// Full type as written: `ns::Base<T>`.
    pub text: String,
    /// Unqualified name without template arguments (`Base`), as used in a member initializer.
    pub name: String,
}

/// Extra information about a class, struct or union with a body.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub key: ClassKey,
    pub base_classes: Vec<BaseClass>,
    /// Access specifier labels in the body (`public:`, range includes the colon), in order.
    pub access_specifiers: Vec<(Range<usize>, Access)>,
}

/// One declarator of a variable declaration (`int a, *b;` has two).
#[derive(Debug, Clone)]
pub struct VariableInfo {
    pub name: String,
    pub name_range: Range<usize>,
    pub is_static: bool,
    /// Top-level `const` (`const int x`, `int* const p`, not `const char* p`).
    pub is_const: bool,
    pub is_reference: bool,
    pub is_pointer: bool,
    pub is_array: bool,
    /// Has a default member initializer (`= value` or `{value}`).
    pub has_initializer: bool,
}

impl VariableInfo {
    /// Const and reference members without a default member initializer must be initialized
    /// by every constructor.
    pub fn requires_initialization(&self) -> bool {
        !self.is_static && !self.has_initializer && (self.is_const || self.is_reference)
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub kind: SymbolKind,
    /// Display name (namespaces: `a::b`; classes: `Foo`).
    pub name: String,
    /// From the first template statement (if any) to the end, including a trailing `;`.
    pub range: Range<usize>,
    /// Start of the leading comment, or `range.start` if there is none.
    pub comment_start: usize,
    /// Body of a namespace or class (`{ ... }`).
    pub body: Option<Range<usize>>,
    /// Enclosing scopes, outermost first.
    pub scopes: Vec<Scope>,
    /// The namespace/class as a scope of its children (empty for other symbols).
    pub own_scopes: Vec<Scope>,
    pub children: Vec<Symbol>,
    pub function: Option<FunctionInfo>,
    /// Set for classes, structs and unions with a body.
    pub class: Option<ClassInfo>,
    /// Declarators of a variable declaration (member or not); empty for other symbols.
    pub variables: Vec<VariableInfo>,
}

impl Symbol {
    /// Non-static data members of this class, in declaration order.
    pub fn non_static_member_variables(&self) -> impl Iterator<Item = &VariableInfo> {
        self.children
            .iter()
            .flat_map(|c| &c.variables)
            .filter(|v| !v.is_static)
    }

    /// Constructor declarations and definitions directly inside this class.
    pub fn constructors(&self) -> impl Iterator<Item = &Symbol> {
        self.children
            .iter()
            .filter(|c| c.function.as_ref().is_some_and(|f| f.is_constructor))
    }

    /// The access in effect at `offset` inside this class's body.
    pub fn access_at(&self, offset: usize) -> Access {
        let Some(class) = &self.class else {
            return Access::Public;
        };
        let mut access = match class.key {
            ClassKey::Class => Access::Private,
            ClassKey::Struct | ClassKey::Union => Access::Public,
        };
        for (range, label) in &class.access_specifiers {
            if range.end <= offset {
                access = *label;
            }
        }
        access
    }

    pub fn is_function_declaration(&self) -> bool {
        self.function
            .as_ref()
            .is_some_and(FunctionInfo::is_declaration)
    }

    /// Name path used to match a declaration with its definition.
    pub fn qualified_name(&self) -> Vec<String> {
        let Some(func) = &self.function else {
            return Vec::new();
        };
        let mut path: Vec<String> = self
            .scopes
            .iter()
            .filter(|s| !func.is_friend || s.kind == ScopeKind::Namespace)
            .map(|s| s.name.clone())
            .collect();
        path.extend(func.qualifiers.iter().cloned());
        path.push(func.name.clone());
        path
    }

    /// Whether `self` (a declaration) and `other` (a definition) are the same function.
    pub fn matches_function(&self, other: &Symbol) -> bool {
        let (Some(a), Some(b)) = (&self.function, &other.function) else {
            return false;
        };
        a.is_const == b.is_const
            && a.ref_qualifier == b.ref_qualifier
            && a.param_types == b.param_types
            && self.qualified_name() == other.qualified_name()
    }

    /// Whether this symbol or any of its enclosing classes is an unspecialized template.
    pub fn has_unspecialized_template(&self) -> bool {
        self.scopes.iter().any(|s| s.template.is_some())
            || self
                .function
                .as_ref()
                .is_some_and(FunctionInfo::is_unspecialized_template)
    }

    /// The parent namespace or class, if any.
    pub fn parent_scope(&self) -> Option<&Scope> {
        self.scopes.last()
    }

    pub fn is_class_member(&self) -> bool {
        self.parent_scope()
            .is_some_and(|s| s.kind == ScopeKind::Class)
            && !self.function.as_ref().is_some_and(|f| f.is_friend)
    }
}

/// The symbols of one file.
#[derive(Debug, Default)]
pub struct SymbolTree {
    pub symbols: Vec<Symbol>,
}

impl SymbolTree {
    pub fn new(text: &str) -> Self {
        let Some(tree) = parse(text) else {
            return Self::default();
        };
        Self::from_tree(text, &tree)
    }

    pub fn from_tree(text: &str, tree: &Tree) -> Self {
        let mut symbols = Vec::new();
        Collector { text }.collect_children(tree.root_node(), &[], &mut symbols);
        Self { symbols }
    }

    /// The innermost symbol whose range contains `offset` (end inclusive), with its siblings.
    pub fn symbol_at(&self, offset: usize) -> Option<SymbolPath<'_>> {
        let mut siblings: &[Symbol] = &self.symbols;
        let mut parent: Option<&Symbol> = None;
        let mut found: Option<SymbolPath> = None;
        loop {
            let Some(index) = siblings
                .iter()
                .position(|s| s.range.start <= offset && offset <= s.range.end)
            else {
                return found;
            };
            found = Some(SymbolPath {
                parent,
                siblings,
                index,
            });
            parent = Some(&siblings[index]);
            siblings = &siblings[index].children;
        }
    }

    /// Depth-first iterator over all symbols.
    pub fn all(&self) -> impl Iterator<Item = &Symbol> {
        let mut stack: Vec<&Symbol> = self.symbols.iter().rev().collect();
        std::iter::from_fn(move || {
            let symbol = stack.pop()?;
            stack.extend(symbol.children.iter().rev());
            Some(symbol)
        })
    }

    /// Finds a function definition matching the declaration `decl` (which may live in another file).
    pub fn find_definition(&self, decl: &Symbol) -> Option<&Symbol> {
        self.all().find(|s| {
            s.function.as_ref().is_some_and(FunctionInfo::is_definition) && decl.matches_function(s)
        })
    }

    /// Finds the first namespace whose full name path equals `path`.
    pub fn find_namespace(&self, path: &[String]) -> Option<&Symbol> {
        self.all().find(|s| {
            s.kind == SymbolKind::Namespace
                && s.own_scopes.len() + s.scopes.len() == path.len()
                && s.scopes
                    .iter()
                    .chain(&s.own_scopes)
                    .map(|scope| &scope.name)
                    .eq(path.iter())
        })
    }

    /// The namespaces and classes whose range strictly contains `offset`, outermost first.
    pub fn scopes_at(&self, offset: usize) -> Vec<Scope> {
        let mut scopes = Vec::new();
        let mut symbols: &[Symbol] = &self.symbols;
        while let Some(symbol) = symbols
            .iter()
            .find(|s| !s.own_scopes.is_empty() && s.range.start < offset && offset < s.range.end)
        {
            scopes.extend(symbol.own_scopes.iter().cloned());
            symbols = &symbol.children;
        }
        scopes
    }
}

/// A symbol located in the tree, with access to its parent and siblings.
#[derive(Debug, Clone, Copy)]
pub struct SymbolPath<'a> {
    pub parent: Option<&'a Symbol>,
    pub siblings: &'a [Symbol],
    pub index: usize,
}

impl<'a> SymbolPath<'a> {
    pub fn symbol(&self) -> &'a Symbol {
        &self.siblings[self.index]
    }
}

struct Collector<'t> {
    text: &'t str,
}

impl<'t> Collector<'t> {
    fn node_text(&self, node: Node) -> &'t str {
        &self.text[node.byte_range()]
    }

    /// Collects symbols from the children of a container node (translation unit, namespace
    /// body, class body, preprocessor block, ...).
    fn collect_children(&self, container: Node, scopes: &[Scope], out: &mut Vec<Symbol>) {
        let mut cursor = container.walk();
        let children: Vec<Node> = container.children(&mut cursor).collect();
        let mut comments: Vec<Range<usize>> = Vec::new();
        for (i, &child) in children.iter().enumerate() {
            match child.kind() {
                "comment" => {
                    comments.push(child.byte_range());
                    continue;
                }
                "preproc_if" | "preproc_ifdef" | "preproc_else" | "preproc_elif"
                | "preproc_elifdef" | "ERROR" => {
                    self.collect_children(child, scopes, out);
                }
                "linkage_specification" => match child.child_by_field_name("body") {
                    Some(body) if body.kind() == "declaration_list" => {
                        self.collect_children(body, scopes, out);
                    }
                    Some(body) => {
                        let comment_start =
                            self.leading_comment_start(&comments, child.start_byte());
                        if let Some(mut symbol) = self.symbol(body, child, &[], scopes) {
                            symbol.range.start = child.start_byte();
                            symbol.comment_start = comment_start.min(symbol.range.start);
                            out.push(symbol);
                        }
                    }
                    None => {}
                },
                _ => {
                    let (inner, templates) = self.unwrap_templates(child);
                    if let Some(mut symbol) = self.symbol(inner, child, &templates, scopes) {
                        // Class specifiers are followed by a separate `;` token.
                        if let Some(next) = children.get(i + 1)
                            && next.kind() == ";"
                            && matches!(symbol.kind, SymbolKind::Class | SymbolKind::Other)
                            && symbol.range.end == child.end_byte()
                        {
                            symbol.range.end = next.end_byte();
                        }
                        symbol.comment_start =
                            self.leading_comment_start(&comments, symbol.range.start);
                        out.push(symbol);
                    }
                }
            }
            comments.clear();
        }
    }

    /// Start of the comments directly above `start` (no blank line in between, each comment
    /// on its own line), or `start` if there are none.
    fn leading_comment_start(&self, comments: &[Range<usize>], start: usize) -> usize {
        let mut result = start;
        for comment in comments.iter().rev() {
            let gap = &self.text[comment.end..result];
            let own_line = self.text[line_start(self.text, comment.start)..comment.start]
                .trim()
                .is_empty();
            if !gap.trim().is_empty() || gap.matches('\n').count() > 1 || !own_line {
                break;
            }
            result = comment.start;
        }
        result
    }

    /// Strips nested `template_declaration` wrappers, returning the inner node and the
    /// template statements (outermost first, default arguments removed).
    fn unwrap_templates(&self, node: Node<'t>) -> (Node<'t>, Vec<String>) {
        let mut templates = Vec::new();
        let mut node = node;
        while node.kind() == "template_declaration" {
            let Some(params) = node.child_by_field_name("parameters") else {
                break;
            };
            templates.push(self.template_statement(node, params));
            let mut cursor = node.walk();
            let inner = node
                .named_children(&mut cursor)
                .filter(|c| c.kind() != "comment" && c.id() != params.id())
                .last();
            match inner {
                Some(inner) => node = inner,
                None => break,
            }
        }
        (node, templates)
    }

    /// `template <typename T, int N>` from a template declaration, with defaults removed and
    /// continuation lines de-indented.
    fn template_statement(&self, node: Node, params: Node) -> String {
        let mut text = String::new();
        let mut pos = node.start_byte();
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            if let Some(range) = default_arg_range(self.text, param) {
                text.push_str(&self.text[pos..range.start]);
                pos = range.end;
            }
        }
        text.push_str(&self.text[pos..params.end_byte()]);
        let indent = leading_whitespace(&self.text[line_start(self.text, node.start_byte())..]);
        dedent(&text.replace("\r\n", "\n"), indent)
    }

    /// Builds a symbol for `node` (with template wrappers removed). `outer` is the node whose
    /// range the symbol covers (including template statements).
    fn symbol(
        &self,
        node: Node,
        outer: Node,
        templates: &[String],
        scopes: &[Scope],
    ) -> Option<Symbol> {
        let range = outer.byte_range();
        match node.kind() {
            "namespace_definition" => self.namespace(node, range, scopes),
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                match node.child_by_field_name("body") {
                    Some(_) => self.class(node, range, templates, scopes),
                    None => Some(self.other(range, scopes)),
                }
            }
            "declaration" | "field_declaration" => {
                if let Some(ty) = node.child_by_field_name("type")
                    && matches!(
                        ty.kind(),
                        "class_specifier" | "struct_specifier" | "union_specifier"
                    )
                    && ty.child_by_field_name("body").is_some()
                {
                    return self.class(ty, range, templates, scopes);
                }
                match self.function(node, templates, scopes, false) {
                    Some(func) => Some(self.function_symbol(func, range, scopes)),
                    None => {
                        let mut symbol = self.other(range, scopes);
                        symbol.variables = self.variables(node);
                        Some(symbol)
                    }
                }
            }
            "function_definition" => {
                let func = self.function(node, templates, scopes, false)?;
                Some(self.function_symbol(func, range, scopes))
            }
            "friend_declaration" => {
                let mut cursor = node.walk();
                let inner = node
                    .named_children(&mut cursor)
                    .find(|c| matches!(c.kind(), "declaration" | "function_definition"))?;
                let func = self.function(inner, templates, scopes, true)?;
                Some(self.function_symbol(func, range, scopes))
            }
            "enum_specifier" | "alias_declaration" | "type_definition" | "concept_definition" => {
                Some(self.other(range, scopes))
            }
            _ => None,
        }
    }

    fn other(&self, range: Range<usize>, scopes: &[Scope]) -> Symbol {
        Symbol {
            kind: SymbolKind::Other,
            name: String::new(),
            comment_start: range.start,
            range,
            body: None,
            scopes: scopes.to_vec(),
            own_scopes: Vec::new(),
            children: Vec::new(),
            function: None,
            class: None,
            variables: Vec::new(),
        }
    }

    fn namespace(&self, node: Node, range: Range<usize>, scopes: &[Scope]) -> Option<Symbol> {
        let body = node.child_by_field_name("body")?;
        let names: Vec<String> = match node.child_by_field_name("name") {
            Some(name) if name.kind() == "nested_namespace_specifier" => {
                let mut cursor = name.walk();
                name.named_children(&mut cursor)
                    .filter(|c| c.kind() == "namespace_identifier")
                    .map(|c| self.node_text(c).to_string())
                    .collect()
            }
            Some(name) => vec![self.node_text(name).to_string()],
            None => vec![String::new()],
        };
        let own_scopes: Vec<Scope> = names
            .iter()
            .enumerate()
            .map(|(i, name)| Scope {
                kind: ScopeKind::Namespace,
                name: name.clone(),
                qualifier: name.clone(),
                template: None,
                range: range.clone(),
                qualified_namespace: i > 0,
            })
            .collect();
        let mut child_scopes = scopes.to_vec();
        child_scopes.extend(own_scopes.iter().cloned());
        let mut children = Vec::new();
        self.collect_children(body, &child_scopes, &mut children);
        Some(Symbol {
            kind: SymbolKind::Namespace,
            name: names.join("::"),
            comment_start: range.start,
            range,
            body: Some(body.byte_range()),
            scopes: scopes.to_vec(),
            own_scopes,
            children,
            function: None,
            class: None,
            variables: Vec::new(),
        })
    }

    fn class(
        &self,
        node: Node,
        range: Range<usize>,
        templates: &[String],
        scopes: &[Scope],
    ) -> Option<Symbol> {
        let body = node.child_by_field_name("body")?;
        let name_node = node.child_by_field_name("name");
        let (name, qualifier, template) = match name_node {
            // Specialization: `Foo<int>` / `Foo<T*>`.
            Some(n) if n.kind() == "template_type" => {
                let base = n
                    .child_by_field_name("name")
                    .map_or_else(|| self.node_text(n), |b| self.node_text(b));
                let unspecialized = templates
                    .iter()
                    .rev()
                    .find(|t| !is_specialization(t))
                    .cloned();
                (
                    base.to_string(),
                    normalize_whitespace(self.node_text(n)),
                    unspecialized,
                )
            }
            Some(n) => {
                let name = self.node_text(n).to_string();
                let template = templates.last().filter(|t| !is_specialization(t)).cloned();
                let params = template
                    .as_deref()
                    .and_then(|_| self.template_parameter_names(node))
                    .unwrap_or_default();
                (name.clone(), format!("{name}{params}"), template)
            }
            None => (String::new(), String::new(), None),
        };
        let scope = Scope {
            kind: ScopeKind::Class,
            name: name.clone(),
            qualifier,
            template,
            range: range.clone(),
            qualified_namespace: false,
        };
        let mut child_scopes = scopes.to_vec();
        child_scopes.push(scope.clone());
        let mut children = Vec::new();
        self.collect_children(body, &child_scopes, &mut children);
        Some(Symbol {
            kind: SymbolKind::Class,
            name,
            comment_start: range.start,
            range,
            body: Some(body.byte_range()),
            scopes: scopes.to_vec(),
            own_scopes: vec![scope],
            children,
            function: None,
            class: Some(ClassInfo {
                key: match node.kind() {
                    "struct_specifier" => ClassKey::Struct,
                    "union_specifier" => ClassKey::Union,
                    _ => ClassKey::Class,
                },
                base_classes: self.base_classes(node),
                access_specifiers: self.access_specifiers(body),
            }),
            variables: Vec::new(),
        })
    }

    /// `<T, N>` for the class template enclosing `class_node` (the closest template statement).
    fn template_parameter_names(&self, class_node: Node) -> Option<String> {
        let template = class_node
            .parent()
            .filter(|p| p.kind() == "template_declaration")?;
        let params = template.child_by_field_name("parameters")?;
        let mut cursor = params.walk();
        let names: Vec<String> = params
            .named_children(&mut cursor)
            .filter_map(|p| self.template_parameter_name(p))
            .collect();
        Some(format!("<{}>", names.join(", ")))
    }

    fn template_parameter_name(&self, param: Node) -> Option<String> {
        let variadic = matches!(
            param.kind(),
            "variadic_type_parameter_declaration" | "variadic_parameter_declaration"
        );
        let name = match param.kind() {
            "type_parameter_declaration" | "variadic_type_parameter_declaration" => {
                let mut cursor = param.walk();
                param
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() == "type_identifier")
                    .last()
            }
            "optional_type_parameter_declaration" => param.child_by_field_name("name"),
            "parameter_declaration"
            | "optional_parameter_declaration"
            | "variadic_parameter_declaration" => param
                .child_by_field_name("declarator")
                .and_then(innermost_identifier),
            "template_template_parameter_declaration" => {
                let mut cursor = param.walk();
                let inner = param
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() != "template_parameter_list")
                    .last()?;
                return self.template_parameter_name(inner);
            }
            _ => None,
        }?;
        let name = self.node_text(name).trim_start_matches("...").trim();
        Some(if variadic {
            format!("{name}...")
        } else {
            name.to_string()
        })
    }

    fn function_symbol(&self, func: FunctionInfo, range: Range<usize>, scopes: &[Scope]) -> Symbol {
        Symbol {
            kind: SymbolKind::Function,
            name: func.name.clone(),
            comment_start: range.start,
            range,
            body: None,
            scopes: scopes.to_vec(),
            own_scopes: Vec::new(),
            children: Vec::new(),
            function: Some(func),
            class: None,
            variables: Vec::new(),
        }
    }

    fn base_classes(&self, class_node: Node) -> Vec<BaseClass> {
        let mut cursor = class_node.walk();
        let Some(clause) = class_node
            .children(&mut cursor)
            .find(|c| c.kind() == "base_class_clause")
        else {
            return Vec::new();
        };
        let mut cursor = clause.walk();
        clause
            .named_children(&mut cursor)
            .filter(|c| {
                matches!(
                    c.kind(),
                    "type_identifier" | "qualified_identifier" | "template_type"
                )
            })
            .map(|c| {
                let mut name = innermost_name(c);
                if name.kind() == "template_type"
                    && let Some(base) = name.child_by_field_name("name")
                {
                    name = base;
                }
                BaseClass {
                    text: normalize_whitespace(self.node_text(c)),
                    name: self.node_text(name).to_string(),
                }
            })
            .collect()
    }

    fn access_specifiers(&self, body: Node) -> Vec<(Range<usize>, Access)> {
        let mut cursor = body.walk();
        let children: Vec<Node> = body.children(&mut cursor).collect();
        let mut out = Vec::new();
        for (i, child) in children.iter().enumerate() {
            if child.kind() != "access_specifier" {
                continue;
            }
            let access = match self.node_text(*child).trim() {
                "public" => Access::Public,
                "protected" => Access::Protected,
                _ => Access::Private,
            };
            let end = children
                .get(i + 1)
                .filter(|n| n.kind() == ":")
                .map_or(child.end_byte(), |n| n.end_byte());
            out.push((child.start_byte()..end, access));
        }
        out
    }

    /// The declarators of a (member) variable declaration.
    fn variables(&self, node: Node) -> Vec<VariableInfo> {
        let mut is_static = false;
        let mut decl_const = false;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "storage_class_specifier" if self.node_text(child) == "static" => is_static = true,
                "type_qualifier" if self.node_text(child) == "const" => decl_const = true,
                _ => {}
            }
        }
        let mut cursor = node.walk();
        let declarators: Vec<Node> = node
            .children_by_field_name("declarator", &mut cursor)
            .collect();
        let mut cursor = node.walk();
        let default_values: Vec<Node> = node
            .children_by_field_name("default_value", &mut cursor)
            .collect();
        let mut out = Vec::new();
        for (i, declarator) in declarators.iter().enumerate() {
            let initialized = declarator.kind() == "init_declarator";
            let declarator = &match declarator.kind() {
                "init_declarator" => declarator
                    .child_by_field_name("declarator")
                    .unwrap_or(*declarator),
                _ => *declarator,
            };
            let Some(id) = innermost_identifier(*declarator) else {
                continue;
            };
            let is_pointer = declarator.kind() == "pointer_declarator";
            let is_const = if is_pointer {
                let mut cursor = declarator.walk();
                declarator
                    .children(&mut cursor)
                    .any(|c| c.kind() == "type_qualifier" && self.node_text(c) == "const")
            } else {
                decl_const
            };
            // A default member initializer belongs to the declarator before it.
            let next_start = declarators
                .get(i + 1)
                .map_or(usize::MAX, |d| d.start_byte());
            let has_initializer = initialized
                || default_values.iter().any(|v| {
                    v.start_byte() >= declarator.end_byte() && v.start_byte() < next_start
                });
            out.push(VariableInfo {
                name: self.node_text(id).to_string(),
                name_range: id.byte_range(),
                is_static,
                is_const,
                is_reference: declarator.kind() == "reference_declarator",
                is_pointer,
                is_array: declarator.kind() == "array_declarator",
                has_initializer,
            });
        }
        out
    }

    /// Analyses a `declaration`, `field_declaration` or `function_definition`.
    fn function(
        &self,
        node: Node,
        templates: &[String],
        scopes: &[Scope],
        is_friend: bool,
    ) -> Option<FunctionInfo> {
        let declarator = node.child_by_field_name("declarator")?;
        let found = find_function_declarator(declarator)?;
        let (name_node, params, qualifier_holder) = match found.kind() {
            "function_declarator" => (
                found.child_by_field_name("declarator")?,
                found.child_by_field_name("parameters")?,
                found,
            ),
            // operator_cast, possibly inside a qualified_identifier.
            _ => {
                let cast = if found.kind() == "qualified_identifier" {
                    innermost_name(found)
                } else {
                    found
                };
                let abstract_decl = cast.child_by_field_name("declarator")?;
                (
                    found,
                    abstract_decl.child_by_field_name("parameters")?,
                    abstract_decl,
                )
            }
        };

        let mut qualifiers = Vec::new();
        let mut name = name_node;
        while name.kind() == "qualified_identifier" {
            if let Some(scope) = name.child_by_field_name("scope") {
                let base = match scope.kind() {
                    "template_type" => scope.child_by_field_name("name").unwrap_or(scope),
                    _ => scope,
                };
                qualifiers.push(self.node_text(base).to_string());
            }
            match name.child_by_field_name("name") {
                Some(inner) => name = inner,
                None => break,
            }
        }
        let (name_text, name_range) = if name.kind() == "operator_cast" {
            // `operator bool` (the cast's declarator holds the parameters).
            let end = name
                .child_by_field_name("declarator")
                .map_or(name.end_byte(), |d| d.start_byte());
            let text = self.text[name.start_byte()..end].trim_end();
            (
                normalize_whitespace(text),
                name.start_byte()..name.start_byte() + text.len(),
            )
        } else {
            (normalize_name(self.node_text(name)), name.byte_range())
        };

        let mut is_const = false;
        let mut ref_qualifier = String::new();
        let mut cursor = qualifier_holder.walk();
        for child in qualifier_holder.children(&mut cursor) {
            if child.start_byte() < params.end_byte() {
                continue;
            }
            match child.kind() {
                "type_qualifier" if self.node_text(child) == "const" => is_const = true,
                "ref_qualifier" => ref_qualifier = self.node_text(child).to_string(),
                _ => {}
            }
        }

        let (param_types, default_args) = self.parameters(params);

        let body = match node.kind() {
            "function_definition" => {
                if node.child_by_field_name("body").is_some() {
                    FunctionBody::Body
                } else {
                    FunctionBody::DefaultedOrDeleted
                }
            }
            _ if node.child_by_field_name("default_value").is_some() => FunctionBody::PureVirtual,
            _ => FunctionBody::None,
        };

        let class_name = if qualifiers.is_empty() {
            scopes
                .last()
                .filter(|s| s.kind == ScopeKind::Class)
                .map(|s| s.name.as_str())
        } else {
            qualifiers.last().map(String::as_str)
        };
        let has_return_type = node.child_by_field_name("type").is_some();
        let is_destructor = name_text.starts_with('~');
        let is_constructor =
            !has_return_type && !is_destructor && class_name == Some(name_text.as_str());

        Some(FunctionInfo {
            name: name_text,
            name_range,
            scope_start: name_node.start_byte().min(name.start_byte()),
            qualifiers,
            decl_start: node.start_byte(),
            decl_end: declarator.end_byte(),
            params: params.byte_range(),
            default_args,
            param_types,
            is_const,
            ref_qualifier,
            body,
            templates: templates.to_vec(),
            is_friend,
            is_constructor,
            is_destructor,
        })
    }

    /// Normalized parameter types and the ranges of default arguments.
    fn parameters(&self, params: Node) -> (Vec<String>, Vec<Range<usize>>) {
        let mut types = Vec::new();
        let mut defaults = Vec::new();
        let mut cursor = params.walk();
        for param in params.children(&mut cursor) {
            match param.kind() {
                "..." => types.push("...".to_string()),
                "parameter_declaration"
                | "optional_parameter_declaration"
                | "variadic_parameter_declaration" => {
                    let mut end = param.end_byte();
                    if let Some(range) = default_arg_range(self.text, param) {
                        end = range.start;
                        defaults.push(range);
                    }
                    let mut text = String::new();
                    match param
                        .child_by_field_name("declarator")
                        .and_then(innermost_identifier)
                    {
                        Some(id) if id.end_byte() <= end => {
                            text.push_str(&self.text[param.start_byte()..id.start_byte()]);
                            text.push(' ');
                            text.push_str(&self.text[id.end_byte()..end]);
                        }
                        _ => text.push_str(&self.text[param.start_byte()..end]),
                    }
                    types.push(normalize_type(&text));
                }
                _ => {}
            }
        }
        if types.len() == 1 && types[0] == "void" {
            types.clear();
        }
        (types, defaults)
    }
}

/// Removes `prefix` from the start of every line after the first.
pub fn dedent(text: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return text.to_string();
    }
    let mut lines = text.split('\n');
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        out.push_str(line.strip_prefix(prefix).unwrap_or(line));
    }
    out
}

/// Range of ` = value` in an optional (template) parameter, including the whitespace before `=`.
fn default_arg_range(text: &str, param: Node) -> Option<Range<usize>> {
    let value = param.child_by_field_name("default_value").or_else(|| {
        (param.kind() == "optional_type_parameter_declaration")
            .then(|| param.child_by_field_name("default_type"))
            .flatten()
    })?;
    let mut cursor = param.walk();
    let eq = param.children(&mut cursor).find(|c| c.kind() == "=")?;
    let start = text[..eq.start_byte()].trim_end().len();
    Some(start..value.end_byte())
}

/// Finds the function declarator inside a declaration's declarator, looking through pointer
/// and reference declarators. Returns `None` for function pointers and non-functions.
fn find_function_declarator(mut node: Node) -> Option<Node> {
    loop {
        match node.kind() {
            "function_declarator" => {
                let inner = node.child_by_field_name("declarator")?;
                return (inner.kind() != "parenthesized_declarator").then_some(node);
            }
            "operator_cast" => return Some(node),
            "qualified_identifier" => {
                return (innermost_name(node).kind() == "operator_cast").then_some(node);
            }
            "pointer_declarator" | "reference_declarator" | "attributed_declarator" => {
                let mut cursor = node.walk();
                node = node
                    .child_by_field_name("declarator")
                    .or_else(|| node.named_children(&mut cursor).last())?;
            }
            _ => return None,
        }
    }
}

fn innermost_name(mut node: Node) -> Node {
    while node.kind() == "qualified_identifier" {
        match node.child_by_field_name("name") {
            Some(name) => node = name,
            None => break,
        }
    }
    node
}

/// The identifier declared by a (parameter) declarator, if any.
fn innermost_identifier(mut node: Node) -> Option<Node> {
    loop {
        match node.kind() {
            "identifier" | "field_identifier" => return Some(node),
            "pointer_declarator"
            | "reference_declarator"
            | "array_declarator"
            | "function_declarator"
            | "parenthesized_declarator"
            | "attributed_declarator" => {
                let mut cursor = node.walk();
                node = node
                    .child_by_field_name("declarator")
                    .or_else(|| node.named_children(&mut cursor).next())?;
            }
            _ => return None,
        }
    }
}

/// Normalizes a function name (`operator ==` -> `operator==`, `~ Foo` -> `~Foo`).
fn normalize_name(name: &str) -> String {
    let normalized = normalize_whitespace(name);
    match normalized.strip_prefix("operator ") {
        Some(rest) if !rest.starts_with(|c: char| c.is_alphabetic()) => format!("operator{rest}"),
        _ => normalized,
    }
}

/// Normalizes a parameter type for comparison: no comments, canonical whitespace, no scope
/// qualifiers and no elaborated type keywords.
pub fn normalize_type(text: &str) -> String {
    let text = strip_comments(text);
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if crate::text::is_word_char(c) {
            let mut word = c.to_string();
            while let Some(&n) = chars.peek() {
                if !crate::text::is_word_char(n) {
                    break;
                }
                word.push(n);
                chars.next();
            }
            tokens.push(word);
        } else if c == ':' && chars.peek() == Some(&':') {
            chars.next();
            tokens.push("::".into());
        } else {
            tokens.push(c.to_string());
        }
    }
    let mut out = String::new();
    let mut previous_word = false;
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        if token == "::" || tokens.get(i + 1).is_some_and(|t| t == "::") && is_word(token) {
            // Drop scope qualifiers (`std::`, `ns::Foo::`).
            i += 1;
            continue;
        }
        if matches!(
            token.as_str(),
            "struct" | "class" | "enum" | "union" | "typename"
        ) {
            i += 1;
            continue;
        }
        let word = is_word(token);
        if word && previous_word {
            out.push(' ');
        }
        out.push_str(token);
        previous_word = word;
        i += 1;
    }
    out
}

fn is_word(token: &str) -> bool {
    token.chars().next().is_some_and(crate::text::is_word_char)
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let line = rest.find("//");
        let block = rest.find("/*");
        match (line, block) {
            (Some(l), b) if b.is_none_or(|b| l < b) => {
                out.push_str(&rest[..l]);
                rest = rest[l..].find('\n').map_or("", |n| &rest[l + n..]);
            }
            (_, Some(b)) => {
                out.push_str(&rest[..b]);
                out.push(' ');
                rest = rest[b + 2..]
                    .find("*/")
                    .map_or("", |e| &rest[b + 2 + e + 2..]);
            }
            _ => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// Whether the text before the name contains a specifier like `inline` or `constexpr`.
pub fn leading_text_has(text: &str, symbol: &Symbol, word: &str) -> bool {
    symbol
        .function
        .as_ref()
        .is_some_and(|f| contains_word(&text[symbol.range.start..f.name_range.start], word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn functions(text: &str) -> Vec<Symbol> {
        SymbolTree::new(text)
            .all()
            .filter(|s| s.kind == SymbolKind::Function)
            .cloned()
            .collect()
    }

    #[test]
    fn qualified_names_and_params() {
        let text = "#ifndef X\n#define X\nnamespace a::b {\ntemplate <typename T, int N = 3>\nclass Foo {\n    int get(const std::string& s, int x = 4) const;\n};\n}\n#endif\n";
        let funcs = functions(text);
        assert_eq!(funcs.len(), 1);
        let f = &funcs[0];
        assert_eq!(f.qualified_name(), vec!["a", "b", "Foo", "get"]);
        let info = f.function.as_ref().unwrap();
        assert_eq!(info.param_types, vec!["const string&", "int"]);
        assert!(info.is_const);
        assert_eq!(f.scopes[2].qualifier, "Foo<T, N>");
        assert_eq!(
            f.scopes[2].template.as_deref(),
            Some("template <typename T, int N>")
        );
        assert!(f.has_unspecialized_template());
    }

    #[test]
    fn matches_out_of_class_definition() {
        let header = "namespace ns { class Foo { void f(int a, char* = nullptr) &&; operator bool() const; ~Foo(); }; }";
        let source = "void ns::Foo::f(int b, char *c) && {}\nns::Foo::operator bool() const { return true; }\nns::Foo::~Foo() {}";
        let decls = functions(header);
        let tree = SymbolTree::new(source);
        for decl in &decls {
            assert!(tree.find_definition(decl).is_some(), "{}", decl.name);
        }
        assert!(decls[2].function.as_ref().unwrap().is_destructor);
    }

    #[test]
    fn classifies_bodies() {
        let text = "struct A { A() = default; virtual void p() = 0; void d() = delete; void x() {} void y(); };";
        let bodies: Vec<FunctionBody> = functions(text)
            .iter()
            .map(|f| f.function.as_ref().unwrap().body)
            .collect();
        assert_eq!(
            bodies,
            vec![
                FunctionBody::DefaultedOrDeleted,
                FunctionBody::PureVirtual,
                FunctionBody::DefaultedOrDeleted,
                FunctionBody::Body,
                FunctionBody::None
            ]
        );
    }

    #[test]
    fn class_members_bases_and_access() {
        let text = "class Foo : public ns::Base<int>, private virtual Other {\npublic:\n    Foo(int x);\n    static int s;\n    const int c = 1;\n    int& r;\n    int* const p, q[3];\n    const char* cp;\n    int b : 3;\n    std::string name{\"x\"};\nprotected:\n    mutable int m;\n};\n";
        let tree = SymbolTree::new(text);
        let class = &tree.symbols[0];
        let info = class.class.as_ref().unwrap();
        assert_eq!(info.key, ClassKey::Class);
        let bases: Vec<(&str, &str)> = info
            .base_classes
            .iter()
            .map(|b| (b.text.as_str(), b.name.as_str()))
            .collect();
        assert_eq!(bases, vec![("ns::Base<int>", "Base"), ("Other", "Other")]);
        let members: Vec<&str> = class
            .non_static_member_variables()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(members, vec!["c", "r", "p", "q", "cp", "b", "name", "m"]);
        let required: Vec<&str> = class
            .non_static_member_variables()
            .filter(|v| v.requires_initialization())
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(required, vec!["r", "p"]);
        assert_eq!(class.constructors().count(), 1);
        assert_eq!(
            class.access_at(text.find("Foo(int").unwrap()),
            Access::Public
        );
        assert_eq!(
            class.access_at(text.find("mutable").unwrap()),
            Access::Protected
        );
        assert_eq!(
            class.access_at(text.find('{').unwrap() + 1),
            Access::Private
        );
    }

    #[test]
    fn leading_comments() {
        let text = "int a; // trailing\n// doc 1\n// doc 2\nvoid f();\n\n// detached\n\nvoid g();";
        let tree = SymbolTree::new(text);
        let f = &tree.symbols[1];
        assert_eq!(
            &text[f.comment_start..f.range.start],
            "// doc 1\n// doc 2\n"
        );
        let g = &tree.symbols[2];
        assert_eq!(g.comment_start, g.range.start);
    }
}
