//! The host half of the Log page (`git_log`): reading history off the
//! UI thread, a commit's files and their diffs on demand, and the jobs
//! its commit menu asks for -- logged, like the status page's, so `U`
//! takes them back.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::git_log::{Detail, GitLog, GraphLine, LogAction, LogData, Scope};
use crate::git_status::{Action, Compose, DiffState};

/// Commits read for a page.
const LIMIT: usize = 300;

fn read_log(root: &Path, scope: &Scope, words: Option<String>, author: Option<String>, style: crate::graph_view::GraphStyle) -> LogData {
    let mut patches = HashMap::new();
    let commits = match scope {
        Scope::Lines { path, start, end } => match fenix_git::line_log(root, path, *start, *end, LIMIT) {
            Ok(entries) => entries
                .into_iter()
                .map(|(commit, patch)| {
                    patches.insert(commit.hash.clone(), fenix_diff::parse(&patch));
                    commit
                })
                .collect(),
            Err(_) => Vec::new(),
        },
        _ => {
            let query = fenix_git::LogQuery {
                all: *scope == Scope::All,
                path: match scope {
                    Scope::File(path) => Some(path.clone()),
                    _ => None,
                },
                grep: words,
                author,
                limit: LIMIT,
            };
            fenix_git::log(root, &query)
        }
    };
    let rows = fenix_git::assign_lanes(&commits);
    let panel = crate::graph_view::render_graph(&commits, &rows, style);
    let lines = panel
        .text
        .lines()
        .zip(panel.lines)
        .map(|(text, line)| {
            let (spans, commit) = line.map(|l| (l.spans, l.commit)).unwrap_or_default();
            GraphLine { text: text.to_string(), spans, commit }
        })
        .collect();
    LogData {
        on_head: fenix_git::reachable(root, "HEAD", LIMIT * 4).into_iter().collect(),
        head: Some(fenix_git::oplog::rev(root, "HEAD")).filter(|h| !h.is_empty()),
        branch: fenix_git::oplog::current_branch(root).unwrap_or_else(|| "HEAD (detached)".to_string()),
        lines,
        commits,
        patches,
    }
}

impl App {
    fn git_log(&mut self, id: BufferId) -> Option<&mut GitLog> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Log(l)) => Some(l),
            _ => None,
        }
    }

    /// Opens the Log page on `scope` for the focused file's repository --
    /// the one already showing it, or a new one.
    pub(crate) fn open_git_log(&mut self, scope: Scope) {
        let start = match self.open().buffer.path() {
            Some(path) if self.focused_git_page_root().is_none() => path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.git_action_repo_root()),
            _ => self.git_action_repo_root(),
        };
        let Some(root) = super::git_editor::repository_of(&start) else {
            self.set_error(format!("{} isn't in a git repository", readable_path(&start)));
            return;
        };
        let root = fenix_lsp::normalize(root);
        let id = match self.find_page(|m| matches!(m, PageModel::Log(l) if l.root == root && l.scope == scope)) {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => self.open_page(PageModel::Log(Box::new(GitLog::new(root, scope)))),
        };
        self.git_log_refresh(id);
    }

    /// `SPC g h`: the focused file's history.
    pub(crate) fn open_file_history(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("the history of a file needs a file");
            return;
        };
        let Some(root) = super::git_editor::repository_of(&path) else {
            self.set_error("this file isn't in a git repository");
            return;
        };
        let relative = super::git_editor::relative_to(&path, &root);
        self.open_git_log(Scope::File(relative));
    }

    /// `SPC g H`: the history of the lines last selected in Visual mode
    /// (`gv`'s), or of the cursor's line.
    pub(crate) fn open_line_history(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("the history of some lines needs a file");
            return;
        };
        let Some(root) = super::git_editor::repository_of(&path) else {
            self.set_error("this file isn't in a git repository");
            return;
        };
        let cursor = self.cursor().char_idx;
        let buffer = &self.open().buffer;
        let (a, b) = self.vim.last_visual_range().filter(|(a, b)| (*a..=*b).contains(&cursor)).unwrap_or((cursor, cursor));
        let start = buffer.line_col(&Cursor { char_idx: a, sticky_col: 0 }).0 + 1;
        let end = buffer.line_col(&Cursor { char_idx: b, sticky_col: 0 }).0 + 1;
        let relative = super::git_editor::relative_to(&path, &root);
        self.open_git_log(Scope::Lines { path: relative, start, end });
    }

    pub(super) fn git_log_refresh(&mut self, id: BufferId) {
        let style = crate::graph_view::GraphStyle::from_config(self.config.git_graph_style.as_deref());
        let Some(log) = self.git_log(id) else { return };
        log.loading = true;
        let (root, scope) = (log.root.clone(), log.scope.clone());
        let (words, author) = log.query();
        self.page_spawn(move |send| {
            let data = read_log(&root, &scope, words, author, style);
            send(PageEvent::GitLogData { buffer: id, data: Box::new(data) });
        });
    }

    /// Reads every Log page again -- after an operation changed history.
    pub(super) fn refresh_git_logs(&mut self) {
        let ids: Vec<BufferId> = self.pages.iter().filter(|(_, s)| matches!(s.model, PageModel::Log(_))).map(|(id, _)| *id).collect();
        for id in ids {
            self.git_log_refresh(id);
        }
    }

    pub(super) fn git_log_action(&mut self, id: BufferId, action: LogAction) {
        let Some(root) = self.git_log(id).map(|l| l.root.clone()) else { return };
        match action {
            LogAction::LoadCommit(hash) => self.page_spawn(move |send| {
                let files = fenix_git::commit_files(&root, &hash);
                send(PageEvent::GitLogFiles { buffer: id, hash, files });
            }),
            LogAction::LoadFile { hash, path } => self.page_spawn(move |send| {
                let diff = match fenix_git::commit_file_diff(&root, &hash, &path) {
                    Ok(text) => match fenix_diff::parse(&text).into_iter().next() {
                        Some(d) if !d.hunks.is_empty() => DiffState::Loaded(Box::new(d)),
                        Some(d) if d.is_binary => DiffState::Empty("a binary file".to_string()),
                        _ => DiffState::Empty("no textual change".to_string()),
                    },
                    Err(e) => DiffState::Empty(e.lines().next().unwrap_or("").to_string()),
                };
                send(PageEvent::GitLogDiff { buffer: id, hash, path, diff });
            }),
            LogAction::Git(Action::Refresh) => self.git_log_refresh(id),
            LogAction::Git(Action::Close) => self.close_page(id),
            LogAction::Git(Action::Run(job)) => {
                let Some(log) = self.git_log(id) else { return };
                if let Some(busy) = &log.busy {
                    log.message = Some((format!("still running {busy}"), true));
                    return;
                }
                let label = job.label();
                log.busy = Some(label.clone());
                log.message = None;
                self.page_spawn(move |send| {
                    let result = super::git_page::run_logged(&root, &job);
                    send(PageEvent::GitDone { buffer: id, label, result });
                });
            }
            LogAction::Git(Action::Compose(compose @ Compose::Reword(_))) => {
                let seed = fenix_git::commit_message(&root, "HEAD").unwrap_or_default();
                self.open_compose_seeded(ComposePurpose::GitCommit { repo_root: root, compose }, &seed);
            }
            LogAction::Git(Action::OpenFile { path, line }) => {
                let full = root.join(&path);
                if !full.is_file() {
                    self.set_error(format!("{path} isn't in the working tree any more"));
                    return;
                }
                self.open_file_from_picker(&full);
                if let Some(line) = line {
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path: full, line, col: 1, text: String::new() });
                }
            }
            LogAction::Git(Action::Copy(text)) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text.clone());
                }
                if let Some(log) = self.git_log(id) {
                    log.message = Some((format!("copied {}", crate::git_status::short(&text)), false));
                }
            }
            LogAction::Git(Action::ShowOutput) => {
                let Some(log) = self.git_log(id) else { return };
                let text = if log.output.is_empty() { "(nothing has run yet)".to_string() } else { log.output.join("\n") };
                let buffer = self.buffers.open_text_view(&text);
                self.open_buffer_in_focused_pane(buffer);
            }
            LogAction::Git(Action::Rebase(from)) => self.open_rebase(from),
            LogAction::Git(_) => {}
        }
    }

    pub(super) fn apply_git_log_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::GitLogData { buffer, data } => {
                if let Some(log) = self.git_log(buffer) {
                    log.set_data(*data);
                }
            }
            PageEvent::GitLogFiles { buffer, hash, files } => {
                if let Some(log) = self.git_log(buffer) {
                    log.set_files(hash, files);
                }
            }
            PageEvent::GitLogDiff { buffer, hash, path, diff } => {
                if let Some(log) = self.git_log(buffer) {
                    log.set_diff(hash, path, diff);
                }
            }
            PageEvent::GitDone { buffer, label, result } => {
                let Some(log) = self.git_log(buffer) else { return };
                log.busy = None;
                let (text, failed) = match &result {
                    Ok(out) => (out.clone(), false),
                    Err(err) => (err.clone(), true),
                };
                log.output = std::iter::once(format!("$ {label}")).chain(text.lines().map(str::to_string)).collect();
                let first = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("hint:")).unwrap_or("").to_string();
                log.message = Some(if failed { (format!("{label} failed: {first}  ($ shows everything)"), true) } else { (format!("{label} ✓ -- U on the Git page undoes it"), false) });
                // A commit opened before may not exist any more.
                log.open.retain(|_, d| *d != Detail::Loading);
                self.git_refresh_all_views();
            }
            _ => {}
        }
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

    struct Repo(PathBuf);
    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-git-log-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
            for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@e.com"], &["config", "user.name", "Tess"], &["config", "core.autocrlf", "false"]] {
                git(&dir, args);
            }
            std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "first"]);
            std::fs::write(dir.join("a.txt"), "one\nTWO\n").unwrap();
            git(&dir, &["commit", "-q", "-am", "second"]);
            git(&dir, &["switch", "-q", "-c", "side", "HEAD~1"]);
            std::fs::write(dir.join("b.txt"), "b\n").unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "side work"]);
            git(&dir, &["switch", "-q", "main"]);
            Repo(dir)
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn log_page(app: &mut App) -> &mut GitLog {
        let id = app.focused_buffer_id();
        match app.pages.get_mut(&id).map(|s| &mut s.model) {
            Some(PageModel::Log(l)) => l,
            _ => panic!("the log page isn't focused"),
        }
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let key = match c {
                '\n' => KeyPress::named(FenixNamedKey::Enter),
                '\t' => KeyPress::named(FenixNamedKey::Tab),
                c => KeyPress::char(c),
            };
            assert!(app.page_key(key), "the page claims {c:?}");
        }
    }

    fn text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 120);
        app.open().buffer.text()
    }

    #[test]
    fn every_branch_and_a_cherry_pick_from_one() {
        let repo = Repo::new("pick");
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        app.open_git_log(Scope::Branch);
        let shown = text(&mut app);
        assert!(shown.contains("History of main") && shown.contains("second") && !shown.contains("side work"), "{shown}");
        press(&mut app, "a");
        assert!(text(&mut app).contains("side work"));
        let side = git(&repo.0, &["rev-parse", "side"]);
        let log = log_page(&mut app);
        log.cursor = log.rows().iter().position(|r| *r == crate::git_log::Row::Commit(side.clone())).unwrap();
        press(&mut app, "\nc");
        assert_eq!(git(&repo.0, &["log", "-1", "--format=%s"]), "side work", "picked onto main");
        assert!(text(&mut app).contains("cherry-pick"));
        assert!(fenix_git::oplog::entries(&repo.0, 1)[0].undo.possible(), "logged, undoable");
    }

    #[test]
    fn a_files_history_opens_a_commit_to_its_diff() {
        let repo = Repo::new("file");
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        app.open_file_history();
        let shown = text(&mut app);
        assert!(shown.contains("History of a.txt") && shown.contains("second") && shown.contains("first"), "{shown}");
        press(&mut app, "\tj\t");
        let shown = text(&mut app);
        assert!(shown.contains("a.txt") && shown.contains("+TWO") && shown.contains("-two"), "{shown}");
    }

    #[test]
    fn line_history_starts_from_the_cursors_line() {
        let repo = Repo::new("lines");
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(1);
        app.open_line_history();
        let shown = text(&mut app);
        assert!(shown.contains("History of line 2 of a.txt") && shown.contains("+TWO"), "{shown}");
    }
}
