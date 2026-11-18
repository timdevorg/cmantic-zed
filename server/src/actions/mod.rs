//! Refactorings offered as code actions.

pub mod accessors;
pub mod add_definition;
pub mod member;
pub mod operators;

use tower_lsp_server::ls_types::Position;

use crate::symbol::SymbolTree;
use crate::text::position_to_offset;
use add_definition::{FileInput, is_on_class_head};

/// Cheap pre-check: whether the cursor is somewhere any action may be offered (a function
/// declaration, a class head or a member variable).
pub fn may_offer_actions_at(file: &FileInput, position: Position) -> bool {
    let tree = SymbolTree::new(file.text);
    let offset = position_to_offset(file.text, position);
    tree.symbol_at(offset).is_some_and(|path| {
        let symbol = path.symbol();
        symbol.is_function_declaration()
            || is_on_class_head(symbol, offset)
            || !symbol.variables.is_empty() && path.parent.is_some_and(|p| p.class.is_some())
    })
}
