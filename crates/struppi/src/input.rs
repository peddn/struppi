//! The single-line input editor.

use unicode_width::UnicodeWidthChar;

/// Text being typed, with a cursor.
#[derive(Debug, Default)]
pub struct Editor {
    text: String,
    /// Byte offset of the cursor; always on a character boundary.
    cursor: usize,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replaces the text and puts the cursor at the end.
    pub fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = self.text.len();
    }

    /// Returns the text and clears the editor.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            self.insert(c);
        }
    }

    fn previous_boundary(&self) -> Option<usize> {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    pub fn backspace(&mut self) {
        if let Some(start) = self.previous_boundary() {
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub fn delete(&mut self) {
        if let Some(end) = self.next_boundary() {
            self.text.replace_range(self.cursor..end, "");
        }
    }

    pub fn left(&mut self) {
        if let Some(index) = self.previous_boundary() {
            self.cursor = index;
        }
    }

    pub fn right(&mut self) {
        if let Some(index) = self.next_boundary() {
            self.cursor = index;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Start of the word before the cursor.
    fn word_start(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(char::is_whitespace);
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(index, c)| index + c.len_utf8())
    }

    /// End of the word after the cursor.
    fn word_end(&self) -> usize {
        let after = &self.text[self.cursor..];
        let skipped = after.len() - after.trim_start_matches(char::is_whitespace).len();
        let rest = &after[skipped..];
        self.cursor + skipped + rest.find(char::is_whitespace).unwrap_or(rest.len())
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start();
    }

    pub fn word_right(&mut self) {
        self.cursor = self.word_end();
    }

    pub fn delete_word_back(&mut self) {
        let start = self.word_start();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn kill_to_start(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    pub fn kill_to_end(&mut self) {
        self.text.truncate(self.cursor);
    }

    /// The part of the text that fits into `width` columns such that the
    /// cursor is visible, and the cursor's column within it. With `mask`,
    /// every character is shown as `*`.
    pub fn visible(&self, width: usize, mask: bool) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }
        let shown = |c: char| if mask { '*' } else { c };
        let width_of = |c: char| shown(c).width().unwrap_or(0);

        let cursor_column: usize = self.text[..self.cursor].chars().map(width_of).sum();

        // Scroll horizontally so that the cursor sits in the last column at
        // most. `skip` is the number of columns hidden on the left.
        let skip = cursor_column.saturating_sub(width - 1);

        let mut out = String::new();
        let mut column = 0;
        let mut start_column = None;
        for c in self.text.chars() {
            let w = width_of(c);
            if column >= skip {
                let start = *start_column.get_or_insert(column);
                if column + w - start > width {
                    break;
                }
                out.push(shown(c));
            }
            column += w;
        }
        let start = start_column.unwrap_or(cursor_column);
        (out, cursor_column.saturating_sub(start).min(width - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.set(text);
        editor
    }

    #[test]
    fn insert_and_delete_with_multibyte_characters() {
        let mut e = Editor::default();
        e.insert_str("grüße");
        e.left();
        e.left();
        e.backspace();
        assert_eq!(e.text(), "grße");
        e.delete();
        assert_eq!(e.text(), "gre");
        e.insert('ö');
        assert_eq!(e.text(), "gröe");
        e.home();
        e.backspace();
        e.end();
        e.delete();
        assert_eq!(e.text(), "gröe");
    }

    #[test]
    fn control_characters_are_not_inserted() {
        let mut e = Editor::default();
        e.insert_str("a\nb\tc\x1b");
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn word_motions() {
        let mut e = editor("kill  the goblin");
        e.word_left();
        e.insert('X');
        assert_eq!(e.text(), "kill  the Xgoblin");
        e.word_left();
        e.word_left();
        e.insert('Y');
        assert_eq!(e.text(), "kill  Ythe Xgoblin");
        e.home();
        e.word_right();
        e.insert('Z');
        assert_eq!(e.text(), "killZ  Ythe Xgoblin");
        e.word_right();
        e.word_right();
        e.word_right();
        e.insert('!');
        assert_eq!(e.text(), "killZ  Ythe Xgoblin!");
    }

    #[test]
    fn kill_commands() {
        let mut e = editor("say hello  world");
        e.delete_word_back();
        assert_eq!(e.text(), "say hello  ");
        e.delete_word_back();
        assert_eq!(e.text(), "say ");
        e.insert_str("abc def");
        e.word_left();
        e.kill_to_end();
        assert_eq!(e.text(), "say abc ");
        e.left();
        e.kill_to_start();
        assert_eq!(e.text(), " ");
        e.kill_to_start();
        assert_eq!(e.text(), " ");
    }

    #[test]
    fn take_clears() {
        let mut e = editor("look");
        assert_eq!(e.take(), "look");
        assert!(e.is_empty());
        e.insert('x');
        assert_eq!(e.text(), "x");
    }

    #[test]
    fn visible_without_scrolling() {
        let mut e = editor("look");
        assert_eq!(e.visible(10, false), ("look".to_owned(), 4));
        e.home();
        assert_eq!(e.visible(10, false), ("look".to_owned(), 0));
    }

    #[test]
    fn visible_scrolls_to_keep_the_cursor_in_view() {
        let mut e = editor("0123456789");
        assert_eq!(e.visible(5, false), ("6789".to_owned(), 4));
        e.left();
        assert_eq!(e.visible(5, false), ("56789".to_owned(), 4));
        e.home();
        assert_eq!(e.visible(5, false), ("01234".to_owned(), 0));
    }

    #[test]
    fn visible_handles_wide_characters() {
        let e = editor("日本語テキスト");
        let (text, cursor) = e.visible(6, false);
        assert!(unicode_width::UnicodeWidthStr::width(text.as_str()) <= 6);
        assert!(cursor <= 5);
        assert!(text.ends_with('ト'));
    }

    #[test]
    fn visible_masks_passwords() {
        let e = editor("geheim");
        assert_eq!(e.visible(10, true), ("******".to_owned(), 6));
    }

    #[test]
    fn zero_width_area() {
        assert_eq!(editor("abc").visible(0, false), (String::new(), 0));
    }
}
