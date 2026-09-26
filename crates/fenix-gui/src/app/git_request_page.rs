//! The host half of the new pull request page (`git_request`): reading
//! the branch for it, asking the forge whether one is open already, and
//! opening it -- pushing first when the branch isn't -- then asking the
//! reviewers. Also the status page's header line for the branch's
//! request.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::git_request::{self, Commit, Pushed, RequestAction, RequestPage, Standing};
use crate::git_status::{overall, Job, RequestLine};

/// Who a new request asks for a review: the project's own reviewers
/// (`.fenix/settings.toml`, or the `project.ini` it replaced) when it
/// names anyone, else yours.
fn default_reviewers(root: &Path, configured: &[String]) -> Vec<String> {
    fenix_project::meta::reviewers(root).unwrap_or_else(|| configured.to_vec())
}

impl App {
    fn request_page(&mut self, id: BufferId) -> Option<&mut RequestPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Request(r)) => Some(r),
            _ => None,
        }
    }

    /// `SPC g P`: a pull request for the focused file's branch.
    pub(crate) fn open_new_request(&mut self) {
        let root = self.focused_git_page_root().or_else(|| self.review_root());
        match root {
            Some(root) => self.open_new_request_at(root),
            None => self.set_error("not in a git repository"),
        }
    }

    /// The new request page for the branch checked out at `root`.
    pub(super) fn open_new_request_at(&mut self, root: PathBuf) {
        let Some(branch) = fenix_git::oplog::current_branch(&root) else {
            self.set_error("not on a branch -- switch to one (SPC g w) to open a pull request from it");
            return;
        };
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::Request(r) if r.root == root && r.branch == branch)) {
            self.show_page(id);
            return;
        }
        let client = match self.forge_client(&root) {
            Ok(client) => client,
            Err(err) => {
                self.set_error(err);
                return;
            }
        };
        let Some(base_ref) = fenix_git::resolve_base(&root, self.base_branch_for(&root).as_deref()) else {
            self.set_error("no base branch to open it against -- set one in SPC , (Git)");
            return;
        };
        let base = base_ref.strip_prefix("origin/").unwrap_or(&base_ref).to_string();
        if base == branch {
            self.set_error(format!("{branch} is the base branch -- make a branch for the change first (b c on the Git page)"));
            return;
        }
        let commits: Vec<Commit> = fenix_git::commits_between(&root, &base_ref, "HEAD", 200)
            .into_iter()
            .map(|c| {
                let message = fenix_git::commit_message(&root, &c.hash).unwrap_or_default();
                let body = message.split_once('\n').map(|(_, b)| b.trim().to_string()).unwrap_or_default();
                Commit { short: c.short_hash, subject: c.message, body }
            })
            .collect();
        let status = fenix_git::status(&root);
        let pushed = match status.as_ref().and_then(|s| s.upstream.as_ref().map(|u| (u.clone(), s.ahead))) {
            None => Pushed::NotYet,
            Some((_, ahead)) if ahead > 0 => Pushed::Ahead(ahead),
            Some((upstream, _)) => Pushed::UpToDate(upstream.split('/').next().unwrap_or("origin").to_string()),
        };
        let fixups = commits.iter().filter(|c| c.subject.starts_with("fixup! ") || c.subject.starts_with("squash! ") || c.subject.starts_with("amend! ")).count();
        let standing = Standing {
            pushed,
            merges_cleanly: fenix_git::merges_cleanly(&root, &base_ref, "HEAD"),
            base_moved: fenix_git::commits_between(&root, "HEAD", &base_ref, 1000).len(),
            fixups,
        };
        let jira = git_request::jira_key(&branch, fenix_project::meta::jira_key(&root).as_deref());
        let stat = fenix_git::shortstat(&root, &base_ref, "HEAD");
        let mut page = RequestPage::new(root.clone(), super::review_host::forge_name(&root), client.project().to_string(), branch.clone(), base, commits, stat, standing, jira);
        page.reviewers = default_reviewers(&root, &self.config.git_reviewers).join(", ");
        let id = self.open_page(PageModel::Request(Box::new(page)));
        self.page_spawn(move |send| {
            let result = client.request_for_branch(&branch);
            let me = client.current_user().ok();
            send(PageEvent::RequestExisting { buffer: id, result, me });
        });
    }

    pub(super) fn request_action(&mut self, id: BufferId, action: RequestAction) {
        let Some(root) = self.request_page(id).map(|r| r.root.clone()) else { return };
        match action {
            RequestAction::None => {}
            RequestAction::Close => self.close_page(id),
            RequestAction::EditDescription => {
                let seed = self.request_page(id).map(|r| r.description.clone()).unwrap_or_default();
                self.open_compose_seeded(ComposePurpose::RequestDescription { page: id }, &seed);
            }
            RequestAction::Browser(url) => self.open_url(&url),
            RequestAction::Review(number) => {
                self.close_page(id);
                self.open_review(root, number, false);
            }
            RequestAction::Open { request, reviewers, push } => {
                let client = match self.forge_client(&root) {
                    Ok(client) => client,
                    Err(err) => {
                        if let Some(page) = self.request_page(id) {
                            page.message = Some((err, true));
                        }
                        return;
                    }
                };
                // Where it goes: the upstream's remote, else the default.
                let status = fenix_git::status(&root);
                let upstream = status.as_ref().and_then(|s| s.upstream.clone());
                let remote = upstream.as_deref().and_then(|u| u.split('/').next()).map(str::to_string).or_else(|| fenix_git::default_remote(&root));
                let push = match (push, remote) {
                    (false, _) => None,
                    (true, Some(remote)) => Some(fenix_git::PushOptions {
                        remote,
                        branch: request.source_branch.clone(),
                        set_upstream: upstream.is_none(),
                        force_with_lease: false,
                        dry_run: false,
                    }),
                    (true, None) => {
                        if let Some(page) = self.request_page(id) {
                            page.message = Some(("no remote to push the branch to -- git remote add origin <url>".to_string(), true));
                        }
                        return;
                    }
                };
                let Some(page) = self.request_page(id) else { return };
                page.busy = true;
                page.message = None;
                self.page_spawn(move |send| {
                    let result = (|| {
                        if let Some(options) = push {
                            super::git_page::run_logged(&root, &Job::Push(options)).map_err(|e| format!("the push failed, so nothing was opened: {}", e.trim()))?;
                        }
                        let made = client.create_request(&request)?;
                        let mut warning = None;
                        // No forge lets you review your own -- drop yourself
                        // from a list that came from the config.
                        let me = client.current_user().unwrap_or_default();
                        let reviewers: Vec<String> = reviewers.into_iter().filter(|r| !r.eq_ignore_ascii_case(&me)).collect();
                        if !reviewers.is_empty() {
                            if let Err(err) = client.request_review(made.number, &reviewers) {
                                warning = Some(format!("but asking {} to review didn't work: {err}", reviewers.join(", ")));
                            }
                        }
                        Ok((made, warning))
                    })();
                    send(PageEvent::RequestOpened { buffer: id, result });
                });
            }
        }
    }

    /// The description, written in the compose buffer.
    pub(super) fn request_description(&mut self, page: BufferId, body: String) {
        self.close_compose();
        if let Some(r) = self.request_page(page) {
            r.set_description(body);
        }
        if self.pages.contains_key(&page) {
            self.show_page(page);
        }
    }

    /// Asks the forge about the status page's branch's request, for its
    /// header. Quietly nothing when there's no forge to ask.
    pub(super) fn git_page_request(&mut self, id: BufferId) {
        let Some(root) = self.pages.get(&id).and_then(|s| match &s.model {
            PageModel::Git(g) => Some(g.root.clone()),
            _ => None,
        }) else {
            return;
        };
        let Some(branch) = fenix_git::oplog::current_branch(&root) else { return };
        let Ok(client) = self.forge_client(&root) else { return };
        self.page_spawn(move |send| {
            let result = client.request_for_branch(&branch).map(|found| {
                found.map(|r| {
                    let checks = client.checks(r.number, &r.sha).ok().and_then(|c| overall(&c)).or(r.pipeline.clone());
                    let approved_by = client.approvals(r.number).map(|a| a.approved_by).unwrap_or_default();
                    let unresolved = client.discussions(r.number).map(|d| d.iter().filter(|d| d.is_human() && d.resolvable && !d.resolved).count()).unwrap_or(0);
                    RequestLine { number: r.number, reference: r.reference(), title: r.title, draft: r.draft, checks, approved_by, unresolved }
                })
            });
            send(PageEvent::GitRequest { buffer: id, result });
        });
    }

    pub(super) fn apply_request_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::RequestExisting { buffer, result, me } => {
                let Some(page) = self.request_page(buffer) else { return };
                if let Some(me) = me {
                    page.assign_to_me(&me);
                }
                match result {
                    Ok(found) => page.existing = Some(found),
                    Err(err) => {
                        page.existing = Some(None);
                        page.message = Some((format!("couldn't ask {} whether one is open already: {err}", page.forge), true));
                    }
                }
            }
            PageEvent::RequestOpened { buffer, result } => {
                let Some(page) = self.request_page(buffer) else { return };
                page.busy = false;
                match result {
                    Ok((made, warning)) => {
                        let root = page.root.clone();
                        self.close_page(buffer);
                        let opened = format!("opened {} {}", made.reference(), made.title);
                        match warning {
                            Some(warning) => self.set_error(format!("{opened} -- {warning}")),
                            None => self.set_message(format!("{opened} ✓ -- o on the Git page reviews it")),
                        }
                        self.git_refresh_all_views();
                        let pages: Vec<BufferId> = self.pages.iter().filter(|(_, s)| matches!(&s.model, PageModel::Git(g) if g.root == root)).map(|(id, _)| *id).collect();
                        for id in pages {
                            self.git_page_request(id);
                        }
                    }
                    Err(err) => page.message = Some((err, true)),
                }
            }
            PageEvent::GitRequest { buffer, result } => {
                if let (Some(PageModel::Git(g)), Ok(line)) = (self.pages.get_mut(&buffer).map(|s| {
                    s.stale = true;
                    &mut s.model
                }), result)
                {
                    g.request = Some(line);
                }
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

    fn clone(url: &str, name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-request-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let status = Command::new("git").args(["clone", "-q", url, &dir.to_string_lossy()]).status().unwrap();
        assert!(status.success(), "clone {url}");
        let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
        for args in [&["config", "user.email", "fenix@example.com"][..], &["config", "user.name", "Fenix live tests"], &["config", "core.autocrlf", "false"]] {
            git(&dir, args);
        }
        dir
    }

    /// A new branch with two commits, not pushed.
    fn branch(root: &Path) -> String {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let branch = format!("live/request-{stamp}");
        git(root, &["switch", "-q", "-c", &branch]);
        std::fs::write(root.join(format!("notes-{stamp}.md")), "one\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "Add notes", "-m", "Somewhere to write things down."]);
        std::fs::write(root.join(format!("notes-{stamp}.md")), "one\ntwo\n").unwrap();
        git(root, &["commit", "-q", "-am", "Add a second note"]);
        branch
    }

    fn text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 140);
        app.open().buffer.text()
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            assert!(app.page_key(KeyPress::char(c)), "{c}");
        }
    }

    /// Fills in the page the way you would and opens it: labels typed in
    /// place, draft on, then `C-c C-c`.
    fn fill_and_open(app: &mut App) {
        press(app, "jjj");
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        press(app, "fenix");
        assert!(app.page_key(KeyPress::named(FenixNamedKey::Enter)));
        press(app, "d");
        let ctrl_c = KeyPress::char('c').with_ctrl();
        assert!(app.page_key(ctrl_c));
        assert!(app.page_key(ctrl_c));
    }

    #[test]
    fn reviewers_come_from_the_project_before_the_config() {
        let dir = std::env::temp_dir().join(format!("fenix-request-reviewers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".fenix")).unwrap();
        let configured = vec!["alex".to_string()];
        assert_eq!(default_reviewers(&dir, &configured), ["alex"]);
        std::fs::write(dir.join(".fenix").join("settings.toml"), "[project]\njira = \"FNX\"\n").unwrap();
        assert_eq!(default_reviewers(&dir, &configured), ["alex"], "a project that names nobody");
        std::fs::write(dir.join(".fenix").join("project.ini"), "[git]\nreviewers = @sam, jo\n").unwrap();
        assert_eq!(default_reviewers(&dir, &configured), ["sam", "jo"], "a project not moved yet");
        std::fs::write(dir.join(".fenix").join("settings.toml"), "[git]\nreviewers = [\"kim\"]\n").unwrap();
        assert_eq!(default_reviewers(&dir, &configured), ["kim"], "its settings.toml wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn live_spc_g_p_pushes_the_branch_and_opens_a_labelled_draft_on_github() {
        let repo = std::env::var("FENIX_GITHUB_SANDBOX").unwrap_or_else(|_| "tpedneault/fenix-review-sandbox".to_string());
        let root = clone(&format!("https://github.com/{repo}.git"), "github");
        let branch = branch(&root);
        let mut app = App::with_file(Some(root.join("decoder.py").to_string_lossy().into_owned()));
        app.config.git_layout = None;
        // Yourself, from the config: shown, then dropped when it's opened.
        let me = app.forge_client(&root).unwrap().current_user().unwrap();
        app.config.git_reviewers = vec![me.clone()];
        app.open_new_request();
        let shown = text(&mut app);
        assert!(shown.contains(&format!("Reviewers    {me}")), "{shown}");
        for want in [format!("New pull request · {repo}"), format!("{branch} → main"), "2 COMMITS".into(), "not pushed yet".into(), "- Add notes\n".into()] {
            assert!(shown.contains(&want), "{want}:\n{shown}");
        }
        fill_and_open(&mut app);

        let said = app.status_message.as_ref().map(|m| (m.text.clone(), m.is_error));
        let client = app.forge_client(&root).unwrap();
        let made = client.request_for_branch(&branch).unwrap().expect("opened");
        let labels = Command::new("gh").args(["api", &format!("repos/{repo}/issues/{}/labels", made.number), "--jq", ".[].name"]).output().unwrap();
        app.open_git_status();
        let header = text(&mut app);
        let _ = Command::new("gh").args(["api", "-X", "PATCH", &format!("repos/{repo}/pulls/{}", made.number), "-f", "state=closed"]).output();
        let _ = git(&root, &["push", "-q", "origin", "--delete", &branch]);
        let _ = std::fs::remove_dir_all(&root);

        assert!(made.draft && made.title.starts_with("Request "), "the branch's name, as a sentence: {made:?}");
        assert!(made.description.contains("- Add notes\n- Add a second note"), "{}", made.description);
        assert_eq!(String::from_utf8_lossy(&labels.stdout).trim(), "fenix");
        assert!(said.as_ref().is_some_and(|(text, error)| !error && text.starts_with(&format!("opened #{}", made.number))), "{said:?}");
        assert!(header.contains(&format!("Review  #{} ", made.number)) && header.contains("draft"), "{header}");
    }

    #[test]
    #[ignore]
    fn live_spc_g_p_opens_a_draft_merge_request_on_gitlab() {
        let root = clone("http://root:fenix-dev-token-0123456789@localhost:8929/fenix-dev/widget.git", "gitlab");
        let branch = branch(&root);
        let mut app = App::with_file(Some(root.join("widget.rs").to_string_lossy().into_owned()));
        app.config.gitlab_base_url = Some("http://localhost:8929".into());
        app.config.gitlab_token = Some("fenix-dev-token-0123456789".into());
        app.config.git_layout = None;
        app.config.git_reviewers = vec!["root".into()];
        app.open_new_request();
        let shown = text(&mut app);
        assert!(shown.contains("Reviewers    root"), "{shown}");
        assert!(shown.contains("New merge request · fenix-dev/widget") && shown.contains("! not pushed yet"), "{shown}");
        fill_and_open(&mut app);

        let said = app.status_message.as_ref().map(|m| (m.text.clone(), m.is_error));
        let client = app.forge_client(&root).unwrap();
        let made = client.request_for_branch(&branch).unwrap().expect("opened");
        app.open_git_status();
        let header = text(&mut app);
        let _ = Command::new("curl")
            .args(["-s", "-X", "PUT", "-H", "PRIVATE-TOKEN: fenix-dev-token-0123456789", &format!("http://localhost:8929/api/v4/projects/fenix-dev%2Fwidget/merge_requests/{}?state_event=close", made.number)])
            .output();
        let _ = git(&root, &["push", "-q", "origin", "--delete", &branch]);
        let _ = std::fs::remove_dir_all(&root);

        assert!(made.draft, "{made:?}");
        assert!(said.as_ref().is_some_and(|(_, error)| !error), "{said:?}");
        assert!(header.contains(&format!("Review  !{} ", made.number)) && header.contains("draft"), "{header}");
    }
}
