//! Every endpoint this crate talks to, against a real GitHub repository.
//!
//! Ignored by default: these need a signed-in GitHub CLI (`gh auth
//! token`) and a scratch repository you own with a `feature/pus17`
//! branch that changes `decoder.py` -- `FENIX_GITHUB_SANDBOX` names it
//! (`owner/repo`, default `tpedneault/fenix-review-sandbox`). They write:
//! a pull request, review comments, a reply, a resolve. Run them one at
//! a time:
//!
//! ```text
//! cargo test -p fenix-github --test live -- --ignored --test-threads=1
//! ```
//!
//! GitHub won't let you approve or request changes on your own pull
//! request, so those are checked by the error GitHub gives -- which is
//! also what shows the message reaches the page intact.

use fenix_forge::{DraftComment, Forge, MrFilter, NewRequest, Position, Verdict};
use fenix_github::GitHub;

fn client() -> GitHub {
    let repo = std::env::var("FENIX_GITHUB_SANDBOX").unwrap_or_else(|_| "tpedneault/fenix-review-sandbox".to_string());
    let (owner, name) = repo.split_once('/').expect("owner/repo");
    GitHub::new(fenix_github::gh_token().expect("gh auth token"), owner, name)
}

/// The sandbox's pull request from `feature/pus17`, opened if it isn't.
fn pull() -> fenix_forge::MergeRequest {
    let gh = client();
    if let Some(pr) = gh.request_for_branch("feature/pus17").unwrap() {
        return pr;
    }
    let request = NewRequest {
        source_branch: "feature/pus17".into(),
        target_branch: "main".into(),
        title: "Decode PUS-17 connection tests".into(),
        description: "Opened by Fenix's live tests.".into(),
        draft: false,
    };
    let created = gh.create_request(&request).unwrap();
    gh.merge_request(created.number).unwrap()
}

#[test]
#[ignore]
fn a_request_is_opened_found_by_its_branch_and_listed() {
    let pr = pull();
    assert_eq!(pr.source_branch, "feature/pus17");
    assert_eq!(pr.reference(), format!("#{}", pr.number));
    assert!(!pr.sha.is_empty() && !pr.diff_refs.base_sha.is_empty());
    let gh = client();
    let mine = gh.list_merge_requests(MrFilter::Mine).unwrap();
    assert!(mine.iter().any(|m| m.number == pr.number), "the author's own list has it");
    assert!(gh.current_user().unwrap().len() > 1);
}

#[test]
#[ignore]
fn the_files_come_back_as_diffs_fenix_can_parse() {
    let pr = pull();
    let files = client().changed_files(pr.number).unwrap();
    let decoder = files.iter().find(|f| f.new_path == "decoder.py").expect("decoder.py changed");
    let parsed = fenix_diff::parse(&decoder.unified_diff());
    assert_eq!(parsed.len(), 1);
    assert!(parsed[0].hunks[0].lines.iter().any(|l| l.text.contains("connection test")));
}

#[test]
#[ignore]
fn a_review_goes_out_with_its_comments_and_threads_can_be_answered_and_resolved() {
    let pr = pull();
    let gh = client();
    let before = gh.discussions(pr.number).unwrap().len();
    let at = |line| Position::on_new_line(&pr.diff_refs, "decoder.py", "decoder.py", line);
    let comments = vec![
        DraftComment { position: at(3), start_line: None, body: "Should 17 be a named constant?".into() },
        DraftComment { position: at(4), start_line: Some(3), body: "```suggestion\n    if header[0] == PUS_TEST:\n        return \"connection test\", None\n```".into() },
    ];
    gh.submit_review(pr.number, &pr.sha, Verdict::Comment, "Two things.", &comments).unwrap();
    let threads = gh.discussions(pr.number).unwrap();
    let ours: Vec<_> = threads.iter().filter(|d| d.position.is_some()).collect();
    assert!(threads.len() >= before + 2, "both comments became threads");
    let thread = ours.iter().find(|d| d.first().is_some_and(|n| n.body.contains("named constant"))).expect("the first comment");
    assert_eq!(thread.position.as_ref().unwrap().new_line, Some(3));

    gh.reply(pr.number, &thread.id, "Done.").unwrap();
    gh.resolve(pr.number, &thread.id, true).unwrap();
    let again = gh.discussions(pr.number).unwrap();
    let thread = again.iter().find(|d| d.id == thread.id).unwrap();
    assert!(thread.resolved && thread.notes.iter().any(|n| n.body == "Done."));
    gh.resolve(pr.number, &thread.id, false).unwrap();
}

#[test]
#[ignore]
fn approving_your_own_request_is_refused_in_githubs_words() {
    let pr = pull();
    let err = client().submit_review(pr.number, &pr.sha, Verdict::Approve, "", &[]).expect_err("GitHub refuses self-approval");
    assert!(err.starts_with("HTTP 422") && err.to_lowercase().contains("own pull request"), "{err}");
}

#[test]
#[ignore]
fn a_line_comment_posts_straight_away() {
    let pr = pull();
    let gh = client();
    let position = Position::on_new_line(&pr.diff_refs, "test_decoder.py", "test_decoder.py", 5);
    gh.comment_on_line(pr.number, &position, "A test for a truncated reply too?").unwrap();
    assert!(gh.discussions(pr.number).unwrap().iter().any(|d| d.first().is_some_and(|n| n.body.contains("truncated"))));
}
