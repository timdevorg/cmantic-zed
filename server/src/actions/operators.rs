//! Generate equality, relational and stream output operators (port of
//! `commands/generateOperators.ts` and `Operator.ts`).
//!
//! C-Mantic asks which operands to compare and where to define each operator; this port
//! always uses every base class and non-static data member and defines the operators inline.

use tower_lsp_server::ls_types::Position;

use crate::actions::add_definition::{FileInput, indent_unit, is_on_class_head};
use crate::actions::member::{ClassEditor, GeneratedEdit, NewMember};
use crate::config::{Config, DefinitionLocation};
use crate::placement::{Side, declares_name};
use crate::symbol::{ClassKey, Symbol, SymbolTree};
use crate::text::position_to_offset;

pub const TITLE_EQUALITY: &str = "Generate Equality Operators";
pub const TITLE_RELATIONAL: &str = "Generate Relational Operators";
pub const TITLE_STREAM_OUTPUT: &str = "Generate Stream Output Operator";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorKind {
    /// `==` and `!=`.
    Equality,
    /// `<`, `>`, `<=` and `>=`.
    Relational,
    /// `<<` with `std::ostream`.
    StreamOutput,
}

impl OperatorKind {
    pub const ALL: [Self; 3] = [Self::Equality, Self::Relational, Self::StreamOutput];

    fn title_prefix(self) -> &'static str {
        match self {
            Self::Equality => TITLE_EQUALITY,
            Self::Relational => TITLE_RELATIONAL,
            Self::StreamOutput => TITLE_STREAM_OUTPUT,
        }
    }

    fn names(self) -> &'static [&'static str] {
        match self {
            Self::Equality => &["operator==", "operator!="],
            Self::Relational => &["operator<", "operator>", "operator<=", "operator>="],
            Self::StreamOutput => &["operator<<"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorAction {
    pub kind: OperatorKind,
    /// `Generate Equality Operators for "Foo"`.
    pub title: String,
}

/// The named class or struct whose head is at `offset`.
fn class_at_head(tree: &SymbolTree, offset: usize) -> Option<&Symbol> {
    let symbol = tree.symbol_at(offset)?.symbol();
    let info = symbol.class.as_ref()?;
    (is_on_class_head(symbol, offset) && info.key != ClassKey::Union && !symbol.name.is_empty())
        .then_some(symbol)
}

fn missing<'a>(class: &Symbol, kind: OperatorKind) -> Vec<&'a str> {
    kind.names()
        .iter()
        .copied()
        .filter(|name| !class.children.iter().any(|c| declares_name(c, name)))
        .collect()
}

/// Operator actions for the class head at `position`; operators that already exist in the
/// class (as members or friends) are left out.
pub fn available_operators(file: &FileInput, position: Position) -> Vec<OperatorAction> {
    if file.is_c {
        return Vec::new();
    }
    let tree = SymbolTree::new(file.text);
    let Some(class) = class_at_head(&tree, position_to_offset(file.text, position)) else {
        return Vec::new();
    };
    OperatorKind::ALL
        .into_iter()
        .filter(|&kind| !missing(class, kind).is_empty())
        .map(|kind| OperatorAction {
            kind,
            title: format!("{} for \"{}\"", kind.title_prefix(), class.name),
        })
        .collect()
}

/// A base class or data member to compare or print.
enum Operand {
    Base(String),
    Member(String),
}

/// Texts shared by all operators of a class.
struct Context {
    /// `Foo` / `Foo<T>`.
    type_name: String,
    friend: bool,
    explicit_this: bool,
    unit: String,
    operands: Vec<Operand>,
}

impl Context {
    fn lhs(&self) -> &'static str {
        if self.friend {
            "lhs."
        } else if self.explicit_this {
            "this->"
        } else {
            ""
        }
    }

    fn lhs_cast(&self) -> &'static str {
        if self.friend { "(lhs)" } else { "(*this)" }
    }

    fn rhs(&self) -> &'static str {
        if self.friend { "rhs." } else { "other." }
    }

    fn rhs_cast(&self) -> &'static str {
        if self.friend { "(rhs)" } else { "(other)" }
    }

    /// `ComparisonOperator.declaration`.
    fn comparison_declaration(&self, name: &str) -> String {
        let ty = format!("const {} &", self.type_name);
        if self.friend {
            format!("friend bool {name}({ty}lhs, {ty}rhs)")
        } else {
            format!("bool {name}({ty}other) const")
        }
    }

    /// `EqualOperator.setOperands`.
    fn equal_body(&self) -> String {
        if self.operands.is_empty() {
            return "return true;".into();
        }
        let alignment = if self.unit.contains(' ') {
            "    "
        } else {
            &self.unit
        };
        let separator = format!("\n{}{alignment}&& ", self.unit);
        let terms: Vec<String> = self
            .operands
            .iter()
            .map(|operand| match operand {
                Operand::Base(base) => {
                    let cast = format!("static_cast<const {base} &>");
                    format!("{cast}{} == {cast}{}", self.lhs_cast(), self.rhs_cast())
                }
                Operand::Member(name) => {
                    format!("{}{name} == {}{name}", self.lhs(), self.rhs())
                }
            })
            .collect();
        format!("return {};", terms.join(&separator))
    }

    /// `LessThanOperator.setOperands`.
    fn less_body(&self) -> String {
        let Some((last, rest)) = self.operands.split_last() else {
            return "return false;".into();
        };
        let unit = &self.unit;
        let return_true = format!("\n{unit}{unit}return true;\n{unit}");
        let return_false = format!("\n{unit}{unit}return false;\n{unit}");
        let sides = |operand: &Operand| match operand {
            Operand::Base(base) => {
                let cast = format!("static_cast<const {base} &>");
                (
                    format!("{cast}{}", self.lhs_cast()),
                    format!("{cast}{}", self.rhs_cast()),
                )
            }
            Operand::Member(name) => (
                format!("{}{name}", self.lhs()),
                format!("{}{name}", self.rhs()),
            ),
        };
        let mut body = String::new();
        for operand in rest {
            let (lhs, rhs) = sides(operand);
            body.push_str(&format!("if ({lhs} < {rhs}){return_true}"));
            body.push_str(&format!("if ({rhs} < {lhs}){return_false}"));
        }
        let (lhs, rhs) = sides(last);
        body.push_str(&format!("return {lhs} < {rhs};"));
        body
    }

    /// Bodies of `!=`, `>`, `<=` and `>=`, defined in terms of `==` and `<`.
    fn derived_body(&self, name: &str) -> String {
        let (a, b) = if self.friend {
            ("lhs", "rhs")
        } else {
            ("*this", "other")
        };
        match name {
            "operator!=" => format!("return !({a} == {b});"),
            "operator>" => format!("return {b} < {a};"),
            "operator<=" => format!("return !({b} < {a});"),
            _ => format!("return !({a} < {b});"),
        }
    }

    /// `StreamOutputOperator.setOperands`.
    fn stream_body(&self) -> String {
        if self.operands.is_empty() {
            return "return os;".into();
        }
        let unit = &self.unit;
        let alignment = if unit.contains(' ') { "   " } else { unit };
        let mut body = String::new();
        let mut spacer = "";
        for operand in &self.operands {
            match operand {
                Operand::Base(base) => {
                    body.push_str(&format!("<< static_cast<const {base} &>(rhs)"));
                }
                Operand::Member(name) => {
                    body.push_str(&format!("<< \"{spacer}{name}: \" << rhs.{name}"));
                }
            }
            body.push_str(&format!("\n{unit}{alignment}"));
            spacer = " ";
        }
        format!("os {};\n{unit}return os;", body.trim_end())
    }
}

/// An inline definition: on one line if the body is, else like `Operator.definition`.
fn inline_member(
    name: &str,
    declaration: String,
    body: String,
    unit: &str,
    config: &Config,
) -> NewMember {
    let inline_text = if body.contains('\n') {
        let separator = if config.function_brace_format(false).brace_on_new_line(false) {
            "\n"
        } else {
            " "
        };
        format!("{declaration}{separator}{{\n{unit}{body}\n}}")
    } else {
        format!("{declaration} {{ {body} }}")
    };
    NewMember {
        name: name.into(),
        declaration,
        inline_text,
        body,
        location: DefinitionLocation::Inline,
        relative: None,
    }
}

/// Generates the missing operators of `kind` for the class at `position`, defined inline in
/// the public section.
pub fn generate_operators(
    file: &FileInput,
    position: Position,
    kind: OperatorKind,
    config: &Config,
) -> Option<GeneratedEdit> {
    if file.is_c {
        return None;
    }
    let tree = SymbolTree::new(file.text);
    let class = class_at_head(&tree, position_to_offset(file.text, position))?;
    let names = missing(class, kind);
    if names.is_empty() {
        return None;
    }
    let operands: Vec<Operand> = class
        .class
        .iter()
        .flat_map(|c| &c.base_classes)
        .map(|b| Operand::Base(b.text.clone()))
        .chain(
            class
                .non_static_member_variables()
                .map(|v| Operand::Member(v.name.clone())),
        )
        .collect();
    let context = Context {
        type_name: class
            .own_scopes
            .first()
            .map_or_else(|| class.name.clone(), |s| s.qualifier.clone()),
        friend: config.cpp.friend_comparison_operators,
        explicit_this: config.cpp.use_explicit_this_pointer,
        unit: indent_unit(file.text, file.text),
        operands,
    };

    let mut editor = ClassEditor::new(*file, None, class.range.start, config);
    let all = kind.names();
    for (i, name) in all.iter().enumerate() {
        if !names.contains(name) {
            continue;
        }
        let (declaration, body) = match *name {
            "operator<<" => (
                format!(
                    "friend std::ostream &operator<<(std::ostream &os, const {} &rhs)",
                    context.type_name
                ),
                context.stream_body(),
            ),
            "operator==" => (context.comparison_declaration(name), context.equal_body()),
            "operator<" => (context.comparison_declaration(name), context.less_body()),
            _ => (
                context.comparison_declaration(name),
                context.derived_body(name),
            ),
        };
        let mut member = inline_member(name, declaration, body, &context.unit, config);
        // Next to the previous operator of the group (just added or already there).
        member.relative = i
            .checked_sub(1)
            .map(|previous| (all[previous].to_string(), Side::After));
        editor.add(&member)?;
    }
    Some(editor.finish())
}
