use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use ropey::{Rope, RopeSlice};

use crate::Cursor;

/// A rope-backed text buffer.
///
/// Editing and movement methods take a `&mut Cursor` argument instead of
/// owning a cursor internally, so multiple cursors can later be driven over
/// the same buffer without changing this API.
pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    dirty: bool,
}

impl Buffer {
    pub fn empty() -> Self {
        Self { rope: Rope::new(), path: None, dirty: false }
    }

    pub fn from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let rope = Rope::from_reader(BufReader::new(File::open(path)?))?;
        Ok(Self { rope, path: Some(path.to_path_buf()), dirty: false })
    }

    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "buffer has no path"))?;
        self.save_as(path)
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        self.rope.write_to(BufWriter::new(File::create(path)?))?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line(&self, line_idx: usize) -> RopeSlice<'_> {
        self.rope.line(line_idx)
    }

    /// (line, column) for a cursor, both zero-based, in chars.
    pub fn line_col(&self, cursor: &Cursor) -> (usize, usize) {
        let line = self.rope.char_to_line(cursor.char_idx);
        let col = cursor.char_idx - self.rope.line_to_char(line);
        (line, col)
    }

    /// Length of a line's content in chars, excluding its line terminator.
    fn line_content_len(&self, line_idx: usize) -> usize {
        let line = self.rope.line(line_idx);
        let mut len = line.len_chars();
        if len > 0 && line.char(len - 1) == '\n' {
            len -= 1;
            if len > 0 && line.char(len - 1) == '\r' {
                len -= 1;
            }
        }
        len
    }

    pub fn insert_char(&mut self, cursor: &mut Cursor, ch: char) {
        self.rope.insert_char(cursor.char_idx, ch);
        cursor.char_idx += 1;
        self.dirty = true;
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;
    }

    pub fn delete_backward(&mut self, cursor: &mut Cursor) {
        if cursor.char_idx == 0 {
            return;
        }
        self.rope.remove(cursor.char_idx - 1..cursor.char_idx);
        cursor.char_idx -= 1;
        self.dirty = true;
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;
    }

    pub fn delete_forward(&mut self, cursor: &mut Cursor) {
        if cursor.char_idx >= self.rope.len_chars() {
            return;
        }
        self.rope.remove(cursor.char_idx..cursor.char_idx + 1);
        self.dirty = true;
    }

    pub fn move_left(&self, cursor: &mut Cursor) {
        if cursor.char_idx > 0 {
            cursor.char_idx -= 1;
        }
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;
    }

    pub fn move_right(&self, cursor: &mut Cursor) {
        if cursor.char_idx < self.rope.len_chars() {
            cursor.char_idx += 1;
        }
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;
    }

    pub fn move_home(&self, cursor: &mut Cursor) {
        let (line, _) = self.line_col(cursor);
        cursor.char_idx = self.rope.line_to_char(line);
        cursor.sticky_col = 0;
    }

    pub fn move_end(&self, cursor: &mut Cursor) {
        let (line, _) = self.line_col(cursor);
        let len = self.line_content_len(line);
        cursor.char_idx = self.rope.line_to_char(line) + len;
        cursor.sticky_col = len;
    }

    pub fn move_up(&self, cursor: &mut Cursor) {
        self.move_vertical(cursor, -1);
    }

    pub fn move_down(&self, cursor: &mut Cursor) {
        self.move_vertical(cursor, 1);
    }

    pub fn move_page(&self, cursor: &mut Cursor, lines: usize, down: bool) {
        let delta = if down { lines as isize } else { -(lines as isize) };
        self.move_vertical(cursor, delta);
    }

    fn move_vertical(&self, cursor: &mut Cursor, delta: isize) {
        let (line, _) = self.line_col(cursor);
        let last_line = self.rope.len_lines().saturating_sub(1);
        let target_line = (line as isize + delta).clamp(0, last_line as isize) as usize;
        let target_len = self.line_content_len(target_line);
        let col = cursor.sticky_col.min(target_len);
        cursor.char_idx = self.rope.line_to_char(target_line) + col;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_with(text: &str) -> Buffer {
        Buffer { rope: Rope::from_str(text), path: None, dirty: false }
    }

    #[test]
    fn insert_and_delete_char() {
        let mut buf = buffer_with("");
        let mut cur = Cursor::at_start();
        buf.insert_char(&mut cur, 'h');
        buf.insert_char(&mut cur, 'i');
        assert_eq!(buf.text(), "hi");
        assert_eq!(cur.char_idx, 2);

        buf.delete_backward(&mut cur);
        assert_eq!(buf.text(), "h");
        assert_eq!(cur.char_idx, 1);
    }

    #[test]
    fn delete_forward_leaves_cursor_in_place() {
        let mut buf = buffer_with("abc");
        let mut cur = Cursor { char_idx: 1, sticky_col: 1 };
        buf.delete_forward(&mut cur);
        assert_eq!(buf.text(), "ac");
        assert_eq!(cur.char_idx, 1);
    }

    #[test]
    fn horizontal_movement_clamps_at_buffer_edges() {
        let buf = buffer_with("ab");
        let mut cur = Cursor::at_start();
        buf.move_left(&mut cur);
        assert_eq!(cur.char_idx, 0);

        buf.move_right(&mut cur);
        buf.move_right(&mut cur);
        buf.move_right(&mut cur);
        assert_eq!(cur.char_idx, 2);
    }

    #[test]
    fn home_and_end_use_line_content_length_excluding_newline() {
        let buf = buffer_with("hello\nworld\n");
        let mut cur = Cursor { char_idx: 8, sticky_col: 2 };
        buf.move_home(&mut cur);
        assert_eq!(buf.line_col(&cur), (1, 0));

        buf.move_end(&mut cur);
        assert_eq!(buf.line_col(&cur), (1, 5));
    }

    #[test]
    fn vertical_movement_uses_sticky_column_through_short_lines() {
        let buf = buffer_with("longline\nhi\nlongline");
        let mut cur = Cursor::at_start();
        buf.move_end(&mut cur);
        assert_eq!(buf.line_col(&cur), (0, 8));

        buf.move_down(&mut cur);
        assert_eq!(buf.line_col(&cur), (1, 2)); // clamped to "hi"'s length

        buf.move_down(&mut cur);
        assert_eq!(buf.line_col(&cur), (2, 8)); // sticky column restored
    }

    #[test]
    fn page_movement_clamps_to_last_line() {
        let buf = buffer_with("a\nb\nc");
        let mut cur = Cursor::at_start();
        buf.move_page(&mut cur, 100, true);
        assert_eq!(buf.line_col(&cur), (2, 0));
    }
}
