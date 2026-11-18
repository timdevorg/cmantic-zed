//! Adds new member functions to a class: the declaration (or inline definition) goes into the
//! class body, an out-of-line definition below the class or into the matching source file.
//! Shared by Generate Getters and Setters and Generate Operators (`addNewAccessorToWorkspaceEdit`
//! and `addNewOperatorToWorkspaceEdit` in C-Mantic).

use tower_lsp_server::ls_types::{Position, TextEdit};

use crate::actions::add_definition::{
    Destination, FileInput, Initializers, definition_text, indent_unit, with_eol,
};
use crate::config::{Config, DefinitionLocation};
use crate::edit_buffer::EditBuffer;
use crate::placement::{Side, find_position_for_new_member_function, format_text_to_insert};
use crate::symbol::{Access, Symbol, SymbolKind, SymbolPath, SymbolTree};
use crate::text::{detect_eol, leading_whitespace, line_end, line_start};

/// A member function to add.
#[derive(Debug, Clone)]
pub(crate) struct NewMember {
    pub name: String,
    /// Declaration without the `;`: `int getX() const`.
    pub declaration: String,
    /// What goes into the class for an inline definition: `int getX() const { return x; }`.
    pub inline_text: String,
    /// Body content of an out-of-line definition (continuation lines indented relative to
    /// the function).
    pub body: String,
    pub location: DefinitionLocation,
    /// Put the new member next to this existing one.
    pub relative: Option<(String, Side)>,
}

/// Edits to the current file and (optionally) its matching header/source file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedEdit {
    /// Edits to the current file (for the class editor: the file containing the class),
    /// against its original text.
    pub edits: Vec<TextEdit>,
    /// Edits to the matching file (the source file of a header, or the header of a source
    /// file), against its original text (empty if unchanged).
    pub other_edits: Vec<TextEdit>,
    /// Cursor in the matching file after the edits (e.g. inside the first new definition).
    pub other_cursor: Option<Position>,
}

pub(crate) struct ClassEditor<'a> {
    file: FileInput<'a>,
    buffer: EditBuffer,
    unit: String,
    source: Option<(FileInput<'a>, EditBuffer, String)>,
    other_cursor: Option<usize>,
    /// Start of the class symbol; edits only go into its body or after it.
    class_start: usize,
    config: &'a Config,
    /// Don't separate new members from their neighbours with blank lines (declarations).
    pub compact: bool,
}

/// The class (or struct/union) symbol starting at `start`.
pub(crate) fn class_at(tree: &SymbolTree, start: usize) -> Option<&Symbol> {
    tree.all()
        .find(|s| s.kind == SymbolKind::Class && s.class.is_some() && s.range.start == start)
}

impl<'a> ClassEditor<'a> {
    /// `source` is the matching source file, used for members whose location is
    /// [`DefinitionLocation::MatchingSourceFile`]; without one they are defined below the class.
    pub fn new(
        file: FileInput<'a>,
        source: Option<FileInput<'a>>,
        class_start: usize,
        config: &'a Config,
    ) -> Self {
        // Detected once from the original texts: generated code would skew later detection.
        let unit = indent_unit(file.text, file.text);
        Self {
            buffer: EditBuffer::new(file.text),
            unit,
            source: source
                .filter(|_| file.is_header)
                .map(|s| (s, EditBuffer::new(s.text), indent_unit(s.text, file.text))),
            other_cursor: None,
            class_start,
            file,
            config,
            compact: false,
        }
    }

    /// Adds `member` to the class. Returns `None` if the class can't be found any more.
    pub fn add(&mut self, member: &NewMember) -> Option<()> {
        let text = self.buffer.text().to_string();
        let tree = SymbolTree::new(&text);
        let class = class_at(&tree, self.class_start)?;
        let relative = member
            .relative
            .as_ref()
            .map(|(n, side)| (n.as_str(), *side));
        let mut position = find_position_for_new_member_function(&text, class, relative);
        position.next_to |= self.compact;

        let mut insert = match member.location {
            DefinitionLocation::Inline => member.inline_text.clone(),
            _ => format!("{};", member.declaration),
        };
        let needs_label = class.access_at(position.offset) != Access::Public;
        if needs_label {
            insert = format!("public:\n{insert}");
        }
        let mut formatted = format_text_to_insert(&insert, &position, &text, &self.unit);
        if needs_label {
            formatted = indent_label(&formatted, &label_indentation(&text, class));
        }
        // A class body on one line (`class A { int x; };`): put the rest on its own line.
        let mut replaced = 0;
        let rest = &text[position.offset..line_end(&text, position.offset)];
        if position.after && !rest.trim().is_empty() {
            replaced = rest.len() - rest.trim_start().len();
            if !formatted.ends_with('\n') {
                formatted.push('\n');
            }
            formatted.push_str(leading_whitespace(
                &text[line_start(&text, class.range.start)..],
            ));
        }
        let formatted = with_eol(formatted, detect_eol(self.file.text));
        self.buffer
            .replace(position.offset..position.offset + replaced, &formatted);
        if member.location == DefinitionLocation::Inline {
            return Some(());
        }

        // Out-of-line: find the new declaration and define it like Add Definition does.
        let inserted = position.offset..position.offset + formatted.len();
        let text = self.buffer.text().to_string();
        let tree = SymbolTree::new(&text);
        let class = class_at(&tree, self.class_start)?;
        let index = class.children.iter().position(|c| {
            inserted.contains(&c.range.start)
                && c.function.as_ref().is_some_and(|f| f.name == member.name)
                && c.is_function_declaration()
        })?;
        let path = SymbolPath {
            parent: Some(class),
            siblings: &class.children,
            index,
        };
        let decl = FileInput {
            text: &text,
            ..self.file
        };
        let to_source = member.location == DefinitionLocation::MatchingSourceFile
            && !path.symbol().has_unspecialized_template();
        match self.source.as_mut().filter(|_| to_source) {
            Some((source, buffer, unit)) => {
                let target_text = buffer.text().to_string();
                let target_tree = SymbolTree::new(&target_text);
                let target = FileInput {
                    text: &target_text,
                    ..*source
                };
                let destination = Destination {
                    file: &target,
                    tree: &target_tree,
                    same_file: false,
                };
                let definition = definition_text(
                    &decl,
                    path,
                    destination,
                    Initializers::None,
                    &member.body,
                    unit,
                    self.config,
                );
                let eol = detect_eol(source.text);
                let cursor = definition.cursor
                    + (eol.len() - 1) * definition.text[..definition.cursor].matches('\n').count();
                let new_text = with_eol(definition.text, eol);
                match self.other_cursor.as_mut() {
                    Some(c) if definition.offset <= *c => *c += new_text.len(),
                    Some(_) => {}
                    None => self.other_cursor = Some(definition.offset + cursor),
                }
                buffer.insert(definition.offset, &new_text);
            }
            None => {
                let destination = Destination {
                    file: &decl,
                    tree: &tree,
                    same_file: true,
                };
                let definition = definition_text(
                    &decl,
                    path,
                    destination,
                    Initializers::None,
                    &member.body,
                    &self.unit,
                    self.config,
                );
                let new_text = with_eol(definition.text, detect_eol(self.file.text));
                if definition.offset <= self.class_start {
                    self.class_start += new_text.len();
                }
                self.buffer.insert(definition.offset, &new_text);
            }
        }
        Some(())
    }

    pub fn finish(self) -> GeneratedEdit {
        let (other_edits, other_cursor) = match &self.source {
            Some((_, buffer, _)) if buffer.is_changed() => (
                buffer.edits(),
                self.other_cursor.map(|c| buffer.position(c)),
            ),
            _ => (Vec::new(), None),
        };
        GeneratedEdit {
            edits: self.buffer.edits(),
            other_edits,
            other_cursor,
        }
    }
}

/// Indentation for a new `public:` label: like the class's existing labels, else like the
/// class head.
fn label_indentation(text: &str, class: &Symbol) -> String {
    let anchor = class
        .class
        .as_ref()
        .and_then(|c| c.access_specifiers.first())
        .map_or(class.range.start, |(range, _)| range.start);
    leading_whitespace(&text[line_start(text, anchor)..]).to_string()
}

/// Re-indents a `public:` label that starts a line of `formatted`.
fn indent_label(formatted: &str, indentation: &str) -> String {
    let Some(index) = formatted.find("public:\n") else {
        return formatted.to_string();
    };
    match formatted[..index].rfind('\n') {
        Some(newline) if formatted[newline + 1..index].trim().is_empty() => format!(
            "{}{indentation}{}",
            &formatted[..=newline],
            &formatted[index..]
        ),
        _ => formatted.to_string(),
    }
}
