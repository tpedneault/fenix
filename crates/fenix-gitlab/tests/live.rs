//! Every endpoint this crate talks to, against a real GitLab.
//!
//! Ignored by default: these need the dev instance from `dev/gitlab`
//! running and seeded, which is a container and several minutes of
//! boot, not something `cargo test` should depend on.
//!
//! ```text
//! docker compose -f dev/gitlab/docker-compose.yml up -d
//! ./dev/gitlab/wait-ready.sh && ./dev/gitlab/seed.sh
//! cargo test -p fenix-gitlab --test live -- --ignored --test-threads=1
//! ```
//!
//! What these are for that the unit tests aren't: a stub accepts
//! whatever it's sent, so it can confirm the shape Fenix *believes* is
//! right and nothing more. GitLab's own validator is the only thing
//! that can say a `position` payload is wrong, or that a query
//! parameter is being ignored. Everything here is asserted against
//! what `dev/gitlab/seed.sh` creates, so a failure is a real
//! disagreement rather than drift in someone's test data.
//!
//! Serial (`--test-threads=1`) on purpose: several of these write, and
//! two tests resolving the same thread at once would each see the
//! other's effect.

use fenix_forge::{Forge, MergeOptions, MrFilter, MrState, Position};
use fenix_gitlab::GitLab;

const PROJECT: &str = "fenix-dev/widget";

fn client() -> GitLab {
    let url = std::env::var("GITLAB_URL").unwrap_or_else(|_| "http://localhost:8929".to_string());
    let token = std::env::var("GITLAB_TOKEN").unwrap_or_else(|_| "fenix-dev-token-0123456789".to_string());
    GitLab::new(url, token, PROJECT)
}

/// The seeded merge request with a real diff and a real thread on it.
///
/// Fetched individually rather than taken from the listing, because a
/// listing entry has no `diff_refs` -- which is exactly what the app
/// does, and what `listing_entries_carry_no_diff_refs` pins.
fn timeout_mr() -> fenix_forge::MergeRequest {
    let gl = client();
    let number = gl
        .list_merge_requests(MrFilter::AllOpen)
        .expect("the dev instance should be up and seeded -- see dev/gitlab/README.md")
        .into_iter()
        .find(|mr| mr.title.contains("timeout"))
        .expect("seed.sh creates a 'Make the timeout configurable' request")
        .number;
    gl.merge_request(number).expect("fetching one merge request")
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn listing_returns_the_seeded_requests_with_every_field_the_panel_shows() {
    let gl = client();
    let all = gl.list_merge_requests(MrFilter::AllOpen).expect("listing works");
    assert!(all.len() >= 2, "seed.sh opens two: {all:?}");

    let mr = all.iter().find(|mr| mr.title.contains("timeout")).expect("the timeout request");
    assert_eq!(mr.state, MrState::Open);
    assert_eq!(mr.source_branch, "feature/configurable-timeout");
    assert_eq!(mr.target_branch, "main");
    assert!(!mr.author.is_empty(), "an author name or username");
    assert!(mr.web_url.contains("/merge_requests/"), "got {}", mr.web_url);
    assert!(!mr.sha.is_empty(), "the head commit, which the listing does carry");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn listing_entries_carry_no_diff_refs_but_a_fetched_one_does() {
    // The reason opening a merge request re-fetches it instead of
    // reusing the row already on screen. Reusing the row looks free and
    // silently breaks commenting, because a comment has to quote these
    // three SHAs and a listing entry has none of them.
    let gl = client();
    let listed = gl
        .list_merge_requests(MrFilter::AllOpen)
        .expect("listing")
        .into_iter()
        .find(|mr| mr.title.contains("timeout"))
        .expect("the timeout request");
    assert!(listed.diff_refs.head_sha.is_empty(), "the listing really does omit these");

    let fetched = gl.merge_request(listed.number).expect("fetching one");
    assert!(!fetched.diff_refs.base_sha.is_empty(), "base_sha");
    assert!(!fetched.diff_refs.head_sha.is_empty(), "head_sha");
    assert!(!fetched.diff_refs.start_sha.is_empty(), "start_sha");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn a_draft_is_reported_as_one() {
    let gl = client();
    let all = gl.list_merge_requests(MrFilter::AllOpen).expect("listing works");
    let draft = all.iter().find(|mr| mr.title.contains("README")).expect("the README request");
    assert!(draft.draft, "GitLab should report the `Draft:` prefix as the flag too");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn each_filter_is_accepted_and_scopes_the_listing() {
    let gl = client();
    // The seed runs as root, who authored both -- so `mine` sees them
    // and `for me` (assigned) sees none. The point is less the counts
    // than that GitLab accepts each `scope` rather than ignoring it.
    let all = gl.list_merge_requests(MrFilter::AllOpen).expect("all");
    let mine = gl.list_merge_requests(MrFilter::Mine).expect("mine");
    let for_me = gl.list_merge_requests(MrFilter::ForMe).expect("for me");
    assert!(all.len() >= 2);
    assert_eq!(mine.len(), all.len(), "root authored everything seeded");
    assert!(for_me.len() <= all.len(), "nothing is assigned, so this can only be narrower");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn the_changed_files_endpoint_answers_with_a_parseable_diff() {
    let gl = client();
    let mr = timeout_mr();
    let files = gl.changed_files(mr.number).expect("/diffs works on this version");
    assert_eq!(files.len(), 1, "the seeded branch touches one file: {files:?}");
    assert_eq!(files[0].new_path, "widget.rs");
    assert_eq!(files[0].change, fenix_forge::FileChange::Modified);

    // The whole reason `unified_diff` exists: GitLab hands back bare
    // hunks, and the parser keys off `diff --git` to know a file
    // started.
    let parsed = fenix_diff::parse(&files[0].unified_diff());
    assert_eq!(parsed.len(), 1, "one file entry");
    assert!(!parsed[0].hunks.is_empty(), "with hunks in it");
    assert!(parsed[0].hunks.iter().flat_map(|h| h.lines.iter()).any(|l| l.kind == fenix_diff::LineKind::Added));
    assert!(parsed[0].hunks.iter().flat_map(|h| h.lines.iter()).any(|l| l.kind == fenix_diff::LineKind::Removed));
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn approvals_come_back_even_on_a_free_instance_with_no_rules() {
    // `approvals_required`/`approvals_left` are Premium fields. This
    // pins that their absence reads as "no rule" rather than as an
    // error -- the case a self-hosted Free instance always hits.
    let gl = client();
    let approvals = gl.approvals(timeout_mr().number).expect("/approvals answers");
    assert_eq!(approvals.required, 0, "Free tier has no approval rules");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn discussions_come_back_with_the_diff_thread_anchored_to_a_real_line() {
    let gl = client();
    let mr = timeout_mr();
    let discussions = gl.discussions(mr.number).expect("/discussions answers");

    let on_a_line = discussions.iter().find(|d| d.position.is_some()).expect("seed.sh anchors one thread to a diff line");
    let position = on_a_line.position.as_ref().unwrap();
    assert_eq!(position.new_path, "widget.rs");
    assert_eq!(position.new_line, Some(7), "the line seed.sh commented on");
    assert!(on_a_line.resolvable, "a diff thread can be resolved");
    assert!(on_a_line.notes.iter().any(|n| n.body.contains("const")));

    // And the one that hangs on nothing, which the panel puts in the
    // detail pane rather than on the diff.
    let general = discussions.iter().find(|d| d.position.is_none() && d.is_human()).expect("seed.sh adds a plain comment");
    assert!(general.notes.iter().any(|n| n.body.contains("Overall this reads well")));
    // Resolvable too, as it happens -- a merge request's threads are
    // resolvable whether or not they hang on a line, which is not what
    // "it has no position" would suggest. Read from the flag rather
    // than inferred, which is why this holds.
    assert!(general.resolvable);

    // The forge's own narration is the only thing that comes back
    // unresolvable, and the only thing kept off the diff.
    assert!(discussions.iter().any(|d| !d.is_human()), "approve/unapprove leave system notes behind");
}

/// Posts `position` and reads the thread back off the instance.
///
/// Takes the merge request rather than re-fetching it: each of these
/// tests is several round trips already, and re-listing per call turns
/// a handful into a burst.
fn post_and_read_back(mr: &fenix_forge::MergeRequest, position: &Position, what: &str) -> fenix_forge::Discussion {
    let gl = client();
    let body = format!("Live test, {what}, at {:?}.", std::time::SystemTime::now());
    gl.comment_on_line(mr.number, position, &body).unwrap_or_else(|err| panic!("GitLab rejected a {what} position: {err}"));
    gl.discussions(mr.number)
        .expect("re-read")
        .into_iter()
        .find(|d| d.notes.iter().any(|n| n.body == body))
        .expect("the new thread is there")
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn a_comment_on_an_added_line_is_accepted() {
    // The one thing no stub can tell you. A `position` GitLab dislikes
    // is a 400, not a comment on the wrong line.
    let mr = timeout_mr();
    // Line 6 of the new file is `fn read_timeout() -> u64 {`, added by
    // the seeded branch.
    let position = Position::on_new_line(&mr.diff_refs, "widget.rs", "widget.rs", 6);
    let landed = post_and_read_back(&mr, &position, "new-side").position.expect("it's a diff thread");
    assert_eq!(landed.new_line, Some(6), "and on the line it named");
    assert_eq!(landed.new_path, "widget.rs");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn a_comment_on_an_unchanged_line_needs_both_sides_and_is_accepted_with_them() {
    // The bug this whole dev instance paid for: GitLab derives an
    // internal line code from the `old_line`/`new_line` pair, and a
    // context position carrying only the new side is rejected with
    // "line_code can't be blank". Most of a diff is context.
    let mr = timeout_mr();
    let both = Position::on_context_line(&mr.diff_refs, "widget.rs", "widget.rs", 3, 3);
    let landed = post_and_read_back(&mr, &both, "context").position.expect("it's a diff thread");
    assert_eq!((landed.old_line, landed.new_line), (Some(3), Some(3)));

    // And the shape that used to be sent for the same line really is
    // refused, so this test would have caught it.
    let only_new = Position::on_new_line(&mr.diff_refs, "widget.rs", "widget.rs", 3);
    let err = client().comment_on_line(mr.number, &only_new, "should not post").expect_err("a context line needs both");
    assert!(err.contains("400"), "got: {err}");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn replying_and_resolving_move_a_real_thread() {
    let gl = client();
    let mr = timeout_mr();
    let thread = gl
        .discussions(mr.number)
        .expect("read")
        .into_iter()
        .find(|d| d.position.is_some() && d.resolvable)
        .expect("a resolvable diff thread");
    let body = format!("Reply from the live test at {:?}.", std::time::SystemTime::now());

    gl.reply(mr.number, &thread.id, &body).expect("POST .../notes is the right verb and path");
    gl.resolve(mr.number, &thread.id, true).expect("resolving works");

    let after = gl
        .discussions(mr.number)
        .expect("re-read")
        .into_iter()
        .find(|d| d.id == thread.id)
        .expect("still there");
    assert!(after.notes.iter().any(|n| n.body == body), "the reply landed in this thread");
    assert!(after.resolved, "and the thread reads as resolved");

    // Put it back, so the instance stays in the state seed.sh left it
    // and re-running these doesn't depend on the order they ran in.
    gl.resolve(mr.number, &thread.id, false).expect("reopening works too");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn approving_and_withdrawing_both_work() {
    let gl = client();
    let mr = timeout_mr();

    gl.approve(mr.number, Some(&mr.sha)).expect("the sha parameter is accepted");
    assert!(gl.approvals(mr.number).expect("read back").approved);

    gl.unapprove(mr.number).expect("withdrawing works");
    assert!(!gl.approvals(mr.number).expect("read back").approved);
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn approving_a_stale_head_is_refused_rather_than_recorded() {
    // Why the sha is sent at all: approving a version you haven't read
    // should fail, not quietly count.
    let gl = client();
    let mr = timeout_mr();
    let stale = "0000000000000000000000000000000000000000";
    let result = gl.approve(mr.number, Some(stale));
    assert!(result.is_err(), "GitLab should refuse an approval quoting the wrong head: {result:?}");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn a_merge_request_can_be_fetched_by_its_published_ref() {
    // The refspec `c` uses. Checked here rather than only in
    // fenix-git's own tests, because the *shape* of the ref is
    // GitLab's, not git's.
    let gl = client();
    let mr = timeout_mr();
    let refspec = gl.checkout_refspec(mr.number);
    assert_eq!(refspec, format!("refs/merge-requests/{}/head:mr-{}", mr.number, mr.number));

    let url = std::env::var("GITLAB_URL").unwrap_or_else(|_| "http://localhost:8929".to_string());
    let token = std::env::var("GITLAB_TOKEN").unwrap_or_else(|_| "fenix-dev-token-0123456789".to_string());
    let remote = format!("{}/{PROJECT}.git", url.replace("http://", &format!("http://root:{token}@")));
    let out = std::process::Command::new("git")
        .args(["ls-remote", &remote, &format!("refs/merge-requests/{}/head", mr.number)])
        .output()
        .expect("git ls-remote runs");
    assert!(out.status.success(), "ls-remote failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!out.stdout.is_empty(), "GitLab publishes the merge request's head under that ref");
}

#[test]
#[ignore = "needs the dev GitLab instance; see dev/gitlab/README.md"]
fn a_merge_that_cannot_go_through_reports_the_forges_own_reason() {
    // Deliberately quoting a head that isn't current: the merge must be
    // refused, and the message must be the forge's rather than
    // something invented here.
    let gl = client();
    let mr = timeout_mr();
    let options = MergeOptions {
        squash: false,
        remove_source_branch: false,
        sha: Some("0000000000000000000000000000000000000000".to_string()),
    };
    let err = gl.merge(mr.number, &options).expect_err("a stale sha must not merge");
    assert!(err.starts_with("HTTP "), "the forge's own status and text, not a rewritten one: {err}");
}
