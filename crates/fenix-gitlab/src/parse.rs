//! GitLab's JSON turned into `fenix-forge`'s neutral model.
//!
//! Split out from the client so every shape here is testable against a
//! literal payload, with no network and no instance to point at. That
//! matters more than usual for this crate: the fields are documented
//! rather than inferred, but a self-hosted instance can be several
//! versions behind, so every read is written to degrade to a default
//! rather than to fail.

use fenix_forge::{Approvals, ChangedFile, DiffRefs, Discussion, FileChange, MergeRequest, MrState, Note, PipelineStatus, Position};
use serde_json::Value;

fn string(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn bool_at(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn usize_at(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

/// One merge request from `GET /projects/:id/merge_requests[/:iid]`.
///
/// Every field is optional as far as this is concerned. An older
/// instance won't send `detailed_merge_status`, a merge request with no
/// CI has no `head_pipeline`, and `diff_refs` is absent until GitLab
/// has computed the diff -- none of which should stop the row from
/// rendering.
pub fn merge_request(value: &Value) -> Option<MergeRequest> {
    let number = value.get("iid").and_then(Value::as_u64)?;
    Some(MergeRequest {
        number,
        title: string(value, "title"),
        description: string(value, "description"),
        state: match value.get("state").and_then(Value::as_str).unwrap_or("") {
            "merged" => MrState::Merged,
            "closed" => MrState::Closed,
            "locked" => MrState::Locked,
            _ => MrState::Open,
        },
        draft: bool_at(value, "draft"),
        source_branch: string(value, "source_branch"),
        target_branch: string(value, "target_branch"),
        author: value.get("author").map(|a| string(a, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| {
            value.get("author").map(|a| string(a, "username")).unwrap_or_default()
        }),
        web_url: string(value, "web_url"),
        has_conflicts: bool_at(value, "has_conflicts"),
        sha: string(value, "sha"),
        diff_refs: value
            .get("diff_refs")
            .map(|d| DiffRefs { base_sha: string(d, "base_sha"), head_sha: string(d, "head_sha"), start_sha: string(d, "start_sha") })
            .unwrap_or_default(),
        comment_count: usize_at(value, "user_notes_count"),
        pipeline: value.get("head_pipeline").and_then(|p| p.get("status")).and_then(Value::as_str).map(pipeline_status),
        updated_at: string(value, "updated_at"),
    })
}

/// GitLab's pipeline status strings, as documented for the pipelines
/// API. Anything else is kept verbatim rather than guessed at.
pub fn pipeline_status(raw: &str) -> PipelineStatus {
    match raw {
        "success" => PipelineStatus::Success,
        "failed" => PipelineStatus::Failed,
        "running" => PipelineStatus::Running,
        "pending" | "created" | "waiting_for_resource" | "preparing" | "scheduled" => PipelineStatus::Pending,
        "canceled" | "canceling" => PipelineStatus::Canceled,
        "skipped" => PipelineStatus::Skipped,
        "manual" => PipelineStatus::Manual,
        other => PipelineStatus::Other(other.to_string()),
    }
}

/// `GET /projects/:id/merge_requests/:iid/approvals`.
///
/// `approvals_required`/`approvals_left` are Premium-tier fields: on a
/// Free instance they're simply absent, which reads here as "none
/// required", and the panel then shows who approved without claiming a
/// rule that doesn't exist.
pub fn approvals(value: &Value) -> Approvals {
    Approvals {
        approved: bool_at(value, "approved"),
        required: usize_at(value, "approvals_required"),
        left: usize_at(value, "approvals_left"),
        approved_by: value
            .get("approved_by")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    // Each entry wraps the approver as `{"user": {...}}`.
                    .filter_map(|e| e.get("user").or(Some(e)))
                    .map(|u| {
                        let name = string(u, "name");
                        if name.is_empty() {
                            string(u, "username")
                        } else {
                            name
                        }
                    })
                    .filter(|n| !n.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// One entry from `GET /projects/:id/merge_requests/:iid/discussions`.
///
/// A discussion's own resolved state isn't a field on it: GitLab
/// records resolution per *note*, and a thread counts as resolved when
/// every resolvable note in it is.
///
/// Resolvability is read the same way rather than inferred from whether
/// the thread has a diff position. Checked against a real instance
/// (GitLab 19.3): a plain comment on the merge request comes back as a
/// `DiscussionNote` with `resolvable: true` -- threads on a merge
/// request are resolvable whether or not they hang on a line. Only the
/// forge's own narration (`system: true`) isn't.
pub fn discussion(value: &Value) -> Option<Discussion> {
    let id = value.get("id").and_then(Value::as_str)?.to_string();
    let entries = value.get("notes").and_then(Value::as_array).cloned().unwrap_or_default();
    let notes: Vec<Note> = entries.iter().filter_map(note).collect();
    let resolvable = entries.iter().any(|n| bool_at(n, "resolvable"));
    let resolved = resolvable && entries.iter().filter(|n| bool_at(n, "resolvable")).all(|n| bool_at(n, "resolved"));
    // The position hangs off whichever note carries one -- the first
    // note of a diff thread does; replies to it don't.
    let position = entries.iter().find_map(|n| n.get("position")).and_then(position);
    Some(Discussion { id, notes, resolved, resolvable, position })
}

pub fn note(value: &Value) -> Option<Note> {
    Some(Note {
        id: value.get("id").and_then(Value::as_u64).unwrap_or(0),
        author: value
            .get("author")
            .map(|a| {
                let name = string(a, "name");
                if name.is_empty() {
                    string(a, "username")
                } else {
                    name
                }
            })
            .unwrap_or_default(),
        body: string(value, "body"),
        created_at: string(value, "created_at"),
        system: bool_at(value, "system"),
    })
}

/// A note's `position` object.
///
/// Only `text` positions are understood: an image comment anchors to
/// pixel coordinates on a rendered file, which a text diff has nowhere
/// to put. Returning `None` drops it from the inline rendering rather
/// than putting it on an arbitrary line.
pub fn position(value: &Value) -> Option<Position> {
    if value.get("position_type").and_then(Value::as_str).unwrap_or("text") != "text" {
        return None;
    }
    Some(Position {
        base_sha: string(value, "base_sha"),
        head_sha: string(value, "head_sha"),
        start_sha: string(value, "start_sha"),
        old_path: string(value, "old_path"),
        new_path: string(value, "new_path"),
        old_line: value.get("old_line").and_then(Value::as_u64).map(|n| n as usize),
        new_line: value.get("new_line").and_then(Value::as_u64).map(|n| n as usize),
    })
}

/// One entry from `GET /projects/:id/merge_requests/:iid/diffs`.
///
/// The older `/changes` endpoint (deprecated in GitLab 15.7) returns
/// the same entry shape under a `changes` key, so this parses both.
pub fn changed_file(value: &Value) -> Option<ChangedFile> {
    let old_path = string(value, "old_path");
    let new_path = string(value, "new_path");
    if old_path.is_empty() && new_path.is_empty() {
        return None;
    }
    // Checked most-specific first: a renamed file is also reported with
    // both paths set, and a new file that was also renamed can't
    // happen, so order settles every real combination.
    let change = if bool_at(value, "new_file") {
        FileChange::Added
    } else if bool_at(value, "deleted_file") {
        FileChange::Deleted
    } else if bool_at(value, "renamed_file") {
        FileChange::Renamed
    } else {
        FileChange::Modified
    };
    Some(ChangedFile {
        old_path: if old_path.is_empty() { new_path.clone() } else { old_path },
        new_path: if new_path.is_empty() { string(value, "old_path") } else { new_path },
        change,
        diff: string(value, "diff"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff_thread() -> Value {
        json!({
            "id": "abc123",
            "individual_note": false,
            "notes": [
                {"id": 1, "body": "This looks wrong.", "author": {"name": "Alice"}, "created_at": "2026-09-04T10:00:00Z",
                 "system": false, "resolvable": true, "resolved": false, "type": "DiffNote",
                 "position": {"position_type": "text", "base_sha": "b", "head_sha": "h", "start_sha": "s",
                              "old_path": "src/a.rs", "new_path": "src/a.rs", "old_line": null, "new_line": 12}},
                {"id": 2, "body": "Fixed.", "author": {"name": "Bob"}, "created_at": "2026-09-04T11:00:00Z",
                 "system": false, "resolvable": true, "resolved": false, "type": "DiffNote"}
            ]
        })
    }

    #[test]
    fn a_diff_thread_carries_its_notes_and_where_it_hangs() {
        let d = discussion(&diff_thread()).unwrap();
        assert_eq!(d.id, "abc123");
        assert_eq!(d.notes.len(), 2);
        assert_eq!(d.notes[0].author, "Alice");
        assert_eq!(d.notes[1].body, "Fixed.");
        assert!(d.resolvable);
        assert!(!d.resolved);
        let position = d.position.expect("a diff thread has a position");
        assert_eq!(position.new_path, "src/a.rs");
        assert_eq!(position.new_line, Some(12));
        assert_eq!(position.old_line, None);
    }

    #[test]
    fn a_thread_is_resolved_only_when_every_resolvable_note_is() {
        let mut value = diff_thread();
        value["notes"][0]["resolved"] = json!(true);
        assert!(!discussion(&value).unwrap().resolved, "one note still open keeps the thread open");
        value["notes"][1]["resolved"] = json!(true);
        assert!(discussion(&value).unwrap().resolved);
    }

    /// A plain comment on the merge request, shaped as a real instance
    /// returns it: a `DiscussionNote` with no position, and -- checked
    /// against GitLab 19.3 -- `resolvable: true`, which is not what
    /// "it isn't on a diff line" would suggest.
    #[test]
    fn a_plain_comment_on_the_merge_request_has_no_position_but_is_still_resolvable() {
        let value = json!({
            "id": "plain",
            "notes": [{"id": 9, "body": "Nice work.", "author": {"username": "carol"}, "type": "DiscussionNote",
                       "system": false, "resolvable": true, "resolved": false}]
        });
        let d = discussion(&value).unwrap();
        assert!(d.resolvable, "read from the flag, not inferred from the absence of a position");
        assert_eq!(d.position, None);
        assert_eq!(d.notes[0].author, "carol", "falls back to the username");
    }

    #[test]
    fn only_the_forges_own_narration_comes_back_unresolvable() {
        let value = json!({
            "id": "sys",
            "notes": [{"id": 9, "body": "approved this merge request", "author": {"name": "A"}, "system": true, "resolvable": false}]
        });
        assert!(!discussion(&value).unwrap().resolvable);
    }

    #[test]
    fn the_forges_own_narration_is_kept_but_marked_as_such() {
        let value = json!({
            "id": "sys",
            "notes": [{"id": 3, "body": "changed the description", "author": {"name": "Alice"}, "system": true}]
        });
        let d = discussion(&value).unwrap();
        assert!(d.notes[0].system);
        assert!(!d.is_human(), "so the panel can keep it off the diff, where it is pure noise");
    }

    #[test]
    fn an_image_comment_is_dropped_rather_than_put_on_an_arbitrary_line() {
        let value = json!({"position_type": "image", "base_sha": "b", "head_sha": "h", "start_sha": "s",
                           "new_path": "logo.png", "old_path": "logo.png", "x": 10, "y": 20});
        assert_eq!(position(&value), None);
    }

    #[test]
    fn a_discussion_with_no_id_is_skipped() {
        assert!(discussion(&json!({"notes": []})).is_none());
    }

    fn full_mr() -> Value {
        json!({
            "iid": 42,
            "title": "Add the thing",
            "description": "It does the thing.",
            "state": "opened",
            "draft": false,
            "source_branch": "feature/thing",
            "target_branch": "develop",
            "author": {"name": "Thomas Pedneault", "username": "tp"},
            "web_url": "https://gitlab.example.com/g/p/-/merge_requests/42",
            "has_conflicts": false,
            "sha": "abc1234",
            "diff_refs": {"base_sha": "base", "head_sha": "head", "start_sha": "start"},
            "user_notes_count": 3,
            "head_pipeline": {"status": "success"},
            "updated_at": "2026-09-04T10:00:00Z"
        })
    }

    #[test]
    fn reads_every_field_the_panel_shows() {
        let mr = merge_request(&full_mr()).unwrap();
        assert_eq!(mr.number, 42);
        assert_eq!(mr.title, "Add the thing");
        assert_eq!(mr.state, MrState::Open);
        assert_eq!(mr.source_branch, "feature/thing");
        assert_eq!(mr.target_branch, "develop");
        assert_eq!(mr.author, "Thomas Pedneault");
        assert_eq!(mr.comment_count, 3);
        assert_eq!(mr.pipeline, Some(PipelineStatus::Success));
        assert_eq!(mr.diff_refs.head_sha, "head");
    }

    #[test]
    fn an_mr_with_nothing_but_an_iid_still_parses() {
        // A self-hosted instance several versions behind won't send
        // half of these; a row that doesn't render is worse than a row
        // with blanks in it.
        let mr = merge_request(&json!({"iid": 7})).unwrap();
        assert_eq!(mr.number, 7);
        assert_eq!(mr.state, MrState::Open);
        assert_eq!(mr.pipeline, None);
        assert_eq!(mr.diff_refs, DiffRefs::default());
    }

    #[test]
    fn an_entry_with_no_iid_is_skipped_rather_than_faked() {
        assert!(merge_request(&json!({"title": "no iid"})).is_none());
    }

    #[test]
    fn an_author_with_no_display_name_falls_back_to_the_username() {
        let value = json!({"iid": 1, "author": {"username": "tp"}});
        assert_eq!(merge_request(&value).unwrap().author, "tp");
    }

    #[test]
    fn every_documented_state_maps_to_its_own_variant() {
        for (raw, expected) in
            [("opened", MrState::Open), ("merged", MrState::Merged), ("closed", MrState::Closed), ("locked", MrState::Locked)]
        {
            let value = json!({"iid": 1, "state": raw});
            assert_eq!(merge_request(&value).unwrap().state, expected, "for {raw}");
        }
    }

    #[test]
    fn pipeline_statuses_that_mean_waiting_all_read_as_pending() {
        for raw in ["pending", "created", "waiting_for_resource", "preparing", "scheduled"] {
            assert_eq!(pipeline_status(raw), PipelineStatus::Pending, "for {raw}");
        }
        assert_eq!(pipeline_status("something_new"), PipelineStatus::Other("something_new".to_string()));
    }

    #[test]
    fn approvals_name_the_people_who_approved() {
        let value = json!({
            "approved": true,
            "approvals_required": 2,
            "approvals_left": 0,
            "approved_by": [{"user": {"name": "Alice", "username": "a"}}, {"user": {"username": "bob"}}]
        });
        let approvals = approvals(&value);
        assert!(approvals.approved);
        assert_eq!(approvals.required, 2);
        assert_eq!(approvals.left, 0);
        assert_eq!(approvals.approved_by, vec!["Alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn approvals_on_an_instance_without_the_premium_fields_still_parse() {
        // `approvals_required`/`approvals_left` are Premium-only; their
        // absence means "no rule", not "broken response".
        let approvals = approvals(&json!({"approved": false, "approved_by": []}));
        assert_eq!(approvals.required, 0);
        assert!(approvals.approved_by.is_empty());
    }

    #[test]
    fn a_changed_file_reads_its_status_from_the_three_flags() {
        let cases = [
            (json!({"old_path": "a", "new_path": "a", "new_file": true}), FileChange::Added),
            (json!({"old_path": "a", "new_path": "a", "deleted_file": true}), FileChange::Deleted),
            (json!({"old_path": "a", "new_path": "b", "renamed_file": true}), FileChange::Renamed),
            (json!({"old_path": "a", "new_path": "a"}), FileChange::Modified),
        ];
        for (value, expected) in cases {
            assert_eq!(changed_file(&value).unwrap().change, expected, "for {value}");
        }
    }

    #[test]
    fn a_changed_file_with_no_paths_at_all_is_skipped() {
        assert!(changed_file(&json!({"diff": "@@ -1 +1 @@"})).is_none());
    }

    #[test]
    fn a_changed_files_diff_survives_verbatim() {
        let value = json!({"old_path": "a.rs", "new_path": "a.rs", "diff": "@@ -1,2 +1,2 @@\n-old\n+new\n"});
        assert_eq!(changed_file(&value).unwrap().diff, "@@ -1,2 +1,2 @@\n-old\n+new\n");
    }
}
