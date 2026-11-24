// IME offsets are UTF-16. Remote PTYs receive only committed text.
use std::ops::Range;

#[derive(Default)]
pub struct Composition {
    pub text: String,
    pub selection: Range<usize>,
}

fn byte_offset(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (byte, c) in text.char_indices() {
        if utf16 >= offset {
            return byte;
        }
        utf16 += c.len_utf16();
    }
    text.len()
}
impl Composition {
    pub fn len16(&self) -> usize {
        self.text.encode_utf16().count()
    }
    pub fn slice(&self, range: Range<usize>) -> (String, Range<usize>) {
        let start = byte_offset(&self.text, range.start);
        let end = byte_offset(&self.text, range.end).max(start);
        let actual =
            self.text[..start].encode_utf16().count()..self.text[..end].encode_utf16().count();
        (self.text[start..end].to_owned(), actual)
    }
    pub fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
    ) {
        let range = range.unwrap_or(0..self.len16());
        let start = byte_offset(&self.text, range.start);
        let end = byte_offset(&self.text, range.end).max(start);
        let before = self.text[..start].encode_utf16().count();
        let length = text.encode_utf16().count();
        self.text.replace_range(start..end, text);
        let selected = selection.unwrap_or(length..length);
        self.selection = before + selected.start.min(length)..before + selected.end.min(length);
    }
    pub fn finish(&mut self) -> String {
        self.selection = 0..0;
        std::mem::take(&mut self.text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_utf16_segments_before_a_single_commit() {
        let mut c = Composition::default();
        c.replace(None, "你😀ma", Some(3..5));
        assert_eq!(c.slice(1..3), ("😀".into(), 1..3));
        c.replace(Some(3..5), "吗", None);
        assert_eq!(c.selection, 4..4);
        assert_eq!(c.finish(), "你😀吗");
        assert!(c.finish().is_empty());
    }
}
