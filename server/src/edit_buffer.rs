//! Applies a sequence of edits to an in-memory copy of a file and turns them back into
//! non-overlapping edits against the original text.
//!
//! Refactorings that make several changes (e.g. adding many definitions) apply each change to
//! the buffer, re-parse [`EditBuffer::text`] and compute the next change against the updated
//! text. [`EditBuffer::edits`] then yields edits for one `workspace/applyEdit`.

use std::ops::Range;

use tower_lsp_server::ls_types::{Position, TextEdit};

use crate::text::offset_to_position;

#[derive(Debug, Clone)]
enum Segment {
    /// A slice of the original text.
    Original(Range<usize>),
    /// Inserted text.
    New(String),
}

#[derive(Debug, Clone)]
pub struct EditBuffer {
    original: String,
    current: String,
    segments: Vec<Segment>,
}

impl EditBuffer {
    pub fn new(text: &str) -> Self {
        Self {
            original: text.to_string(),
            current: text.to_string(),
            segments: if text.is_empty() {
                Vec::new()
            } else {
                vec![Segment::Original(0..text.len())]
            },
        }
    }

    /// The text with all edits applied so far.
    pub fn text(&self) -> &str {
        &self.current
    }

    pub fn original(&self) -> &str {
        &self.original
    }

    pub fn is_changed(&self) -> bool {
        self.current != self.original
    }

    pub fn insert(&mut self, offset: usize, text: &str) {
        self.replace(offset..offset, text);
    }

    /// Replaces `range` (byte offsets into [`Self::text`]) with `text`.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let range = range.start.min(self.current.len())..range.end.min(self.current.len());
        let range = range.start..range.end.max(range.start);
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut pos = 0;
        for segment in self.segments.drain(..) {
            let len = match &segment {
                Segment::Original(r) => r.len(),
                Segment::New(s) => s.len(),
            };
            let (start, end) = (pos, pos + len);
            pos = end;
            // Parts of the segment before and after the replaced range.
            let head = range.start.clamp(start, end) - start;
            let tail = range.end.clamp(start, end) - start;
            match &segment {
                Segment::Original(r) => {
                    if head > 0 {
                        before.push(Segment::Original(r.start..r.start + head));
                    }
                    if tail < len {
                        after.push(Segment::Original(r.start + tail..r.end));
                    }
                }
                Segment::New(s) => {
                    if head > 0 {
                        before.push(Segment::New(s[..head].to_string()));
                    }
                    if tail < len {
                        after.push(Segment::New(s[tail..].to_string()));
                    }
                }
            }
        }
        self.segments = before;
        if !text.is_empty() {
            self.segments.push(Segment::New(text.to_string()));
        }
        self.segments.extend(after);
        self.current.replace_range(range.clone(), text);
    }

    /// Byte-offset edits against the original text, sorted and non-overlapping.
    pub fn offset_edits(&self) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        let mut orig_pos = 0;
        let mut pending = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Original(r) => {
                    if r.start != orig_pos || !pending.is_empty() {
                        edits.push((orig_pos..r.start, std::mem::take(&mut pending)));
                    }
                    orig_pos = r.end;
                }
                Segment::New(s) => pending.push_str(s),
            }
        }
        if orig_pos != self.original.len() || !pending.is_empty() {
            edits.push((orig_pos..self.original.len(), pending));
        }
        edits
    }

    /// LSP edits against the original text.
    pub fn edits(&self) -> Vec<TextEdit> {
        self.offset_edits()
            .into_iter()
            .map(|(range, text)| {
                TextEdit::new(
                    tower_lsp_server::ls_types::Range::new(
                        offset_to_position(&self.original, range.start),
                        offset_to_position(&self.original, range.end),
                    ),
                    text,
                )
            })
            .collect()
    }

    /// LSP position of `offset` in [`Self::text`] (i.e. after the edits are applied).
    pub fn position(&self, offset: usize) -> Position {
        offset_to_position(&self.current, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(original: &str, edits: &[(Range<usize>, String)]) -> String {
        let mut out = original.to_string();
        for (range, text) in edits.iter().rev() {
            out.replace_range(range.clone(), text);
        }
        out
    }

    #[test]
    fn sequential_edits_map_back_to_original() {
        let mut buffer = EditBuffer::new("abc\ndef\n");
        buffer.insert(4, "X");
        buffer.insert(5, "Y"); // right after the inserted X
        buffer.insert(0, "<");
        buffer.replace(3..5, "-"); // original "c\n"
        buffer.insert(buffer.text().len(), "end");
        let edits = buffer.offset_edits();
        assert_eq!(apply(buffer.original(), &edits), buffer.text());
        assert_eq!(buffer.text(), "<ab-XYdef\nend");
        for pair in edits.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
    }

    #[test]
    fn replace_inside_new_text_and_delete() {
        let mut buffer = EditBuffer::new("0123456789");
        buffer.insert(5, "abc");
        buffer.replace(6..7, "B");
        buffer.replace(0..2, "");
        let edits = buffer.offset_edits();
        assert_eq!(buffer.text(), "234aBc56789");
        assert_eq!(apply(buffer.original(), &edits), buffer.text());
        assert_eq!(edits.len(), 2);
    }

    #[test]
    fn empty_original() {
        let mut buffer = EditBuffer::new("");
        buffer.insert(0, "a");
        buffer.insert(1, "b");
        assert_eq!(buffer.offset_edits(), vec![(0..0, "ab".to_string())]);
        assert!(EditBuffer::new("x").offset_edits().is_empty());
    }
}
