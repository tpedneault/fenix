use fenix_jira::{IssueDetail, IssueSummary};

/// What a jira-panel line represents -- what the pane-specific action
/// keys (open/select) act on when the cursor is on this line. Mirrors
/// `git_panel::GitEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraEntry {
    Project(String),
    User(String),
    Issue(String),
}

/// How one generated line should be colored -- mirrors
/// `git_panel::GitLineStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraLineStyle {
    Project,
    User,
    Issue,
    /// A `label: value` row in the Detail pane.
    Detail,
    /// A comment's own header (`author @ timestamp`) or body line in the
    /// Detail pane.
    Comment,
    /// Shown when a list comes back empty, or nothing is selected yet.
    Empty,
}

/// A coarse "how's this doing" bucket for a row's badge -- mirrors
/// `git_panel::GitBadgeColor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraBadgeColor {
    Good,
    Warn,
    Bad,
    Neutral,
}

/// Per-line metadata for one line of `JiraPanel::text`, at the matching
/// index in `JiraPanel::lines` -- mirrors `git_panel::GitLine`.
#[derive(Debug, Clone)]
pub struct JiraLine {
    pub style: JiraLineStyle,
    /// `Some` only for a `Project`/`User`/`Issue` row -- what the cursor
    /// resolves to via `*_entry_at_cursor`.
    pub entry: Option<JiraEntry>,
    /// A row's `[X]` prefix: its char length (so the range `0..len` can
    /// be colored) and which color bucket to use.
    pub badge: Option<(usize, JiraBadgeColor)>,
}

/// The generated jira panel: `text` is real content for a real
/// `fenix_core::Buffer`; `lines[i]` describes `text`'s line `i`. Mirrors
/// `git_panel::GitPanel`.
pub struct JiraPanel {
    pub text: String,
    pub lines: Vec<Option<JiraLine>>,
}

struct Builder {
    text: String,
    lines: Vec<Option<JiraLine>>,
}

impl Builder {
    fn new() -> Self {
        Self { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, meta: Option<JiraLine>) {
        self.text.push_str(text);
        self.text.push('\n');
        self.lines.push(meta);
    }

    fn finish(self) -> JiraPanel {
        JiraPanel { text: self.text, lines: self.lines }
    }
}

fn empty_line(message: &str) -> (String, Option<JiraLine>) {
    (format!("    {message}"), Some(JiraLine { style: JiraLineStyle::Empty, entry: None, badge: None }))
}

/// A status name's badge color -- a coarse, best-effort bucket since a
/// self-hosted instance's workflow names aren't known ahead of time:
/// anything that reads as "finished" is `Good`, anything that reads as
/// active work is `Warn`, everything else (todo/backlog/open/unknown)
/// is `Neutral`.
fn status_color(status: &str) -> JiraBadgeColor {
    let lower = status.to_lowercase();
    if lower.contains("done") || lower.contains("closed") || lower.contains("resolved") {
        JiraBadgeColor::Good
    } else if lower.contains("progress") || lower.contains("review") {
        JiraBadgeColor::Warn
    } else {
        JiraBadgeColor::Neutral
    }
}

/// The Projects pane's own content -- the tracked `(key, display name)`
/// pairs, each row led by its key as the badge.
pub fn render_projects(projects: &[(String, String)]) -> JiraPanel {
    let mut b = Builder::new();
    if projects.is_empty() {
        let (text, meta) = empty_line("No projects tracked");
        b.push(&text, meta);
    } else {
        for (key, name) in projects {
            let prefix = format!("  [{key}] ");
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{name}");
            b.push(
                &line,
                Some(JiraLine {
                    style: JiraLineStyle::Project,
                    entry: Some(JiraEntry::Project(key.clone())),
                    badge: Some((badge_len, JiraBadgeColor::Neutral)),
                }),
            );
        }
    }
    b.finish()
}

/// The Users pane's own content -- the tracked `(id, display name)`
/// pairs, rendered as `"{name} ({id})"`.
pub fn render_users(users: &[(String, String)]) -> JiraPanel {
    let mut b = Builder::new();
    if users.is_empty() {
        let (text, meta) = empty_line("No users tracked");
        b.push(&text, meta);
    } else {
        for (id, name) in users {
            let line = format!("  {name} ({id})");
            b.push(&line, Some(JiraLine { style: JiraLineStyle::User, entry: Some(JiraEntry::User(id.clone())), badge: None }));
        }
    }
    b.finish()
}

/// The Issues pane's own content -- the selected user's assigned issues
/// (scoped to every tracked project), each row led by its status badge.
pub fn render_issues(issues: &[IssueSummary]) -> JiraPanel {
    let mut b = Builder::new();
    if issues.is_empty() {
        let (text, meta) = empty_line("No issues");
        b.push(&text, meta);
    } else {
        for issue in issues {
            let prefix = format!("  [{}] ", issue.status);
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{} {}", issue.key, issue.summary);
            b.push(
                &line,
                Some(JiraLine {
                    style: JiraLineStyle::Issue,
                    entry: Some(JiraEntry::Issue(issue.key.clone())),
                    badge: Some((badge_len, status_color(&issue.status))),
                }),
            );
        }
    }
    b.finish()
}

fn push_detail_line(b: &mut Builder, label: &str, value: &str) {
    b.push(&format!("    {label}: {value}"), Some(JiraLine { style: JiraLineStyle::Detail, entry: None, badge: None }));
}

/// The Detail pane's own content -- the selected issue's full detail,
/// including its comments (already embedded in `IssueDetail` -- see
/// `fenix_jira::JiraClient::get_issue`'s own doc comment).
pub fn render_detail(detail: Option<&IssueDetail>) -> JiraPanel {
    let mut b = Builder::new();
    match detail {
        None => {
            let (text, meta) = empty_line("Nothing selected");
            b.push(&text, meta);
        }
        Some(d) => {
            push_detail_line(&mut b, "Key", &d.key);
            push_detail_line(&mut b, "Summary", &d.summary);
            push_detail_line(&mut b, "Status", &d.status);
            push_detail_line(&mut b, "Assignee", d.assignee.as_deref().unwrap_or("Unassigned"));
            push_detail_line(&mut b, "Reporter", d.reporter.as_deref().unwrap_or("Unknown"));
            push_detail_line(&mut b, "Created", &d.created);
            push_detail_line(&mut b, "Updated", &d.updated);
            b.push("", Some(JiraLine { style: JiraLineStyle::Empty, entry: None, badge: None }));
            match &d.description {
                Some(desc) if !desc.is_empty() => {
                    for line in desc.lines() {
                        b.push(line, Some(JiraLine { style: JiraLineStyle::Detail, entry: None, badge: None }));
                    }
                }
                _ => {
                    let (text, meta) = empty_line("(no description)");
                    b.push(&text, meta);
                }
            }
            b.push("", Some(JiraLine { style: JiraLineStyle::Empty, entry: None, badge: None }));
            if d.comments.is_empty() {
                let (text, meta) = empty_line("(no comments)");
                b.push(&text, meta);
            } else {
                for c in &d.comments {
                    let header = format!("  {} @ {}", c.author, c.created);
                    b.push(&header, Some(JiraLine { style: JiraLineStyle::Comment, entry: None, badge: None }));
                    for line in c.body.lines() {
                        b.push(&format!("    {line}"), Some(JiraLine { style: JiraLineStyle::Comment, entry: None, badge: None }));
                    }
                }
            }
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(key: &str, summary: &str, status: &str, assignee: Option<&str>) -> IssueSummary {
        IssueSummary {
            key: key.to_string(),
            summary: summary.to_string(),
            status: status.to_string(),
            assignee: assignee.map(str::to_string),
            updated: "2024-01-15T10:30:00.000+0000".to_string(),
        }
    }

    fn detail(key: &str) -> IssueDetail {
        IssueDetail {
            key: key.to_string(),
            summary: "Fix the thing".to_string(),
            description: Some("Full description".to_string()),
            status: "In Progress".to_string(),
            assignee: Some("John Doe".to_string()),
            reporter: Some("Jane Smith".to_string()),
            created: "2024-01-01T00:00:00.000+0000".to_string(),
            updated: "2024-01-15T10:30:00.000+0000".to_string(),
            comments: vec![],
        }
    }

    #[test]
    fn render_projects_lists_entries_with_the_right_entry() {
        let panel = render_projects(&[("PROJ".to_string(), "My Project".to_string())]);
        assert!(panel.text.contains("[PROJ] My Project"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].entry, Some(JiraEntry::Project("PROJ".to_string())));
    }

    #[test]
    fn render_projects_empty_list_shows_a_placeholder() {
        assert!(render_projects(&[]).text.contains("No projects tracked"));
    }

    #[test]
    fn render_users_renders_name_then_id_in_parens() {
        let panel = render_users(&[("jo1111111".to_string(), "John Doe".to_string())]);
        assert!(panel.text.contains("John Doe (jo1111111)"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].entry, Some(JiraEntry::User("jo1111111".to_string())));
    }

    #[test]
    fn render_users_empty_list_shows_a_placeholder() {
        assert!(render_users(&[]).text.contains("No users tracked"));
    }

    #[test]
    fn render_issues_lists_entries_with_the_right_entry_and_badge() {
        let panel = render_issues(&[issue("PROJ-1", "Fix the thing", "Done", Some("John Doe"))]);
        assert!(panel.text.contains("[Done] PROJ-1 Fix the thing"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].entry, Some(JiraEntry::Issue("PROJ-1".to_string())));
        assert_eq!(entries[0].badge.map(|(_, c)| c), Some(JiraBadgeColor::Good));
    }

    #[test]
    fn render_issues_colors_in_progress_as_warn_and_todo_as_neutral() {
        let panel = render_issues(&[issue("PROJ-1", "A", "In Progress", None), issue("PROJ-2", "B", "To Do", None)]);
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].badge.map(|(_, c)| c), Some(JiraBadgeColor::Warn));
        assert_eq!(entries[1].badge.map(|(_, c)| c), Some(JiraBadgeColor::Neutral));
    }

    #[test]
    fn render_issues_empty_list_shows_a_placeholder() {
        assert!(render_issues(&[]).text.contains("No issues"));
    }

    #[test]
    fn render_detail_none_shows_nothing_selected() {
        assert!(render_detail(None).text.contains("Nothing selected"));
    }

    #[test]
    fn render_detail_shows_key_summary_and_people() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Key: PROJ-1"));
        assert!(panel.text.contains("Summary: Fix the thing"));
        assert!(panel.text.contains("Assignee: John Doe"));
        assert!(panel.text.contains("Reporter: Jane Smith"));
        assert!(panel.text.contains("Full description"));
        assert!(panel.text.contains("(no comments)"));
    }

    #[test]
    fn render_detail_shows_unassigned_and_unknown_reporter_placeholders() {
        let mut d = detail("PROJ-1");
        d.assignee = None;
        d.reporter = None;
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Assignee: Unassigned"));
        assert!(panel.text.contains("Reporter: Unknown"));
    }

    #[test]
    fn render_detail_shows_missing_description_placeholder() {
        let mut d = detail("PROJ-1");
        d.description = None;
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("(no description)"));
    }

    #[test]
    fn render_detail_shows_comments_with_author_and_body() {
        let mut d = detail("PROJ-1");
        d.comments = vec![fenix_jira::Comment {
            author: "Jane Smith".to_string(),
            body: "Looking into it".to_string(),
            created: "2024-01-02T00:00:00.000+0000".to_string(),
        }];
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Jane Smith @ 2024-01-02T00:00:00.000+0000"));
        assert!(panel.text.contains("Looking into it"));
        assert!(!panel.text.contains("(no comments)"));
    }

    #[test]
    fn render_detail_lines_stay_the_same_length_as_text() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }
}
