//! Copying from a terminal, pasting into it, and following what it
//! printed -- for the panel (`SPC o t`) and a terminal in a pane
//! (`SPC o T`) alike.
//!
//! - Dragging across a terminal's text selects it; a plain click lets it
//!   go. `Ctrl-Shift-C` copies it, `Ctrl-Shift-V` pastes, and a right
//!   click does whichever fits: copies the selection when there is one,
//!   pastes when there isn't (Windows Terminal's convention).
//! - A paste is typed as the program asked: bracketed, when it turned
//!   that on, so a shell doesn't run a multi-line paste line by line.
//! - `Ctrl`-click on a `path:line:col` (a compiler's error, a stack
//!   trace, PowerShell's `At C:\x.ps1:12 char:5`) opens it at that line,
//!   beside the terminal; on a URL, in the browser. A relative path is
//!   read from where the shell said it is (`OSC 7` / `OSC 9;9`), else
//!   where it started, else the project.
//!
//! The selection is the terminal's screen cells as shown, so it stays
//! where it was drawn; typing into that terminal lets it go.

use super::*;

/// Which terminal: the panel, or the one a buffer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TermTarget {
    Panel,
    Buffer(BufferId),
}

/// Cells of a terminal selected with the mouse: (row, column), the row
/// counted from the first its shell printed (`Terminal::top_row`), so the
/// selection stays on its text as more comes in or the view scrolls.
/// Where the drag started and where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TermSelection {
    pub target: TermTarget,
    pub anchor: (u64, u16),
    pub head: (u64, u16),
    /// The button is still down, after a single click: a drag goes on
    /// from it. A double or triple click's word or line stays as it is.
    pub dragging: bool,
}

/// What a `Ctrl`-click in a terminal's text follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TermLink {
    Url(String),
    File { path: String, line: Option<usize>, col: Option<usize> },
}

/// The link under column `col` of a terminal's `row` text: a URL, or a
/// path with the line and column after it, if any.
pub(super) fn link_in(row: &str, col: usize) -> Option<TermLink> {
    let (token, _) = crate::file_link::link_at(row, col)?;
    if token.starts_with("http://") || token.starts_with("https://") {
        return Some(TermLink::Url(token));
    }
    let at = fenix_tasks::parse_path_position(&token);
    let path = at.path.to_string_lossy().into_owned();
    // A word with no sign of being a path isn't followed.
    let looks_like_a_path = path.contains(['/', '\\']) || path.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    looks_like_a_path.then_some(TermLink::File { path, line: at.line, col: at.column })
}

/// The link under column `col` of `row`, and the columns it takes: what
/// a `Ctrl` held over it underlines.
pub(super) fn link_span_in(row: &str, col: usize) -> Option<(TermLink, usize, usize)> {
    let link = link_in(row, col)?;
    let chars: Vec<char> = row.chars().collect();
    let stop = |c: char| c.is_whitespace() || "()[]<>\"'`|".contains(c);
    let start = (0..col).rev().take_while(|&i| !stop(chars[i])).last().unwrap_or(col);
    let mut end = (col..chars.len()).take_while(|&i| !stop(chars[i])).last()? + 1;
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?') {
        end -= 1;
    }
    Some((link, start, end))
}

/// The cells from `a` to `b` (either way round), rows counted from the
/// first printed, as the rows of a screen `rows` x `cols` whose first is
/// `top` shows them: (row, first column, end column). A part scrolled out
/// of view isn't drawn.
pub(super) fn selection_segments(a: (u64, u16), b: (u64, u16), top: u64, rows: u16, cols: u16) -> Vec<(usize, usize, usize)> {
    let (from, to) = if a <= b { (a, b) } else { (b, a) };
    let shown = top..top + rows as u64;
    (from.0..=to.0)
        .filter(|row| shown.contains(row))
        .map(|row| {
            let start = if row == from.0 { from.1 } else { 0 };
            let end = if row == to.0 { to.1 + 1 } else { cols };
            ((row - top) as usize, start as usize, end.min(cols).max(start) as usize)
        })
        .collect()
}

/// The word under column `col` of `row`: a run of anything but spaces,
/// brackets and quotes -- a path, a URL or a hash whole, as a terminal
/// selects it. (first column, last column), or none on a space.
pub(super) fn word_in(row: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = row.chars().collect();
    let stop = |c: char| c.is_whitespace() || "()[]<>{}\"'`|,;".contains(c);
    if col >= chars.len() || stop(chars[col]) {
        return None;
    }
    let start = (0..col).rev().take_while(|&i| !stop(chars[i])).last().unwrap_or(col);
    let end = (col..chars.len()).take_while(|&i| !stop(chars[i])).last().unwrap_or(col);
    Some((start, end))
}

impl App {
    fn term_session(&self, target: TermTarget) -> Option<&fenix_terminal::Terminal> {
        match target {
            TermTarget::Panel => self.terminal.as_ref().map(|t| &*t.session),
            TermTarget::Buffer(id) => self.terminal_buffers.get(&id).map(|t| &*t.session),
        }
    }

    fn term_session_mut(&mut self, target: TermTarget) -> Option<&mut fenix_terminal::Terminal> {
        match target {
            TermTarget::Panel => self.terminal.as_mut().map(|t| &mut *t.session),
            TermTarget::Buffer(id) => self.terminal_buffers.get_mut(&id).map(|t| &mut *t.session),
        }
    }

    /// The terminal cell under `pos`, if it's on one: the panel's, or a
    /// pane's showing a terminal.
    pub(super) fn terminal_cell_at(&self, pos: (f32, f32)) -> Option<(TermTarget, (u16, u16))> {
        let (window_width, window_height) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32)).unwrap_or((800.0, 600.0));
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        let char_width = self.text.as_ref().map(|t| t.char_width()).unwrap_or(text::CHAR_WIDTH);
        let (target, origin) = if let Some(rect) = geometry.terminal_rect.filter(|r| r.contains_point(pos.0, pos.1)) {
            (TermTarget::Panel, (text::PAD_LEFT + text::STRIP_INSET, rect.y + text::PAD_TOP + text::STRIP_INSET / 2.0))
        } else {
            let (pane, rect) = geometry.panes.iter().find(|(_, r)| r.contains_point(pos.0, pos.1))?;
            let id = *self.windows().content(*pane)?;
            if !self.terminal_buffers.contains_key(&id) {
                return None;
            }
            let content = pane_content_rect(*rect, line_height, self.pane_has_breadcrumb(*pane));
            if pos.1 < content.y {
                return None;
            }
            (TermTarget::Buffer(id), (content.x + text::PAD_LEFT, content.y + text::PAD_TOP))
        };
        let (rows, cols) = self.term_session(target)?.screen().size();
        let row = (((pos.1 - origin.1) / line_height).max(0.0) as u16).min(rows.saturating_sub(1));
        let col = (((pos.0 - origin.0) / char_width).max(0.0) as u16).min(cols.saturating_sub(1));
        Some((target, (row, col)))
    }

    /// The pointer at `pos`, `Ctrl` held or not: the link under it, in a
    /// terminal, is underlined and the pointer a hand -- what a click
    /// would follow. True when there's one.
    pub(super) fn terminal_hover(&mut self, pos: Option<(f32, f32)>) -> bool {
        let found = pos.filter(|_| self.modifiers.control_key()).and_then(|pos| {
            let (target, (row, col)) = self.terminal_cell_at(pos)?;
            let text = self.term_session(target)?.row_text(row);
            let (_, a, b) = link_span_in(&text, col as usize)?;
            Some((target, row, a, b))
        });
        if found != self.term_hover {
            self.term_hover = found;
            self.wake_caret();
        }
        found.is_some()
    }

    /// The link `Ctrl` is over in `target`, to underline: (row, first
    /// column, end).
    pub(super) fn terminal_hover_for(&self, target: TermTarget) -> Option<(usize, usize, usize)> {
        self.term_hover.filter(|h| h.0 == target).map(|(_, row, a, b)| (row as usize, a, b))
    }

    /// The left button down at `pos`: on a terminal's text, a selection
    /// starts there -- or, with `Ctrl`, the link there is followed.
    /// True when the click was the link's alone.
    pub(super) fn terminal_mouse_press(&mut self, pos: (f32, f32)) -> bool {
        let Some((target, cell)) = self.terminal_cell_at(pos) else {
            self.term_select = None;
            return false;
        };
        if self.modifiers.control_key() {
            let row = self.term_session(target).map(|s| s.row_text(cell.0)).unwrap_or_default();
            if let Some(link) = link_in(&row, cell.1 as usize) {
                self.terminal_follow(target, link);
                return true;
            }
        }
        // A second click selects the word, a third the line.
        let clicks = self.term_clicks.press(Instant::now(), pos);
        let Some(session) = self.term_session(target) else { return false };
        let top = session.top_row();
        let row = top + cell.0 as u64;
        let text = session.row_text(cell.0);
        let (anchor, head, dragging) = match clicks {
            2 => match word_in(&text, cell.1 as usize) {
                Some((a, b)) => ((row, a as u16), (row, b as u16), false),
                None => ((row, cell.1), (row, cell.1), true),
            },
            3 => ((row, 0), (row, text.trim_end().chars().count().saturating_sub(1) as u16), false),
            _ => ((row, cell.1), (row, cell.1), true),
        };
        self.term_select = Some(TermSelection { target, anchor, head, dragging });
        false
    }

    /// The pointer moved with the button down: the selection follows it.
    /// True while there's one being made.
    pub(super) fn terminal_mouse_drag(&mut self, pos: (f32, f32)) -> bool {
        let Some(sel) = self.term_select.filter(|s| s.dragging) else { return false };
        if let Some((target, cell)) = self.terminal_cell_at(pos) {
            let top = self.term_session(target).map(|s| s.top_row()).unwrap_or(0);
            let cell = (top + cell.0 as u64, cell.1);
            if target == sel.target && cell != sel.head {
                self.term_select = Some(TermSelection { head: cell, ..sel });
                self.wake_caret();
            }
        }
        true
    }

    /// The button let go: a drag's selection stays; a click's is none.
    pub(super) fn terminal_mouse_release(&mut self) {
        if let Some(sel) = self.term_select {
            // A click without a drag selects nothing; a word or a line
            // double or triple clicked stays, one letter long or not.
            self.term_select = (!sel.dragging || sel.anchor != sel.head).then_some(TermSelection { dragging: false, ..sel });
        }
    }

    /// A right click on a terminal: copies what's selected in it, or
    /// pastes when nothing is. True when it was on one.
    pub(super) fn terminal_right_click(&mut self, pos: (f32, f32)) -> bool {
        let Some((target, _)) = self.terminal_cell_at(pos) else { return false };
        if !self.terminal_copy(target) {
            self.terminal_paste(target);
        }
        true
    }

    /// `Ctrl-Shift-C` and `Ctrl-Shift-V` in a terminal that has the
    /// keyboard: copy and paste. True when `key` was one of them.
    pub(super) fn terminal_clipboard_key(&mut self, target: TermTarget, key: KeyPress) -> bool {
        if !(key.mods.ctrl && self.modifiers.shift_key()) {
            return false;
        }
        match key.code {
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') => {
                if !self.terminal_copy(target) {
                    self.set_message("nothing selected -- drag across the terminal's text to select it");
                }
                true
            }
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'v') => {
                self.terminal_paste(target);
                true
            }
            _ => false,
        }
    }

    /// Copies `target`'s selection and lets it go. False when it has none.
    pub(super) fn terminal_copy(&mut self, target: TermTarget) -> bool {
        let Some(sel) = self.term_select.filter(|s| s.target == target && !s.dragging) else { return false };
        // The part still on the screen shown.
        let Some(text) = self.term_session(target).and_then(|s| {
            let (top, rows) = (s.top_row(), s.screen().size().0 as u64);
            let (from, to) = if sel.anchor <= sel.head { (sel.anchor, sel.head) } else { (sel.head, sel.anchor) };
            if to.0 < top || from.0 >= top + rows {
                return None;
            }
            let from = if from.0 < top { (0, 0) } else { ((from.0 - top) as u16, from.1) };
            let to = if to.0 >= top + rows { ((rows - 1) as u16, u16::MAX) } else { ((to.0 - top) as u16, to.1) };
            Some(s.text_between(from, to))
        }) else { return false };
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(text.clone());
        }
        self.term_select = None;
        self.set_message(format!("copied {} lines", text.lines().count().max(1)));
        true
    }

    /// Pastes the clipboard's text into `target`.
    pub(super) fn terminal_paste(&mut self, target: TermTarget) {
        let Some(text) = self.clipboard.as_mut().and_then(|c| c.get_text().ok()).filter(|t| !t.is_empty()) else { return };
        self.term_select = None;
        if let Some(session) = self.term_session_mut(target) {
            let _ = session.paste(&text);
        }
        self.wake_caret();
    }

    /// Typing into `target` lets its selection go.
    pub(super) fn terminal_typed(&mut self, target: TermTarget) {
        if self.term_select.is_some_and(|s| s.target == target) {
            self.term_select = None;
        }
    }

    /// `target`'s selection, as rows to tint.
    pub(super) fn terminal_selection_for(&self, target: TermTarget) -> Segments {
        let Some(sel) = self.term_select.filter(|s| s.target == target && (!s.dragging || s.anchor != s.head)) else { return Segments::new() };
        let Some(session) = self.term_session(target) else { return Segments::new() };
        let (rows, cols) = session.screen().size();
        selection_segments(sel.anchor, sel.head, session.top_row(), rows, cols)
    }

    /// What a terminal is running and where: "pwsh · orbit-ground", and
    /// "· running" while a command runs, when its shell marks its prompts.
    pub(super) fn terminal_title(&self, target: TermTarget) -> Option<String> {
        let session = self.term_session(target)?;
        let folder = self.terminal_base_dir(target);
        let folder = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.display().to_string());
        let running = if session.running() == Some(true) { " · running" } else { "" };
        Some(format!("{} · {folder}{running}", session.program()))
    }

    /// Where a relative path a terminal printed is read from: where the
    /// shell said it is, else where it started, else the project. Where it
    /// started is the session's own, not where a terminal opened now
    /// would: the panel's start follows the buffer in front.
    fn terminal_base_dir(&self, target: TermTarget) -> PathBuf {
        let session = self.term_session(target);
        let said = session.and_then(|s| s.cwd()).map(Path::to_path_buf);
        let started = session.and_then(|s| s.start_dir()).map(Path::to_path_buf).or_else(|| match target {
            TermTarget::Panel => self.terminal_start_dir(),
            TermTarget::Buffer(id) => self.terminal_buffer_cwds.get(&id).cloned(),
        });
        said.or(started).unwrap_or_else(|| self.find_file_root())
    }

    /// Follows `link`: a URL in the browser, a file in the editor at its
    /// line and column -- beside the terminal, not in its place.
    fn terminal_follow(&mut self, target: TermTarget, link: TermLink) {
        self.term_select = None;
        match link {
            TermLink::Url(url) => {
                // A test only notes it, as an extension's `open-url` does.
                #[cfg(test)]
                self.wasm.opened.push(url.clone());
                #[cfg(not(test))]
                if let Err(e) = super::open_in_browser(&url) {
                    self.set_error(format!("couldn't open {url}: {e}"));
                }
            }
            TermLink::File { path, line, col } => {
                let given = PathBuf::from(&path);
                let full = if given.is_absolute() { given } else { self.terminal_base_dir(target).join(given) };
                if !full.is_file() {
                    self.set_error(format!("no file {}", full.display()));
                    return;
                }
                match target {
                    TermTarget::Panel => self.terminal_focused = false,
                    TermTarget::Buffer(_) => {
                        self.unfocus_terminal_buffer();
                        // Another pane that isn't a terminal, else a new
                        // one beside this.
                        let other = self.windows().windows().into_iter().find(|p| self.windows().content(*p).is_none_or(|b| !self.terminal_buffers.contains_key(b)));
                        match other {
                            Some(pane) => {
                                self.windows_mut().focus(pane);
                            }
                            None => self.split_vertical(),
                        }
                    }
                }
                self.open_file_from_picker(&full);
                if let Some(line) = line {
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path: full, line: line.max(1), col: col.unwrap_or(1).max(1), text: String::new() });
                }
            }
        }
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// What a click follows: a compiler's `path:line:col`, PowerShell's
    /// `At C:\x.ps1:12 char:5`, a URL; not a plain word.
    #[test]
    fn links_in_a_terminals_output() {
        let rust = "  --> src/frames.rs:46:9";
        assert_eq!(link_in(rust, rust.find("frames").unwrap()), Some(TermLink::File { path: "src/frames.rs".into(), line: Some(46), col: Some(9) }));
        let ps = r"At C:\src\build.ps1:12 char:5";
        assert_eq!(link_in(ps, 6), Some(TermLink::File { path: r"C:\src\build.ps1".into(), line: Some(12), col: None }));
        let url = "see https://example.com/docs, then";
        assert_eq!(link_in(url, 8), Some(TermLink::Url("https://example.com/docs".into())));
        assert_eq!(link_in("error: build failed", 2), None, "a word isn't a path");
        assert_eq!(link_in("open notes.md", 7), Some(TermLink::File { path: "notes.md".into(), line: None, col: None }));
    }

    /// A selection over several rows runs to the end of the first and
    /// from the start of the last, whichever way it was dragged.
    #[test]
    fn a_selection_is_rows_of_cells() {
        assert_eq!(selection_segments((0, 4), (2, 1), 0, 24, 10), [(0, 4, 10), (1, 0, 10), (2, 0, 2)]);
        assert_eq!(selection_segments((2, 1), (0, 4), 0, 24, 10), [(0, 4, 10), (1, 0, 10), (2, 0, 2)]);
        assert_eq!(selection_segments((1, 3), (1, 6), 0, 24, 10), [(1, 3, 7)]);
        // Three rows of output later, it's three rows up; scrolled out,
        // it isn't drawn.
        assert_eq!(selection_segments((5, 3), (5, 6), 3, 24, 10), [(2, 3, 7)]);
        assert_eq!(selection_segments((0, 4), (2, 1), 2, 24, 10), [(0, 0, 2)], "only its last row is still shown");
        assert!(selection_segments((0, 0), (1, 1), 5, 24, 10).is_empty());
    }

    /// End to end with a real shell: what it printed is selected and
    /// read back as text, and a `path:line:col` in it opens the file
    /// beside the terminal, at that line, read from where the shell runs.
    #[test]
    fn a_path_a_real_shell_printed_is_selected_and_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src").join("lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        // `cat` is a shell's own and PowerShell's alias: what it prints is
        // the error, without the command line also holding it.
        std::fs::write(dir.path().join("out.txt"), "error: oops\n  --> src/lib.rs:2:4\n").unwrap();
        let mut app = App::with_file(None);
        let id = app.buffers.open_terminal();
        app.open_buffer_in_focused_pane(id);
        let (session, mut reader) = fenix_terminal::Terminal::spawn_in(24, 80, Some(dir.path())).expect("a real shell");
        app.terminal_buffers.insert(id, TerminalState::new(session, None));
        app.terminal_buffer_cwds.insert(id, dir.path().to_path_buf());
        app.focus_terminal_buffer(id);
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
        });
        let target = TermTarget::Buffer(id);
        // A row holding `want`; "" for a prompt (`>`, `$`, `#`, `%`).
        let row_of = |app: &App, want: &str| -> Option<u16> {
            let session = app.term_session(target)?;
            (0..session.screen().size().0).find(|&r| {
                let text = session.row_text(r);
                if want.is_empty() { text.trim_end().ends_with(['>', '$', '#', '%']) } else { text.contains(want) }
            })
        };
        let pump = |app: &mut App, want: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while row_of(app, want).is_none() && std::time::Instant::now() < deadline {
                if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    app.terminal_buffers.get_mut(&id).unwrap().session.process(&chunk);
                }
            }
            row_of(app, want).unwrap_or_else(|| panic!("never printed {want:?}"))
        };
        pump(&mut app, "");
        for c in "cat out.txt\r".chars() {
            app.terminal_buffers.get_mut(&id).unwrap().session.write_input(c.to_string().as_bytes()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let row = pump(&mut app, "--> src/lib.rs:2:4");

        // Its title: what it runs, and where.
        let program = app.term_session(target).unwrap().program().to_string();
        let folder = dir.path().file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(app.terminal_title(target), Some(format!("{program} · {folder}")));

        // Selected across the row, it reads back as it was printed.
        let line = app.term_session(target).unwrap().row_text(row);
        let at = line.find("src/").unwrap() as u16;
        app.term_select = Some(TermSelection { target, anchor: (row as u64, at), head: (row as u64, at + 13), dragging: false });
        assert_eq!(app.term_session(target).unwrap().text_between((row, at), (row, at + 13)), "src/lib.rs:2:4");
        assert_eq!(app.terminal_selection_for(target), [(row as usize, at as usize, at as usize + 14)]);

        // Followed: the file, beside the terminal, at line 2 column 4.
        let link = link_in(&line, at as usize + 5).expect("a link");
        app.terminal_follow(target, link);
        assert_eq!(app.windows().window_count(), 2, "beside the terminal, not in its place");
        let ob = app.buffers.get(app.focused_buffer_id()).unwrap();
        assert!(ob.buffer.path().is_some_and(|p| p.ends_with("src/lib.rs") || p.ends_with("src\\lib.rs")), "{:?}", ob.buffer.path());
        let cursor = app.pane_state(app.focused_pane_id()).cursor;
        assert_eq!(ob.buffer.line_col(&cursor), (1, 3));
        if let Some(state) = app.terminal_buffers.get_mut(&id) {
            state.session.kill();
        }
    }

    /// A double click selects a word: a path, a URL or a hash whole.
    #[test]
    fn a_word_is_a_run_of_anything_but_spaces_brackets_and_quotes() {
        let row = "  --> src/frames.rs:46:9 (here)";
        let (a, b) = word_in(row, row.find("frames").unwrap()).unwrap();
        assert_eq!(&row[a..=b], "src/frames.rs:46:9");
        let (a, b) = word_in(row, row.find("here").unwrap()).unwrap();
        assert_eq!(&row[a..=b], "here");
        assert_eq!(word_in(row, 0), None);
    }

    /// The underline goes under the whole of the link, and stops before
    /// what follows it.
    #[test]
    fn a_links_span_is_the_whole_of_it() {
        let row = "  --> src/frames.rs:46:9, then";
        let (_, a, b) = link_span_in(row, row.find("frames").unwrap()).unwrap();
        assert_eq!(&row[a..b], "src/frames.rs:46:9");
        assert_eq!(link_span_in("error: oops", 2), None);
    }

    /// Dragging in a pane's terminal selects; letting go keeps it; a
    /// click lets it go; typing into it too.
    #[test]
    fn dragging_selects_and_a_click_or_typing_lets_go() {
        let mut app = App::with_file(None);
        let id = app.buffers.open_scratch();
        let target = TermTarget::Buffer(id);
        app.term_select = Some(TermSelection { target, anchor: (0, 0), head: (0, 0), dragging: true });
        app.terminal_mouse_release();
        assert_eq!(app.term_select, None, "a click selects nothing");
        app.term_select = Some(TermSelection { target, anchor: (0, 0), head: (1, 3), dragging: true });
        app.terminal_mouse_release();
        assert_eq!(app.term_select.map(|s| s.dragging), Some(false), "a drag's stays");
        app.terminal_typed(TermTarget::Panel);
        assert!(app.term_select.is_some(), "another terminal's typing leaves it");
        app.terminal_typed(target);
        assert_eq!(app.term_select, None);
    }
}
