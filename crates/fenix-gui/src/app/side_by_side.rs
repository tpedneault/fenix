//! Side by side (`SPC g v`): the file you're editing beside itself as
//! it is at another ref -- the ref's version read-only on the left, your
//! buffer on the right, still yours to edit. Blank filler rows keep the
//! two lined up, the way a merge view does: a block you added has empty
//! space beside it, a block only the ref has leaves a gap on your side.
//! The two panes scroll together and the cursor's line is matched across.
//!
//! The alignment (`fenix_diff::align`) is worked out in memory against
//! the buffer's text, again after every edit, so it follows typing
//! without saving or running git. `]c`/`[c` walk the changes; `do`
//! takes the ref's version of the one under the cursor.

use super::*;
use fenix_diff::{Alignment, BlockKind, Side};

/// A filler row in a pane's display lines: no buffer line, blank space.
pub(crate) const FILLER: usize = usize::MAX;

/// `lines` (a pane's display lines, folds applied) with filler rows
/// inserted: `fillers` is (before which buffer line, how many). A filler
/// before a line that's folded away goes before the next shown one.
pub(super) fn with_fillers(lines: Vec<usize>, fillers: &[(usize, usize)]) -> Vec<usize> {
    if fillers.is_empty() {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len() + fillers.iter().map(|f| f.1).sum::<usize>());
    let mut pending = fillers.iter().peekable();
    for line in lines {
        while let Some(&&(before, count)) = pending.peek() {
            if before > line {
                break;
            }
            out.extend(std::iter::repeat_n(FILLER, count));
            pending.next();
        }
        out.push(line);
    }
    for &(_, count) in pending {
        out.extend(std::iter::repeat_n(FILLER, count));
    }
    out
}

/// Whole rows tinted, `(row, colour)`, and changed words, `(row, from,
/// to, colour)` in buffer columns.
pub(super) type Tints = (Vec<(usize, [f32; 4])>, Vec<(usize, usize, usize, [f32; 4])>);

pub(super) struct SideBySide {
    pub(super) workspace: usize,
    pub(super) ref_pane: fenix_window::WindowId,
    pub(super) live_pane: fenix_window::WindowId,
    pub(super) ref_buffer: BufferId,
    pub(super) live_buffer: BufferId,
    pub(super) rev: String,
    pub(super) alignment: Alignment,
    /// The live buffer's edit count the alignment was made at.
    aligned_at: u64,
    /// The (pane, line) the other pane's cursor was last matched to.
    synced: Option<(fenix_window::WindowId, usize)>,
}

impl SideBySide {
    fn side_of(&self, pane: fenix_window::WindowId) -> Option<Side> {
        if pane == self.ref_pane {
            Some(Side::Old)
        } else if pane == self.live_pane {
            Some(Side::New)
        } else {
            None
        }
    }

    fn other(&self, pane: fenix_window::WindowId) -> fenix_window::WindowId {
        if pane == self.ref_pane {
            self.live_pane
        } else {
            self.ref_pane
        }
    }
}

/// How a row is tinted: a line only one side has, a changed line, or
/// blank filler.
fn tint(kind: BlockKind, side: Side, theme: &theme::Theme) -> Option<[f32; 4]> {
    let color = match (kind, side) {
        (BlockKind::Equal, _) => return None,
        (BlockKind::Added, _) => theme.git_staged,
        (BlockKind::Removed, _) => theme.git_conflicted,
        (BlockKind::Changed, _) => theme.git_modified,
    };
    let [r, g, b, _] = glyphon_to_rgba(color);
    Some([r, g, b, 0.16])
}

/// A buffer's lines, as `align` splits them.
fn buffer_text(ob: &OpenBuffer) -> String {
    ob.buffer.text()
}

impl App {
    /// The side-by-side session `pane` belongs to, in the workspace in
    /// front.
    fn side_by_side_at(&self, pane: fenix_window::WindowId) -> Option<&SideBySide> {
        let here = self.workspaces.active_index();
        self.side_by_sides.iter().find(|s| s.workspace == here && s.side_of(pane).is_some())
    }

    /// A pane's display lines: its buffer's lines with folds applied,
    /// and -- in a side-by-side pane -- the filler rows that keep it lined
    /// up with the other side.
    pub(super) fn pane_display_lines(&self, pane: fenix_window::WindowId, buffer: BufferId) -> Vec<usize> {
        let lines = self.folded_display_lines(buffer);
        match self.side_by_side_at(pane) {
            Some(s) if (pane == s.ref_pane && buffer == s.ref_buffer) || (pane == s.live_pane && buffer == s.live_buffer) => {
                let fillers = if pane == s.ref_pane { &s.alignment.old_fillers } else { &s.alignment.new_fillers };
                with_fillers(lines, fillers)
            }
            _ => lines,
        }
    }

    /// `SPC g v`: which ref to set the focused file beside.
    pub(crate) fn start_side_by_side_picker(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("side by side needs a file");
            return;
        };
        let Some(root) = super::git_editor::repository_of(&path) else {
            self.set_error("this file isn't in a git repository");
            return;
        };
        let mut refs = vec!["HEAD".to_string()];
        refs.extend(self.compare_targets(&fenix_lsp::normalize(root)));
        let candidates = refs.into_iter().map(|r| fenix_picker::Candidate::new(r.clone(), r)).collect();
        self.enter_picker(ActivePicker::Versus { path, picker: fenix_picker::PickerState::new(candidates) });
    }

    /// Sets the file at `path` -- open in the focused pane -- beside its
    /// version at `rev`.
    pub(crate) fn open_side_by_side(&mut self, path: &Path, rev: &str) {
        let Some(root) = super::git_editor::repository_of(path).map(fenix_lsp::normalize) else {
            self.set_error("this file isn't in a git repository");
            return;
        };
        let relative = super::git_editor::relative_to(path, &root);
        let text = match fenix_git::show_at(&root, rev, &relative) {
            Ok(text) => text,
            Err(_) => {
                self.set_error(format!("{relative} isn't on {rev} -- nothing to set beside it"));
                return;
            }
        };
        let live_pane = self.focused_pane_id();
        let live_buffer = self.focused_buffer_id();
        if let Some(i) = self.side_by_sides.iter().position(|s| s.live_buffer == live_buffer && s.workspace == self.workspaces.active_index()) {
            let old = self.side_by_sides.remove(i);
            self.side_by_side_teardown(old);
            self.windows_mut().focus(live_pane);
        }
        let ref_buffer = self.buffers.open_text_view_like(&text, path);
        let name = format!("{rev}:{relative}");
        self.ref_view_names.insert(ref_buffer, name.clone());
        // A tab strip like any file's, not a fixed title: both panes
        // then start their text on the same row, which the alignment
        // depends on.
        let ref_pane = self.windows_mut().split_before(SplitKind::Vertical, ref_buffer);
        self.set_pane_content(ref_pane, ref_buffer);
        self.windows_mut().focus(live_pane);
        let live_text = self.buffers.get(live_buffer).map(buffer_text).unwrap_or_default();
        let aligned_at = self.buffers.get(live_buffer).map(|ob| ob.buffer.edit_count()).unwrap_or(0);
        let alignment = fenix_diff::align(&text, &live_text);
        let changes = alignment.changes().count();
        self.side_by_sides.push(SideBySide {
            workspace: self.workspaces.active_index(),
            ref_pane,
            live_pane,
            ref_buffer,
            live_buffer,
            rev: rev.to_string(),
            alignment,
            aligned_at,
            synced: None,
        });
        self.set_message(match changes {
            0 => format!("no differences from {rev}"),
            n => format!("{} from {rev} -- ]c / [c walk them, do takes {rev}'s, q in the left pane closes", crate::git_status::count(n, "change")),
        });
        self.wake_caret();
    }

    /// Closes a session: its read-only pane and buffer go, your buffer
    /// stays where it was.
    fn side_by_side_teardown(&mut self, session: SideBySide) {
        if session.workspace == self.workspaces.active_index() && self.windows().windows().contains(&session.ref_pane) {
            self.windows_mut().focus(session.ref_pane);
            if self.windows_mut().close_focused() {
                self.workspaces.active_pane_states_mut().remove(&session.ref_pane);
                self.workspaces.active_scroll_anims_mut().remove(&session.ref_pane);
                self.workspaces.active_pane_tabs_mut().remove(&session.ref_pane);
            }
            if self.windows().windows().contains(&session.live_pane) {
                self.windows_mut().focus(session.live_pane);
            }
        }
        self.buffers.close(session.ref_buffer);
        self.ref_view_names.remove(&session.ref_buffer);
    }

    /// `q` in the left pane, or `SPC g q`: closes the side-by-side the
    /// focused pane belongs to. `false` when it belongs to none.
    pub(crate) fn side_by_side_close(&mut self) -> bool {
        let pane = self.focused_pane_id();
        let here = self.workspaces.active_index();
        let Some(i) = self.side_by_sides.iter().position(|s| s.workspace == here && s.side_of(pane).is_some()) else { return false };
        let session = self.side_by_sides.remove(i);
        self.side_by_side_teardown(session);
        self.wake_caret();
        true
    }

    /// Whether `q` should close a side-by-side: in its read-only pane.
    pub(super) fn focused_is_side_by_side_ref(&self) -> bool {
        let pane = self.focused_pane_id();
        self.side_by_side_at(pane).is_some_and(|s| s.ref_pane == pane)
    }

    /// Once a frame, before drawing: drops sessions whose panes went,
    /// lines up again after edits, and puts the other pane's cursor on
    /// the line matching the focused one's.
    pub(super) fn side_by_side_frame(&mut self) {
        let here = self.workspaces.active_index();
        let panes = self.windows().windows();
        let gone: Vec<usize> = self
            .side_by_sides
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.workspace == here
                    && (!panes.contains(&s.ref_pane)
                        || !panes.contains(&s.live_pane)
                        || self.windows().content(s.ref_pane) != Some(&s.ref_buffer)
                        || self.windows().content(s.live_pane) != Some(&s.live_buffer))
            })
            .map(|(i, _)| i)
            .collect();
        for i in gone.into_iter().rev() {
            let session = self.side_by_sides.remove(i);
            self.side_by_side_teardown(session);
        }
        // Sessions in workspaces that no longer exist.
        let count = self.workspaces.len();
        self.side_by_sides.retain(|s| s.workspace < count);

        let focused = self.focused_pane_id();
        for i in 0..self.side_by_sides.len() {
            if self.side_by_sides[i].workspace != here {
                continue;
            }
            let (live_buffer, ref_buffer) = (self.side_by_sides[i].live_buffer, self.side_by_sides[i].ref_buffer);
            let Some(edits) = self.buffers.get(live_buffer).map(|ob| ob.buffer.edit_count()) else { continue };
            if edits != self.side_by_sides[i].aligned_at {
                let live = self.buffers.get(live_buffer).map(buffer_text).unwrap_or_default();
                let old = self.buffers.get(ref_buffer).map(buffer_text).unwrap_or_default();
                let session = &mut self.side_by_sides[i];
                session.alignment = fenix_diff::align(&old, &live);
                session.aligned_at = edits;
                session.synced = None;
            }
            let session = &self.side_by_sides[i];
            let Some(side) = session.side_of(focused) else { continue };
            let other = session.other(focused);
            let line = self.buffers.get(if side == Side::Old { ref_buffer } else { live_buffer }).map(|ob| ob.buffer.line_col(&self.pane_state(focused).cursor).0).unwrap_or(0);
            if session.synced == Some((focused, line)) {
                continue;
            }
            let target = session.alignment.counterpart(side, line);
            let other_buffer = if side == Side::Old { live_buffer } else { ref_buffer };
            if let Some(ob) = self.buffers.get(other_buffer) {
                let target = target.min(ob.buffer.line_count().saturating_sub(1));
                let char_idx = ob.buffer.line_start_char(target);
                if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&other) {
                    state.cursor = Cursor { char_idx, sticky_col: 0 };
                }
            }
            self.side_by_sides[i].synced = Some((focused, line));
        }
    }

    /// For the pane not focused in a side-by-side whose other pane is:
    /// scrolls it to exactly where the focused one is, after that one has
    /// followed its cursor (`visible` is the height both share).
    pub(super) fn side_by_side_follow(&mut self, pane: fenix_window::WindowId, visible: usize) {
        let focused = self.focused_pane_id();
        let Some(session) = self.side_by_side_at(pane) else { return };
        if pane == focused || session.other(pane) != focused {
            return;
        }
        self.ensure_cursor_visible(visible);
        let leader = *self.pane_state(focused);
        let state = self.pane_state_mut(pane);
        state.scroll_line = leader.scroll_line;
        state.rendered_scroll = leader.rendered_scroll;
        state.scroll_col = leader.scroll_col;
    }

    /// The tints for a side-by-side pane's visible rows: whole rows
    /// (`(row, colour)`) and changed words (`(row, from, to, colour)`, in
    /// buffer columns).
    pub(super) fn side_by_side_tints(&self, pane: fenix_window::WindowId, rows: &[usize]) -> Tints {
        let (mut whole, mut words) = (Vec::new(), Vec::new());
        let Some(session) = self.side_by_side_at(pane) else { return (whole, words) };
        let Some(side) = session.side_of(pane) else { return (whole, words) };
        let (Some(mine), Some(theirs)) = (
            self.buffers.get(if side == Side::Old { session.ref_buffer } else { session.live_buffer }),
            self.buffers.get(if side == Side::Old { session.live_buffer } else { session.ref_buffer }),
        ) else {
            return (whole, words);
        };
        let line_text = |ob: &OpenBuffer, line: usize| {
            let start = ob.buffer.line_start_char(line);
            ob.buffer.text_range(start, start + ob.buffer.line_len(line)).trim_end_matches(['\r', '\n']).to_string()
        };
        let [r, g, b, _] = glyphon_to_rgba(self.theme.gutter_fg);
        let filler = [r, g, b, 0.10];
        for (row, &line) in rows.iter().enumerate() {
            if line == FILLER {
                whole.push((row, filler));
                continue;
            }
            let Some(block) = session.alignment.blocks.iter().find(|b| if side == Side::Old { b.old.contains(&line) } else { b.new.contains(&line) }) else { continue };
            let Some(color) = tint(block.kind, side, self.theme) else { continue };
            whole.push((row, color));
            if block.kind != BlockKind::Changed {
                continue;
            }
            let (from, to) = if side == Side::Old { (&block.old, &block.new) } else { (&block.new, &block.old) };
            let other_line = to.start + (line - from.start);
            if !to.contains(&other_line) || line >= mine.buffer.line_count() || other_line >= theirs.buffer.line_count() {
                continue;
            }
            let (here, there) = (line_text(mine, line), line_text(theirs, other_line));
            let (old, new) = if side == Side::Old { (here.as_str(), there.as_str()) } else { (there.as_str(), here.as_str()) };
            let (old_spans, new_spans) = fenix_diff::changed_spans(old, new);
            let spans = if side == Side::Old { old_spans } else { new_spans };
            let base = if side == Side::Old { self.theme.git_conflicted } else { self.theme.git_staged };
            let [r, g, b, _] = glyphon_to_rgba(base);
            words.extend(spans.into_iter().map(|s| (row, s.start, s.end, [r, g, b, 0.30])));
        }
        (whole, words)
    }

    /// `]c` / `[c`: the cursor to the next or previous change.
    pub(super) fn side_by_side_jump(&mut self, forward: bool, count: u32) {
        let pane = self.focused_pane_id();
        let Some(session) = self.side_by_side_at(pane) else {
            self.set_message("]c and [c walk a side-by-side diff -- SPC g v opens one");
            return;
        };
        let Some(side) = session.side_of(pane) else { return };
        let buffer = if side == Side::Old { session.ref_buffer } else { session.live_buffer };
        let starts: Vec<usize> = session.alignment.changes().map(|b| if side == Side::Old { b.old.start } else { b.new.start }).collect();
        let Some(ob) = self.buffers.get(buffer) else { return };
        let line = ob.buffer.line_col(&self.pane_state(pane).cursor).0;
        let mut target = None;
        let mut at = line;
        for _ in 0..count.max(1) {
            let next = if forward { starts.iter().copied().find(|&s| s > at) } else { starts.iter().rev().copied().find(|&s| s < at) };
            match next {
                Some(s) => {
                    target = Some(s);
                    at = s;
                }
                None => break,
            }
        }
        let Some(target) = target else {
            self.set_message(if forward { "no more changes below" } else { "no more changes above" });
            return;
        };
        let target = target.min(ob.buffer.line_count().saturating_sub(1));
        let char_idx = ob.buffer.line_start_char(target);
        let (_, cursor) = self.focused_buffer_and_cursor_mut();
        *cursor = Cursor { char_idx, sticky_col: 0 };
        self.wake_caret();
    }

    /// `do` (in your pane) or `dp` (in the ref's): your buffer takes the
    /// ref's version of the change under the cursor, as one undo step.
    pub(super) fn side_by_side_take(&mut self, put: bool) {
        let pane = self.focused_pane_id();
        let Some(session) = self.side_by_side_at(pane) else {
            self.set_message("do and dp take a change across a side-by-side diff -- SPC g v opens one");
            return;
        };
        let Some(side) = session.side_of(pane) else { return };
        // Only your buffer can change: `do` there, or `dp` from the ref.
        if (side == Side::New) == put {
            let rev = session.rev.clone();
            self.set_error(format!("the {rev} side is read-only -- do in your pane takes its version, dp in its pane gives it to you"));
            return;
        }
        let (ref_buffer, live_buffer, live_pane) = (session.ref_buffer, session.live_buffer, session.live_pane);
        let buffer = if side == Side::Old { ref_buffer } else { live_buffer };
        let Some(line) = self.buffers.get(buffer).map(|ob| ob.buffer.line_col(&self.pane_state(pane).cursor).0) else { return };
        let Some(block) = session.alignment.block_at(side, line).cloned() else { return };
        if block.kind == BlockKind::Equal {
            self.set_message("no change here");
            return;
        }
        let old_text = self.buffers.get(ref_buffer).map(buffer_text).unwrap_or_default();
        let old_lines: Vec<&str> = old_text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
        let Some(ob) = self.buffers.get_mut(live_buffer) else { return };
        let crlf = ob.buffer.text_range(0, ob.buffer.len_chars().min(4096)).contains("\r\n");
        let eol = if crlf { "\r\n" } else { "\n" };
        let line_count = ob.buffer.line_count();
        let start = if block.new.start < line_count { ob.buffer.line_start_char(block.new.start) } else { ob.buffer.len_chars() };
        let end = if block.new.end < line_count { ob.buffer.line_start_char(block.new.end) } else { ob.buffer.len_chars() };
        let mut text = old_lines[block.old.clone()].join(eol);
        // Whole lines in, whole lines out: the line break after the
        // block stays, unless the block ran to the end of the file.
        if !block.old.is_empty() && block.new.end < line_count {
            text.push_str(eol);
        }
        let mut cursor = Cursor { char_idx: start, sticky_col: 0 };
        ob.buffer.replace_range(&mut cursor, start, end, &text);
        if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&live_pane) {
            state.cursor = Cursor { char_idx: start, sticky_col: 0 };
        }
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git").current_dir(dir).args(args).output().expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A repository with `a.txt` committed as one, two, three.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-side-by-side-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
            git(&dir, &["init", "-q", "-b", "main"]);
            git(&dir, &["config", "user.email", "t@example.com"]);
            git(&dir, &["config", "user.name", "T"]);
            git(&dir, &["config", "core.autocrlf", "false"]);
            std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "initial"]);
            Repo(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("a.txt")
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Inserts `text` at the start of `line` in the focused buffer.
    fn insert_at_line(app: &mut App, line: usize, text: &str) {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        let at = buffer.line_start_char(line);
        buffer.replace_range(cursor, at, at, text);
    }

    fn opened(name: &str, edit: impl FnOnce(&mut App)) -> (Repo, App) {
        let repo = Repo::new(name);
        let mut app = App::with_file(Some(repo.file().to_string_lossy().into_owned()));
        edit(&mut app);
        app.open_side_by_side(&repo.file(), "HEAD");
        assert_eq!(app.side_by_sides.len(), 1, "opened");
        (repo, app)
    }

    fn lines_of(app: &App, ref_side: bool) -> Vec<usize> {
        let s = &app.side_by_sides[0];
        if ref_side {
            app.pane_display_lines(s.ref_pane, s.ref_buffer)
        } else {
            app.pane_display_lines(s.live_pane, s.live_buffer)
        }
    }

    fn live_text(app: &App) -> String {
        app.buffers.get(app.side_by_sides[0].live_buffer).unwrap().buffer.text()
    }

    #[test]
    fn an_unsaved_added_block_leaves_space_beside_it_on_the_left() {
        let (_repo, app) = opened("added", |app| insert_at_line(app, 1, "new A\nnew B\n"));
        let s = &app.side_by_sides[0];
        assert_eq!(app.focused_pane_id(), s.live_pane, "your buffer keeps the keyboard");
        let layout = app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 });
        assert_eq!(layout.first().map(|(id, _)| *id), Some(s.ref_pane), "the ref's version is on the left");
        assert_eq!(lines_of(&app, true), vec![0, FILLER, FILLER, 1, 2, 3]);
        assert_eq!(lines_of(&app, false), vec![0, 1, 2, 3, 4, 5]);
        let (tints, _) = app.side_by_side_tints(s.live_pane, &lines_of(&app, false));
        assert_eq!(tints.iter().map(|t| t.0).collect::<Vec<_>>(), vec![1, 2], "the two added lines are tinted");
        assert_eq!(app.buffer_display_name(s.ref_buffer), "HEAD:a.txt");
    }

    #[test]
    fn typing_lines_them_up_again() {
        let (_repo, mut app) = opened("typing", |_| {});
        assert_eq!(lines_of(&app, true), vec![0, 1, 2, 3]);
        insert_at_line(&mut app, 3, "four\n");
        app.side_by_side_frame();
        assert_eq!(lines_of(&app, true), vec![0, 1, 2, FILLER, 3]);
    }

    #[test]
    fn do_takes_the_refs_version_back() {
        let (_repo, mut app) = opened("obtain", |app| {
            let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
            let (from, to) = (buffer.line_start_char(1), buffer.line_start_char(2));
            buffer.replace_range(cursor, from, to, "");
        });
        assert_eq!(live_text(&app), "one\nthree\n");
        assert_eq!(lines_of(&app, false), vec![0, FILLER, 1, 2], "space where two was");
        // On the line under the removal, as in Vim.
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(1);
        app.side_by_side_take(false);
        assert_eq!(live_text(&app), "one\ntwo\nthree\n");
        app.side_by_side_frame();
        assert_eq!(app.side_by_sides[0].alignment.changes().count(), 0);
    }

    #[test]
    fn the_ref_side_cant_be_changed() {
        let (_repo, mut app) = opened("readonly", |app| insert_at_line(app, 0, "zero\n"));
        let ref_pane = app.side_by_sides[0].ref_pane;
        app.windows_mut().focus(ref_pane);
        app.side_by_side_take(false);
        assert!(app.status_message.as_ref().is_some_and(|m| m.is_error), "do in the ref's pane would change the ref");
        assert!(app.buffer_edits_are_reverted(app.side_by_sides[0].ref_buffer, BufferKind::Text));
    }

    #[test]
    fn bracket_c_walks_the_changes() {
        let (_repo, mut app) = opened("walk", |app| {
            insert_at_line(app, 3, "four\n");
            insert_at_line(app, 1, "one and a half\n");
        });
        app.focused_buffer_and_cursor_mut().1.char_idx = 0;
        app.side_by_side_jump(true, 1);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1);
        app.side_by_side_jump(true, 1);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 4);
        app.side_by_side_jump(false, 1);
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 1);
    }

    #[test]
    fn the_other_pane_follows_the_cursor_and_the_scroll() {
        let (_repo, mut app) = opened("follow", |app| insert_at_line(app, 1, "a\nb\nc\nd\n"));
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(6);
        app.side_by_side_frame();
        let ref_pane = app.side_by_sides[0].ref_pane;
        let ref_buffer = app.side_by_sides[0].ref_buffer;
        let ref_cursor = app.pane_state(ref_pane).cursor;
        assert_eq!(app.buffers.get(ref_buffer).unwrap().buffer.line_col(&ref_cursor).0, 2, "three to three");
        app.side_by_side_follow(ref_pane, 2);
        let live = *app.pane_state(app.focused_pane_id());
        assert!(live.scroll_line > 0, "scrolled to keep the cursor in a two-row pane");
        assert_eq!(app.pane_state(ref_pane).scroll_line, live.scroll_line);
    }

    #[test]
    fn a_file_not_on_the_ref_says_so_and_opens_nothing() {
        let repo = Repo::new("missing");
        std::fs::write(repo.0.join("new.txt"), "fresh\n").unwrap();
        let path = repo.0.join("new.txt");
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        app.open_side_by_side(&path, "HEAD");
        assert!(app.side_by_sides.is_empty());
        assert_eq!(app.windows().windows().len(), 1);
        assert!(app.status_message.as_ref().is_some_and(|m| m.text.contains("isn't on HEAD")));
    }

    #[test]
    fn q_in_the_left_pane_closes_it_and_leaves_your_buffer() {
        let (_repo, mut app) = opened("close", |app| insert_at_line(app, 0, "zero\n"));
        let (ref_pane, ref_buffer, live_buffer) = (app.side_by_sides[0].ref_pane, app.side_by_sides[0].ref_buffer, app.side_by_sides[0].live_buffer);
        app.windows_mut().focus(ref_pane);
        assert!(app.focused_is_side_by_side_ref());
        assert!(app.side_by_side_close());
        assert!(app.side_by_sides.is_empty());
        assert_eq!(app.windows().windows().len(), 1);
        assert_eq!(app.focused_buffer_id(), live_buffer);
        assert!(app.buffers.get(ref_buffer).is_none());
        assert!(app.buffers.get(live_buffer).unwrap().buffer.text().starts_with("zero\n"), "unsaved edits kept");
    }

    #[test]
    fn fillers_go_before_their_line_and_at_the_end() {
        assert_eq!(with_fillers(vec![0, 1, 2], &[(1, 2), (3, 1)]), vec![0, FILLER, FILLER, 1, 2, FILLER]);
        assert_eq!(with_fillers(vec![0, 1], &[]), vec![0, 1]);
    }

    #[test]
    fn a_filler_before_a_folded_line_goes_before_the_next_shown_one() {
        // Lines 1-2 folded under 0.
        assert_eq!(with_fillers(vec![0, 3], &[(2, 1)]), vec![0, FILLER, 3]);
    }
}
