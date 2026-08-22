use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use ropey::{Rope, RopeSlice};

use crate::Cursor;

/// A single undoable change: replaces `removed` with `inserted` at `at`.
/// Applying it forward performs the edit; applying its inverse reverses it.
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
    cursor_before: Cursor,
    cursor_after: Cursor,
}

#[derive(PartialEq)]
enum PendingKind {
    Insert,
    DeleteBackward,
    DeleteForward,
}

/// An in-progress run of same-kind, contiguous edits (e.g. a typed word, or
/// a run of backspaces) not yet committed to the undo stack, so that undo
/// removes a whole run at once instead of one character at a time.
struct Pending {
    kind: PendingKind,
    at: usize,
    removed: String,
    inserted: String,
    cursor_before: Cursor,
    cursor_after: Cursor,
}

/// A rope-backed text buffer.
///
/// Editing and movement methods take a `&mut Cursor` argument instead of
/// owning a cursor internally, so multiple cursors can later be driven over
/// the same buffer without changing this API.
pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    dirty: bool,
    undo_stack: Vec<Edit>,
    redo_stack: Vec<Edit>,
    pending: Option<Pending>,
}

impl Buffer {
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending: None,
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let rope = Rope::from_reader(BufReader::new(File::open(path)?))?;
        Ok(Self {
            rope,
            path: Some(path.to_path_buf()),
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending: None,
        })
    }

    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "buffer has no path"))?;
        self.save_as(path)
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.flush_pending();
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

    /// The concatenated content of lines `[first_line, first_line + max_lines)`,
    /// for windowed rendering of large files instead of laying out the whole
    /// buffer every frame.
    pub fn visible_text(&self, first_line: usize, max_lines: usize) -> String {
        if first_line >= self.rope.len_lines() {
            return String::new();
        }
        let last_line = (first_line + max_lines).min(self.rope.len_lines());
        let start = self.rope.line_to_char(first_line);
        let end = self.rope.line_to_char(last_line);
        self.rope.slice(start..end).to_string()
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

    /// The char offset where `line` begins.
    pub fn line_start_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    /// The character at `idx`, or `None` past the end of the buffer.
    pub fn char_at(&self, idx: usize) -> Option<char> {
        if idx >= self.rope.len_chars() {
            None
        } else {
            Some(self.rope.char(idx))
        }
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
        self.redo_stack.clear();
        let cursor_before = *cursor;
        let at = cursor.char_idx;

        self.rope.insert_char(at, ch);
        cursor.char_idx += 1;
        self.dirty = true;
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;

        match &mut self.pending {
            Some(p) if p.kind == PendingKind::Insert && p.at + p.inserted.chars().count() == at => {
                p.inserted.push(ch);
                p.cursor_after = *cursor;
            }
            _ => {
                self.flush_pending();
                self.pending = Some(Pending {
                    kind: PendingKind::Insert,
                    at,
                    removed: String::new(),
                    inserted: ch.to_string(),
                    cursor_before,
                    cursor_after: *cursor,
                });
            }
        }
    }

    pub fn delete_backward(&mut self, cursor: &mut Cursor) {
        if cursor.char_idx == 0 {
            return;
        }
        self.redo_stack.clear();
        let cursor_before = *cursor;
        let removed_at = cursor.char_idx - 1;
        let removed_char = self.rope.char(removed_at);

        self.rope.remove(removed_at..cursor.char_idx);
        cursor.char_idx = removed_at;
        self.dirty = true;
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;

        match &mut self.pending {
            Some(p) if p.kind == PendingKind::DeleteBackward && p.at == removed_at + 1 => {
                p.removed.insert(0, removed_char);
                p.at = removed_at;
                p.cursor_after = *cursor;
            }
            _ => {
                self.flush_pending();
                self.pending = Some(Pending {
                    kind: PendingKind::DeleteBackward,
                    at: removed_at,
                    removed: removed_char.to_string(),
                    inserted: String::new(),
                    cursor_before,
                    cursor_after: *cursor,
                });
            }
        }
    }

    pub fn delete_forward(&mut self, cursor: &mut Cursor) {
        if cursor.char_idx >= self.rope.len_chars() {
            return;
        }
        self.redo_stack.clear();
        let cursor_before = *cursor;
        let at = cursor.char_idx;
        let removed_char = self.rope.char(at);

        self.rope.remove(at..at + 1);
        self.dirty = true;

        match &mut self.pending {
            Some(p) if p.kind == PendingKind::DeleteForward && p.at == at => {
                p.removed.push(removed_char);
                p.cursor_after = *cursor;
            }
            _ => {
                self.flush_pending();
                self.pending = Some(Pending {
                    kind: PendingKind::DeleteForward,
                    at,
                    removed: removed_char.to_string(),
                    inserted: String::new(),
                    cursor_before,
                    cursor_after: *cursor,
                });
            }
        }
    }

    /// Removes `start..end` as a single atomic edit (one undo step, not
    /// coalesced char-by-char), returning the removed text. Used by Vim
    /// operators (`dw`, `diw`, `dd`, ...) and as the yank source for `y`.
    pub fn delete_range(&mut self, cursor: &mut Cursor, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        self.flush_pending();
        self.redo_stack.clear();
        let cursor_before = *cursor;
        let removed = self.rope.slice(start..end).to_string();

        self.rope.remove(start..end);
        cursor.char_idx = start;
        self.dirty = true;
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;

        self.undo_stack.push(Edit {
            at: start,
            removed: removed.clone(),
            inserted: String::new(),
            cursor_before,
            cursor_after: *cursor,
        });
        removed
    }

    /// Inserts `text` at the cursor as a single atomic edit (one undo step).
    /// Used for paste (`p`/`P`).
    pub fn insert_str(&mut self, cursor: &mut Cursor, text: &str) {
        if text.is_empty() {
            return;
        }
        self.flush_pending();
        self.redo_stack.clear();
        let cursor_before = *cursor;
        let at = cursor.char_idx;

        self.rope.insert(at, text);
        cursor.char_idx = at + text.chars().count();
        self.dirty = true;
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;

        self.undo_stack.push(Edit {
            at,
            removed: String::new(),
            inserted: text.to_string(),
            cursor_before,
            cursor_after: *cursor,
        });
    }

    /// Commits the in-progress coalesced edit run (if any) to the undo
    /// stack, ending it as an undo boundary. Movement calls this so undo
    /// removes one "run" of typing/deleting at a time, not one char.
    fn flush_pending(&mut self) {
        if let Some(p) = self.pending.take() {
            self.undo_stack.push(Edit {
                at: p.at,
                removed: p.removed,
                inserted: p.inserted,
                cursor_before: p.cursor_before,
                cursor_after: p.cursor_after,
            });
        }
    }

    fn apply_forward(&mut self, edit: &Edit) {
        if !edit.removed.is_empty() {
            let end = edit.at + edit.removed.chars().count();
            self.rope.remove(edit.at..end);
        }
        if !edit.inserted.is_empty() {
            self.rope.insert(edit.at, &edit.inserted);
        }
    }

    fn apply_inverse(&mut self, edit: &Edit) {
        if !edit.inserted.is_empty() {
            let end = edit.at + edit.inserted.chars().count();
            self.rope.remove(edit.at..end);
        }
        if !edit.removed.is_empty() {
            self.rope.insert(edit.at, &edit.removed);
        }
    }

    /// Undoes the most recent edit run, restoring the cursor to where it was
    /// before that run started. Returns `false` if there was nothing to undo.
    pub fn undo(&mut self, cursor: &mut Cursor) -> bool {
        self.flush_pending();
        let Some(edit) = self.undo_stack.pop() else { return false };
        self.apply_inverse(&edit);
        *cursor = edit.cursor_before;
        self.dirty = true;
        self.redo_stack.push(edit);
        true
    }

    /// Reapplies the most recently undone edit run. Returns `false` if there
    /// was nothing to redo.
    pub fn redo(&mut self, cursor: &mut Cursor) -> bool {
        self.flush_pending();
        let Some(edit) = self.redo_stack.pop() else { return false };
        self.apply_forward(&edit);
        *cursor = edit.cursor_after;
        self.dirty = true;
        self.undo_stack.push(edit);
        true
    }

    pub fn move_left(&mut self, cursor: &mut Cursor) {
        self.flush_pending();
        if cursor.char_idx > 0 {
            cursor.char_idx -= 1;
        }
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;
    }

    pub fn move_right(&mut self, cursor: &mut Cursor) {
        self.flush_pending();
        if cursor.char_idx < self.rope.len_chars() {
            cursor.char_idx += 1;
        }
        let (_, col) = self.line_col(cursor);
        cursor.sticky_col = col;
    }

    pub fn move_home(&mut self, cursor: &mut Cursor) {
        self.flush_pending();
        let (line, _) = self.line_col(cursor);
        cursor.char_idx = self.rope.line_to_char(line);
        cursor.sticky_col = 0;
    }

    pub fn move_end(&mut self, cursor: &mut Cursor) {
        self.flush_pending();
        let (line, _) = self.line_col(cursor);
        let len = self.line_content_len(line);
        cursor.char_idx = self.rope.line_to_char(line) + len;
        cursor.sticky_col = len;
    }

    pub fn move_up(&mut self, cursor: &mut Cursor) {
        self.flush_pending();
        self.move_vertical(cursor, -1);
    }

    pub fn move_down(&mut self, cursor: &mut Cursor) {
        self.flush_pending();
        self.move_vertical(cursor, 1);
    }

    pub fn move_page(&mut self, cursor: &mut Cursor, lines: usize, down: bool) {
        self.flush_pending();
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
        Buffer {
            rope: Rope::from_str(text),
            path: None,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending: None,
        }
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
        let mut buf = buffer_with("ab");
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
        let mut buf = buffer_with("hello\nworld\n");
        let mut cur = Cursor { char_idx: 8, sticky_col: 2 };
        buf.move_home(&mut cur);
        assert_eq!(buf.line_col(&cur), (1, 0));

        buf.move_end(&mut cur);
        assert_eq!(buf.line_col(&cur), (1, 5));
    }

    #[test]
    fn vertical_movement_uses_sticky_column_through_short_lines() {
        let mut buf = buffer_with("longline\nhi\nlongline");
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
        let mut buf = buffer_with("a\nb\nc");
        let mut cur = Cursor::at_start();
        buf.move_page(&mut cur, 100, true);
        assert_eq!(buf.line_col(&cur), (2, 0));
    }

    #[test]
    fn visible_text_windows_a_line_range() {
        let buf = buffer_with("one\ntwo\nthree\nfour\nfive\n");
        assert_eq!(buf.visible_text(1, 2), "two\nthree\n");
        assert_eq!(buf.visible_text(4, 10), "five\n");
        assert_eq!(buf.visible_text(10, 2), "");
    }

    #[test]
    fn char_at_and_line_start_char() {
        let buf = buffer_with("ab\ncd\n");
        assert_eq!(buf.char_at(0), Some('a'));
        assert_eq!(buf.char_at(3), Some('c'));
        assert_eq!(buf.char_at(100), None);
        assert_eq!(buf.line_start_char(0), 0);
        assert_eq!(buf.line_start_char(1), 3);
    }

    #[test]
    fn delete_range_removes_atomically_and_is_one_undo_step() {
        let mut buf = buffer_with("hello world");
        let mut cur = Cursor { char_idx: 11, sticky_col: 11 };
        let removed = buf.delete_range(&mut cur, 5, 11);
        assert_eq!(removed, " world");
        assert_eq!(buf.text(), "hello");
        assert_eq!(cur.char_idx, 5);

        assert!(buf.undo(&mut cur));
        assert_eq!(buf.text(), "hello world");
        assert_eq!(cur.char_idx, 11);
        assert!(!buf.undo(&mut cur)); // one step, not six
    }

    #[test]
    fn insert_str_inserts_atomically_and_is_one_undo_step() {
        let mut buf = buffer_with("hello");
        let mut cur = Cursor { char_idx: 5, sticky_col: 5 };
        buf.insert_str(&mut cur, " world");
        assert_eq!(buf.text(), "hello world");
        assert_eq!(cur.char_idx, 11);

        assert!(buf.undo(&mut cur));
        assert_eq!(buf.text(), "hello");
        assert!(!buf.undo(&mut cur));
    }

    #[test]
    fn delete_range_and_insert_str_are_no_ops_on_empty_input() {
        let mut buf = buffer_with("abc");
        let mut cur = Cursor { char_idx: 1, sticky_col: 1 };
        assert_eq!(buf.delete_range(&mut cur, 2, 2), "");
        assert_eq!(buf.text(), "abc");
        buf.insert_str(&mut cur, "");
        assert_eq!(buf.text(), "abc");
    }

    #[test]
    fn undo_removes_a_whole_coalesced_typing_run() {
        let mut buf = buffer_with("");
        let mut cur = Cursor::at_start();
        for ch in "hello".chars() {
            buf.insert_char(&mut cur, ch);
        }
        assert_eq!(buf.text(), "hello");

        assert!(buf.undo(&mut cur));
        assert_eq!(buf.text(), "");
        assert_eq!(cur.char_idx, 0);
        assert!(!buf.undo(&mut cur));
    }

    #[test]
    fn redo_reapplies_an_undone_run() {
        let mut buf = buffer_with("");
        let mut cur = Cursor::at_start();
        for ch in "hi".chars() {
            buf.insert_char(&mut cur, ch);
        }
        buf.undo(&mut cur);
        assert_eq!(buf.text(), "");

        assert!(buf.redo(&mut cur));
        assert_eq!(buf.text(), "hi");
        assert_eq!(cur.char_idx, 2);
        assert!(!buf.redo(&mut cur));
    }

    #[test]
    fn editing_after_undo_clears_the_redo_stack() {
        let mut buf = buffer_with("");
        let mut cur = Cursor::at_start();
        buf.insert_char(&mut cur, 'a');
        buf.undo(&mut cur);
        buf.insert_char(&mut cur, 'b');
        assert!(!buf.redo(&mut cur));
        assert_eq!(buf.text(), "b");
    }

    #[test]
    fn cursor_movement_ends_the_coalescing_run() {
        let mut buf = buffer_with("");
        let mut cur = Cursor::at_start();
        buf.insert_char(&mut cur, 'a');
        buf.insert_char(&mut cur, 'b');
        buf.move_left(&mut cur); // boundary: commits "ab" as its own run
        buf.insert_char(&mut cur, 'c'); // starts a new run at position 1

        buf.undo(&mut cur); // undoes only "c"
        assert_eq!(buf.text(), "ab");

        buf.undo(&mut cur); // undoes "ab"
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn backspace_run_coalesces_into_one_undo_step() {
        let mut buf = buffer_with("hello");
        let mut cur = Cursor { char_idx: 5, sticky_col: 5 };
        for _ in 0..5 {
            buf.delete_backward(&mut cur);
        }
        assert_eq!(buf.text(), "");

        assert!(buf.undo(&mut cur));
        assert_eq!(buf.text(), "hello");
        assert_eq!(cur.char_idx, 5);
    }
}
