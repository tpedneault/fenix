use crate::client::JiraClient;

/// One row of a search result -- just enough to list (key, summary,
/// status, assignee, when it last changed), not the full issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSummary {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub assignee: Option<String>,
    pub updated: String,
}

/// One issue's own comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub created: String,
}

/// A single issue's full detail (`GET /rest/api/2/issue/{key}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueDetail {
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: String,
    pub assignee: Option<String>,
    pub reporter: Option<String>,
    pub created: String,
    pub updated: String,
    pub comments: Vec<Comment>,
}

/// The JQL for "every issue assigned to `user_id`, scoped to whichever
/// projects are currently tracked" -- pure and directly testable. The
/// `AND project IN (...)` clause is omitted entirely when `project_keys`
/// is empty (an empty `IN ()` is invalid JQL, and "no projects tracked
/// yet" should mean "search everywhere," not "search nothing"), and
/// project keys are joined verbatim (Jira project keys are always plain
/// alphanumeric identifiers, no quoting/escaping needed the way a
/// free-text value would).
pub fn build_jql(user_id: &str, project_keys: &[String]) -> String {
    let mut jql = format!("assignee = \"{user_id}\"");
    if !project_keys.is_empty() {
        jql.push_str(&format!(" AND project IN ({})", project_keys.join(",")));
    }
    jql.push_str(" ORDER BY updated DESC");
    jql
}

impl JiraClient {
    /// `GET /rest/api/2/search` -- runs `jql`, returns up to
    /// `max_results` matching issues as `IssueSummary`s.
    pub fn search_issues(&self, jql: &str, max_results: u32) -> Result<Vec<IssueSummary>, String> {
        let max_results = max_results.to_string();
        let body = self.request(
            "/rest/api/2/search",
            &[("jql", jql), ("maxResults", &max_results), ("fields", "summary,status,assignee,updated")],
        )?;
        let issues = body.get("issues").and_then(|v| v.as_array()).ok_or_else(|| "unexpected search response shape".to_string())?;
        Ok(issues.iter().filter_map(parse_issue_summary).collect())
    }

    /// `GET /rest/api/2/issue/{key}` -- Jira embeds comments under
    /// `fields.comment.comments[]` when that field is requested, so one
    /// call covers the full detail view including comments, no separate
    /// paginated fetch needed.
    pub fn get_issue(&self, key: &str) -> Result<IssueDetail, String> {
        let path = format!("/rest/api/2/issue/{key}");
        let body = self.request(&path, &[("fields", "summary,description,status,assignee,reporter,created,updated,comment")])?;
        parse_issue_detail(&body).ok_or_else(|| "unexpected issue response shape".to_string())
    }
}

fn text_field(fields: &serde_json::Value, name: &str) -> Option<String> {
    fields.get(name).and_then(|v| v.as_str()).map(str::to_string)
}

fn person_name(fields: &serde_json::Value, name: &str) -> Option<String> {
    fields.get(name)?.get("displayName")?.as_str().map(str::to_string)
}

fn parse_issue_summary(v: &serde_json::Value) -> Option<IssueSummary> {
    let key = v.get("key")?.as_str()?.to_string();
    let fields = v.get("fields")?;
    Some(IssueSummary {
        key,
        summary: text_field(fields, "summary").unwrap_or_default(),
        status: fields.get("status").and_then(|s| s.get("name")).and_then(|n| n.as_str()).unwrap_or("Unknown").to_string(),
        assignee: person_name(fields, "assignee"),
        updated: text_field(fields, "updated").unwrap_or_default(),
    })
}

fn parse_comment(v: &serde_json::Value) -> Option<Comment> {
    Some(Comment {
        author: v.get("author").and_then(|a| a.get("displayName")).and_then(|n| n.as_str()).unwrap_or("Unknown").to_string(),
        body: v.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string(),
        created: v.get("created").and_then(|c| c.as_str()).unwrap_or_default().to_string(),
    })
}

fn parse_issue_detail(v: &serde_json::Value) -> Option<IssueDetail> {
    let key = v.get("key")?.as_str()?.to_string();
    let fields = v.get("fields")?;
    let comments = fields
        .get("comment")
        .and_then(|c| c.get("comments"))
        .and_then(|c| c.as_array())
        .map(|arr| arr.iter().filter_map(parse_comment).collect())
        .unwrap_or_default();
    Some(IssueDetail {
        key,
        summary: text_field(fields, "summary").unwrap_or_default(),
        description: text_field(fields, "description"),
        status: fields.get("status").and_then(|s| s.get("name")).and_then(|n| n.as_str()).unwrap_or("Unknown").to_string(),
        assignee: person_name(fields, "assignee"),
        reporter: person_name(fields, "reporter"),
        created: text_field(fields, "created").unwrap_or_default(),
        updated: text_field(fields, "updated").unwrap_or_default(),
        comments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_jql_with_no_tracked_projects_omits_the_project_clause() {
        assert_eq!(build_jql("jo1111111", &[]), r#"assignee = "jo1111111" ORDER BY updated DESC"#);
    }

    #[test]
    fn build_jql_with_one_project() {
        assert_eq!(
            build_jql("jo1111111", &["PROJ".to_string()]),
            r#"assignee = "jo1111111" AND project IN (PROJ) ORDER BY updated DESC"#
        );
    }

    #[test]
    fn build_jql_with_several_projects_joins_them_with_commas() {
        assert_eq!(
            build_jql("jo1111111", &["PROJ".to_string(), "OTHER".to_string()]),
            r#"assignee = "jo1111111" AND project IN (PROJ,OTHER) ORDER BY updated DESC"#
        );
    }

    fn search_response(issues: &str) -> serde_json::Value {
        serde_json::from_str(&format!(r#"{{"issues": [{issues}]}}"#)).unwrap()
    }

    #[test]
    fn parse_issue_summary_reads_a_typical_search_result_row() {
        let v = search_response(
            r#"{"key": "PROJ-1", "fields": {"summary": "Fix the thing", "status": {"name": "In Progress"}, "assignee": {"displayName": "John Doe"}, "updated": "2024-01-15T10:30:00.000+0000"}}"#,
        );
        let issue = parse_issue_summary(&v["issues"][0]).unwrap();
        assert_eq!(issue.key, "PROJ-1");
        assert_eq!(issue.summary, "Fix the thing");
        assert_eq!(issue.status, "In Progress");
        assert_eq!(issue.assignee, Some("John Doe".to_string()));
        assert_eq!(issue.updated, "2024-01-15T10:30:00.000+0000");
    }

    #[test]
    fn parse_issue_summary_handles_an_unassigned_issue() {
        let v = search_response(r#"{"key": "PROJ-2", "fields": {"summary": "Unassigned", "status": {"name": "Open"}, "assignee": null, "updated": "2024-01-01T00:00:00.000+0000"}}"#);
        let issue = parse_issue_summary(&v["issues"][0]).unwrap();
        assert_eq!(issue.assignee, None);
    }

    #[test]
    fn parse_issue_summary_returns_none_for_a_malformed_row() {
        let v: serde_json::Value = serde_json::from_str(r#"{"nope": true}"#).unwrap();
        assert!(parse_issue_summary(&v).is_none());
    }

    #[test]
    fn parse_issue_detail_reads_description_status_people_and_comments() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
                "key": "PROJ-1",
                "fields": {
                    "summary": "Fix the thing",
                    "description": "Full description here",
                    "status": {"name": "In Progress"},
                    "assignee": {"displayName": "John Doe"},
                    "reporter": {"displayName": "Jane Smith"},
                    "created": "2024-01-01T00:00:00.000+0000",
                    "updated": "2024-01-15T10:30:00.000+0000",
                    "comment": {"comments": [
                        {"author": {"displayName": "Jane Smith"}, "body": "Looking into it", "created": "2024-01-02T00:00:00.000+0000"}
                    ]}
                }
            }"#,
        )
        .unwrap();
        let detail = parse_issue_detail(&v).unwrap();
        assert_eq!(detail.key, "PROJ-1");
        assert_eq!(detail.description, Some("Full description here".to_string()));
        assert_eq!(detail.reporter, Some("Jane Smith".to_string()));
        assert_eq!(detail.comments.len(), 1);
        assert_eq!(detail.comments[0].author, "Jane Smith");
        assert_eq!(detail.comments[0].body, "Looking into it");
    }

    #[test]
    fn parse_issue_detail_handles_a_missing_description_and_no_comments() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"key": "PROJ-2", "fields": {"summary": "No description", "description": null, "status": {"name": "Open"}, "assignee": null, "reporter": null, "created": "2024-01-01T00:00:00.000+0000", "updated": "2024-01-01T00:00:00.000+0000"}}"#,
        )
        .unwrap();
        let detail = parse_issue_detail(&v).unwrap();
        assert_eq!(detail.description, None);
        assert!(detail.comments.is_empty());
    }

    #[test]
    fn parse_issue_detail_returns_none_for_a_malformed_body() {
        let v: serde_json::Value = serde_json::from_str(r#"{"nope": true}"#).unwrap();
        assert!(parse_issue_detail(&v).is_none());
    }
}
