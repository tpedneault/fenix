//! The host half of the interactive rebase page (`git_rebase`): working
//! out what a rebase would replay, a reword's message in the compose
//! buffer, and running the plan -- logged, so `U` takes the whole rebase
//! back.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::git_rebase::{RebaseAction, RebasePage};

impl App {
    fn git_rebase_page(&mut self, id: BufferId) -> Option<&mut RebasePage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Rebase(r)) => Some(r),
            _ => None,
        }
    }

    /// Opens the rebase page. `from` is the oldest commit to replay
    /// (it and everything after it); `None` replays what's not on the
    /// upstream yet -- or, with none, not on the base branch.
    pub(crate) fn open_rebase(&mut self, from: Option<String>) {
        let root = self.git_action_repo_root();
        let Some(root) = super::git_editor::repository_of(&root) else {
            self.set_error("not in a git repository");
            return;
        };
        let root = fenix_lsp::normalize(root);
        if let Some(op) = fenix_git::in_progress(&root) {
            self.set_error(format!("{} is still going -- finish or abort it first (r on the Git page)", op.label()));
            return;
        }
        let status = fenix_git::status(&root);
        let upstream = status.as_ref().and_then(|s| s.upstream.clone());
        let (base, label) = match from {
            Some(hash) => {
                let parent = fenix_git::oplog::rev(&root, &format!("{hash}^"));
                let label = if parent.is_empty() { "the root".to_string() } else { parent[..7.min(parent.len())].to_string() };
                (Some(parent).filter(|p| !p.is_empty()), label)
            }
            None => {
                let target = upstream.clone().or_else(|| fenix_git::resolve_base(&root, self.base_branch_for(&root).as_deref()));
                let Some(target) = target else {
                    self.set_error("no upstream or base branch to rebase from -- use i on a commit in the Log page (SPC g l)");
                    return;
                };
                (fenix_git::merge_base(&root, "HEAD", &target), target)
            }
        };
        let commits = match fenix_git::replayed(&root, base.as_deref()) {
            Ok(commits) if commits.is_empty() => {
                self.set_message("nothing to rebase -- no commits of your own after that");
                return;
            }
            Ok(commits) => commits,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let pushed = match &upstream {
            Some(upstream) => {
                let on_upstream: std::collections::HashSet<String> = fenix_git::reachable(&root, upstream, 2000).into_iter().collect();
                commits.iter().filter(|c| on_upstream.contains(&c.hash)).count()
            }
            None => 0,
        };
        let branch = status.map(|s| s.branch).unwrap_or_else(|| "HEAD".to_string());
        let page = RebasePage::new(root, branch, base, label, commits, pushed);
        self.open_page(PageModel::Rebase(Box::new(page)));
    }

    pub(super) fn git_rebase_action(&mut self, id: BufferId, action: RebaseAction) {
        let Some(root) = self.git_rebase_page(id).map(|r| r.root.clone()) else { return };
        match action {
            RebaseAction::None => {}
            RebaseAction::Close => self.close_page(id),
            RebaseAction::EditMessage { hash } => {
                let seed = fenix_git::commit_message(&root, &hash).unwrap_or_default();
                self.open_compose_seeded(ComposePurpose::RebaseMessage { page: id, hash }, &seed);
            }
            RebaseAction::ShowCommit(hash) => {
                let text = fenix_git::commit_diff(&root, &hash).unwrap_or_else(|e| e);
                let buffer = self.buffers.open_text_view(&text);
                self.open_buffer_in_focused_pane(buffer);
            }
            RebaseAction::Start { base, plan } => {
                let Some(page) = self.git_rebase_page(id) else { return };
                page.busy = true;
                let job = crate::git_status::Job::RebasePlan { base, plan };
                let label = job.label();
                self.page_spawn(move |send| {
                    let result = super::git_page::run_logged(&root, &job);
                    send(PageEvent::GitDone { buffer: id, label, result });
                });
            }
        }
    }

    /// A reword's message, written in the compose buffer.
    pub(super) fn git_rebase_message(&mut self, page: BufferId, hash: String, body: String) {
        self.close_compose();
        if let Some(r) = self.git_rebase_page(page) {
            r.set_message(&hash, body);
        }
        if self.pages.contains_key(&page) {
            self.show_page(page);
        }
    }

    /// The rebase ran: close the page and show where the branch is now.
    pub(super) fn apply_git_rebase_done(&mut self, id: BufferId, label: String, result: Result<String, String>) {
        let root = self.git_rebase_page(id).map(|r| r.root.clone());
        self.close_page(id);
        let Some(root) = root else { return };
        let stopped = fenix_git::in_progress(&root).map(|op| op.label());
        match (&result, stopped) {
            (_, Some(op)) => self.set_message(format!("{op} -- the rebase stopped to let you amend or resolve; r c on the Git page carries on")),
            (Ok(_), None) => self.set_message(format!("{label} ✓ -- U on the Git page takes it back")),
            (Err(err), None) => self.set_error(format!("{label} failed: {}", err.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("hint:")).unwrap_or(""))),
        }
        self.git_refresh_all_views();
        self.open_git_status();
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
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `main` with one commit, and a `topic` branch on top: a change, an
    /// unrelated one, and a fixup for the first.
    fn repo(name: &str) -> Repo {
        let dir = std::env::temp_dir().join(format!("fenix-git-rebase-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@e.com"], &["config", "user.name", "T"], &["config", "core.autocrlf", "false"]] {
            git(&dir, args);
        }
        let commit = |file: &str, text: &str, message: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", message]);
        };
        commit("a.txt", "a\n", "base");
        git(&dir, &["switch", "-q", "-c", "topic"]);
        commit("b.txt", "b\n", "add b");
        commit("c.txt", "c\n", "add c");
        commit("b.txt", "b, fixed\n", "fixup! add b");
        Repo(dir)
    }

    #[test]
    fn r_i_on_the_git_page_rebases_from_the_base_with_the_fixup_folded_in() {
        let repo = repo("status");
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        app.open_git_status();
        for c in ['r', 'i'] {
            assert!(app.page_key(KeyPress::char(c)));
        }
        let id = app.focused_buffer_id();
        let (pane, _) = (app.focused_pane_id(), ());
        app.ensure_page_layout(id, pane, 110);
        let text = app.open().buffer.text();
        assert!(text.contains("Rebase topic onto main") && text.contains("fixup") && text.contains("RESULT · 2 COMMITS, WAS 3"), "{text}");
        let ctrl_c = KeyPress::char('c').with_ctrl();
        assert!(app.page_key(ctrl_c));
        assert!(app.page_key(ctrl_c));
        assert_eq!(git(&repo.0, &["log", "--format=%s"]), "add c\nadd b\nbase");
        assert_eq!(git(&repo.0, &["show", "HEAD~1:b.txt"]), "b, fixed");
        assert!(fenix_git::oplog::entries(&repo.0, 1)[0].undo.possible(), "undoable");
    }

    #[test]
    fn a_reword_takes_its_message_from_the_compose_buffer() {
        let repo = repo("reword");
        let mut app = App::with_file(Some(repo.0.join("a.txt").to_string_lossy().into_owned()));
        let add_c = git(&repo.0, &["rev-parse", "HEAD~1"]);
        app.open_rebase(Some(add_c));
        assert!(app.page_key(KeyPress::char('j')), "down to add c (the fixup is the newest)");
        assert!(app.page_key(KeyPress::char('r')));
        let compose = app.compose.as_ref().expect("the message editor opened").buffer;
        if let Some(ob) = app.buffers.get_mut(compose) {
            let end = ob.buffer.len_chars();
            let mut c = Cursor::at_start();
            ob.buffer.replace_range(&mut c, 0, end, "Add c, properly");
        }
        app.compose_submit();
        let ctrl_c = KeyPress::char('c').with_ctrl();
        assert!(app.page_key(ctrl_c));
        assert!(app.page_key(ctrl_c));
        assert_eq!(git(&repo.0, &["log", "--format=%s", "-2"]), "fixup! add b\nAdd c, properly");
    }
}
