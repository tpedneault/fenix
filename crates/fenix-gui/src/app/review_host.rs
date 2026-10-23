//! The host half of the review pages (`review_inbox`, `review_page`):
//! everything that talks to the forge, off the UI thread; what a review
//! remembers (`review_store`); reviewing in a worktree of its own; and
//! the compose buffer for comments, replies and the summary.

use super::feature::page_as;
use super::pages::{PageEvent, PageModel};
use super::*;
use crate::forge_panel;
use crate::review_inbox::{Entry, Inbox, InboxAction};
use crate::review_page::{FileView, ReviewAction, ReviewData, ReviewPage};
use crate::review_store::{self, Pending};
use fenix_forge::{Forge, Verdict};

/// What the review pages' background jobs send back (as the review
/// feature's page events).
#[derive(Debug)]
pub(crate) enum ReviewJob {
    InboxData { buffer: BufferId, result: Result<Vec<crate::review_inbox::Entry>, String> },
    ReviewData { buffer: BufferId, result: Result<Box<crate::review_page::ReviewData>, String> },
    ReviewSince { buffer: BufferId, result: Result<Vec<crate::review_page::FileView>, String> },
    ReviewDone { buffer: BufferId, label: String, after: super::review_host::After, result: Result<(), String> },
    ReviewLog { buffer: BufferId, name: String, result: Result<String, String> },
}

/// Code review on GitHub and GitLab: the reviews (`SPC g M`) and opening
/// a pull request (`SPC g P`). One feature while both forges share this
/// code; they become `fenix.github` and `fenix.gitlab` over a `forge`
/// provider once there's a contribution point for it. Core Git stays.
pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::Bind;
    super::feature::FeatureSpec {
        id: "fenix.review",
        name: "Review",
        about: "GitHub pull requests and GitLab merge requests",
        commands: &[
            C { id: "git.pull_request", description: "Open a pull (merge) request for this branch, prefilled from its commits", external: false, run: |app| app.open_new_request() },
            C { id: "git.merge_requests", description: "Open the Merge Requests view (GitLab)", external: false, run: |app| app.open_reviews() },
            C { id: "git.merge_requests_close", description: "Close the Merge Requests view", external: false, run: |app| app.forge_close() },
        ],
        leader: &[
            // `M`, not `m` -- `SPC g m` is the merge *operation*, and the two
            // are asked for in completely different moods.
            Bind(&['g', 'P'], "open a pull request", "git.pull_request"),
            Bind(&['g', 'M'], "reviews", "git.merge_requests"),
        ],
        local_menu: &[],
        local_context: None,
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        picker_confirm: None,
        picker_cancel: None,
        modeline_clock: None,
        // What a key on the inbox, a review or a new request asked for.
        page_action: Some(|app, id, action| {
            let action = match action.downcast::<InboxAction>() {
                Ok(action) => return app.inbox_action(id, *action),
                Err(action) => action,
            };
            let action = match action.downcast::<ReviewAction>() {
                Ok(action) => return app.review_action(id, *action),
                Err(action) => action,
            };
            if let Ok(action) = action.downcast::<crate::git_request::RequestAction>() {
                app.request_action(id, *action);
            }
        }),
        page_context: None,
        page_title: None,
        page_after_layout: None,
        on_event: Some(|app, event| {
            let event = match event.downcast::<ReviewJob>() {
                Ok(job) => return app.apply_review_event(*job),
                Err(event) => event,
            };
            let event = match event.downcast::<super::git_request_page::RequestJob>() {
                Ok(job) => return app.apply_request_event(*job),
                Err(event) => event,
            };
            match event.downcast::<ForgeEvent>().map(|e| *e) {
                Ok(ForgeEvent::List { request_id, result }) => app.apply_forge_list(request_id, result),
                Ok(ForgeEvent::Detail { request_id, number, result }) => app.apply_forge_detail(request_id, number, *result),
                Err(_) => {}
            }
        }),
        settings: &[],
        services: &[],
        contributes: &[],
        session_documents: None,
        session_reopen: None,
        home: None,
        home_activate: None,
        doctor: None,
        opener: None,
        lsp_first: None,
        prompt_key: None,
        wake: None,
        links: None,
        on_start: None,
        before_key: None,
        on_tick: None,
        compose: Some(compose),
        pane_keys: Some(forge_pane_key),
        close_view: Some(|app, here| {
            if !app.features.review().session.as_ref().is_some_and(|s| s.workspace_index == here) {
                return false;
            }
            app.forge_close();
            true
        }),
    }
};

/// The review feature's state: the Merge Requests view's panes, when
/// `git_layout = "panes"` opens it in place of the review inbox. Its
/// code is still `App` methods (its pages are core `PageModel`s), so it
/// reaches this by type (`Features::review`), and it has no instance
/// hooks that run code.
#[derive(Default)]
pub(crate) struct ReviewHost {
    pub(super) state: ReviewState,
}

#[derive(Default)]
pub(crate) struct ReviewState {
    /// The open Merge Requests view (`SPC g M` with the panes layout).
    pub(super) session: Option<ForgeSession>,
}

impl super::feature::Feature for ReviewHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    /// The Merge Requests view's panes.
    fn holds_focused_pane(&self, app: &App) -> bool {
        let focused = app.focused_pane_id();
        self.state.session.as_ref().is_some_and(|s| focused == s.list_pane || focused == s.detail_pane || focused == s.review_pane)
    }

    /// `x` in the Merge Requests view.
    fn menu_bindings(&self, app: &App) -> Option<Vec<(&'static str, &'static str)>> {
        let session = self.state.session.as_ref()?;
        let focused = app.focused_pane_id();
        if focused != session.list_pane && focused != session.detail_pane && focused != session.review_pane {
            return None;
        }
        let mut bindings = vec![
            ("1/2/3", "go to pane"),
            ("Enter", "show this one"),
            ("f", "cycle filter"),
            ("c", "check it out locally"),
            ("u", "refresh"),
            ("A", "approve / withdraw"),
            ("m", "merge (press twice)"),
        ];
        if focused == session.review_pane {
            // Only offered where they mean something: these act on a
            // diff line or a thread, and neither exists in the other two
            // panes.
            bindings.extend([
                ("C", "comment on this line"),
                ("r", "reply to this thread"),
                ("R", "resolve / reopen"),
                ("Tab", "fold this file"),
                ("]/[", "next/prev hunk"),
            ]);
        }
        Some(bindings)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl super::feature::Features {
    pub(super) fn review(&self) -> &ReviewState {
        &self.get::<ReviewHost>().expect("review has no instance hooks that run code, so it's always in place").state
    }

    pub(super) fn review_mut(&mut self) -> &mut ReviewState {
        &mut self.get_mut::<ReviewHost>().expect("review has no instance hooks that run code, so it's always in place").state
    }
}

/// The reviews (`SPC g M`): the open requests to look at.
impl super::feature::FeaturePage for Inbox {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        format!("*reviews: {}*", self.project)
    }

    fn layout(&mut self, _ctx: Box<dyn std::any::Any>, cols: usize) -> crate::page::Page {
        crate::review_inbox::layout(self, cols)
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(Inbox::key(self, key))
    }
}

/// One request under review.
impl super::feature::FeaturePage for ReviewPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        format!("*review: {}*", self.reference())
    }

    fn layout(&mut self, _ctx: Box<dyn std::any::Any>, cols: usize) -> crate::page::Page {
        crate::review_page::layout(self, cols)
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(ReviewPage::key(self, key))
    }
}

/// A new pull request for the branch (`SPC g P`).
impl super::feature::FeaturePage for crate::git_request::RequestPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        format!("*new request: {}*", self.branch)
    }

    fn typing(&self) -> bool {
        self.editing.is_some()
    }

    /// Space ticks Draft.
    fn claims_space(&self) -> bool {
        self.field == crate::git_request::Field::Draft
    }

    fn paste(&mut self, text: &str) {
        crate::git_request::RequestPage::paste(self, text);
    }

    fn layout(&mut self, _ctx: Box<dyn std::any::Any>, cols: usize) -> crate::page::Page {
        crate::git_request::layout(self, cols)
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(crate::git_request::RequestPage::key(self, key))
    }
}

/// What the Merge Requests view's (`git_layout = "panes"`) forge calls
/// send back, each under the `request_id` it was issued with: network
/// calls routinely answer out of order, and a stale one is dropped.
pub(crate) enum ForgeEvent {
    List { request_id: u64, result: Result<Vec<fenix_forge::MergeRequest>, String> },
    /// One request's detail: the request, its approvals, its files.
    Detail { request_id: u64, number: u64, result: Box<Result<ForgeDetail, String>> },
}

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
        self.feature_page_mut::<ReviewPage>(id)
    }

    fn inbox(&mut self, id: BufferId) -> Option<&mut Inbox> {
        self.feature_page_mut::<Inbox>(id)
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
        let id = match self.find_page(|m| page_as::<Inbox>(m).is_some_and(|i| i.root == root)) {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => self.open_page(PageModel::Feature(Box::new(Inbox::new(root.clone(), forge_name(&root), project)))),
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
            send(PageEvent::feature(SPEC.id, "InboxData", ReviewJob::InboxData { buffer: id, result }));
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
        let id = match self.find_page(|m| page_as::<ReviewPage>(m).is_some_and(|r| r.root == root && r.number == number)) {
            Some(id) => {
                self.show_page(id);
                id
            }
            None => {
                let state = review_store::load(&root, &project, number);
                let mut page = ReviewPage::new(root, forge, project, number, state);
                page.in_worktree = in_worktree;
                self.open_page(PageModel::Feature(Box::new(page)))
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
            send(PageEvent::feature(SPEC.id, "ReviewData", ReviewJob::ReviewData { buffer: id, result: result.map(Box::new) }));
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
            send(PageEvent::feature(SPEC.id, "ReviewDone", ReviewJob::ReviewDone { buffer: id, label, after, result }));
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
                self.open_compose_seeded(review_purpose(ReviewCompose::Pending { page: id, pending: Box::new(pending), index }), &seed);
            }
            ReviewAction::Reply(thread) => self.open_compose(review_purpose(ReviewCompose::ThreadReply { page: id, thread })),
            ReviewAction::Summary => {
                let seed = page.summary.clone();
                self.open_compose_seeded(review_purpose(ReviewCompose::Summary { page: id }), &seed);
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
                    send(PageEvent::feature(SPEC.id, "ReviewLog", ReviewJob::ReviewLog { buffer: id, name: check.name.clone(), result }));
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
                    send(PageEvent::feature(SPEC.id, "ReviewSince", ReviewJob::ReviewSince { buffer: id, result }));
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
    pub(super) fn review_compose(&mut self, purpose: ReviewCompose, body: String) {
        self.close_compose();
        match purpose {
            ReviewCompose::Pending { page, pending, index } => {
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
            ReviewCompose::ThreadReply { page, thread } => {
                self.show_page(page);
                let Some(number) = self.review_page(page).map(|r| r.number) else { return };
                self.review_write(page, "reply", After::Refresh, move |c| c.reply(number, &thread, &body));
            }
            ReviewCompose::Summary { page } => {
                if let Some(r) = self.review_page(page) {
                    r.summary = body;
                }
                self.show_page(page);
            }
            _ => {}
        }
    }

    pub(super) fn apply_review_event(&mut self, event: ReviewJob) {
        match event {
            ReviewJob::InboxData { buffer, result } => {
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
            ReviewJob::ReviewData { buffer, result } => {
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
            ReviewJob::ReviewSince { buffer, result } => {
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
            ReviewJob::ReviewDone { buffer, label, after, result } => {
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
            ReviewJob::ReviewLog { buffer, name, result } => match result {
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

/// What the review writes in the compose pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewCompose {
    /// Another note on a thread, in the Merge Requests view.
    Reply { number: u64, discussion: String },
    /// A new thread on a diff line, in the Merge Requests view. The
    /// position is boxed: it's much larger than the rest.
    NewComment { number: u64, position: Box<fenix_forge::Position> },
    /// A comment for the review on page `page`, held until it's
    /// submitted -- a new one, or pending comment `index` edited.
    Pending { page: BufferId, pending: Box<Pending>, index: Option<usize> },
    /// A reply to a review thread, sent straight away.
    ThreadReply { page: BufferId, thread: String },
    /// The summary a review is submitted with.
    Summary { page: BufferId },
    /// The description of the request on new request page `page`.
    RequestDescription { page: BufferId },
}

/// The compose pane for `data`, titled for it.
pub(super) fn review_purpose(data: ReviewCompose) -> ComposePurpose {
    let (what, verb) = match &data {
        ReviewCompose::Reply { number, .. } => (format!("Reply to !{number}"), "send"),
        ReviewCompose::NewComment { number, position } => {
            let line = position.new_line.or(position.old_line).map(|l| l.to_string()).unwrap_or_default();
            (format!("Comment on !{number} {}:{line}", position.new_path), "send")
        }
        ReviewCompose::Pending { pending, .. } => (format!("Comment on {}:{} (held for your review)", pending.path, pending.line().unwrap_or(0)), "keep"),
        ReviewCompose::ThreadReply { .. } => ("Reply".to_string(), "send"),
        ReviewCompose::Summary { .. } => ("Review summary".to_string(), "keep"),
        ReviewCompose::RequestDescription { .. } => ("Description".to_string(), "keep"),
    };
    ComposePurpose::Feature { feature: SPEC.id, data: ComposeData::new(data), what, verb, allow_empty: false }
}

/// The compose pane's text, sent where it was written for.
fn compose(app: &mut App, data: &ComposeData, body: String) {
    let Some(purpose) = data.get::<ReviewCompose>().cloned() else { return };
    match purpose {
        ReviewCompose::RequestDescription { page } => app.request_description(page, body),
        ReviewCompose::Pending { .. } | ReviewCompose::ThreadReply { .. } | ReviewCompose::Summary { .. } => app.review_compose(purpose, body),
        ReviewCompose::Reply { .. } | ReviewCompose::NewComment { .. } => {
        let repo_root = match app.features.review().session.as_ref() {
                Some(session) => session.repo_root.clone(),
                None => {
                    app.set_error("the Merge Requests view was closed -- nothing to send this to".to_string());
                    return;
                }
            };
            let client = match app.forge_client(&repo_root) {
                Ok(client) => client,
                Err(err) => {
                    app.set_error(err);
                    return;
                }
            };
            let (number, result) = match &purpose {
                ReviewCompose::Reply { number, discussion } => (*number, fenix_forge::Forge::reply(client.as_ref(), *number, discussion, &body)),
                ReviewCompose::NewComment { number, position } => {
                    (*number, fenix_forge::Forge::comment_on_line(client.as_ref(), *number, position, &body))
                }
                _ => return,
            };
            match result {
                Ok(()) => {
                    // Only torn down on success: a failed send leaves the
                    // draft in the buffer, which is the difference between
                    // retrying and retyping.
                    app.close_compose();
                    app.set_message(format!("posted to !{number}"));
                    app.forge_select(number);
                }
                Err(err) => app.set_error(format!("couldn't post that: {err} -- the draft is still here")),
            }
        }
    }
}

/// Everything one forge round-trip fetched for a merge request's
/// detail pane. Bundled because all three come from the same click and
/// are shown together -- and because a partial arrival would leave the
/// pane claiming an approval count from one request beside a file list
/// from another.
#[derive(Debug, Clone)]
pub(crate) struct ForgeDetail {
    request: fenix_forge::MergeRequest,
    approvals: Option<fenix_forge::Approvals>,
    files: Vec<fenix_forge::ChangedFile>,
    discussions: Vec<fenix_forge::Discussion>,
    /// A failure fetching the *files* specifically. The request itself
    /// having arrived is worth showing even when its diff didn't.
    error: Option<String>,
}

/// The Merge Requests view (`SPC g M`): the project's open merge
/// requests on the left, the selected one in full on the right.
///
/// Same shape as every other session here -- fixed pane/buffer ids,
/// cached domain data, a `request_id` guard so a slow answer can't
/// overwrite a newer one. The guard matters more here than anywhere
/// else in the Git views: these are network round-trips over someone
/// else's VPN, and arriving out of order is normal rather than
/// theoretical.
pub(crate) struct ForgeSession {
    workspace_index: usize,
    list_pane: fenix_window::WindowId,
    detail_pane: fenix_window::WindowId,
    review_pane: fenix_window::WindowId,
    list_buffer: BufferId,
    detail_buffer: BufferId,
    review_buffer: BufferId,
    repo_root: PathBuf,
    /// `group/project`, inferred from the repo's own `origin` remote.
    project: String,
    filter: fenix_forge::MrFilter,
    requests: Vec<fenix_forge::MergeRequest>,
    /// Which request the detail pane is showing.
    selected: Option<u64>,
    detail: Option<ForgeDetail>,
    /// A failure listing merge requests, rendered into the list pane --
    /// an empty list beside an error that has scrolled past reads as
    /// "there are none", a different answer entirely.
    list_error: Option<String>,
    list_request_id: u64,
    detail_request_id: u64,
    /// Armed by `m` until the next keypress confirms -- merging is the
    /// one action here that can't be taken back, so it doesn't happen
    /// on a single keystroke. Mirrors `GitConfirmAction`'s own
    /// arm-then-confirm shape.
    merge_armed: bool,
}

/// The three forge calls one merge request's detail pane needs.
///
/// Run together on the worker thread rather than as three separate
/// events, so the pane can never show one request's approval count
/// beside another's file list. The request itself is the only one that
/// has to succeed: an instance without approval rules answers 404 on
/// `/approvals`, and a diff that's too large to return shouldn't take
/// the description and branch names down with it.
/// A forge client, GitHub's or GitLab's.
pub(crate) type ForgeClient = Box<dyn fenix_forge::Forge + Send + Sync>;

fn fetch_forge_detail(client: &dyn fenix_forge::Forge, number: u64) -> Result<ForgeDetail, String> {
    let request = client.merge_request(number)?;
    let approvals = client.approvals(number).ok();
    let (files, error) = match client.changed_files(number) {
        Ok(files) => (files, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    // Threads are a bonus on top of the diff, not a precondition for
    // it: an instance that refuses this endpoint should still show the
    // changes.
    let discussions = client.discussions(number).unwrap_or_default();
    Ok(ForgeDetail { request, approvals, files, discussions, error })
}

impl App {
    // -- Merge Requests view (`SPC g M`) -------------------------------

    /// A GitLab client for `repo_root`, or the reason there isn't one.
    ///
    /// Three things have to line up, and each failure says which:
    /// `[gitlab] base_url`/`token` in `config.ini`, and an `origin`
    /// remote whose URL names a project. Nothing is configured per
    /// repo -- the checkout already knows which project it came from.
    pub(super) fn forge_client(&self, repo_root: &Path) -> Result<ForgeClient, String> {
        let Some(url) = fenix_git::remote_url(repo_root, "origin") else {
            return Err("this repo has no `origin` remote to work out the project from".to_string());
        };
        // GitHub by its host; anything else is taken to be GitLab, which
        // is routinely self-hosted under any name.
        if fenix_github::repository(&url).is_some() {
            let token = self.config.token(fenix_config::Secret::GitHub).or_else(fenix_github::gh_token).ok_or_else(|| {
                "sign the GitHub CLI in (gh auth login), or set the GitHub token in SPC , (Forges)".to_string()
            })?;
            return fenix_github::GitHub::from_remote(token, &url)
                .map(|c| Box::new(c) as ForgeClient)
                .ok_or_else(|| format!("couldn't read owner/repo out of origin's URL ({url})"));
        }
        let Some(base_url) = self.config.gitlab_base_url.clone().filter(|u| !u.trim().is_empty()) else {
            return Err("set the GitLab server in SPC , (Forges) first".to_string());
        };
        let Some(token) = self.config.token(fenix_config::Secret::GitLab) else {
            return Err("set the GitLab token in SPC , (Forges) first -- a personal access token with the api scope".to_string());
        };
        fenix_gitlab::GitLab::from_remote(base_url, token, &url)
            .map(|c| Box::new(c) as ForgeClient)
            .ok_or_else(|| format!("couldn't read a GitLab project path out of origin's URL ({url})"))
    }

    /// `SPC g M`: opens (or refocuses and refreshes) the Merge Requests
    /// view.
    pub(crate) fn open_forge_view(&mut self) {
        if let Some(session) = &self.features.review().session {
            let (workspace_index, list_pane) = (session.workspace_index, session.list_pane);
            self.workspaces.switch_to_index(workspace_index);
            self.windows_mut().focus(list_pane);
            self.forge_refresh_list();
            self.wake_caret();
            return;
        }
        let repo_root = self.compare_repo_root();
        // Checked before a workspace is opened: a view that can only
        // ever show one error message isn't worth two panes.
        let project = match self.forge_client(&repo_root) {
            Ok(client) => fenix_forge::Forge::project(client.as_ref()).to_string(),
            Err(err) => {
                self.set_error(err);
                return;
            }
        };

        let list_buffer = self.buffers.open_git("");
        let detail_buffer = self.buffers.open_git("");
        let empty = diff_view::render(&[], &HashSet::new());
        let review_buffer = self.buffers.open_diff(&empty.text);
        self.diff_lines.insert(review_buffer, empty.lines);

        let cursor = Cursor::at_start();
        self.workspaces.new_workspace(list_buffer, cursor);
        let workspace_index = self.workspaces.active_index();
        let list_pane = self.focused_pane_id();
        let detail_pane = self.windows_mut().split(SplitKind::Vertical, detail_buffer);
        self.workspaces.active_pane_states_mut().insert(detail_pane, PaneState::seeded_at(cursor));
        // The right half stacks: what the request *is* on top, the diff
        // being reviewed below. The diff takes most of the height --
        // the description is read once, the changes are read for as
        // long as the review takes.
        let review_pane = self.windows_mut().split(SplitKind::Horizontal, review_buffer);
        self.workspaces.active_pane_states_mut().insert(review_pane, PaneState::seeded_at(cursor));
        self.windows_mut().resize_focused(-0.15);
        // An even split. `resize_focused`'s delta grows the *first*
        // child, so the `0.3` this used to pass gave the list 80% and
        // squeezed the detail -- the pane with all the reading in it --
        // into 20%. Left at the default rather than corrected to a
        // negative delta, because 50/50 is what's wanted here.
        self.windows_mut().focus(list_pane);

        self.pane_titles.insert(list_pane, "1. Merge Requests".to_string());
        self.pane_titles.insert(detail_pane, "2. Detail".to_string());
        self.pane_titles.insert(review_pane, "3. Review".to_string());

        self.features.review_mut().session = Some(ForgeSession {
            workspace_index,
            list_pane,
            detail_pane,
            review_pane,
            list_buffer,
            detail_buffer,
            review_buffer,
            repo_root,
            project,
            filter: fenix_forge::MrFilter::AllOpen,
            requests: Vec::new(),
            selected: None,
            detail: None,
            list_error: None,
            list_request_id: 0,
            detail_request_id: 0,
            merge_armed: false,
        });
        self.forge_render();
        self.forge_refresh_list();
        self.wake_caret();
    }

    /// Re-renders both panes from what's already cached -- no network.
    pub(super) fn forge_render(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        // Wrapped to the panes they're going into, not to a fixed
        // width: this view is an even split, so each pane is about half
        // as wide as the fixed default assumes.
        let (list_cols, detail_cols) = (self.pane_cols(session.list_pane), self.pane_cols(session.detail_pane));
        let Some(session) = self.features.review().session.as_ref() else { return };
        let list = forge_panel::render_list(&session.project, session.filter, &session.requests, session.list_error.as_deref(), list_cols);
        let detail = match &session.detail {
            Some(detail) => forge_panel::render_detail(
                Some(&detail.request),
                detail.approvals.as_ref(),
                &detail.files,
                &detail.discussions,
                detail.error.as_deref(),
                detail_cols,
            ),
            None => forge_panel::render_detail(None, None, &[], &[], None, detail_cols),
        };
        let (list_buffer, detail_buffer) = (session.list_buffer, session.detail_buffer);
        self.set_git_buffer_preserving_line(list_buffer, list);
        self.set_git_buffer_preserving_line(detail_buffer, detail);
        self.forge_render_review();
    }

    /// Renders the selected merge request's whole diff into the review
    /// pane, with its threads drawn in under the lines they hang on.
    ///
    /// The diff is reassembled from the forge's per-file hunks (see
    /// `ChangedFile::unified_diff`) and handed to the same parser and
    /// the same viewer every other diff in Fenix goes through -- so
    /// folding, hunk navigation and "open the real file here" work
    /// without this view knowing anything about them.
    pub(super) fn forge_render_review(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let (review_buffer, repo_root) = (session.review_buffer, session.repo_root.clone());
        let Some(detail) = session.detail.as_ref() else {
            self.set_diff_buffer(review_buffer, None, DiffSource::ReadOnly, &repo_root);
            return;
        };
        let diff: String = detail.files.iter().map(|f| f.unified_diff()).collect();
        let threads = forge_thread_annotations(&detail.discussions);
        let header = vec![(
            format!("{} {}  --  r reply, R resolve, C comment on this line", detail.request.reference(), detail.request.title),
            diff_view::DiffStyle::Title,
        )];
        self.set_diff_buffer_with_header(review_buffer, &header, Some(&diff), DiffSource::ReadOnly, &repo_root);
        if let Some(model) = self.diff_models.get_mut(&review_buffer) {
            model.threads = threads;
        }
        self.rerender_diff_buffer(review_buffer);
    }

    /// The review thread under the cursor, if the cursor is on one.
    pub(super) fn forge_thread_at_cursor(&self) -> Option<String> {
        let session = self.features.review().session.as_ref()?;
        if self.focused_pane_id() != session.review_pane {
            return None;
        }
        let line = self.open().buffer.line_col(&self.cursor()).0;
        self.diff_lines.get(&session.review_buffer)?.get(line)?.as_ref()?.thread.clone()
    }

    /// `r` in the review pane: open a compose buffer to reply to the
    /// thread under the cursor.
    pub(crate) fn forge_reply_at_cursor(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let Some(number) = session.selected else { return };
        let Some(discussion) = self.forge_thread_at_cursor() else {
            self.set_message("put the cursor on a review comment to reply to it");
            return;
        };
        self.open_compose(review_host::review_purpose(review_host::ReviewCompose::Reply { number, discussion }));
    }

    /// `R` in the review pane: resolve the thread under the cursor, or
    /// reopen it if it's already resolved.
    pub(crate) fn forge_resolve_at_cursor(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let Some(number) = session.selected else { return };
        let Some(discussion) = self.forge_thread_at_cursor() else {
            self.set_message("put the cursor on a review comment to resolve it");
            return;
        };
        let was_resolved = session
            .detail
            .as_ref()
            .and_then(|d| d.discussions.iter().find(|t| t.id == discussion))
            .map(|t| t.resolved)
            .unwrap_or(false);
        let repo_root = session.repo_root.clone();
        let client = match self.forge_client(&repo_root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        match fenix_forge::Forge::resolve(client.as_ref(), number, &discussion, !was_resolved) {
            Ok(()) => {
                self.set_message(if was_resolved { "thread reopened" } else { "thread resolved" });
                self.forge_select(number);
            }
            Err(err) => self.set_error(format!("couldn't resolve that thread: {err}")),
        }
        self.wake_caret();
    }

    /// `C` in the review pane: start a new thread on the diff line
    /// under the cursor.
    pub(crate) fn forge_comment_at_cursor(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        if self.focused_pane_id() != session.review_pane {
            self.set_message("comment from the Review pane (3)");
            return;
        }
        let Some(number) = session.selected else { return };
        let Some(detail) = session.detail.as_ref() else { return };
        let refs = detail.request.diff_refs.clone();
        if refs.head_sha.is_empty() {
            // Without the three SHAs a comment either lands on the
            // wrong line or is rejected; saying so beats posting it and
            // finding out on the forge.
            self.set_error("this merge request has no diff version to anchor a comment to yet".to_string());
            return;
        }
        let Some((_, anchor)) = self.diff_anchor_at_cursor() else {
            self.set_message("put the cursor on a line of the diff".to_string());
            return;
        };
        let Some(model) = self.diff_models.get(&session.review_buffer) else { return };
        let Some(file) = model.files.get(anchor.file) else { return };
        let (old_path, new_path) = (file.old_path.clone(), file.new_path.clone());
        // All three cases, because the forge distinguishes them: an
        // added line exists only on the new side, a removed line only
        // on the old, and an unchanged line on both -- and a context
        // position that names only one side is rejected outright (see
        // `Position::on_context_line`). Most of a diff is context, so
        // collapsing this into "new side where there is one" made most
        // of the lines you'd want to comment on refuse the comment.
        let position = match (anchor.old_line, anchor.new_line) {
            (Some(old), Some(new)) => fenix_forge::Position::on_context_line(&refs, &old_path, &new_path, old, new),
            (None, Some(new)) => fenix_forge::Position::on_new_line(&refs, &old_path, &new_path, new),
            (Some(old), None) => fenix_forge::Position::on_old_line(&refs, &old_path, &new_path, old),
            (None, None) => {
                self.set_message("put the cursor on a line of the diff, not on a header");
                return;
            }
        };
        self.open_compose(review_host::review_purpose(review_host::ReviewCompose::NewComment { number, position: Box::new(position) }));
    }

    /// `A`: approve the selected merge request, or withdraw an approval
    /// already given.
    pub(crate) fn forge_toggle_approval(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let Some(number) = session.selected else {
            self.set_message("no merge request selected");
            return;
        };
        let (repo_root, detail) = (session.repo_root.clone(), session.detail.clone());
        let approved = detail.as_ref().and_then(|d| d.approvals.as_ref()).map(|a| a.approved).unwrap_or(false);
        let sha = detail.as_ref().map(|d| d.request.sha.clone()).filter(|s| !s.is_empty());
        let client = match self.forge_client(&repo_root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let result = if approved {
            fenix_forge::Forge::unapprove(client.as_ref(), number)
        } else {
            fenix_forge::Forge::approve(client.as_ref(), number, sha.as_deref())
        };
        match result {
            Ok(()) => {
                self.set_message(if approved { format!("approval withdrawn from !{number}") } else { format!("approved !{number}") });
                self.forge_select(number);
            }
            Err(err) => self.set_error(format!("couldn't change the approval: {err}")),
        }
        self.wake_caret();
    }

    /// Backs out of an armed merge. Called from the key handler for
    /// every key in this view that isn't a second `m`, so an arm left
    /// hanging can't fire later on a keystroke meant for something
    /// else.
    pub(super) fn forge_disarm_merge(&mut self) {
        if let Some(session) = self.features.review_mut().session.as_mut() {
            session.merge_armed = false;
        }
    }

    /// `m`: arm the merge; a second `m` performs it.
    ///
    /// The one action in this view that can't be taken back, so it
    /// doesn't happen on a single keystroke -- the same arm-then-confirm
    /// the Git panel's destructive actions already use.
    pub(crate) fn forge_merge_request(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let Some(number) = session.selected else {
            self.set_message("no merge request selected");
            return;
        };
        if !session.merge_armed {
            if let Some(session) = self.features.review_mut().session.as_mut() {
                session.merge_armed = true;
            }
            self.set_message(format!("merge !{number}? press m again to confirm, any other key cancels"));
            self.wake_caret();
            return;
        }
        let (repo_root, detail) = (session.repo_root.clone(), session.detail.clone());
        if let Some(session) = self.features.review_mut().session.as_mut() {
            session.merge_armed = false;
        }
        let client = match self.forge_client(&repo_root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let options = fenix_forge::MergeOptions {
            squash: false,
            remove_source_branch: false,
            // Sent so the forge refuses rather than merging something
            // that moved between reading the diff and pressing the key.
            sha: detail.as_ref().map(|d| d.request.sha.clone()).filter(|s| !s.is_empty()),
            ..Default::default()
        };
        match fenix_forge::Forge::merge(client.as_ref(), number, &options) {
            Ok(()) => {
                self.set_message(format!("merged !{number}"));
                self.forge_refresh_list();
            }
            // The forge's own words: "unresolved discussions", "pipeline
            // must succeed", "branch cannot be merged" are all far more
            // useful than anything this could say instead.
            Err(err) => self.set_error(format!("couldn't merge !{number}: {err}")),
        }
        self.wake_caret();
    }

    /// Fetches the merge request list for the current filter.
    pub(super) fn forge_refresh_list(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let (repo_root, filter) = (session.repo_root.clone(), session.filter);
        let client = match self.forge_client(&repo_root) {
            Ok(client) => client,
            Err(err) => {
                if let Some(session) = self.features.review_mut().session.as_mut() {
                    session.list_error = Some(err.clone());
                    session.requests.clear();
                }
                self.set_error(err);
                self.forge_render();
                return;
            }
        };
        let Some(session) = self.features.review_mut().session.as_mut() else { return };
        session.list_request_id += 1;
        let request_id = session.list_request_id;
        self.set_message(format!("loading merge requests ({})...", filter.label()));

        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = client.list_merge_requests(filter);
                    let _ = proxy.send_event(FenixUserEvent::feature(review_host::SPEC.id, "ForgeListReady", review_host::ForgeEvent::List { request_id, result }));
                });
            }
            None => {
                let result = client.list_merge_requests(filter);
                self.apply_forge_list(request_id, result);
            }
        }
    }

    pub(super) fn apply_forge_list(&mut self, request_id: u64, result: Result<Vec<fenix_forge::MergeRequest>, String>) {
        let Some(session) = self.features.review_mut().session.as_mut() else { return };
        // A slower earlier request answering after a newer one would
        // otherwise put the previous filter's list back on screen.
        if request_id != session.list_request_id {
            return;
        }
        match result {
            Ok(requests) => {
                let count = requests.len();
                let filter = session.filter;
                session.requests = requests;
                session.list_error = None;
                // Keep the current selection if it survived the new
                // filter; otherwise show the first, so the detail pane
                // is never left describing something the list no longer
                // contains.
                let keep = session.selected.filter(|n| session.requests.iter().any(|r| r.number == *n));
                let next = keep.or_else(|| session.requests.first().map(|r| r.number));
                self.set_message(format!("{count} merge request(s) ({})", filter.label()));
                self.forge_render();
                if let Some(number) = next {
                    self.forge_select(number);
                } else if let Some(session) = self.features.review_mut().session.as_mut() {
                    session.selected = None;
                    session.detail = None;
                    self.forge_render();
                }
            }
            Err(err) => {
                session.requests.clear();
                session.list_error = Some(err.clone());
                session.selected = None;
                session.detail = None;
                self.set_error(err);
                self.forge_render();
            }
        }
    }

    /// Loads one merge request's detail into the right pane.
    pub(super) fn forge_select(&mut self, number: u64) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let repo_root = session.repo_root.clone();
        let client = match self.forge_client(&repo_root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let Some(session) = self.features.review_mut().session.as_mut() else { return };
        session.selected = Some(number);
        session.detail_request_id += 1;
        let request_id = session.detail_request_id;

        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = Box::new(fetch_forge_detail(client.as_ref(), number));
                    let _ = proxy.send_event(FenixUserEvent::feature(review_host::SPEC.id, "ForgeDetailReady", review_host::ForgeEvent::Detail { request_id, number, result }));
                });
            }
            None => {
                let result = fetch_forge_detail(client.as_ref(), number);
                self.apply_forge_detail(request_id, number, result);
            }
        }
    }

    pub(super) fn apply_forge_detail(&mut self, request_id: u64, number: u64, result: Result<ForgeDetail, String>) {
        let Some(session) = self.features.review_mut().session.as_mut() else { return };
        if request_id != session.detail_request_id {
            return;
        }
        match result {
            Ok(detail) => session.detail = Some(detail),
            Err(err) => {
                session.detail = None;
                self.set_error(format!("couldn't load !{number}: {err}"));
            }
        }
        self.forge_render();
        self.wake_caret();
    }

    /// The merge request number under the cursor in the list pane.
    pub(super) fn forge_number_at_cursor(&self) -> Option<u64> {
        let session = self.features.review().session.as_ref()?;
        if self.focused_pane_id() != session.list_pane {
            return None;
        }
        match self.git_entry_at_cursor()? {
            git_panel::GitEntry::Commit(number) => number.parse().ok(),
            _ => None,
        }
    }

    /// `Enter` in the list pane: show that merge request.
    pub(crate) fn forge_open_at_cursor(&mut self) {
        let Some(number) = self.forge_number_at_cursor() else {
            self.set_message("put the cursor on a merge request");
            return;
        };
        self.forge_select(number);
        self.wake_caret();
    }

    /// `f`: cycle the list filter (all open / mine / assigned to me).
    pub(crate) fn forge_cycle_filter(&mut self) {
        let Some(session) = self.features.review_mut().session.as_mut() else { return };
        session.filter = session.filter.next();
        self.forge_render();
        self.forge_refresh_list();
        self.wake_caret();
    }

    /// `c`: fetch the selected merge request's head into a local branch
    /// and check it out.
    ///
    /// Fetched from the *project's* own published ref rather than from
    /// the source branch, so a request opened from a fork needs no
    /// extra remote -- and lands on `mr-42` rather than the source
    /// branch's name, which may not exist locally, may point somewhere
    /// else, or may collide with an unrelated branch.
    pub(crate) fn forge_checkout_selected(&mut self) {
        let Some(session) = self.features.review().session.as_ref() else { return };
        let (repo_root, selected) = (session.repo_root.clone(), session.selected);
        let Some(number) = self.forge_number_at_cursor().or(selected) else {
            self.set_message("no merge request selected");
            return;
        };
        let client = match self.forge_client(&repo_root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let branch = format!("mr-{number}");
        let refspec = fenix_forge::Forge::checkout_refspec(client.as_ref(), number);
        if let Err(err) = fenix_git::fetch_refspec(&repo_root, "origin", &refspec) {
            self.set_error(format!("couldn't fetch !{number}: {err}"));
            return;
        }
        match fenix_git::checkout_branch(&repo_root, &branch) {
            Ok(_) => self.set_message(format!("checked out !{number} as {branch}")),
            Err(err) => self.set_error(format!("fetched !{number}, but couldn't check out {branch}: {err}")),
        }
        // The checkout rewrote the working tree, so every other Git
        // view -- and every open file -- has to catch up.
        self.git_refresh_all_views();
        self.wake_caret();
    }

    pub(super) fn forge_pane_by_number(&self, n: u32) -> Option<fenix_window::WindowId> {
        let session = self.features.review().session.as_ref()?;
        match n {
            1 => Some(session.list_pane),
            2 => Some(session.detail_pane),
            3 => Some(session.review_pane),
            _ => None,
        }
    }

    /// `SPC g q` (in its workspace): closes the Merge Requests view.
    pub(crate) fn forge_close(&mut self) {
        let Some(session) = self.features.review_mut().session.take() else { return };
        for id in [session.list_buffer, session.detail_buffer, session.review_buffer] {
            self.buffers.close(id);
            self.git_lines.remove(&id);
            self.diff_lines.remove(&id);
            self.diff_models.remove(&id);
        }
        for pane in [session.list_pane, session.detail_pane, session.review_pane] {
            self.pane_titles.remove(&pane);
        }
        self.workspaces.switch_to_index(session.workspace_index);
        self.workspaces.remove_active();
        self.refresh_project_root();
        self.wake_caret();
    }
}

/// A key in the Merge Requests view's panes, ahead of the core's.
fn forge_pane_key(app: &mut App, keypress: KeyPress) -> bool {
    if let Some((on_list, on_detail, on_review)) = app.features.review().session.as_ref().map(|s| {
            let focused = app.focused_pane_id();
            (focused == s.list_pane, focused == s.detail_pane, focused == s.review_pane)
        }) {
            if (on_list || on_detail || on_review) && keypress.mods == Mods::default() {
                // Any key that isn't a second `m` cancels an armed
                // merge -- the same "arm, then anything else backs out"
                // the Git panel's destructive actions already use.
                if keypress.code != KeyCode::Char('m') {
                    app.forge_disarm_merge();
                }
                match keypress.code {
                    KeyCode::Char('r') if on_review => {
                        app.forge_reply_at_cursor();
                        return true;
                    }
                    KeyCode::Char('R') if on_review => {
                        app.forge_resolve_at_cursor();
                        return true;
                    }
                    KeyCode::Char('C') if on_review => {
                        app.forge_comment_at_cursor();
                        return true;
                    }
                    KeyCode::Char('A') => {
                        app.forge_toggle_approval();
                        return true;
                    }
                    KeyCode::Char('m') => {
                        app.forge_merge_request();
                        return true;
                    }
                    KeyCode::Named(fenix_keymap::NamedKey::Enter) if on_list => {
                        app.forge_open_at_cursor();
                        return true;
                    }
                    KeyCode::Char('f') => {
                        app.forge_cycle_filter();
                        return true;
                    }
                    KeyCode::Char('c') => {
                        app.forge_checkout_selected();
                        return true;
                    }
                    KeyCode::Char('u') => {
                        app.forge_refresh_list();
                        return true;
                    }
                    KeyCode::Char('x') => {
                        app.git_menu_open = true;
                        app.wake_caret();
                        return true;
                    }
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        if let Some(pane) = app.forge_pane_by_number(c.to_digit(10).unwrap_or(0)) {
                            app.windows_mut().focus(pane);
                            app.wake_caret();
                        }
                        return true;
                    }
                    _ => {}
                }
            }
        }

    false
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
        app.feature_page_mut::<ReviewPage>(id).expect("the review page isn't focused")
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
        let i = app.feature_page::<Inbox>(app.focused_buffer_id()).expect("the inbox");
        let number = i.entries.as_ref().unwrap().iter().find(|e| e.request.title.contains("timeout")).unwrap().request.number;
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

    // Through its spec, the way `App` reaches review now.

    #[test]
    fn review_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let ids: Vec<&str> = SPEC.commands.iter().map(|c| c.id).collect();
        assert_eq!(ids, ["git.pull_request", "git.merge_requests", "git.merge_requests_close"]);
        assert!(ids.iter().all(|id| registry.iter().any(|c| c.id == *id && !c.external())));
        let keys = keymap::leader_keys();
        assert_eq!(keys.get("git.pull_request").map(String::as_str), Some("SPC g P"));
        assert_eq!(keys.get("git.merge_requests").map(String::as_str), Some("SPC g M"));
        assert_eq!(keys.get("git.merge").map(String::as_str), Some("SPC g m"), "core Git's merge is still beside it");
    }
}
