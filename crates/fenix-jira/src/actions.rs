//! The write side of the API -- create/update issues, comments,
//! transitions, worklogs. Mirrors `fenix_git`'s own read (`status.rs`/
//! `diff.rs`/`log.rs`) versus write (`actions.rs`) split: `issue.rs`
//! stays read-only (search + single-issue fetch), everything that
//! mutates something on the Jira side lives here instead.

use crate::client::JiraClient;

/// One available workflow transition for an issue (`GET .../
/// transitions`) -- `id` is what `apply_transition` needs to send
/// back; `name` is what a user picks by (the workflow's own status
/// name, e.g. "In Progress"). Entirely workflow-defined per project/
/// issue type, not a fixed enum -- there's no way to know the real set
/// without asking the issue itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub id: String,
    pub name: String,
}

impl JiraClient {
    /// `POST /rest/api/2/issue` -- creates a new issue, returns its key
    /// (e.g. `PROJ-123`). `issue_type` is a plain typed name (`"Task"`,
    /// `"Bug"`, ...), not validated against the project's actual
    /// allowed types before sending -- no `createmeta` lookup endpoint,
    /// matching `fenix-jira`'s existing "typed in, not looked up"
    /// posture for tracked projects/users.
    pub fn create_issue(&self, project_key: &str, issue_type: &str, summary: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "fields": {
                "project": {"key": project_key},
                "issuetype": {"name": issue_type},
                "summary": summary,
            }
        });
        let response = self.send("POST", "/rest/api/2/issue", &body)?;
        response
            .and_then(|v| v.get("key").and_then(|k| k.as_str()).map(str::to_string))
            .ok_or_else(|| "unexpected create-issue response shape".to_string())
    }

    /// `PUT /rest/api/2/issue/{key}` with just `{"fields": {"summary":
    /// ...}}` -- updates the title, leaving every other field alone.
    pub fn update_summary(&self, key: &str, summary: &str) -> Result<(), String> {
        self.update_fields(key, serde_json::json!({"summary": summary}))
    }

    /// Same shape as `update_summary`, for the description field.
    pub fn update_description(&self, key: &str, description: &str) -> Result<(), String> {
        self.update_fields(key, serde_json::json!({"description": description}))
    }

    fn update_fields(&self, key: &str, fields: serde_json::Value) -> Result<(), String> {
        let path = format!("/rest/api/2/issue/{key}");
        self.send("PUT", &path, &serde_json::json!({"fields": fields}))?;
        Ok(())
    }

    /// `POST /rest/api/2/issue/{key}/comment`.
    pub fn add_comment(&self, key: &str, body: &str) -> Result<(), String> {
        let path = format!("/rest/api/2/issue/{key}/comment");
        self.send("POST", &path, &serde_json::json!({"body": body}))?;
        Ok(())
    }

    /// `GET /rest/api/2/issue/{key}/transitions` -- every transition the
    /// issue's *current* status can move to right now.
    pub fn list_transitions(&self, key: &str) -> Result<Vec<Transition>, String> {
        let path = format!("/rest/api/2/issue/{key}/transitions");
        let body = self.request(&path, &[])?;
        let transitions =
            body.get("transitions").and_then(|v| v.as_array()).ok_or_else(|| "unexpected transitions response shape".to_string())?;
        Ok(transitions.iter().filter_map(parse_transition).collect())
    }

    /// `POST /rest/api/2/issue/{key}/transitions` with the chosen
    /// transition's id (from `list_transitions`).
    pub fn apply_transition(&self, key: &str, transition_id: &str) -> Result<(), String> {
        let path = format!("/rest/api/2/issue/{key}/transitions");
        self.send("POST", &path, &serde_json::json!({"transition": {"id": transition_id}}))?;
        Ok(())
    }

    /// `POST /rest/api/2/issue/{key}/worklog` -- `time_spent` in Jira's
    /// own duration syntax (`"2h 30m"`, `"1d"`, ...), sent verbatim; no
    /// client-side validation of the format.
    pub fn add_worklog(&self, key: &str, time_spent: &str) -> Result<(), String> {
        let path = format!("/rest/api/2/issue/{key}/worklog");
        self.send("POST", &path, &serde_json::json!({"timeSpent": time_spent}))?;
        Ok(())
    }
}

fn parse_transition(v: &serde_json::Value) -> Option<Transition> {
    Some(Transition { id: v.get("id")?.as_str()?.to_string(), name: v.get("name")?.as_str()?.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_transition_reads_a_typical_row() {
        let v: serde_json::Value = serde_json::from_str(r#"{"id": "31", "name": "In Progress"}"#).unwrap();
        let transition = parse_transition(&v).unwrap();
        assert_eq!(transition.id, "31");
        assert_eq!(transition.name, "In Progress");
    }

    #[test]
    fn parse_transition_returns_none_for_a_malformed_row() {
        let v: serde_json::Value = serde_json::from_str(r#"{"nope": true}"#).unwrap();
        assert!(parse_transition(&v).is_none());
    }

    #[test]
    fn parse_transition_returns_none_when_name_is_missing() {
        let v: serde_json::Value = serde_json::from_str(r#"{"id": "31"}"#).unwrap();
        assert!(parse_transition(&v).is_none());
    }
}
