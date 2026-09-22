//! TODO-style comments (`TODO`, `FIXME`, `HACK`, `NOTE`, ...): stepping
//! through them in a buffer (`]t`/`[t`), listing a buffer's (`SPC s t`),
//! and sweeping the whole project for them (`SPC s T`). What counts as
//! one is `fenix_syntax::todo`'s call; this is only where they're found
//! and how you get to them.

use super::*;
use fenix_syntax::{LanguageId, TodoItem, TodoKind};

/// Finds the TODO items in `text`, a file in `language`. A grammar with
/// real comments decides from the parse; Markdown has no comment syntax
/// a grammar marks, and a file with no grammar has no parse at all, so
/// both fall back to reading every line as if it were comment text
/// (`todo::find_in_plain_text`, which still wants the keyword to open a
/// line or carry a colon). JSON has neither comments nor prose, only
/// data, so a `"TODO"` in it is never one.
pub(super) fn scan_todos(language: Option<LanguageId>, text: &str) -> Vec<TodoItem> {
    match language {
        Some(LanguageId::Json) => Vec::new(),
        Some(LanguageId::Markdown) | None => fenix_syntax::todo::find_in_plain_text(text),
        Some(language) => fenix_syntax::SyntaxState::new(language, text).todo_items(text),
    }
}

/// A picker row: kind first, so typing `fix` narrows to just those.
fn todo_label(kind: TodoKind, location: &str, message: &str) -> String {
    format!("{:<4}  {location}  {message}", kind.label())
}

/// "12 TODOs in 4 files -- FIX 2, TODO 9, NOTE 1", counts per kind in
/// `TodoKind` order, which runs most to least urgent.
fn todo_summary(kinds: &[TodoKind], files: usize) -> String {
    let counts: Vec<String> = TodoKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let n = kinds.iter().filter(|k| **k == kind).count();
            (n > 0).then(|| format!("{} {n}", kind.label()))
        })
        .collect();
    let noun = if kinds.len() == 1 { "TODO" } else { "TODOs" };
    let files = if files == 1 { "1 file".to_string() } else { format!("{files} files") };
    format!("{} {noun} in {files} -- {}", kinds.len(), counts.join(", "))
}

impl App {
    /// The focused buffer's TODO items, with its parse brought current
    /// first.
    fn focused_todo_items(&mut self) -> Vec<TodoItem> {
        let id = self.focused_buffer_id();
        // Force an incremental parse, the same way folding does, so an
        // item typed a moment ago is already in the tree.
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let ob = self.open();
        let text = ob.buffer.text();
        match (&ob.syntax, ob.buffer.path().and_then(fenix_syntax::detect_language_from_path)) {
            (Some(syntax), Some(language)) if !matches!(language, LanguageId::Markdown | LanguageId::Json) => syntax.todo_items(&text),
            (_, language) => scan_todos(language, &text),
        }
    }

    /// `]t`/`[t`: moves to the `count`th TODO after (or before) the
    /// cursor. Stops at the last one there is rather than wrapping, the
    /// way `]`/`[` motions do in Vim, and says so when there's none.
    pub(crate) fn jump_to_todo(&mut self, forward: bool, count: u32) {
        let items = self.focused_todo_items();
        let cursor_idx = self.cursor().char_idx;
        let buffer = &self.open().buffer;
        let starts: Vec<usize> = items.iter().map(|item| buffer.byte_to_char(item.range.start)).collect();
        let candidates: Vec<usize> = if forward {
            starts.into_iter().filter(|&c| c > cursor_idx).collect()
        } else {
            starts.into_iter().rev().filter(|&c| c < cursor_idx).collect()
        };
        let Some(&target) = candidates.get((count.max(1) as usize - 1).min(candidates.len().saturating_sub(1))) else {
            self.set_message(if items.is_empty() {
                "no TODO comments in this buffer"
            } else if forward {
                "no more TODO comments below"
            } else {
                "no more TODO comments above"
            });
            return;
        };
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: cursor_idx };
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        cursor.char_idx = target;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        self.record_jump(from);
        self.wake_caret();
    }

    /// `SPC s t`: a picker over the focused buffer's TODO comments.
    pub(crate) fn picker_buffer_todos(&mut self) {
        let items = self.focused_todo_items();
        if items.is_empty() {
            self.set_message("no TODO comments in this buffer");
            return;
        }
        let buffer = &self.open().buffer;
        let kinds: Vec<TodoKind> = items.iter().map(|item| item.kind).collect();
        let candidates = items
            .into_iter()
            .map(|item| {
                let offset = buffer.byte_to_char(item.range.start);
                fenix_picker::Candidate::new(todo_label(item.kind, &format!("{:>4}", item.line + 1), &item.message), offset)
            })
            .collect();
        self.enter_picker(ActivePicker::BufferTodos(fenix_picker::PickerState::new(candidates)));
        self.set_message(todo_summary(&kinds, 1));
    }

    /// `SPC s T`: every TODO comment in the project. `rg` narrows the
    /// project down to the files that mention a keyword at all (ignore
    /// files respected, the same as `SPC p s`); each of those is then
    /// read properly -- parsed, if it has a grammar -- so only real
    /// comments are listed, not every string or identifier that happens
    /// to spell `TODO`. An open buffer is read from memory, so unsaved
    /// edits are what's listed. The result is also the new quickfix
    /// list, so `SPC p n`/`SPC p N` walk it without reopening anything.
    pub(crate) fn picker_project_todos(&mut self) {
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let files = match fenix_project::files_matching(&root, fenix_syntax::todo::TODO_SEARCH_PATTERN) {
            Ok(files) => files,
            Err(err) => {
                self.set_error(format!("TODO search failed: {err}"));
                return;
            }
        };
        let mut matches: Vec<(TodoKind, fenix_project::GrepMatch)> = Vec::new();
        let mut files_with_todos = 0;
        for path in files {
            let text = match self.buffers.id_for_path(&path).and_then(|id| self.buffers.get(id)) {
                Some(ob) => ob.buffer.text(),
                None => match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    // Not UTF-8 (a binary rg decided to search anyway) or
                    // gone since the search: nothing to list.
                    Err(_) => continue,
                },
            };
            let items = scan_todos(fenix_syntax::detect_language_from_path(&path), &text);
            if !items.is_empty() {
                files_with_todos += 1;
            }
            for item in items {
                let text = format!("{} {}", item.kind.label(), item.message);
                matches.push((item.kind, fenix_project::GrepMatch { path: path.clone(), line: item.line + 1, col: item.col + 1, text }));
            }
        }
        if matches.is_empty() {
            self.set_message("no TODO comments in this project");
            return;
        }
        let kinds: Vec<TodoKind> = matches.iter().map(|(kind, _)| *kind).collect();
        self.quickfix = matches.iter().map(|(_, m)| QuickfixEntry::Grep(m.clone())).collect();
        self.quickfix_index = None;
        let candidates = matches
            .into_iter()
            .map(|(kind, m)| {
                let location = format!("{}:{}", Self::relative_label(&root, &m.path), m.line);
                let message = m.text.split_once(' ').map_or("", |(_, rest)| rest).to_string();
                fenix_picker::Candidate::new(todo_label(kind, &location, &message), m)
            })
            .collect();
        self.enter_picker(ActivePicker::ProjectTodos(fenix_picker::PickerState::new(candidates)));
        self.set_message(todo_summary(&kinds, files_with_todos));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_todos_uses_the_grammar_when_there_is_one() {
        let text = "let s = \"TODO: no\"; // TODO: yes\n";
        let items = scan_todos(Some(LanguageId::Rust), text);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message, "yes");
    }

    #[test]
    fn scan_todos_reads_plain_text_and_markdown_line_by_line() {
        let text = "TODO buy milk\nsee the NOTE field\n- [ ] FIXME: flaky test\n";
        assert_eq!(scan_todos(None, text).len(), 2);
        assert_eq!(scan_todos(Some(LanguageId::Markdown), text).len(), 2);
        assert!(scan_todos(Some(LanguageId::Json), "{\"a\": \"TODO: x\"}").is_empty());
    }

    #[test]
    fn summary_counts_per_kind_most_urgent_first() {
        let kinds = [TodoKind::Todo, TodoKind::Fix, TodoKind::Todo];
        assert_eq!(todo_summary(&kinds, 2), "3 TODOs in 2 files -- FIX 1, TODO 2");
        assert_eq!(todo_summary(&[TodoKind::Note], 1), "1 TODO in 1 file -- NOTE 1");
    }
}
