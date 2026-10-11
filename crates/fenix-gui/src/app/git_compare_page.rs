//! The host half of the Compare page (`git_compare`): reading a
//! comparison off the UI thread, a file's or a commit's diff on demand,
//! fetching the target's remote, and the ref pickers that point the page
//! somewhere.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::git_compare::{CompareAction, CompareData, DiffKey, GitCompare, COMMIT_LIMIT};
use crate::git_status::DiffState;

fn read_compare(root: &Path, range: fenix_git::Range, target: &str, count_uncommitted: bool) -> CompareData {
    let head = match &range.head {
        fenix_git::Side::Rev(rev) => rev.clone(),
        fenix_git::Side::WorkTree => "HEAD".to_string(),
    };
    CompareData {
        branch: fenix_git::oplog::current_branch(root).unwrap_or_else(|| "HEAD (detached)".to_string()),
        merge_base: fenix_git::merge_base(root, &range.base, &head),
        ahead: fenix_git::commits_between(root, &range.base, &head, COMMIT_LIMIT),
        behind: fenix_git::commits_between(root, &head, &range.base, COMMIT_LIMIT),
        files: fenix_git::changed_files(root, &range),
        remote: fenix_git::remote_of(root, target),
        fetched_secs_ago: fenix_git::seconds_since_fetch(root),
        uncommitted: if count_uncommitted { fenix_git::status_and_files(root).1.len() } else { 0 },
        range,
    }
}

/// A diff's text as the page shows it.
fn diff_state(text: Result<String, String>) -> DiffState {
    match text {
        Ok(text) => match fenix_diff::parse(&text).into_iter().next() {
            Some(d) if d.is_binary => DiffState::Empty("a binary file -- no text to show".to_string()),
            Some(d) if !d.hunks.is_empty() => DiffState::Loaded(Box::new(d)),
            _ => DiffState::Empty("no textual change".to_string()),
        },
        Err(e) => DiffState::Empty(e.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("couldn't read the diff").to_string()),
    }
}

impl App {
    fn git_compare(&mut self, id: BufferId) -> Option<&mut GitCompare> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Compare(c)) => Some(c),
            _ => None,
        }
    }

    /// The repository a comparison is about: the focused Compare page's,
    /// else the focused file's, else the project's.
    fn compare_root(&self) -> Option<PathBuf> {
        if let Some(PageModel::Compare(c)) = self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            return Some(c.root.clone());
        }
        let start = match self.open().buffer.path() {
            Some(path) if self.focused_git_page_root().is_none() => path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.git_action_repo_root()),
            _ => self.git_action_repo_root(),
        };
        super::git_editor::repository_of(&start).map(fenix_lsp::normalize)
    }

    /// Every ref that can be compared with, the base branch first: as
    /// configured, else `main`/`master` -- its remote copy first, since
    /// that's what a pull request targets -- then local branches, remote
    /// branches and tags.
    fn compare_targets(&self, root: &Path) -> Vec<String> {
        let mut refs: Vec<String> = fenix_git::list_branches(root).into_iter().map(|b| b.name).collect();
        refs.extend(fenix_git::list_remote_branches(root));
        refs.extend(fenix_git::list_tags(root));
        let base = super::settings_host::base_branch_in(root, self.config.git_base_branch.clone());
        let preferred: Vec<String> = base.into_iter().chain(["main".to_string(), "master".to_string()]).collect();
        let mut lead = Vec::new();
        for name in &preferred {
            for candidate in [format!("origin/{name}"), name.clone()] {
                if let Some(i) = refs.iter().position(|r| *r == candidate) {
                    lead.push(refs.remove(i));
                }
            }
            if !lead.is_empty() {
                break;
            }
        }
        lead.extend(refs);
        lead
    }

    /// `SPC g c`, or `r` on the Compare page: the ref to compare HEAD with.
    pub(crate) fn start_compare_picker(&mut self) {
        let Some(root) = self.compare_root() else {
            self.set_error("comparing needs a git repository");
            return;
        };
        let refs = self.compare_targets(&root);
        if refs.is_empty() {
            self.set_error("no refs to compare with -- is there a commit yet?");
            return;
        }
        let candidates = refs.into_iter().map(|r| fenix_picker::Candidate::new(r.clone(), r)).collect();
        self.enter_picker(ActivePicker::CompareTarget(fenix_picker::PickerState::new(candidates)));
    }

    /// `R` on the Compare page: your own side, other than HEAD.
    fn start_compare_own_picker(&mut self, root: &Path, target: String) {
        let mut refs = vec!["HEAD".to_string()];
        refs.extend(self.compare_targets(root).into_iter().filter(|r| *r != target));
        let candidates = refs.into_iter().map(|r| fenix_picker::Candidate::new(r.clone(), r)).collect();
        self.enter_picker(ActivePicker::CompareOwn { target, picker: fenix_picker::PickerState::new(candidates) });
    }

    /// Opens the Compare page on `target` (with `own` as your side; HEAD
    /// when `None`) -- retargeting the one open for this repository.
    pub(crate) fn open_git_compare(&mut self, target: String, own: Option<String>) {
        let Some(root) = self.compare_root() else {
            self.set_error("comparing needs a git repository");
            return;
        };
        self.open_git_compare_in(root, target, own);
    }

    /// `open_git_compare`, for a repository already known.
    pub(crate) fn open_git_compare_in(&mut self, root: PathBuf, target: String, own: Option<String>) {
        let own = own.filter(|o| o != "HEAD");
        let id = match self.find_page(|m| matches!(m, PageModel::Compare(c) if c.root == root)) {
            Some(id) => {
                self.show_page(id);
                if let Some(page) = self.git_compare(id) {
                    page.retarget(target, own);
                }
                id
            }
            None => self.open_page(PageModel::Compare(Box::new(GitCompare::new(root, target, own)))),
        };
        self.git_compare_refresh(id);
    }

    pub(super) fn git_compare_refresh(&mut self, id: BufferId) {
        let Some(page) = self.git_compare(id) else { return };
        page.loading = true;
        let (root, range, target) = (page.root.clone(), page.range(), page.target.clone());
        let count_uncommitted = page.own.is_none() && !page.worktree;
        let open: Vec<DiffKey> = page.diffs.keys().cloned().collect();
        let commits: Vec<String> = page.open.keys().cloned().collect();
        self.page_spawn(move |send| {
            let data = read_compare(&root, range.clone(), &target, count_uncommitted);
            let files = data.files.clone().unwrap_or_default();
            send(PageEvent::CompareData { buffer: id, data: Box::new(data) });
            // What was open stays open, read again.
            for hash in commits {
                let files = fenix_git::commit_files(&root, &hash);
                send(PageEvent::CompareFiles { buffer: id, hash, files });
            }
            for key in open {
                let text = match &key {
                    DiffKey::Range(path) => match files.iter().find(|f| &f.path == path) {
                        Some(file) => fenix_git::changed_file_diff(&root, &range, file),
                        None => continue,
                    },
                    DiffKey::Commit { hash, path } => fenix_git::commit_file_diff(&root, hash, path),
                };
                send(PageEvent::CompareDiff { buffer: id, key, diff: diff_state(text) });
            }
        });
    }

    /// Reads every Compare page again -- after an operation moved a ref.
    pub(super) fn refresh_git_compares(&mut self) {
        let ids: Vec<BufferId> = self.pages.iter().filter(|(_, s)| matches!(s.model, PageModel::Compare(_))).map(|(id, _)| *id).collect();
        for id in ids {
            self.git_compare_refresh(id);
        }
    }

    pub(super) fn git_compare_action(&mut self, id: BufferId, action: CompareAction) {
        let Some(page) = self.git_compare(id) else { return };
        let (root, range, target) = (page.root.clone(), page.range(), page.target.clone());
        match action {
            CompareAction::None => {}
            CompareAction::Refresh => self.git_compare_refresh(id),
            CompareAction::Close => self.close_page(id),
            CompareAction::LoadCommit(hash) => self.page_spawn(move |send| {
                let files = fenix_git::commit_files(&root, &hash);
                send(PageEvent::CompareFiles { buffer: id, hash, files });
            }),
            CompareAction::LoadDiff(key) => {
                let file = match &key {
                    DiffKey::Range(path) => self.git_compare(id).and_then(|p| p.changed(path).cloned()),
                    DiffKey::Commit { .. } => None,
                };
                self.page_spawn(move |send| {
                    let text = match (&key, file) {
                        (DiffKey::Range(_), Some(file)) => fenix_git::changed_file_diff(&root, &range, &file),
                        (DiffKey::Range(path), None) => Err(format!("{path} isn't in the comparison any more")),
                        (DiffKey::Commit { hash, path }, _) => fenix_git::commit_file_diff(&root, hash, path),
                    };
                    send(PageEvent::CompareDiff { buffer: id, key, diff: diff_state(text) });
                });
            }
            CompareAction::OpenFile { path, line } => {
                let full = root.join(&path);
                if !full.is_file() {
                    self.set_error(format!("{path} isn't in the working tree -- o shows it as it is on {target}"));
                    return;
                }
                self.open_file_from_picker(&full);
                if let Some(line) = line {
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path: full, line, col: 1, text: String::new() });
                }
            }
            CompareAction::OpenAt { rev, path } => match fenix_git::show_at(&root, &rev, &path) {
                Ok(text) => {
                    let buffer = self.buffers.open_text_view_like(&text, Path::new(&path));
                    self.ref_view_names.insert(buffer, format!("{rev}:{path}"));
                    self.open_buffer_in_focused_pane(buffer);
                }
                Err(err) => self.set_error(err.trim().to_string()),
            },
            CompareAction::PickTarget => self.start_compare_picker(),
            CompareAction::PickOwn => self.start_compare_own_picker(&root, target),
            CompareAction::Fetch(remote) => self.page_spawn(move |send| {
                let result = fenix_git::fetch_remote(&root, &remote).map(|_| remote.clone()).map_err(|e| format!("fetching {remote} failed: {}", e.trim()));
                send(PageEvent::CompareFetched { buffer: id, result });
            }),
            CompareAction::Copy(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text.clone());
                }
                if let Some(page) = self.git_compare(id) {
                    page.message = Some((format!("copied {text}"), false));
                }
            }
        }
    }

    pub(super) fn apply_git_compare_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::CompareData { buffer, data } => {
                if let Some(page) = self.git_compare(buffer) {
                    page.set_data(*data);
                }
            }
            PageEvent::CompareFiles { buffer, hash, files } => {
                if let Some(page) = self.git_compare(buffer) {
                    page.set_files(hash, files);
                }
            }
            PageEvent::CompareDiff { buffer, key, diff } => {
                if let Some(page) = self.git_compare(buffer) {
                    page.set_diff(key, diff);
                }
            }
            PageEvent::CompareFetched { buffer, result } => {
                let Some(page) = self.git_compare(buffer) else { return };
                page.busy = None;
                page.message = Some(match result {
                    Ok(remote) => (format!("fetched {remote}"), false),
                    Err(err) => (err, true),
                });
                // Every page's view of the remote just changed, this one's too.
                self.refresh_git_pages(false);
            }
            _ => {}
        }
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

    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-compare-page-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]);
            git(&dir, &["config", "user.email", "t@example.com"]);
            git(&dir, &["config", "user.name", "T"]);
            git(&dir, &["config", "core.autocrlf", "false"]);
            std::fs::write(dir.join("a.txt"), "one\n").unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "initial"]);
            git(&dir, &["checkout", "-q", "-b", "side"]);
            std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
            git(&dir, &["commit", "-q", "-am", "on side"]);
            Repo(dir)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reading_a_comparison_counts_both_sides_and_whats_on_disk() {
        let repo = Repo::new("read");
        std::fs::write(repo.0.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let range = fenix_git::Range { base: "main".into(), head: fenix_git::Side::Rev("HEAD".into()), three_dot: true };
        let data = read_compare(&repo.0, range, "main", true);
        assert_eq!(data.branch, "side");
        assert_eq!((data.ahead.len(), data.behind.len()), (1, 0));
        assert_eq!(data.files.as_ref().unwrap().len(), 1);
        assert_eq!(data.files.as_ref().unwrap()[0].added, Some(1), "committed only: the third line isn't in it");
        assert_eq!(data.uncommitted, 1);
        assert_eq!(data.remote, None);
    }

    #[test]
    fn a_diff_that_failed_says_why() {
        assert!(matches!(diff_state(Err("\nfatal: bad revision\n".into())), DiffState::Empty(why) if why == "fatal: bad revision"));
    }
}
