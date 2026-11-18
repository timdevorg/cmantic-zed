//! Open documents and their tree-sitter parse trees (counterpart of `SourceDocument.ts`).

use tree_sitter::{Parser, Tree};

pub struct Document {
    pub text: String,
    pub version: i32,
    pub tree: Option<Tree>,
}

impl Document {
    pub fn new(text: String, version: i32) -> Self {
        let tree = parse(&text);
        Self {
            text,
            version,
            tree,
        }
    }

    /// Replaces the full text (the server uses full document sync) and reparses.
    pub fn update(&mut self, text: String, version: i32) {
        self.tree = parse(&text);
        self.text = text;
        self.version = version;
    }
}

pub fn parse(text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("tree-sitter-cpp grammar is compatible with the tree-sitter crate");
    parser.parse(text, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_class_with_method_declaration() {
        let doc = Document::new(
            "class Foo {\npublic:\n    int bar(int x) const;\n};\n".into(),
            1,
        );
        let tree = doc.tree.expect("tree");
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(root.child(0).unwrap().kind(), "class_specifier");
    }
}
