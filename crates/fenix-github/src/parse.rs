//! GitHub's JSON, read into `fenix-forge`'s model. Kept apart from the
//! client so every shape is testable against a literal payload.

use fenix_forge::{Approvals, ChangedFile, Check, DiffRefs, Discussion, FileChange, MergeRequest, MrState, Note, PipelineStatus, Position};
use serde_json::Value;

fn string(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |v, key| v.get(*key))
}

fn string_at(value: &Value, path: &[&str]) -> String {
    at(value, path).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// A pull request, from `GET /repos/:o/:r/pulls/:n` or a listing.
pub fn pull_request(value: &Value) -> Option<MergeRequest> {
    let number = value.get("number")?.as_u64()?;
    let merged = value.get("merged").and_then(Value::as_bool).unwrap_or(false) || value.get("merged_at").is_some_and(|m| !m.is_null());
    let state = match (string(value, "state").as_str(), merged, value.get("locked").and_then(Value::as_bool).unwrap_or(false)) {
        (_, true, _) => MrState::Merged,
        ("closed", _, _) => MrState::Closed,
        (_, _, true) => MrState::Locked,
        _ => MrState::Open,
    };
    let base_sha = string_at(value, &["base", "sha"]);
    let head_sha = string_at(value, &["head", "sha"]);
    let comments = value.get("comments").and_then(Value::as_u64).unwrap_or(0) + value.get("review_comments").and_then(Value::as_u64).unwrap_or(0);
    Some(MergeRequest {
        number,
        title: string(value, "title"),
        description: string(value, "body"),
        state,
        draft: value.get("draft").and_then(Value::as_bool).unwrap_or(false),
        source_branch: string_at(value, &["head", "ref"]),
        target_branch: string_at(value, &["base", "ref"]),
        author: string_at(value, &["user", "login"]),
        web_url: string(value, "html_url"),
        // `mergeable` is `null` until GitHub has worked it out.
        has_conflicts: value.get("mergeable").and_then(Value::as_bool) == Some(false),
        sha: head_sha.clone(),
        // GitHub anchors a comment by the head commit alone; the base
        // stands in for the start.
        diff_refs: DiffRefs { start_sha: base_sha.clone(), base_sha, head_sha },
        comment_count: comments as usize,
        pipeline: None,
        updated_at: string(value, "updated_at"),
    })
}

/// Logins in a listed pull request's `requested_reviewers`.
pub fn requested_reviewers(value: &Value) -> Vec<String> {
    value.get("requested_reviewers").and_then(Value::as_array).map(|l| l.iter().map(|u| string(u, "login")).collect()).unwrap_or_default()
}

pub fn assignees(value: &Value) -> Vec<String> {
    value.get("assignees").and_then(Value::as_array).map(|l| l.iter().map(|u| string(u, "login")).collect()).unwrap_or_default()
}

/// `GET /repos/:o/:r/pulls/:n/reviews`: whoever's latest review is an
/// approval has approved. GitHub keeps required counts in branch
/// protection, which needs admin rights to read; none are claimed.
pub fn approvals(reviews: &Value) -> Approvals {
    let mut latest: Vec<(String, String)> = Vec::new();
    for review in reviews.as_array().into_iter().flatten() {
        let user = string_at(review, &["user", "login"]);
        let state = string(review, "state");
        // A plain comment doesn't change where someone stands.
        if state == "COMMENTED" || user.is_empty() {
            continue;
        }
        latest.retain(|(u, _)| *u != user);
        latest.push((user, state));
    }
    let approved_by: Vec<String> = latest.iter().filter(|(_, s)| s == "APPROVED").map(|(u, _)| u.clone()).collect();
    Approvals { approved: !approved_by.is_empty(), required: 0, left: 0, approved_by }
}

/// One entry of `GET /repos/:o/:r/pulls/:n/files`.
pub fn changed_file(value: &Value) -> Option<ChangedFile> {
    let new_path = string(value, "filename");
    if new_path.is_empty() {
        return None;
    }
    let change = match string(value, "status").as_str() {
        "added" => FileChange::Added,
        "removed" => FileChange::Deleted,
        "renamed" => FileChange::Renamed,
        _ => FileChange::Modified,
    };
    let old_path = value.get("previous_filename").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| new_path.clone());
    // A binary or very large file has no `patch`.
    let mut diff = string(value, "patch");
    if !diff.is_empty() && !diff.ends_with('\n') {
        diff.push('\n');
    }
    Some(ChangedFile { old_path, new_path, change, diff })
}

/// A check run's state, from its `status` and `conclusion`.
pub fn check_status(status: &str, conclusion: &str) -> PipelineStatus {
    match (status, conclusion) {
        ("completed", "success") => PipelineStatus::Success,
        ("completed", "failure" | "timed_out" | "startup_failure") => PipelineStatus::Failed,
        ("completed", "cancelled") => PipelineStatus::Canceled,
        ("completed", "skipped" | "neutral" | "stale") => PipelineStatus::Skipped,
        ("completed", "action_required") => PipelineStatus::Manual,
        ("completed", other) => PipelineStatus::Other(other.to_string()),
        ("in_progress", _) => PipelineStatus::Running,
        _ => PipelineStatus::Pending,
    }
}

/// One of `GET /repos/:o/:r/commits/:sha/check-runs`'s `check_runs`.
pub fn check_run(value: &Value) -> Option<Check> {
    let id = value.get("id")?.as_u64()?;
    Some(Check {
        id: id.to_string(),
        name: string(value, "name"),
        group: string_at(value, &["app", "name"]),
        status: check_status(&string(value, "status"), &string(value, "conclusion")),
        url: string(value, "html_url"),
        seconds: None,
    })
}

/// The worst of several checks, for a request's one-word summary.
pub fn overall(checks: &[Check]) -> Option<PipelineStatus> {
    if checks.is_empty() {
        return None;
    }
    let any = |f: &dyn Fn(&PipelineStatus) -> bool| checks.iter().any(|c| f(&c.status));
    Some(if any(&|s| *s == PipelineStatus::Failed) {
        PipelineStatus::Failed
    } else if any(&|s| *s == PipelineStatus::Running) {
        PipelineStatus::Running
    } else if any(&|s| *s == PipelineStatus::Pending) {
        PipelineStatus::Pending
    } else if any(&|s| *s == PipelineStatus::Canceled) {
        PipelineStatus::Canceled
    } else {
        PipelineStatus::Success
    })
}

fn note(value: &Value) -> Note {
    Note {
        id: value.get("databaseId").and_then(Value::as_u64).unwrap_or(0),
        author: string_at(value, &["author", "login"]),
        body: string(value, "body"),
        created_at: string(value, "createdAt"),
        system: false,
    }
}

/// The GraphQL `reviewThreads` and issue `comments` of a pull request:
/// review threads anchored to lines, and the conversation below the
/// description as threads of one comment each.
pub fn discussions(pull: &Value) -> Vec<Discussion> {
    let mut out = Vec::new();
    for thread in at(pull, &["reviewThreads", "nodes"]).and_then(Value::as_array).into_iter().flatten() {
        let path = string(thread, "path");
        // An outdated thread's `line` is null; where it was is in
        // `originalLine`.
        let line = thread.get("line").and_then(Value::as_u64).or_else(|| thread.get("originalLine").and_then(Value::as_u64)).map(|l| l as usize);
        let left = string(thread, "diffSide") == "LEFT";
        let position = Position {
            base_sha: String::new(),
            head_sha: String::new(),
            start_sha: String::new(),
            old_path: path.clone(),
            new_path: path,
            old_line: if left { line } else { None },
            new_line: if left { None } else { line },
        };
        out.push(Discussion {
            id: string(thread, "id"),
            notes: at(thread, &["comments", "nodes"]).and_then(Value::as_array).map(|l| l.iter().map(note).collect()).unwrap_or_default(),
            resolved: thread.get("isResolved").and_then(Value::as_bool).unwrap_or(false),
            resolvable: true,
            position: Some(position),
        });
    }
    for comment in at(pull, &["comments", "nodes"]).and_then(Value::as_array).into_iter().flatten() {
        let n = note(comment);
        out.push(Discussion { id: format!("issue-{}", n.id), notes: vec![n], resolved: false, resolvable: false, position: None });
    }
    out
}

/// Whether a thread is outdated: its line changed after the comment.
pub fn outdated_threads(pull: &Value) -> Vec<String> {
    at(pull, &["reviewThreads", "nodes"])
        .and_then(Value::as_array)
        .map(|l| l.iter().filter(|t| t.get("isOutdated").and_then(Value::as_bool) == Some(true)).map(|t| string(t, "id")).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_pull_request_reads_its_branches_shas_and_state() {
        let pr = pull_request(&json!({
            "number": 22, "title": "Git: a status page", "body": "Does things", "state": "open", "draft": true,
            "head": {"ref": "feature/git", "sha": "aaa"}, "base": {"ref": "master", "sha": "bbb"},
            "user": {"login": "tpedneault"}, "html_url": "https://github.com/t/f/pull/22", "mergeable": false,
            "comments": 2, "review_comments": 3, "updated_at": "2026-09-24T12:00:00Z"
        }))
        .unwrap();
        assert_eq!(pr.reference(), "#22");
        assert_eq!((pr.source_branch.as_str(), pr.target_branch.as_str(), pr.sha.as_str()), ("feature/git", "master", "aaa"));
        assert!(pr.draft && pr.has_conflicts);
        assert_eq!(pr.comment_count, 5);
        assert_eq!(pr.diff_refs.base_sha, "bbb");
        let merged = pull_request(&json!({"number": 1, "state": "closed", "merged_at": "2026-01-01T00:00:00Z"})).unwrap();
        assert_eq!(merged.state, MrState::Merged);
    }

    #[test]
    fn the_latest_review_per_person_decides_approval() {
        let a = approvals(&json!([
            {"user": {"login": "alex"}, "state": "CHANGES_REQUESTED"},
            {"user": {"login": "sam"}, "state": "APPROVED"},
            {"user": {"login": "alex"}, "state": "APPROVED"},
            {"user": {"login": "sam"}, "state": "COMMENTED"}
        ]));
        assert!(a.approved);
        assert_eq!(a.approved_by, ["sam", "alex"]);
    }

    #[test]
    fn a_changed_file_keeps_its_old_name_and_patch() {
        let f = changed_file(&json!({"filename": "b.rs", "previous_filename": "a.rs", "status": "renamed", "patch": "@@ -1 +1 @@\n-x\n+y"})).unwrap();
        assert_eq!((f.old_path.as_str(), f.new_path.as_str(), f.change), ("a.rs", "b.rs", FileChange::Renamed));
        assert!(f.diff.ends_with("+y\n"));
    }

    #[test]
    fn check_runs_map_onto_pipeline_states() {
        assert_eq!(check_status("completed", "failure"), PipelineStatus::Failed);
        assert_eq!(check_status("in_progress", ""), PipelineStatus::Running);
        assert_eq!(check_status("queued", ""), PipelineStatus::Pending);
        let runs = [
            check_run(&json!({"id": 1, "name": "test", "status": "completed", "conclusion": "success", "app": {"name": "GitHub Actions"}})).unwrap(),
            check_run(&json!({"id": 2, "name": "lint", "status": "completed", "conclusion": "failure"})).unwrap(),
        ];
        assert_eq!(runs[0].group, "GitHub Actions");
        assert_eq!(overall(&runs), Some(PipelineStatus::Failed));
    }

    #[test]
    fn review_threads_and_comments_become_discussions() {
        let pull = json!({
            "reviewThreads": {"nodes": [
                {"id": "T1", "isResolved": false, "isOutdated": true, "path": "a.rs", "line": null, "originalLine": 12, "diffSide": "RIGHT",
                 "comments": {"nodes": [{"databaseId": 5, "author": {"login": "alex"}, "body": "Why?", "createdAt": "t"}]}},
                {"id": "T2", "isResolved": true, "isOutdated": false, "path": "b.rs", "line": 3, "diffSide": "LEFT",
                 "comments": {"nodes": []}}
            ]},
            "comments": {"nodes": [{"databaseId": 9, "author": {"login": "sam"}, "body": "LGTM", "createdAt": "t"}]}
        });
        let d = discussions(&pull);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].position.as_ref().unwrap().new_line, Some(12), "outdated: where it was");
        assert_eq!(d[1].position.as_ref().unwrap().old_line, Some(3));
        assert!(d[1].resolved);
        assert_eq!(d[2].id, "issue-9");
        assert!(!d[2].resolvable);
        assert_eq!(outdated_threads(&pull), ["T1"]);
    }
}
