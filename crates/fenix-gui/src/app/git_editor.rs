//! Git from the file you're editing: the changed hunks the gutter marks
//! (`]h`/`[h` to move between them, `SPC g a` stages the one under the
//! cursor, `SPC g d` discards it, `SPC g i` shows it), blame beside the
//! text (`SPC g B`, `SPC g e` explains a line), and switching branch
//! with the local changes handled (`SPC g w`).

use super::pages::PageEvent;
use super::*;
use fenix_git::oplog::{self, Undo};

/// How wide the blame column is, in cells.
const BLAME_WIDTH: usize = 30;

/// A file's blame, one entry per line of the buffer as it was blamed.
pub(super) struct Blame {
    lines: Vec<fenix_git::BlameLine>,
    /// The buffer's edit count when it was blamed; when it moves on, the
    /// lines no longer line up and the blame is read again.
    edits: u64,
    loading: bool,
}

/// The repository a file is in, found from its absolute path -- a
/// buffer opened from the command line can hold a relative one, whose
/// ancestors run out before the repository's root.
pub(super) fn repository_of(path: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(path).map(fenix_lsp::normalize).unwrap_or_else(|_| path.to_path_buf());
    fenix_project::vcs::repository_root(&absolute)
}

/// `path` as git names it inside `root`: from the top, with `/`.
pub(super) fn relative_to(path: &Path, root: &Path) -> String {
    let absolute = std::fs::canonicalize(path).map(fenix_lsp::normalize).unwrap_or_else(|_| path.to_path_buf());
    absolute.strip_prefix(root).unwrap_or(&absolute).to_string_lossy().replace('\\', "/")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "3h", "2d", "5w", "1y" -- as short as a narrow column needs.
fn short_age(secs: u64) -> String {
    let (m, h, d) = (secs / 60, secs / 3600, secs / 86400);
    match () {
        _ if m < 60 => format!("{m}m"),
        _ if h < 24 => format!("{h}h"),
        _ if d < 14 => format!("{d}d"),
        _ if d < 365 => format!("{}w", d / 7),
        _ => format!("{}y", d / 365),
    }
}

/// Where a hunk's change starts, as a 1-based line of the file: its
/// first added line, or for a pure deletion the line after it -- not the
/// context git puts around it.
fn change_start(h: &fenix_diff::Hunk) -> usize {
    let first = h.lines.iter().position(|l| matches!(l.kind, fenix_diff::LineKind::Added | fenix_diff::LineKind::Removed)).unwrap_or(0);
    h.lines[first..].iter().find_map(|l| l.new_line).unwrap_or(h.new_start + h.new_len).max(1)
}

/// The hunk of `file` that covers 1-based `line`: its changed lines, or,
/// for a pure deletion, the line after it (where the gutter marks it).
fn hunk_covering(file: &fenix_diff::FileDiff, line: usize) -> Option<usize> {
    file.hunks.iter().position(|h| {
        let start = h.new_start.max(1);
        let end = if h.new_len == 0 { start + 1 } else { h.new_start + h.new_len };
        line >= start && line < end
    })
}

impl App {
    /// The focused buffer's hunk under the cursor: its buffer, repository
    /// root, diff and hunk index.
    fn hunk_under_cursor(&mut self) -> Option<(BufferId, PathBuf, fenix_diff::FileDiff, usize)> {
        let buffer = self.focused_buffer_id();
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let Some((root, file)) = self.gutter_diffs.get(&buffer).cloned() else {
            self.set_message("no changes in this file since it was last staged");
            return None;
        };
        match hunk_covering(&file, line + 1) {
            Some(hunk) => Some((buffer, root, file, hunk)),
            None => {
                self.set_message("the cursor isn't on a changed hunk -- ]h goes to the next one");
                None
            }
        }
    }

    /// `]h`/`[h`: the `count`th next or previous changed hunk.
    pub(crate) fn jump_to_hunk(&mut self, forward: bool, count: u32) {
        let buffer = self.focused_buffer_id();
        let Some((_, file)) = self.gutter_diffs.get(&buffer) else {
            self.set_message("no changed hunks in this file");
            return;
        };
        let starts: Vec<usize> = file.hunks.iter().map(|h| change_start(h) - 1).collect();
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let candidates: Vec<usize> = if forward {
            starts.into_iter().filter(|&s| s > line).collect()
        } else {
            starts.into_iter().rev().filter(|&s| s < line).collect()
        };
        let Some(&target) = candidates.get((count.max(1) as usize - 1).min(candidates.len().saturating_sub(1))) else {
            self.set_message(if forward { "no more changed hunks below" } else { "no more changed hunks above" });
            return;
        };
        let from = JumpEntry { buffer, char_idx: self.cursor().char_idx };
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let target = target.min(buffer.line_count().saturating_sub(1));
        cursor.char_idx = buffer.line_start_char(target);
        cursor.sticky_col = 0;
        self.record_jump(from);
        self.wake_caret();
    }

    /// `SPC g a`: stages the hunk under the cursor.
    pub(crate) fn git_hunk_stage(&mut self) {
        let Some((buffer, root, file, hunk)) = self.hunk_under_cursor() else { return };
        let dirty = self.open().buffer.is_dirty();
        let patch = fenix_diff::hunk_patch(&file, &file.hunks[hunk]);
        match fenix_git::apply_patch(&root, &patch, fenix_git::ApplyTarget::Stage) {
            Ok(_) => self.set_message(if dirty { "staged the saved version of this hunk -- the unsaved edits aren't in it" } else { "hunk staged" }),
            Err(err) => self.set_error(format!("couldn't stage the hunk: {}", err.lines().next().unwrap_or(""))),
        }
        self.refresh_gutter_hunks(buffer);
        self.refresh_git_pages(false);
    }

    /// `SPC g d`: asks before discarding the hunk under the cursor.
    pub(crate) fn git_hunk_discard_prompt(&mut self) {
        if self.open().buffer.is_dirty() {
            self.set_error("save first -- discarding works on what's on disk");
            return;
        }
        let Some((buffer, _, file, hunk)) = self.hunk_under_cursor() else { return };
        let line = file.hunks[hunk].new_start.max(1);
        self.git_confirm = Some(GitConfirmAction::DiscardEditorHunk { buffer, line });
    }

    /// Discards the hunk of `buffer` covering `line`, logged so `U` can
    /// bring it back, and re-reads the file.
    pub(super) fn git_hunk_discard(&mut self, buffer: BufferId, line: usize) {
        self.refresh_gutter_hunks(buffer);
        let Some((root, file)) = self.gutter_diffs.get(&buffer).cloned() else { return };
        let Some(hunk) = hunk_covering(&file, line) else {
            self.set_error("that hunk isn't there any more");
            return;
        };
        let patch = fenix_diff::hunk_patch(&file, &file.hunks[hunk]);
        let saved = oplog::save_patch(&root, &patch);
        let label = format!("discard a hunk of {}", file.display_path());
        let result = oplog::logged(&root, &label, || fenix_git::apply_patch(&root, &patch, fenix_git::ApplyTarget::Discard), move |_| match saved {
            Some(blob) => Undo::ApplyPatch(blob),
            None => Undo::Not("the discarded lines couldn't be saved".to_string()),
        });
        match result {
            Ok(_) => self.set_message("hunk discarded -- U on the Git page (SPC g z) brings it back"),
            Err(err) => self.set_error(format!("couldn't discard the hunk: {}", err.lines().next().unwrap_or(""))),
        }
        self.reload_buffers_changed_on_disk();
        self.refresh_gutter_hunks(buffer);
        self.refresh_git_pages(false);
    }

    /// `SPC g i`: the hunk under the cursor, in the hover popup.
    pub(crate) fn git_hunk_preview(&mut self) {
        let Some((_, _, file, hunk)) = self.hunk_under_cursor() else { return };
        let h = &file.hunks[hunk];
        let added = h.lines.iter().filter(|l| l.kind == fenix_diff::LineKind::Added).count();
        let removed = h.lines.iter().filter(|l| l.kind == fenix_diff::LineKind::Removed).count();
        let mut text = format!("Hunk {} of {} · +{added} −{removed} · not staged\n", hunk + 1, file.hunks.len());
        for line in h.lines.iter().filter(|l| l.kind != fenix_diff::LineKind::Context).take(30) {
            text.push_str(&line.raw());
            text.push('\n');
        }
        text.push_str("\nSPC g a stage · SPC g d discard · ]h next");
        self.lsp_hover = Some(text);
        self.wake_caret();
    }

    // -- Blame --------------------------------------------------------------

    /// How wide `ob`'s blame column is: 0 when it isn't blamed.
    pub(super) fn blame_width(&self, ob: &OpenBuffer) -> usize {
        match ob.buffer.path() {
            Some(path) if self.blames.get(path).is_some_and(|b| !b.lines.is_empty()) => BLAME_WIDTH,
            _ => 0,
        }
    }

    /// The blame column's text for one line: the commit, author and age
    /// on the first line of each run from one commit, blank below it --
    /// coloured by how recent the change is.
    pub(super) fn blame_cell(&self, ob: &OpenBuffer, line: usize, width: usize) -> (String, glyphon::Color) {
        let theme = self.theme;
        let blank = (" ".repeat(width), theme.gutter_fg);
        let Some(blame) = ob.buffer.path().and_then(|p| self.blames.get(p)) else { return blank };
        let Some(this) = blame.lines.get(line) else { return blank };
        if line > 0 && blame.lines.get(line - 1).is_some_and(|prev| prev.hash == this.hash) {
            return blank;
        }
        if this.uncommitted() {
            return (format!("{:<width$}", crate::page::fit("  not committed yet", width - 1)), theme.git_modified);
        }
        let age = now_secs().saturating_sub(this.time);
        let author: String = this.author.split_whitespace().next().unwrap_or("").chars().take(10).collect();
        let text = format!("{} {} {}", &this.hash[..7.min(this.hash.len())], author, short_age(age));
        let color = if age < 7 * 86400 {
            theme.git_modified
        } else if age < 90 * 86400 {
            theme.fg
        } else {
            theme.gutter_fg
        };
        (format!("{:<width$}", crate::page::fit(&text, width - 1)), color)
    }

    /// `SPC g B`: shows who last changed each line of the focused file,
    /// or hides it again.
    pub(crate) fn git_blame_toggle(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("blame needs a file on disk");
            return;
        };
        if self.blames.remove(&path).is_some() {
            self.set_message("blame hidden");
            return;
        }
        if repository_of(&path).is_none() {
            self.set_error("this file isn't in a git repository");
            return;
        }
        self.blames.insert(path.clone(), Blame { lines: Vec::new(), edits: 0, loading: false });
        self.request_blame(&path);
        self.set_message("reading blame… SPC g e explains a line, SPC g B hides it");
    }

    /// Reads `path`'s blame off the UI thread, from the buffer's text so
    /// the lines match what's on screen.
    fn request_blame(&mut self, path: &Path) {
        let Some(id) = self.buffers.id_for_path(path) else { return };
        let Some(ob) = self.buffers.get(id) else { return };
        let text = ob.buffer.text();
        let edits = ob.buffer.edit_count();
        let Some(root) = repository_of(path) else { return };
        if let Some(blame) = self.blames.get_mut(path) {
            blame.loading = true;
        }
        let path = path.to_path_buf();
        self.page_spawn(move |send| {
            let relative = relative_to(&path, &root);
            let result = fenix_git::blame(&root, &relative, Some(&text));
            send(PageEvent::Blame { path, edits, result });
        });
    }

    pub(super) fn apply_blame(&mut self, path: PathBuf, edits: u64, result: Result<Vec<fenix_git::BlameLine>, String>) {
        let Some(blame) = self.blames.get_mut(&path) else { return };
        blame.loading = false;
        match result {
            Ok(lines) => {
                blame.lines = lines;
                blame.edits = edits;
            }
            Err(err) => {
                self.blames.remove(&path);
                self.set_error(format!("couldn't blame it: {}", err.lines().next().unwrap_or("")));
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Reads blame again for files edited since it was read -- the
    /// lines have moved.
    pub(super) fn refresh_stale_blames(&mut self) {
        let stale: Vec<PathBuf> = self
            .blames
            .iter()
            .filter(|(path, blame)| {
                !blame.loading
                    && self.buffers.id_for_path(path).and_then(|id| self.buffers.get(id)).is_some_and(|ob| ob.buffer.edit_count() != blame.edits)
            })
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale {
            self.request_blame(&path);
        }
    }

    /// `SPC g e`: the commit that last changed the cursor's line, in full.
    pub(crate) fn git_blame_explain(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else { return };
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let Some(root) = repository_of(&path) else {
            self.set_error("this file isn't in a git repository");
            return;
        };
        let this = match self.blames.get(&path).and_then(|b| b.lines.get(line)).cloned() {
            Some(this) => this,
            None => {
                let relative = relative_to(&path, &root);
                let text = self.open().buffer.text();
                match fenix_git::blame(&root, &relative, Some(&text)).ok().and_then(|lines| lines.get(line).cloned()) {
                    Some(this) => this,
                    None => {
                        self.set_error("couldn't blame this line");
                        return;
                    }
                }
            }
        };
        if this.uncommitted() {
            self.lsp_hover = Some("Not committed yet -- this line is yours, since the last commit.".to_string());
            self.wake_caret();
            return;
        }
        let message = fenix_git::commit_message(&root, &this.hash).unwrap_or_else(|| this.summary.clone());
        let age = crate::git_status::ago(now_secs().saturating_sub(this.time));
        self.lsp_hover = Some(format!("{} · {} · {age}\n\n{message}", &this.hash[..7.min(this.hash.len())], this.author));
        self.wake_caret();
    }

    // -- Switching branch ---------------------------------------------------

    /// `SPC g w` (and `b b` on the Git page): a branch to switch to, the
    /// one you left most recently first.
    pub(crate) fn start_switch_picker(&mut self) {
        let root = self.git_action_repo_root();
        let local = fenix_git::list_branches(&root);
        let ages = fenix_git::branch_ages(&root);
        let mut candidates = Vec::new();
        for name in fenix_git::branches_by_recency(&root) {
            let Some(branch) = local.iter().find(|b| b.name == name) else { continue };
            if branch.current {
                continue;
            }
            let mut label = name.clone();
            if branch.upstream_gone {
                label.push_str("  [gone]");
            } else if branch.upstream.is_some() && (branch.ahead > 0 || branch.behind > 0) {
                label.push_str(&format!("  ↑{} ↓{}", branch.ahead, branch.behind));
            }
            if let Some(age) = ages.get(&name) {
                label.push_str(&format!("  · {age}"));
            }
            candidates.push(fenix_picker::Candidate::new(label, name));
        }
        for remote in fenix_git::list_remote_branches(&root) {
            let short = remote.split_once('/').map(|(_, b)| b.to_string()).unwrap_or_default();
            if short != "HEAD" && !short.is_empty() && !local.iter().any(|b| b.name == short) {
                candidates.push(fenix_picker::Candidate::new(format!("{remote}  · remote"), remote));
            }
        }
        if candidates.is_empty() {
            self.set_error("no other branch to switch to".to_string());
            return;
        }
        self.enter_picker(ActivePicker::SwitchBranch(fenix_picker::PickerState::new(candidates)));
    }

    /// Switches to `branch`; a remote branch (`origin/x`) becomes a local
    /// `x` tracking it. When local changes are in the way, asks whether to
    /// stash them; changes stashed when leaving this branch come back.
    pub(crate) fn git_switch_to(&mut self, branch: &str) {
        let root = self.git_action_repo_root();
        let remotes = fenix_git::remotes(&root);
        let local = match branch.split_once('/') {
            Some((remote, rest)) if remotes.iter().any(|r| r == remote) => rest.to_string(),
            _ => branch.to_string(),
        };
        let back = oplog::current_branch(&root);
        let result = oplog::logged(&root, &format!("switch to {local}"), || fenix_git::checkout_branch(&root, &local), move |_| match back {
            Some(branch) => Undo::Switch(branch),
            None => Undo::Not("HEAD was detached".to_string()),
        });
        match result {
            Err(err) if err.contains("would be overwritten") || err.contains("commit your changes or stash them") => {
                self.git_confirm = Some(GitConfirmAction::SwitchWithStash { branch: local });
            }
            result => {
                let ok = result.is_ok();
                self.run_git_operation(&format!("switch to {local}"), result);
                if ok {
                    self.bring_back_stash(&root, &local);
                }
            }
        }
    }

    /// Stashes the local changes (untracked included), labelled so they
    /// can come back, then switches.
    pub(super) fn git_switch_stashing(&mut self, branch: &str) {
        let root = self.git_action_repo_root();
        let from = oplog::current_branch(&root).unwrap_or_else(|| "HEAD".to_string());
        let options = fenix_git::StashOptions { message: Some(format!("fenix: left {from} for {branch}")), include_untracked: true, ..Default::default() };
        let stashed = oplog::logged(&root, &format!("stash, leaving {from}"), || fenix_git::stash_with(&root, &options), |r| match r {
            Ok(_) => Undo::PopStash(oplog::rev(&root, "stash@{0}")),
            Err(_) => Undo::Not("nothing was stashed".to_string()),
        });
        if let Err(err) = stashed {
            self.set_error(format!("couldn't stash: {}", err.lines().next().unwrap_or("")));
            return;
        }
        self.git_switch_to(branch);
    }

    /// Pops the changes stashed when `branch` was last left for another,
    /// if there are any.
    fn bring_back_stash(&mut self, root: &Path, branch: &str) {
        let tag = format!("fenix: left {branch} for ");
        let Some(stash) = fenix_git::list_stashes(root).into_iter().find(|s| s.message.contains(&tag)) else { return };
        let commit = oplog::rev(root, &format!("stash@{{{}}}", stash.index));
        let result = oplog::logged(root, &format!("bring back the changes left on {branch}"), || fenix_git::stash_pop(root, stash.index), |_| {
            Undo::Not("the changes went into your files -- stash them again to take them back".to_string())
        });
        match result {
            Ok(_) => self.set_message(format!("switched to {branch} and brought back the changes you left on it")),
            Err(err) => self.set_error(format!(
                "switched to {branch}, but the changes left on it didn't apply cleanly ({}) -- they're still in stash {}",
                err.lines().next().unwrap_or(""),
                &commit[..7.min(commit.len())]
            )),
        }
        self.git_refresh_all_views();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    /// A repository with `a.txt` committed on `main`, removed on drop.
    struct Repo(PathBuf);
    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-git-editor-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
            for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@e.com"], &["config", "user.name", "Tess Ter"], &["config", "core.autocrlf", "false"]] {
                git(&dir, args);
            }
            std::fs::write(dir.join("a.txt"), lines(&[])).unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "first"]);
            Repo(dir)
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Twenty numbered lines, the ones in `upper` shouted -- far enough
    /// apart that each change is its own hunk.
    fn lines(upper: &[usize]) -> String {
        (1..=20).map(|n| if upper.contains(&n) { format!("LINE {n}\n") } else { format!("line {n}\n") }).collect()
    }

    /// An app editing `a.txt` after lines 2 and 18 changed on disk.
    fn edited(repo: &Repo) -> App {
        std::fs::write(repo.0.join("a.txt"), lines(&[2, 18])).unwrap();
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        app.refresh_gutter_hunks(app.focused_buffer_id());
        assert_eq!(app.gutter_diffs.values().next().map(|d| d.1.hunks.len()), Some(2), "two hunks in the gutter");
        app
    }

    fn goto_line(app: &mut App, line: usize) {
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(line);
    }

    fn line(app: &mut App) -> usize {
        app.open().buffer.line_col(&app.cursor()).0
    }

    #[test]
    fn bracket_h_walks_the_hunks() {
        let repo = Repo::new("walk");
        let mut app = edited(&repo);
        goto_line(&mut app, 0);
        app.jump_to_hunk(true, 1);
        assert!(line(&mut app) < 3, "the first hunk starts near line 2");
        let first = line(&mut app);
        app.jump_to_hunk(true, 1);
        assert!(line(&mut app) > 10, "then the one near line 18");
        app.jump_to_hunk(false, 1);
        assert_eq!(line(&mut app), first);
    }

    #[test]
    fn the_hunk_under_the_cursor_is_staged_alone() {
        let repo = Repo::new("stage");
        let mut app = edited(&repo);
        goto_line(&mut app, 17); // line 18
        app.git_hunk_stage();
        assert_eq!(git(&repo.0, &["show", ":a.txt"]) + "\n", lines(&[18]));
        assert_eq!(app.gutter_diffs.values().next().map(|d| d.1.hunks.len()), Some(1), "the gutter caught up");
    }

    #[test]
    fn a_discarded_hunk_is_asked_about_and_comes_back_with_undo() {
        let repo = Repo::new("discard");
        let mut app = edited(&repo);
        goto_line(&mut app, 1); // line 2
        app.git_hunk_discard_prompt();
        assert!(app.git_confirm_text().is_some_and(|t| t.contains("Discard this hunk")));
        app.git_confirm_key(KeyPress::char('y'));
        assert_eq!(std::fs::read_to_string(repo.0.join("a.txt")).unwrap(), lines(&[18]));
        assert_eq!(app.open().buffer.text(), lines(&[18]), "the buffer was re-read, the other hunk kept");
        let log = oplog::entries(&repo.0, 5);
        assert!(log[0].label.starts_with("discard a hunk") && log[0].undo.possible());
        oplog::apply(&repo.0, &log[0].undo).unwrap();
        assert_eq!(std::fs::read_to_string(repo.0.join("a.txt")).unwrap(), lines(&[2, 18]));
    }

    #[test]
    fn blame_fills_a_column_beside_the_text_and_explains_a_line() {
        let repo = Repo::new("blame");
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        let before = app.gutter_chars(app.open());
        app.git_blame_toggle();
        assert_eq!(app.gutter_chars(app.open()), before + BLAME_WIDTH);
        let (first, _) = app.blame_cell(app.open(), 0, BLAME_WIDTH);
        assert!(first.starts_with(&git(&repo.0, &["rev-parse", "--short=7", "HEAD"])) && first.contains("Tess"), "{first:?}");
        assert!(app.blame_cell(app.open(), 1, BLAME_WIDTH).0.trim().is_empty(), "the rest of the run is blank");
        app.git_blame_explain();
        assert!(app.lsp_hover.as_deref().is_some_and(|h| h.contains("Tess Ter") && h.contains("first")));
        app.git_blame_toggle();
        assert_eq!(app.gutter_chars(app.open()), before, "hidden again");
    }

    #[test]
    fn a_switch_blocked_by_changes_stashes_them_and_they_come_back() {
        let repo = Repo::new("switch");
        git(&repo.0, &["switch", "-q", "-c", "topic"]);
        std::fs::write(repo.0.join("a.txt"), "topic\n").unwrap();
        git(&repo.0, &["commit", "-q", "-am", "topic"]);
        git(&repo.0, &["switch", "-q", "main"]);
        std::fs::write(repo.0.join("a.txt"), "main, in progress\n").unwrap();
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        app.git_switch_to("topic");
        assert!(app.git_confirm_text().is_some_and(|t| t.contains("stash them and switch")), "{:?}", app.git_confirm_text());
        app.git_confirm_key(KeyPress::char('y'));
        assert_eq!(git(&repo.0, &["rev-parse", "--abbrev-ref", "HEAD"]), "topic");
        app.git_switch_to("main");
        assert_eq!(git(&repo.0, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
        assert_eq!(std::fs::read_to_string(repo.0.join("a.txt")).unwrap(), "main, in progress\n", "the work came back");
        assert!(git(&repo.0, &["stash", "list"]).is_empty());
    }

    #[test]
    fn short_ages_fit_a_narrow_column() {
        assert_eq!(short_age(30), "0m");
        assert_eq!(short_age(3 * 3600), "3h");
        assert_eq!(short_age(3 * 86400), "3d");
        assert_eq!(short_age(40 * 86400), "5w");
        assert_eq!(short_age(800 * 86400), "2y");
    }
}
