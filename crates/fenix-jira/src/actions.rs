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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transition {
    pub id: String,
    pub name: String,
    /// The status this transition lands on -- id, name and category
    /// (`"new"`/`"indeterminate"`/`"done"`), from the response's own
    /// `to` object. Empty when a server leaves `to` out.
    pub to_id: String,
    pub to_name: String,
    pub to_category: String,
}

/// One status a project's workflows use (`GET .../project/{key}/
/// statuses`), deduplicated across issue types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusInfo {
    pub id: String,
    pub name: String,
    pub category: String,
}

/// One of the instance's real configured priorities (`GET .../priority`)
/// -- `id` is Jira-internal and unused here; `name` is what both the
/// picker shows and `update_priority` sends back (the same name-based
/// convention `update_assignee`'s `name` field and `apply_transition`'s
/// workflow names already use). Fetched live rather than hardcoded --
/// see `list_priorities`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Priority {
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

    /// `PUT /rest/api/2/issue/{key}/assignee` -- its own dedicated
    /// endpoint, *not* the general fields-update `PUT /issue/{key}`
    /// `update_summary`/`update_description` use. Body is `{"name":
    /// user_id}`, Server/DC's own username-based convention (not Cloud's
    /// `accountId`) -- matches the plain-username `id` shape tracked
    /// users already use (e.g. `"jo1111111"`).
    pub fn update_assignee(&self, key: &str, user_id: &str) -> Result<(), String> {
        let path = format!("/rest/api/2/issue/{key}/assignee");
        self.send("PUT", &path, &serde_json::json!({"name": user_id}))?;
        Ok(())
    }

    /// `GET /rest/api/2/priority` -- the instance's real configured
    /// priority scheme, fetched live rather than hardcoded (a guessed
    /// default like Highest/High/Medium/Low/Lowest might not match what
    /// this instance actually has configured). Unlike `list_transitions`'
    /// response, this one is a plain top-level JSON array, not wrapped in
    /// an object field.
    pub fn list_priorities(&self) -> Result<Vec<Priority>, String> {
        let body = self.request("/rest/api/2/priority", &[])?;
        let priorities = body.as_array().ok_or_else(|| "unexpected priority response shape".to_string())?;
        Ok(priorities.iter().filter_map(parse_priority).collect())
    }

    /// Same shape as `update_summary`/`update_description` -- `PUT
    /// /rest/api/2/issue/{key}` via the shared `update_fields` helper,
    /// with `{"priority": {"name": priority_name}}`.
    pub fn update_priority(&self, key: &str, priority_name: &str) -> Result<(), String> {
        self.update_fields(key, serde_json::json!({"priority": {"name": priority_name}}))
    }

    /// `POST .../worklog` with the real start time and an exact length in
    /// seconds -- so a worklog sent on Friday for Tuesday's work lands on
    /// Tuesday. `started` is Jira's own `yyyy-MM-ddTHH:mm:ss.SSS+hhmm`.
    pub fn add_worklog_at(&self, key: &str, seconds: i64, started: &str) -> Result<(), String> {
        let path = format!("/rest/api/2/issue/{key}/worklog");
        self.send("POST", &path, &serde_json::json!({"timeSpentSeconds": seconds, "started": started}))?;
        Ok(())
    }

    /// `GET /rest/api/2/myself` -- the token owner's username.
    pub fn myself(&self) -> Result<String, String> {
        let body = self.request("/rest/api/2/myself", &[])?;
        body.get("name").and_then(|n| n.as_str()).map(str::to_string).ok_or_else(|| "unexpected myself response shape".to_string())
    }

    /// `GET /rest/api/2/project/{key}/statuses` -- every status any of the
    /// project's issue types can be in, each once.
    pub fn list_project_statuses(&self, project_key: &str) -> Result<Vec<StatusInfo>, String> {
        let path = format!("/rest/api/2/project/{project_key}/statuses");
        let body = self.request(&path, &[])?;
        let types = body.as_array().ok_or_else(|| "unexpected project statuses response shape".to_string())?;
        let mut statuses: Vec<StatusInfo> = Vec::new();
        for status in types.iter().filter_map(|t| t.get("statuses").and_then(|s| s.as_array())).flatten().filter_map(parse_status) {
            if !statuses.iter().any(|s| s.id == status.id) {
                statuses.push(status);
            }
        }
        Ok(statuses)
    }

    /// The id of the instance's Flagged custom field (Jira Software's
    /// impediment marker), from `GET /rest/api/2/field`. `Ok(None)` when
    /// the instance has no such field.
    pub fn find_flagged_field(&self) -> Result<Option<String>, String> {
        let body = self.request("/rest/api/2/field", &[])?;
        let fields = body.as_array().ok_or_else(|| "unexpected field response shape".to_string())?;
        Ok(fields
            .iter()
            .find(|f| f.get("name").and_then(|n| n.as_str()) == Some("Flagged"))
            .and_then(|f| f.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_string))
    }

    /// Sets or clears the Flagged field (`field_id` from
    /// `find_flagged_field`).
    pub fn set_flagged(&self, key: &str, field_id: &str, flagged: bool) -> Result<(), String> {
        let value = if flagged { serde_json::json!([{"value": "Impediment"}]) } else { serde_json::Value::Null };
        self.update_fields(key, serde_json::json!({ field_id: value }))
    }
}

fn parse_transition(v: &serde_json::Value) -> Option<Transition> {
    let to = v.get("to");
    let text = |value: Option<&serde_json::Value>| value.and_then(|x| x.as_str()).unwrap_or_default().to_string();
    Some(Transition {
        id: v.get("id")?.as_str()?.to_string(),
        name: v.get("name")?.as_str()?.to_string(),
        to_id: text(to.and_then(|t| t.get("id"))),
        to_name: text(to.and_then(|t| t.get("name"))),
        to_category: text(to.and_then(|t| t.get("statusCategory")).and_then(|c| c.get("key"))),
    })
}

fn parse_status(v: &serde_json::Value) -> Option<StatusInfo> {
    Some(StatusInfo {
        id: v.get("id")?.as_str()?.to_string(),
        name: v.get("name")?.as_str()?.to_string(),
        category: v.get("statusCategory").and_then(|c| c.get("key")).and_then(|k| k.as_str()).unwrap_or("new").to_string(),
    })
}

fn parse_priority(v: &serde_json::Value) -> Option<Priority> {
    Some(Priority { id: v.get("id")?.as_str()?.to_string(), name: v.get("name")?.as_str()?.to_string() })
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
    fn parse_transition_reads_its_target_status() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"id": "41", "name": "Put on hold", "to": {"id": "10103", "name": "On Hold", "statusCategory": {"key": "indeterminate"}}}"#,
        )
        .unwrap();
        let transition = parse_transition(&v).unwrap();
        assert_eq!(transition.to_id, "10103");
        assert_eq!(transition.to_name, "On Hold");
        assert_eq!(transition.to_category, "indeterminate");
    }

    #[test]
    fn parse_status_defaults_a_missing_category_to_new() {
        let v: serde_json::Value = serde_json::from_str(r#"{"id": "1", "name": "Open"}"#).unwrap();
        assert_eq!(parse_status(&v).unwrap().category, "new");
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

    #[test]
    fn parse_priority_reads_a_typical_row() {
        let v: serde_json::Value = serde_json::from_str(r#"{"id": "3", "name": "Medium"}"#).unwrap();
        let priority = parse_priority(&v).unwrap();
        assert_eq!(priority.id, "3");
        assert_eq!(priority.name, "Medium");
    }

    #[test]
    fn parse_priority_returns_none_for_a_malformed_row() {
        let v: serde_json::Value = serde_json::from_str(r#"{"nope": true}"#).unwrap();
        assert!(parse_priority(&v).is_none());
    }

    #[test]
    fn parse_priority_returns_none_when_name_is_missing() {
        let v: serde_json::Value = serde_json::from_str(r#"{"id": "3"}"#).unwrap();
        assert!(parse_priority(&v).is_none());
    }
}
