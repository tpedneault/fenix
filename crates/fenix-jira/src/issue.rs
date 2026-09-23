use crate::client::JiraClient;

/// One row of a search result -- just enough to list (key, summary,
/// status, assignee, when it last changed), not the full issue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueSummary {
    pub key: String,
    pub summary: String,
    pub status: String,
    /// Jira's own bucket for `status`: `"new"`, `"indeterminate"` or
    /// `"done"` -- the one thing about a status every workflow agrees on,
    /// whatever the status itself is called.
    pub status_category: String,
    pub assignee: Option<String>,
    pub updated: String,
}

/// One issue's own comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created: String,
}

/// A single issue's full detail (`GET /rest/api/2/issue/{key}`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueDetail {
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: String,
    /// The status's id -- stable across renames, unlike its name.
    pub status_id: String,
    /// See `IssueSummary::status_category`.
    pub status_category: String,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    /// The assignee's username (`name`), for "is this still mine" checks
    /// -- `assignee` is only the display name.
    pub assignee_id: Option<String>,
    /// Whether the board's Flagged (impediment) field is set -- only ever
    /// true when the fetch was told which field that is.
    pub flagged: bool,
    pub reporter: Option<String>,
    pub created: String,
    pub updated: String,
    pub comments: Vec<Comment>,
}

/// The JQL for "every issue assigned to `user_id`, scoped to whichever
/// projects are currently tracked, minus whichever statuses are
/// currently excluded" -- pure and directly testable. The `AND project
/// IN (...)` clause is omitted entirely when `project_keys` is empty
/// (an empty `IN ()` is invalid JQL, and "no projects tracked yet"
/// should mean "search everywhere," not "search nothing"), and project
/// keys are joined verbatim (Jira project keys are always plain
/// alphanumeric identifiers, no quoting/escaping needed the way a
/// free-text value would). `excluded_statuses` gets the same
/// conditional-clause treatment -- omitted when empty -- but each name
/// *is* quoted (unlike project keys, a status name routinely contains
/// spaces, e.g. `"In Progress"`), with any embedded `"` escaped so a
/// status name can't break out of its own JQL string literal.
/// Quotes `s` as a JQL string literal.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// Every open issue assigned to whoever the token belongs to, across
/// `project_keys` (everywhere when empty) -- what the agenda offers to
/// link or import.
pub fn build_my_open_issues_jql(project_keys: &[String]) -> String {
    let mut jql = "assignee = currentUser() AND statusCategory != Done".to_string();
    if !project_keys.is_empty() {
        jql.push_str(&format!(" AND project IN ({})", project_keys.join(",")));
    }
    jql.push_str(" ORDER BY updated DESC");
    jql
}

/// Exactly these issues -- how the agenda refreshes the ones it links to.
pub fn build_keys_jql(keys: &[String]) -> String {
    let quoted: Vec<String> = keys.iter().map(|k| quote(k)).collect();
    format!("key IN ({})", quoted.join(","))
}

/// The project a key belongs to (`PROJ-12` -> `PROJ`).
pub fn project_of(key: &str) -> &str {
    key.rsplit_once('-').map_or(key, |(project, _)| project)
}

const DETAIL_FIELDS: &str = "summary,description,status,priority,assignee,reporter,created,updated,comment";

pub fn build_jql(user_id: &str, project_keys: &[String], excluded_statuses: &[String]) -> String {
    let mut jql = format!("assignee = \"{user_id}\"");
    if !project_keys.is_empty() {
        jql.push_str(&format!(" AND project IN ({})", project_keys.join(",")));
    }
    if !excluded_statuses.is_empty() {
        let quoted: Vec<String> = excluded_statuses.iter().map(|s| format!("\"{}\"", s.replace('"', "\\\""))).collect();
        jql.push_str(&format!(" AND status NOT IN ({})", quoted.join(",")));
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
        self.get_issue_with(key, None)
    }

    /// `get_issue`, also reading the Flagged field when `flag_field`
    /// names it (see `find_flagged_field`).
    pub fn get_issue_with(&self, key: &str, flag_field: Option<&str>) -> Result<IssueDetail, String> {
        let path = format!("/rest/api/2/issue/{key}");
        let fields = detail_fields(flag_field);
        let body = self.request(&path, &[("fields", &fields)])?;
        parse_issue_detail(&body, flag_field).ok_or_else(|| "unexpected issue response shape".to_string())
    }

    /// `search_issues`, but every row carries the full detail (description,
    /// priority, comments...) -- one request refreshes every linked task.
    pub fn search_details(&self, jql: &str, max_results: u32, flag_field: Option<&str>) -> Result<Vec<IssueDetail>, String> {
        let max_results = max_results.to_string();
        let fields = detail_fields(flag_field);
        let body = self.request("/rest/api/2/search", &[("jql", jql), ("maxResults", &max_results), ("fields", &fields)])?;
        let issues = body.get("issues").and_then(|v| v.as_array()).ok_or_else(|| "unexpected search response shape".to_string())?;
        Ok(issues.iter().filter_map(|v| parse_issue_detail(v, flag_field)).collect())
    }
}

fn detail_fields(flag_field: Option<&str>) -> String {
    match flag_field {
        Some(field) => format!("{DETAIL_FIELDS},{field}"),
        None => DETAIL_FIELDS.to_string(),
    }
}

fn status_category(fields: &serde_json::Value) -> String {
    fields
        .get("status")
        .and_then(|s| s.get("statusCategory"))
        .and_then(|c| c.get("key"))
        .and_then(|k| k.as_str())
        .unwrap_or("new")
        .to_string()
}

/// A multi-checkbox custom field like Flagged is `null` or `[]` when
/// unset and `[{"value": "Impediment"}]` when set.
fn is_set(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
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
        status_category: status_category(fields),
        assignee: person_name(fields, "assignee"),
        updated: text_field(fields, "updated").unwrap_or_default(),
    })
}

fn parse_comment(v: &serde_json::Value) -> Option<Comment> {
    Some(Comment {
        id: v.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string(),
        author: v.get("author").and_then(|a| a.get("displayName")).and_then(|n| n.as_str()).unwrap_or("Unknown").to_string(),
        body: v.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string(),
        created: v.get("created").and_then(|c| c.as_str()).unwrap_or_default().to_string(),
    })
}

fn parse_issue_detail(v: &serde_json::Value, flag_field: Option<&str>) -> Option<IssueDetail> {
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
        status_id: fields.get("status").and_then(|s| s.get("id")).and_then(|n| n.as_str()).unwrap_or_default().to_string(),
        status_category: status_category(fields),
        priority: fields.get("priority").and_then(|p| p.get("name")).and_then(|n| n.as_str()).map(str::to_string),
        assignee: person_name(fields, "assignee"),
        assignee_id: fields.get("assignee").and_then(|a| a.get("name")).and_then(|n| n.as_str()).map(str::to_string),
        flagged: flag_field.is_some_and(|f| is_set(fields.get(f))),
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
        assert_eq!(build_jql("jo1111111", &[], &[]), r#"assignee = "jo1111111" ORDER BY updated DESC"#);
    }

    #[test]
    fn build_jql_with_one_project() {
        assert_eq!(
            build_jql("jo1111111", &["PROJ".to_string()], &[]),
            r#"assignee = "jo1111111" AND project IN (PROJ) ORDER BY updated DESC"#
        );
    }

    #[test]
    fn build_jql_with_several_projects_joins_them_with_commas() {
        assert_eq!(
            build_jql("jo1111111", &["PROJ".to_string(), "OTHER".to_string()], &[]),
            r#"assignee = "jo1111111" AND project IN (PROJ,OTHER) ORDER BY updated DESC"#
        );
    }

    #[test]
    fn build_jql_with_no_excluded_statuses_omits_the_status_clause() {
        assert_eq!(build_jql("jo1111111", &[], &[]), r#"assignee = "jo1111111" ORDER BY updated DESC"#);
    }

    #[test]
    fn build_jql_with_one_excluded_status() {
        assert_eq!(
            build_jql("jo1111111", &[], &["Done".to_string()]),
            r#"assignee = "jo1111111" AND status NOT IN ("Done") ORDER BY updated DESC"#
        );
    }

    #[test]
    fn build_jql_with_several_excluded_statuses_quotes_each_one() {
        assert_eq!(
            build_jql("jo1111111", &[], &["Done".to_string(), "In Progress".to_string()]),
            r#"assignee = "jo1111111" AND status NOT IN ("Done","In Progress") ORDER BY updated DESC"#
        );
    }

    #[test]
    fn build_jql_escapes_an_embedded_double_quote_in_an_excluded_status() {
        // Contrived (real workflow status names essentially never
        // contain a literal `"`), but confirms the escaping code path
        // actually runs rather than producing broken JQL with an
        // unescaped quote breaking out of the string literal.
        assert_eq!(
            build_jql("jo1111111", &[], &["Weird\"Status".to_string()]),
            r#"assignee = "jo1111111" AND status NOT IN ("Weird\"Status") ORDER BY updated DESC"#
        );
    }

    #[test]
    fn build_jql_combines_project_and_status_clauses() {
        assert_eq!(
            build_jql("jo1111111", &["PROJ".to_string()], &["Done".to_string()]),
            r#"assignee = "jo1111111" AND project IN (PROJ) AND status NOT IN ("Done") ORDER BY updated DESC"#
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
        let detail = parse_issue_detail(&v, None).unwrap();
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
        let detail = parse_issue_detail(&v, None).unwrap();
        assert_eq!(detail.description, None);
        assert!(detail.comments.is_empty());
    }

    #[test]
    fn parse_issue_detail_returns_none_for_a_malformed_body() {
        let v: serde_json::Value = serde_json::from_str(r#"{"nope": true}"#).unwrap();
        assert!(parse_issue_detail(&v, None).is_none());
    }

    #[test]
    fn my_open_issues_jql_scopes_to_projects_and_skips_done() {
        assert_eq!(build_my_open_issues_jql(&[]), "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC");
        assert_eq!(
            build_my_open_issues_jql(&["PROJ".to_string(), "OPS".to_string()]),
            "assignee = currentUser() AND statusCategory != Done AND project IN (PROJ,OPS) ORDER BY updated DESC"
        );
    }

    #[test]
    fn keys_jql_quotes_every_key() {
        assert_eq!(build_keys_jql(&["PROJ-1".to_string(), "OPS-22".to_string()]), r#"key IN ("PROJ-1","OPS-22")"#);
    }

    #[test]
    fn project_of_strips_the_issue_number() {
        assert_eq!(project_of("PROJ-12"), "PROJ");
        assert_eq!(project_of("MY-TEAM-3"), "MY-TEAM");
    }

    #[test]
    fn parse_issue_detail_reads_status_category_priority_assignee_id_and_flag() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"key": "PROJ-3", "fields": {
                "summary": "S", "status": {"id": "10103", "name": "On Hold", "statusCategory": {"key": "indeterminate"}},
                "priority": {"name": "Major"}, "assignee": {"name": "jo1", "displayName": "Jo"},
                "customfield_10000": [{"value": "Impediment"}],
                "comment": {"comments": [{"id": "77", "author": {"displayName": "Jo"}, "body": "hi", "created": "x"}]}
            }}"#,
        )
        .unwrap();
        let detail = parse_issue_detail(&v, Some("customfield_10000")).unwrap();
        assert_eq!(detail.status_id, "10103");
        assert_eq!(detail.status_category, "indeterminate");
        assert_eq!(detail.priority.as_deref(), Some("Major"));
        assert_eq!(detail.assignee_id.as_deref(), Some("jo1"));
        assert!(detail.flagged);
        assert_eq!(detail.comments[0].id, "77");
        assert!(!parse_issue_detail(&v, None).unwrap().flagged, "no flag field named means not flagged");
    }
}
