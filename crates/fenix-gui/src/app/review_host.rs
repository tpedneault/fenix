//! The host half of the review pages (`review_inbox`, `review_page`):
//! everything that talks to the forge, off the UI thread; what a review
//! remembers (`review_store`); reviewing in a worktree of its own; and
//! the compose buffer for comments, replies and the summary.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::review_inbox::{Entry, Inbox, InboxAction};
use crate::review_page::{FileView, ReviewAction, ReviewData, ReviewPage};
use crate::review_store::{self, Pending};
use fenix_forge::{Forge, Verdict};

/// What to do once a write to the forge has come back.
#[derive(Debug, Clone, PartialEq)]
pub enum After {
    Refresh,
    /// A review went out, at this head.
    Submitted(String),
    Merged,
}

/// "GitHub" or "GitLab", for a heading.
pub(super) fn forge_name(root: &Path) -> String {
    match fenix_git::remote_url(root, "origin") {
        Some(url) if fenix_github::repository(&url).is_some() => "GitHub".to_string(),
        _ => "GitLab".to_string(),
    }
}

fn file_views(files: &[fenix_forge::ChangedFile]) -> Vec<FileView> {
    files.iter().map(|f| FileView::new(f.display_path().to_string(), f.old_path.clone(), f.change.letter(), &f.unified_diff())).collect()
}

/// Everything the review page shows, in one pass on a worker thread.
fn read_review(client: &dyn Forge, root: &Path, number: u64) -> Result<ReviewData, String> {
    let request = client.merge_request(number)?;
    // The head, fetched into the repository: what `i` diffs against,
    // and what a worktree checks out.
    if let Some(remote) = fenix_git::default_remote(root) {
        let _ = fenix_git::fetch_refspec(root, &remote, &client.checkout_refspec(number));
    }
    let approvals = client.approvals(number).ok();
    let (files, error) = match client.changed_files(number) {
        Ok(files) => (file_views(&files), None),
        Err(err) => (Vec::new(), Some(format!("couldn't read the files: {err}"))),
    };
    let threads = client.discussions(number).unwrap_or_default();
    let checks = client.checks(number, &request.sha).unwrap_or_default();
    Ok(ReviewData { request, approvals, files, threads, checks, since: None, error })
}

/// What changed between the head you reviewed and the head now -- just
/// the author's edits, even across a rebase onto a moved target: when
/// the old head isn't in the new one's history, only the files the
/// request touches are compared.
fn read_since(root: &Path, reviewed: &str, head: &str, paths: &[String]) -> Result<Vec<FileView>, String> {
    let rebased = !fenix_git::contains(root, head, reviewed);
    let mut args: Vec<String> = vec!["diff".into(), "--no-color".into(), reviewed.into(), head.into()];
    if rebased {
        args.push("--".into());
        args.extend(paths.iter().cloned());
    }
    let out = std::process::Command::new("git").current_dir(root).args(&args).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("couldn't diff against your last review: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    Ok(fenix_diff::parse(&text)
        .into_iter()
        .map(|d| {
            let path = d.display_path().to_string();
            let letter = match d.status {
                fenix_diff::FileStatus::Added => 'A',
                fenix_diff::FileStatus::Deleted => 'D',
                fenix_diff::FileStatus::Renamed => 'R',
                fenix_diff::FileStatus::Modified => 'M',
            };
            let mut raw = d.header.join("\n");
            raw.push('\n');
            for hunk in &d.hunks {
                raw.push_str(&hunk.header);
                raw.push('\n');
                for l in &hunk.lines {
                    raw.push_str(&l.raw());
                    raw.push('\n');
                }
            }
            FileView::new(path, if d.old_path == "/dev/null" { d.new_path.clone() } else { d.old_path.clone() }, letter, &raw)
        })
        .collect())
}

impl App {
    fn review_page(&mut self, id: BufferId) -> Option<&mut ReviewPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Review(r)) => Some(r),
            _ => None,
        }
    }

    fn inbox(&mut self, id: BufferId) -> Option<&mut Inbox> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Inbox(i)) => Some(i),
            _ => None,
        }
    }

    /// The repository the review pages act on: the focused file's.
    pub(super) fn review_root(&self) -> Option<PathBuf> {
        // Resolved first: a file opened as `decoder.py` has an empty
        // parent, which names no folder at all.
        let file = self.open().buffer.path().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        let start = match file.as_deref().and_then(Path::parent).filter(|p| !p.as_os_str().is_empty()) {
            Some(folder) => folder.to_path_buf(),
            None => self.git_action_repo_root(),
        };
        super::git_editor::repository_of(&start).map(fenix_lsp::normalize)
    }

    /// `SPC g M`: the review inbox -- or, with `[git] layout = panes`, the
    /// older Merge Requests view.
    pub(crate) fn open_reviews(&mut self) {
        if self.config.git_layout.as_deref() == Some("panes") {
            self.open_forge_view();
            return;
        }
        let Some(root) = self.review_root() else {
            self.set_error("not in a git repository");
            return;
        };
        let project = match self.forge_client(&root) {
            Ok(client) => client.project().to_string(),
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let id = match self.find_page(|m| matches!(m, PageModel::Inbox(i) if i.root == root)) {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => self.open_page(PageModel::Inbox(Box::new(Inbox::new(root.clone(), forge_name(&root), project)))),
        };
        self.inbox_refresh(id);
    }

    fn inbox_refresh(&mut self, id: BufferId) {
        let Some(inbox) = self.inbox(id) else { return };
        inbox.loading = true;
        let (root, filter, project) = (inbox.root.clone(), inbox.filter, inbox.project.clone());
        let client = match self.forge_client(&root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        self.page_spawn(move |send| {
            let result = client.list_merge_requests(filter).map(|list| {
                list.into_iter()
                    .map(|request| {
                        let state = review_store::load(&root, &project, request.number);
                        let new = state.seen.as_deref() != Some(request.updated_at.as_str());
                        Entry { pending: state.pending.len(), new, request }
                    })
                    .collect()
            });
            send(PageEvent::InboxData { buffer: id, result });
        });
    }

    pub(super) fn inbox_action(&mut self, id: BufferId, action: InboxAction) {
        let Some(root) = self.inbox(id).map(|i| i.root.clone()) else { return };
        match action {
            InboxAction::None => {}
            InboxAction::Refresh => self.inbox_refresh(id),
            InboxAction::Close => self.close_page(id),
            InboxAction::Open(number) => self.open_review(root, number, false),
            InboxAction::Worktree(number) => self.review_in_worktree(root, number),
            InboxAction::CheckOut(number) => self.review_check_out(root, number),
            InboxAction::Browser(url) => self.open_url(&url),
        }
    }

    /// Opens the review page for request `number` of the repository at
    /// `root` -- the one already open for it, or a new one.
    pub(crate) fn open_review(&mut self, root: PathBuf, number: u64, in_worktree: bool) {
        let (forge, project) = match self.forge_client(&root) {
            Ok(client) => (forge_name(&root), client.project().to_string()),
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let id = match self.find_page(|m| matches!(m, PageModel::Review(r) if r.root == root && r.number == number)) {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => {
                let state = review_store::load(&root, &project, number);
                let mut page = ReviewPage::new(root, forge, project, number, state);
                page.in_worktree = in_worktree;
                self.open_page(PageModel::Review(Box::new(page)))
            }
        };
        self.review_refresh(id);
    }

    fn review_refresh(&mut self, id: BufferId) {
        let Some(page) = self.review_page(id) else { return };
        page.loading = true;
        let (root, number) = (page.root.clone(), page.number);
        let client = match self.forge_client(&root) {
            Ok(client) => client,
            Err(err) => {
                if let Some(page) = self.review_page(id) {
                    page.message = Some((err, true));
                }
                return;
            }
        };
        self.page_spawn(move |send| {
            let result = read_review(client.as_ref(), &root, number);
            send(PageEvent::ReviewData { buffer: id, result: result.map(Box::new) });
        });
    }

    /// Runs a write against the forge for page `id`, then `after`.
    fn review_write(&mut self, id: BufferId, label: &str, after: After, job: impl FnOnce(&dyn Forge) -> Result<(), String> + Send + 'static) {
        let Some(page) = self.review_page(id) else { return };
        if let Some(busy) = &page.busy {
            page.message = Some((format!("still busy with {busy}"), true));
            return;
        }
        page.busy = Some(label.to_string());
        let root = page.root.clone();
        let client = match self.forge_client(&root) {
            Ok(client) => client,
            Err(err) => {
                if let Some(page) = self.review_page(id) {
                    page.busy = None;
                    page.message = Some((err, true));
                }
                return;
            }
        };
        let label = label.to_string();
        self.page_spawn(move |send| {
            let result = job(client.as_ref());
            send(PageEvent::ReviewDone { buffer: id, label, after, result });
        });
    }

    fn save_review(&mut self, id: BufferId) {
        let Some(page) = self.review_page(id) else { return };
        if let Err(err) = review_store::save(&page.root, &page.project, page.number, &page.state) {
            page.message = Some((format!("couldn't remember the review: {err}"), true));
        }
    }

    pub(super) fn review_action(&mut self, id: BufferId, action: ReviewAction) {
        let Some(page) = self.review_page(id) else { return };
        let (root, number) = (page.root.clone(), page.number);
        let url = page.data.as_ref().map(|d| d.request.web_url.clone()).unwrap_or_default();
        match action {
            ReviewAction::None => {}
            ReviewAction::Refresh => self.review_refresh(id),
            ReviewAction::Close => self.close_page(id),
            ReviewAction::Save => self.save_review(id),
            ReviewAction::Comment { pending, index } => {
                let seed = pending.body.clone();
                self.open_compose_seeded(ComposePurpose::ReviewPending { page: id, pending: Box::new(pending), index }, &seed);
            }
            ReviewAction::Reply(thread) => self.open_compose(ComposePurpose::ReviewReply { page: id, thread }),
            ReviewAction::Summary => {
                let seed = page.summary.clone();
                self.open_compose_seeded(ComposePurpose::ReviewSummary { page: id }, &seed);
            }
            ReviewAction::Resolve { thread, resolved } => {
                let label = if resolved { "resolve the thread" } else { "reopen the thread" };
                self.review_write(id, label, After::Refresh, move |c| c.resolve(number, &thread, resolved));
            }
            ReviewAction::Submit { verdict, body } => {
                let Some(data) = &page.data else { return };
                let head = data.request.sha.clone();
                let drafts: Vec<fenix_forge::DraftComment> = page.state.pending.iter().map(|p| p.draft(&data.request.diff_refs)).collect();
                if drafts.is_empty() && body.trim().is_empty() && verdict == Verdict::Comment {
                    page.message = Some(("nothing to send -- write a comment (C) or a summary (e), or pick approve".to_string(), true));
                    return;
                }
                let label = format!("submit your review ({})", verdict.label());
                let sent = head.clone();
                self.review_write(id, &label, After::Submitted(head), move |c| c.submit_review(number, &sent, verdict, &body, &drafts));
            }
            ReviewAction::Merge(options) => self.review_write(id, "merge", After::Merged, move |c| c.merge(number, &options)),
            ReviewAction::OpenFile { path, line } => {
                let full = root.join(&path);
                if !full.is_file() {
                    self.set_error(format!("{path} isn't in this checkout -- w reviews in a worktree with it"));
                    return;
                }
                self.open_file_from_picker(&full);
                if let Some(line) = line {
                    self.jump_to_grep_match(&fenix_project::GrepMatch { path: full, line, col: 1, text: String::new() });
                }
            }
            ReviewAction::OpenBrowser => self.open_url(&url),
            ReviewAction::CopyUrl => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(url);
                }
                self.set_message("copied the link");
            }
            ReviewAction::ReviewInWorktree => self.review_in_worktree(root, number),
            ReviewAction::CheckOutHere => self.review_check_out(root, number),
            ReviewAction::ShowLog(check) => {
                let Ok(client) = self.forge_client(&root) else { return };
                self.page_spawn(move |send| {
                    let result = client.job_log(&check);
                    send(PageEvent::ReviewLog { buffer: id, name: check.name.clone(), result });
                });
            }
            ReviewAction::Retry(check) => {
                let label = format!("rerun {}", check.name);
                self.review_write(id, &label, After::Refresh, move |c| c.retry(&check));
            }
            ReviewAction::LoadSince => {
                let Some(data) = &page.data else { return };
                let (Some(reviewed), head) = (page.state.reviewed_head.clone(), data.request.sha.clone()) else { return };
                let paths: Vec<String> = data.files.iter().flat_map(|f| [f.path.clone(), f.old_path.clone()]).collect();
                self.page_spawn(move |send| {
                    let result = read_since(&root, &reviewed, &head, &paths);
                    send(PageEvent::ReviewSince { buffer: id, result });
                });
            }
        }
    }

    /// `w`: fetches the request and checks it out beside the repository,
    /// as a workspace of its own, with its review page open there -- so
    /// your own branch and buffers stay exactly as they were.
    fn review_in_worktree(&mut self, root: PathBuf, number: u64) {
        let client = match self.forge_client(&root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let branch = format!("review/{number}");
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".to_string());
        let path = root.parent().map(|p| p.join(format!("{name}.review-{number}"))).unwrap_or_else(|| PathBuf::from(format!("{name}.review-{number}")));
        if !path.is_dir() {
            let Some(remote) = fenix_git::default_remote(&root) else {
                self.set_error("no remote to fetch the request from");
                return;
            };
            let refspec = client.checkout_refspec(number);
            let head = refspec.split(':').next().unwrap_or(&refspec).to_string();
            if let Err(err) = fenix_git::fetch_refspec(&root, &remote, &format!("{head}:refs/heads/{branch}")) {
                self.set_error(format!("couldn't fetch {number}: {}", err.lines().next().unwrap_or("")));
                return;
            }
            if let Err(err) = fenix_git::add_worktree(&root, &path, &branch, None) {
                self.set_error(format!("couldn't make the worktree: {}", err.lines().next().unwrap_or("")));
                return;
            }
        }
        let path = std::fs::canonicalize(&path).map(fenix_lsp::normalize).unwrap_or(path);
        self.open_project(path.clone(), None, false);
        self.open_review(path, number, true);
    }

    /// `c`: checks the request out in this checkout, as `mr-N`.
    fn review_check_out(&mut self, root: PathBuf, number: u64) {
        let client = match self.forge_client(&root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let Some(remote) = fenix_git::default_remote(&root) else {
            self.set_error("no remote to fetch the request from");
            return;
        };
        let result = fenix_git::fetch_refspec(&root, &remote, &client.checkout_refspec(number)).and_then(|_| fenix_git::checkout_branch(&root, &format!("mr-{number}")));
        self.run_git_operation(&format!("check out mr-{number}"), result);
    }

    /// The compose buffer's text, sent where it was written for.
    pub(super) fn review_compose(&mut self, purpose: ComposePurpose, body: String) {
        self.close_compose();
        match purpose {
            ComposePurpose::ReviewPending { page, pending, index } => {
                let Some(r) = self.review_page(page) else { return };
                let pending = Pending { body, ..*pending };
                match index {
                    Some(i) if i < r.state.pending.len() => r.state.pending[i] = pending,
                    _ => r.state.pending.push(pending),
                }
                r.message = Some(("held for your review -- s submits it with the rest".to_string(), false));
                self.save_review(page);
                self.show_page(page);
            }
            ComposePurpose::ReviewReply { page, thread } => {
                self.show_page(page);
                let Some(number) = self.review_page(page).map(|r| r.number) else { return };
                self.review_write(page, "reply", After::Refresh, move |c| c.reply(number, &thread, &body));
            }
            ComposePurpose::ReviewSummary { page } => {
                if let Some(r) = self.review_page(page) {
                    r.summary = body;
                }
                self.show_page(page);
            }
            _ => {}
        }
    }

    pub(super) fn apply_review_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::InboxData { buffer, result } => {
                let Some(inbox) = self.inbox(buffer) else { return };
                match result {
                    Ok(entries) => inbox.set_entries(entries),
                    Err(err) => {
                        inbox.loading = false;
                        inbox.entries.get_or_insert_with(Vec::new);
                        inbox.message = Some((err, true));
                    }
                }
            }
            PageEvent::ReviewData { buffer, result } => {
                let Some(page) = self.review_page(buffer) else { return };
                match result {
                    Ok(data) => {
                        page.state.seen = Some(data.request.updated_at.clone());
                        page.set_data(*data);
                        self.save_review(buffer);
                    }
                    Err(err) => {
                        page.loading = false;
                        page.message = Some((err, true));
                    }
                }
            }
            PageEvent::ReviewSince { buffer, result } => {
                let Some(page) = self.review_page(buffer) else { return };
                match result {
                    Ok(files) => {
                        if let Some(data) = &mut page.data {
                            data.since = Some(files);
                        }
                    }
                    Err(err) => {
                        page.since_review = false;
                        page.message = Some((err, true));
                    }
                }
            }
            PageEvent::ReviewDone { buffer, label, after, result } => {
                let Some(page) = self.review_page(buffer) else { return };
                page.busy = None;
                match result {
                    Err(err) => page.message = Some((format!("{label} failed: {err}"), true)),
                    Ok(()) => {
                        page.message = Some((format!("{label} ✓"), false));
                        match after {
                            After::Submitted(head) => {
                                page.state.pending.clear();
                                page.state.reviewed_head = Some(head);
                                page.summary.clear();
                                self.save_review(buffer);
                            }
                            After::Merged => {
                                let (root, number, worktree) = (page.root.clone(), page.number, page.in_worktree);
                                page.message = Some((
                                    if worktree { "merged ✓ -- q closes this; w d on the Git page removes the review worktree" } else { "merged ✓" }.to_string(),
                                    false,
                                ));
                                let _ = (root, number);
                            }
                            After::Refresh => {}
                        }
                        self.review_refresh(buffer);
                    }
                }
            }
            PageEvent::ReviewLog { buffer, name, result } => match result {
                Ok(log) => {
                    let root = self.review_page(buffer).map(|r| r.root.clone());
                    let tail: Vec<&str> = log.lines().rev().take(400).collect::<Vec<_>>().into_iter().rev().map(without_timestamp).collect();
                    // Lines that name a file and line are the quickfix list.
                    let found: Vec<fenix_project::GrepMatch> = tail
                        .iter()
                        .filter_map(|l| fenix_tasks::parse_line(l).location)
                        .map(|loc| {
                            let path = match &root {
                                Some(root) if loc.path.is_relative() => root.join(&loc.path),
                                _ => loc.path.clone(),
                            };
                            fenix_project::GrepMatch { path, line: loc.line, col: loc.col, text: loc.message }
                        })
                        .collect();
                    let text = format!("{name} -- the last {} lines of its log\n\n{}", tail.len(), tail.join("\n"));
                    let view = self.buffers.open_text_view(&text);
                    self.open_buffer_in_focused_pane(view);
                    if !found.is_empty() {
                        self.set_message(format!("{} in the log -- SPC p n walks them", count_locations(found.len())));
                        self.quickfix = found.into_iter().map(QuickfixEntry::Task).collect();
                        self.quickfix_index = None;
                    }
                }
                Err(err) => {
                    if let Some(page) = self.review_page(buffer) {
                        page.message = Some((format!("couldn't read {name}'s log: {err}"), true));
                    }
                }
            },
            _ => {}
        }
    }
}

/// A CI log line without the timestamp GitHub Actions puts in front of
/// every one (`2026-09-24T20:28:05.1234567Z `), which would stop a
/// `file:line` from being recognised.
fn without_timestamp(line: &str) -> &str {
    let bytes = line.as_bytes();
    let stamped = bytes.len() > 20 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-' && bytes[10] == b'T';
    match (stamped, line.find("Z ")) {
        (true, Some(end)) if end < 40 => &line[end + 2..],
        _ => line,
    }
}

fn count_locations(n: usize) -> String {
    if n == 1 { "1 file location".to_string() } else { format!("{n} file locations") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_page::Row;

    /// The sandbox repository, cloned fresh (`FENIX_GITHUB_SANDBOX`,
    /// default `tpedneault/fenix-review-sandbox`, with an open pull
    /// request from `feature/pus17` -- see `fenix-github`'s live tests).
    fn sandbox(name: &str) -> PathBuf {
        let repo = std::env::var("FENIX_GITHUB_SANDBOX").unwrap_or_else(|_| "tpedneault/fenix-review-sandbox".to_string());
        let dir = std::env::temp_dir().join(format!("fenix-review-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let status = std::process::Command::new("git").args(["clone", "-q", &format!("https://github.com/{repo}.git"), &dir.to_string_lossy()]).status().unwrap();
        assert!(status.success(), "clone {repo}");
        fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap())
    }

    #[test]
    fn a_file_opened_by_a_relative_path_still_names_its_repository() {
        // Tests run in the crate's folder, inside Fenix's own repository.
        let app = App::with_file(Some("Cargo.toml".to_string()));
        let root = app.review_root().expect("the repository");
        assert!(root.join("crates").join("fenix-gui").is_dir(), "{}", root.display());
    }

    #[test]
    fn a_ci_timestamp_comes_off_but_an_ordinary_line_is_left_alone() {
        assert_eq!(without_timestamp("2026-09-24T20:28:05.1234567Z test_decoder.py:12: in test"), "test_decoder.py:12: in test");
        assert_eq!(without_timestamp("src/a.rs:3:1: error"), "src/a.rs:3:1: error");
        assert_eq!(without_timestamp("2026 was a year"), "2026 was a year");
    }

    #[test]
    #[ignore]
    fn live_a_failing_checks_log_fills_the_quickfix_list_and_i_shows_what_changed_since_a_review() {
        let root = sandbox("checks");
        let mut app = App::with_file(Some(root.join("decoder.py").to_string_lossy().into_owned()));
        let client = app.forge_client(&root).unwrap();
        let pr = client.request_for_branch("feature/pus17").unwrap().expect("the sandbox's pull request");
        // As if it had been reviewed one commit before the branch's tip.
        let mut state = review_store::load(&root, client.project(), pr.number);
        std::process::Command::new("git").current_dir(&root).args(["fetch", "-q", "origin", "feature/pus17"]).status().unwrap();
        let older = String::from_utf8(std::process::Command::new("git").current_dir(&root).args(["rev-parse", "FETCH_HEAD~1"]).output().unwrap().stdout).unwrap();
        state.reviewed_head = Some(older.trim().to_string());
        review_store::save(&root, client.project(), pr.number, &state).unwrap();
        app.open_review(root.clone(), pr.number, false);

        assert!(app.page_key(KeyPress::char('i')));
        let shown = text(&mut app);
        assert!(shown.contains("since your review at") && shown.contains("test.yml"), "the CI workflow came after that review:
{shown}");
        assert!(app.page_key(KeyPress::char('i')), "back to the whole change");

        assert!(app.page_key(KeyPress::char('K')));
        let page = page_mut(&mut app);
        page.cursor = page.rows().iter().position(|r| matches!(r, Row::Check(_))).expect("a check");
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        assert!(app.open().buffer.text().contains("pytest -- the last"), "the log opened");
        assert!(app.quickfix.iter().any(|q| matches!(q, QuickfixEntry::Task(m) if m.path.ends_with("test_decoder.py"))), "{:?}", app.quickfix);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn page_mut(app: &mut App) -> &mut ReviewPage {
        let id = app.focused_buffer_id();
        match app.pages.get_mut(&id).map(|s| &mut s.model) {
            Some(PageModel::Review(r)) => r,
            _ => panic!("the review page isn't focused"),
        }
    }

    fn text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 140);
        app.open().buffer.text()
    }

    #[test]
    #[ignore]
    fn live_a_github_review_is_read_written_and_submitted_from_fenix() {
        let root = sandbox("github");
        let mut app = App::with_file(Some(root.join("decoder.py").to_string_lossy().into_owned()));
        app.open_reviews();
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Tab)), "to the ones you opened");
        let inbox = text(&mut app);
        assert!(inbox.contains("Decode PUS-17 connection tests"), "{inbox}");
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        let shown = text(&mut app);
        assert!(shown.contains("feature/pus17 → main") && shown.contains("decoder.py"), "{shown}");

        let page = page_mut(&mut app);
        let before = page.data.as_ref().unwrap().threads.len();
        page.cursor = page.rows().iter().position(|r| *r == Row::File("decoder.py".into())).unwrap();
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Tab)));
        let page = page_mut(&mut app);
        page.cursor = page
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Line { path, .. } if path == "decoder.py") && page.rows().iter().any(|_| true))
            .unwrap()
            + 2;
        assert!(app.page_key(KeyPress::char('C')));
        let compose = app.compose.as_ref().expect("the comment editor opened").buffer;
        if let Some(ob) = app.buffers.get_mut(compose) {
            let mut c = Cursor::at_start();
            ob.buffer.replace_range(&mut c, 0, 0, "Left from Fenix's live test.");
        }
        app.compose_submit();
        assert_eq!(page_mut(&mut app).state.pending.len(), 1, "held, not sent");
        assert!(text(&mut app).contains("PENDING you Left from Fenix's live test."));

        for key in [KeyPress::char('s'), KeyPress::char('c'), KeyPress::char('c').with_ctrl(), KeyPress::char('c').with_ctrl()] {
            assert!(app.page_key(key));
        }
        let page = page_mut(&mut app);
        assert!(page.state.pending.is_empty(), "{:?}", page.message);
        assert!(page.state.reviewed_head.is_some());
        let threads = &page.data.as_ref().unwrap().threads;
        assert!(threads.len() > before && threads.iter().any(|t| t.first().is_some_and(|n| n.body.contains("Fenix's live test"))), "on GitHub now");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The dev GitLab's seeded project, cloned fresh (see `dev/gitlab`).
    fn gitlab_widget(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-review-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let url = "http://root:fenix-dev-token-0123456789@localhost:8929/fenix-dev/widget.git";
        let status = std::process::Command::new("git").args(["clone", "-q", url, &dir.to_string_lossy()]).status().unwrap();
        assert!(status.success(), "clone the dev GitLab's widget -- is the container up?");
        fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap())
    }

    #[test]
    #[ignore]
    fn live_a_gitlab_review_is_read_written_and_submitted_from_fenix() {
        let root = gitlab_widget("gitlab");
        let mut app = App::with_file(Some(root.join("widget.rs").to_string_lossy().into_owned()));
        app.config.gitlab_base_url = Some("http://localhost:8929".into());
        app.config.gitlab_token = Some("fenix-dev-token-0123456789".into());
        app.config.git_layout = None;
        app.open_reviews();
        for _ in 0..2 {
            assert!(app.page_key(KeyPress::named(FenixNamedKey::Tab)), "to every open one");
        }
        let inbox = text(&mut app);
        assert!(inbox.contains("Make the timeout configurable"), "{inbox}");
        let number = match app.pages.get(&app.focused_buffer_id()).map(|s| &s.model) {
            Some(PageModel::Inbox(i)) => i.entries.as_ref().unwrap().iter().find(|e| e.request.title.contains("timeout")).unwrap().request.number,
            _ => panic!(),
        };
        app.open_review(root.clone(), number, false);
        let shown = text(&mut app);
        assert!(shown.contains("feature/configurable-timeout → main") && shown.contains("widget.rs"), "{shown}");

        let page = page_mut(&mut app);
        let before = page.data.as_ref().unwrap().threads.len();
        page.cursor = page.rows().iter().position(|r| *r == Row::File("widget.rs".into())).unwrap();
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Tab)));
        let page = page_mut(&mut app);
        let rows = page.rows();
        page.cursor = rows
            .iter()
            .position(|r| matches!(r, Row::Line { path, hunk, line } if path == "widget.rs" && page.data.as_ref().unwrap().files[0].diff.as_ref().unwrap().hunks[*hunk].lines[*line].kind == fenix_diff::LineKind::Added))
            .unwrap();
        assert!(app.page_key(KeyPress::char('C')));
        let compose = app.compose.as_ref().unwrap().buffer;
        if let Some(ob) = app.buffers.get_mut(compose) {
            let mut c = Cursor::at_start();
            ob.buffer.replace_range(&mut c, 0, 0, "From Fenix's GitLab live test.");
        }
        app.compose_submit();
        for key in [KeyPress::char('s'), KeyPress::char('c'), KeyPress::char('c').with_ctrl(), KeyPress::char('c').with_ctrl()] {
            assert!(app.page_key(key));
        }
        let page = page_mut(&mut app);
        assert!(page.state.pending.is_empty(), "{:?}", page.message);
        let threads = &page.data.as_ref().unwrap().threads;
        assert!(threads.len() > before && threads.iter().any(|t| t.first().is_some_and(|n| n.body.contains("GitLab live test"))), "on GitLab now");
        let _ = std::fs::remove_dir_all(&root);
    }
}
