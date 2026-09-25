//! Creating an issue the way the project wants it: its real issue types,
//! and the fields a type requires, from Jira's create metadata -- rather
//! than a type name typed in and hoped for.

use crate::client::JiraClient;

/// One of a project's issue types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueType {
    pub id: String,
    pub name: String,
    pub subtask: bool,
}

/// A field an issue type has on its create screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateField {
    pub id: String,
    pub name: String,
    pub required: bool,
    /// The values it takes, as (id, name), when it's a choice.
    pub allowed: Vec<(String, String)>,
    /// `"array"` for a multi-value field (components, versions), else the
    /// schema's own type (`"string"`, `"option"`, `"user"`, ...).
    pub kind: String,
}

/// Fields every create form already has its own row for.
const COVERED: [&str; 7] = ["project", "issuetype", "summary", "description", "priority", "assignee", "reporter"];

impl CreateField {
    /// Whether the form must ask for it: required, and not one of its own
    /// rows.
    pub fn asked(&self) -> bool {
        self.required && !COVERED.contains(&self.id.as_str())
    }

    /// The JSON Jira wants for `value` (an allowed value's id, or text).
    pub fn value_json(&self, value: &str) -> serde_json::Value {
        let one = if self.allowed.is_empty() { serde_json::json!(value) } else { serde_json::json!({"id": value}) };
        if self.kind == "array" { serde_json::json!([one]) } else { one }
    }
}

/// What a new issue is made of.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewIssue {
    pub project: String,
    pub issue_type_id: String,
    pub summary: String,
    pub description: String,
    /// A priority's name; `None` leaves the project's default.
    pub priority: Option<String>,
    /// A username; `None` leaves it to the project.
    pub assignee: Option<String>,
    /// Required fields beyond those, as (field id, value json).
    pub extra: Vec<(String, serde_json::Value)>,
}

impl NewIssue {
    pub fn to_json(&self) -> serde_json::Value {
        let mut fields = serde_json::json!({
            "project": {"key": self.project},
            "issuetype": {"id": self.issue_type_id},
            "summary": self.summary,
        });
        let map = fields.as_object_mut().expect("an object");
        if !self.description.trim().is_empty() {
            map.insert("description".into(), serde_json::json!(self.description));
        }
        if let Some(p) = &self.priority {
            map.insert("priority".into(), serde_json::json!({"name": p}));
        }
        // `Some("")` leaves it unassigned.
        match self.assignee.as_deref() {
            Some("") => {
                map.insert("assignee".into(), serde_json::Value::Null);
            }
            Some(a) => {
                map.insert("assignee".into(), serde_json::json!({"name": a}));
            }
            None => {}
        }
        for (id, value) in &self.extra {
            map.insert(id.clone(), value.clone());
        }
        serde_json::json!({"fields": fields})
    }
}

impl JiraClient {
    /// A project's issue types, from `GET .../createmeta/{project}/
    /// issuetypes` (Jira 8.4 and later), falling back to the older
    /// `createmeta?projectKeys=` form.
    pub fn issue_types(&self, project: &str) -> Result<Vec<IssueType>, String> {
        match self.request(&format!("/rest/api/2/issue/createmeta/{project}/issuetypes"), &[]) {
            Ok(body) => Ok(values(&body).iter().filter_map(parse_issue_type).collect()),
            Err(_) => {
                let body = self.request("/rest/api/2/issue/createmeta", &[("projectKeys", project)])?;
                let types = body
                    .get("projects")
                    .and_then(|p| p.as_array())
                    .and_then(|p| p.first())
                    .and_then(|p| p.get("issuetypes"))
                    .and_then(|t| t.as_array())
                    .ok_or_else(|| format!("no issue types for {project} -- is the key right?"))?;
                Ok(types.iter().filter_map(parse_issue_type).collect())
            }
        }
    }

    /// The fields issue type `type_id` has in `project`.
    pub fn create_fields(&self, project: &str, type_id: &str) -> Result<Vec<CreateField>, String> {
        match self.request(&format!("/rest/api/2/issue/createmeta/{project}/issuetypes/{type_id}"), &[]) {
            Ok(body) => Ok(values(&body).iter().filter_map(|v| parse_field(v.get("fieldId").and_then(|f| f.as_str())?, v)).collect()),
            Err(_) => {
                let body = self.request(
                    "/rest/api/2/issue/createmeta",
                    &[("projectKeys", project), ("issuetypeIds", type_id), ("expand", "projects.issuetypes.fields")],
                )?;
                let fields = body
                    .get("projects")
                    .and_then(|p| p.as_array())
                    .and_then(|p| p.first())
                    .and_then(|p| p.get("issuetypes"))
                    .and_then(|t| t.as_array())
                    .and_then(|t| t.first())
                    .and_then(|t| t.get("fields"))
                    .and_then(|f| f.as_object())
                    .ok_or_else(|| "no fields came back for that issue type".to_string())?;
                Ok(fields.iter().filter_map(|(id, v)| parse_field(id, v)).collect())
            }
        }
    }

    /// `POST /rest/api/2/issue` with everything the form collected;
    /// returns the new key.
    pub fn create(&self, issue: &NewIssue) -> Result<String, String> {
        let response = self.send("POST", "/rest/api/2/issue", &issue.to_json())?;
        response
            .and_then(|v| v.get("key").and_then(|k| k.as_str()).map(str::to_string))
            .ok_or_else(|| "unexpected create-issue response shape".to_string())
    }
}

/// `values` of a paged response, or the response itself when it's a list.
fn values(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body.get("values").and_then(|v| v.as_array()).or_else(|| body.as_array()).cloned().unwrap_or_default()
}

fn parse_issue_type(v: &serde_json::Value) -> Option<IssueType> {
    Some(IssueType {
        id: v.get("id")?.as_str()?.to_string(),
        name: v.get("name")?.as_str()?.to_string(),
        subtask: v.get("subtask").and_then(|s| s.as_bool()).unwrap_or(false),
    })
}

fn parse_field(id: &str, v: &serde_json::Value) -> Option<CreateField> {
    let allowed = v
        .get("allowedValues")
        .and_then(|a| a.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let id = item.get("id")?.as_str()?.to_string();
                    let name = item.get("name").or_else(|| item.get("value")).and_then(|n| n.as_str())?.to_string();
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(CreateField {
        id: id.to_string(),
        name: v.get("name").and_then(|n| n.as_str()).unwrap_or(id).to_string(),
        required: v.get("required").and_then(|r| r.as_bool()).unwrap_or(false),
        allowed,
        kind: v.get("schema").and_then(|s| s.get("type")).and_then(|t| t.as_str()).unwrap_or("string").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_types_and_fields_are_read_from_create_metadata() {
        let types: serde_json::Value = serde_json::from_str(r#"{"values": [{"id": "1", "name": "Bug", "subtask": false}, {"id": "5", "name": "Sub-task", "subtask": true}]}"#).unwrap();
        let parsed: Vec<IssueType> = values(&types).iter().filter_map(parse_issue_type).collect();
        assert_eq!(parsed[0], IssueType { id: "1".into(), name: "Bug".into(), subtask: false });
        assert!(parsed[1].subtask);

        let field: serde_json::Value = serde_json::from_str(
            r#"{"fieldId": "components", "name": "Component/s", "required": true, "schema": {"type": "array"},
                "allowedValues": [{"id": "10", "name": "UI"}, {"id": "11", "name": "Sync"}]}"#,
        )
        .unwrap();
        let f = parse_field("components", &field).unwrap();
        assert!(f.asked());
        assert_eq!(f.allowed[1], ("11".to_string(), "Sync".to_string()));
        assert_eq!(f.value_json("11"), serde_json::json!([{"id": "11"}]));
        assert!(!parse_field("summary", &serde_json::json!({"required": true})).unwrap().asked(), "the form has its own row");
    }

    #[test]
    fn a_new_issue_sends_only_what_was_given() {
        let issue = NewIssue {
            project: "FEN".into(),
            issue_type_id: "1".into(),
            summary: "Crash".into(),
            priority: Some("Major".into()),
            extra: vec![("components".into(), serde_json::json!([{"id": "10"}]))],
            ..Default::default()
        };
        let json = issue.to_json();
        assert_eq!(json["fields"]["project"]["key"], "FEN");
        assert_eq!(json["fields"]["priority"]["name"], "Major");
        assert!(json["fields"].get("description").is_none());
        assert!(json["fields"].get("assignee").is_none());
        assert_eq!(json["fields"]["components"][0]["id"], "10");
    }
}
