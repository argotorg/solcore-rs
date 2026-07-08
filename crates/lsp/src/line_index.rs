//! UTF-8 byte offset to LSP UTF-16 position mapping.
//!
//! Solcore compiler spans use UTF-8 byte offsets while LSP positions default
//! to UTF-16 code units. This module wraps rust-analyzer's `line-index` crate
//! so all protocol adapters share the same conversion rules.

use line_index::{LineCol, LineIndex, TextSize, WideEncoding, WideLineCol};
use lsp_types::{Position, Range};

/// Per-document position mapper.
#[derive(Debug, Clone)]
pub struct LineIndexExt {
    index: LineIndex,
    len: u32,
    text: Box<str>,
}

impl LineIndexExt {
    /// Builds a line index for `text`.
    pub fn new(text: &str) -> Self {
        Self {
            index: LineIndex::new(text),
            len: u32::try_from(text.len()).unwrap_or(u32::MAX),
            text: text.into(),
        }
    }

    /// Returns the document text this index was built from.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Converts a UTF-8 byte offset to an LSP UTF-16 position.
    ///
    /// Offsets are clamped to the document length. Compiler spans are expected
    /// to be valid UTF-8 boundaries; if a non-boundary offset is supplied, this
    /// falls back to the byte column rather than panicking.
    pub fn byte_to_position(&self, offset: u32) -> Position {
        let offset = TextSize::new(offset.min(self.len));
        let line_col = self.index.line_col(offset);
        let wide = self
            .index
            .to_wide(WideEncoding::Utf16, line_col)
            .unwrap_or(WideLineCol {
                line: line_col.line,
                col: line_col.col,
            });

        Position::new(wide.line, wide.col)
    }

    /// Converts an LSP UTF-16 position to a UTF-8 byte offset.
    ///
    /// Returns `None` when the position is out of range or lands inside a
    /// multi-byte character (e.g. the middle of a UTF-16 surrogate pair), so
    /// callers never receive a byte offset that is not a UTF-8 char boundary.
    pub fn position_to_byte(&self, position: Position) -> Option<u32> {
        let wide = WideLineCol {
            line: position.line,
            col: position.character,
        };
        let line_col = self.index.to_utf8(WideEncoding::Utf16, wide)?;
        let offset = u32::from(self.index.offset(line_col)?);
        self.text
            .is_char_boundary(offset as usize)
            .then_some(offset)
    }

    /// Converts a UTF-8 byte range to an LSP UTF-16 range.
    pub fn range(&self, start: u32, end: u32) -> Range {
        Range::new(self.byte_to_position(start), self.byte_to_position(end))
    }

    /// Returns the underlying UTF-8 line/column for tests and future features.
    pub fn line_col(&self, offset: u32) -> LineCol {
        self.index.line_col(TextSize::new(offset.min(self.len)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ascii_positions() {
        let index = LineIndexExt::new("abc\nxy");

        assert_eq!(index.byte_to_position(0), Position::new(0, 0));
        assert_eq!(index.byte_to_position(3), Position::new(0, 3));
        assert_eq!(index.byte_to_position(4), Position::new(1, 0));
        assert_eq!(index.byte_to_position(6), Position::new(1, 2));

        assert_eq!(index.position_to_byte(Position::new(0, 0)), Some(0));
        assert_eq!(index.position_to_byte(Position::new(0, 3)), Some(3));
        assert_eq!(index.position_to_byte(Position::new(1, 0)), Some(4));
        assert_eq!(index.position_to_byte(Position::new(1, 2)), Some(6));
    }

    #[test]
    fn maps_two_byte_character() {
        let text = "aéz";
        let index = LineIndexExt::new(text);
        let composed = text.find('é').expect("composed e acute") as u32;

        assert_eq!(index.byte_to_position(composed), Position::new(0, 1));
        assert_eq!(
            index.byte_to_position(composed + "é".len() as u32),
            Position::new(0, 2)
        );
        assert_eq!(index.position_to_byte(Position::new(0, 1)), Some(composed));
        assert_eq!(
            index.position_to_byte(Position::new(0, 2)),
            Some(composed + "é".len() as u32)
        );
    }

    #[test]
    fn maps_three_byte_character() {
        let text = "aあb";
        let index = LineIndexExt::new(text);
        let cjk = text.find('あ').expect("cjk character") as u32;

        assert_eq!(index.byte_to_position(cjk), Position::new(0, 1));
        assert_eq!(
            index.byte_to_position(cjk + "あ".len() as u32),
            Position::new(0, 2)
        );
        assert_eq!(index.position_to_byte(Position::new(0, 1)), Some(cjk));
        assert_eq!(
            index.position_to_byte(Position::new(0, 2)),
            Some(cjk + "あ".len() as u32)
        );
    }

    #[test]
    fn maps_four_byte_character_as_two_utf16_units() {
        let text = "😀";
        let index = LineIndexExt::new(text);

        assert_eq!(index.byte_to_position(0), Position::new(0, 0));
        assert_eq!(
            index.byte_to_position("😀".len() as u32),
            Position::new(0, 2)
        );
        assert_eq!(index.position_to_byte(Position::new(0, 0)), Some(0));
        assert_eq!(
            index.position_to_byte(Position::new(0, 2)),
            Some("😀".len() as u32)
        );
        assert_eq!(index.position_to_byte(Position::new(0, 1)), None);
    }

    #[test]
    fn maps_multibyte_multiple_lines_round_trip() {
        let text = "let x = \"café\";\n😀";
        let index = LineIndexExt::new(text);
        let e_acute = text.find('é').expect("e acute") as u32;
        let emoji = text.find('😀').expect("emoji") as u32;

        assert_eq!(e_acute, 12);
        assert_eq!(emoji, 17);
        assert_eq!(index.byte_to_position(e_acute), Position::new(0, 12));
        assert_eq!(
            index.byte_to_position(e_acute + "é".len() as u32),
            Position::new(0, 13)
        );
        assert_eq!(index.byte_to_position(emoji), Position::new(1, 0));
        assert_eq!(
            index.byte_to_position(emoji + "😀".len() as u32),
            Position::new(1, 2)
        );

        for offset in [
            0,
            e_acute,
            e_acute + "é".len() as u32,
            emoji,
            text.len() as u32,
        ] {
            let position = index.byte_to_position(offset);
            assert_eq!(index.position_to_byte(position), Some(offset));
        }
        assert_eq!(index.position_to_byte(Position::new(1, 1)), None);
    }
}
