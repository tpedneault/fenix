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
    /// The Detail pane's own page title (`"{key}: {summary}"`) -- the
    /// one line that should read as more prominent than everything
    /// else in the pane, the same role a web page's own `<h1>` plays
    /// on Jira's real issue view.
    Title,
    /// A section heading ("Description", "Comments (N)") and its own
    /// underline row, in the Detail pane -- mirrors the section
    /// headers Jira's real issue view breaks description/activity into.
    SectionHeader,
    /// Real prose content in the Detail pane -- the description body, a
    /// comment's body -- full brightness, unlike the dim `Detail`/
    /// `Comment` metadata surrounding it, so the text people actually
    /// came to read doesn't visually recede behind its own metadata.
    Body,
    /// A `label: value` metadata row (Assignee/Reporter/Created/
    /// Updated) in the Detail pane.
    Detail,
    /// A comment's own header line (`author @ timestamp`) in the Detail
    /// pane -- dim, same role real Jira's small-gray "commented 3 days
    /// ago" byline plays relative to the comment body under it.
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

fn push_blank(b: &mut Builder) {
    b.push("", Some(JiraLine { style: JiraLineStyle::Empty, entry: None, badge: None }));
}

fn push_title(b: &mut Builder, d: &IssueDetail) {
    b.push(&format!("{}: {}", d.key, d.summary), Some(JiraLine { style: JiraLineStyle::Title, entry: None, badge: None }));
}

/// `[Status]` on its own line, right under the title -- the same
/// bracketed-badge convention `render_issues`'s own rows use, so the
/// two panes read as one consistent visual language.
fn push_status_badge(b: &mut Builder, status: &str) {
    let line = format!("[{status}]");
    let badge_len = line.chars().count();
    b.push(&line, Some(JiraLine { style: JiraLineStyle::Detail, entry: None, badge: Some((badge_len, status_color(status))) }));
}

/// A section heading plus its own underline row (`"----"`, sized to the
/// heading's own length) -- mirrors a Markdown-style underlined
/// heading, the closest plain-text equivalent to how Jira's real issue
/// view visually breaks Description/Activity into their own sections.
fn push_section_header(b: &mut Builder, title: &str) {
    b.push(title, Some(JiraLine { style: JiraLineStyle::SectionHeader, entry: None, badge: None }));
    let underline = "-".repeat(title.chars().count());
    b.push(&underline, Some(JiraLine { style: JiraLineStyle::Detail, entry: None, badge: None }));
}

fn push_body_line(b: &mut Builder, line: &str) {
    b.push(&format!("  {line}"), Some(JiraLine { style: JiraLineStyle::Body, entry: None, badge: None }));
}

/// Trims a Jira REST API timestamp (`"2024-01-15T10:30:00.000+0000"`)
/// down to `"2024-01-15 10:30"` -- date plus hour:minute, dropping
/// seconds/milliseconds/timezone offset as more noise than signal for
/// a compact detail view (no date/time crate pulled in for this --
/// Jira's own format is fixed-width and ASCII, so a plain string split
/// is enough). Falls back to the raw string unchanged if it doesn't
/// look like Jira's own format -- defensive, not expected in practice,
/// Jira's REST API is consistent about this.
fn format_timestamp(raw: &str) -> String {
    let Some((date, rest)) = raw.split_once('T') else { return raw.to_string() };
    match rest.get(..5) {
        Some(time) => format!("{date} {time}"),
        None => raw.to_string(),
    }
}

/// The Detail pane's own content -- the selected issue's full detail,
/// laid out to read like Jira's real issue view: a page-title line
/// (key + summary), the status badge right under it, a compact
/// metadata block (assignee/reporter/dates), then Description and
/// Comments as their own clearly-headed sections. Comments are already
/// embedded in `IssueDetail` -- see `fenix_jira::JiraClient::get_issue`'s
/// own doc comment.
pub fn render_detail(detail: Option<&IssueDetail>) -> JiraPanel {
    let mut b = Builder::new();
    match detail {
        None => {
            let (text, meta) = empty_line("Nothing selected");
            b.push(&text, meta);
        }
        Some(d) => {
            push_title(&mut b, d);
            push_status_badge(&mut b, &d.status);
            push_blank(&mut b);

            push_detail_line(&mut b, "Assignee", d.assignee.as_deref().unwrap_or("Unassigned"));
            push_detail_line(&mut b, "Reporter", d.reporter.as_deref().unwrap_or("Unknown"));
            push_detail_line(&mut b, "Created", &format_timestamp(&d.created));
            push_detail_line(&mut b, "Updated", &format_timestamp(&d.updated));
            push_blank(&mut b);

            push_section_header(&mut b, "Description");
            match &d.description {
                Some(desc) if !desc.is_empty() => {
                    for line in desc.lines() {
                        push_body_line(&mut b, line);
                    }
                }
                _ => {
                    let (text, meta) = empty_line("No description");
                    b.push(&text, meta);
                }
            }
            push_blank(&mut b);

            let comments_header =
                if d.comments.is_empty() { "Comments".to_string() } else { format!("Comments ({})", d.comments.len()) };
            push_section_header(&mut b, &comments_header);
            if d.comments.is_empty() {
                let (text, meta) = empty_line("No comments yet");
                b.push(&text, meta);
            } else {
                for (i, c) in d.comments.iter().enumerate() {
                    if i > 0 {
                        push_blank(&mut b);
                    }
                    let header = format!("  {} @ {}", c.author, format_timestamp(&c.created));
                    b.push(&header, Some(JiraLine { style: JiraLineStyle::Comment, entry: None, badge: None }));
                    for line in c.body.lines() {
                        b.push(&format!("    {line}"), Some(JiraLine { style: JiraLineStyle::Body, entry: None, badge: None }));
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
    fn render_detail_shows_a_page_title_line_combining_key_and_summary() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        let title_line = panel.text.lines().next().unwrap();
        assert_eq!(title_line, "PROJ-1: Fix the thing");
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].style, JiraLineStyle::Title);
    }

    #[test]
    fn render_detail_shows_the_status_as_a_bracketed_badge_right_under_the_title() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        let status_line = panel.text.lines().nth(1).unwrap();
        assert_eq!(status_line, "[In Progress]");
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[1].badge.map(|(_, c)| c), Some(JiraBadgeColor::Warn));
    }

    #[test]
    fn render_detail_shows_people_and_formatted_dates() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Assignee: John Doe"));
        assert!(panel.text.contains("Reporter: Jane Smith"));
        // Raw Jira timestamps ("...T00:00:00.000+0000") are trimmed down
        // to "date hour:minute" -- the noisy seconds/ms/timezone suffix
        // must not survive into the rendered text.
        assert!(panel.text.contains("Created: 2024-01-01 00:00"));
        assert!(panel.text.contains("Updated: 2024-01-15 10:30"));
        assert!(!panel.text.contains(".000+0000"));
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
    fn render_detail_shows_description_under_its_own_section_header() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Description\n"));
        assert!(panel.text.contains("Full description"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert!(entries.iter().any(|l| l.style == JiraLineStyle::SectionHeader));
        assert!(entries.iter().any(|l| l.style == JiraLineStyle::Body));
    }

    #[test]
    fn render_detail_shows_missing_description_placeholder() {
        let mut d = detail("PROJ-1");
        d.description = None;
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("No description"));
    }

    #[test]
    fn render_detail_comments_header_includes_the_count() {
        let mut d = detail("PROJ-1");
        d.comments = vec![
            fenix_jira::Comment { author: "Jane Smith".to_string(), body: "First".to_string(), created: "2024-01-02T00:00:00.000+0000".to_string() },
            fenix_jira::Comment { author: "John Doe".to_string(), body: "Second".to_string(), created: "2024-01-03T00:00:00.000+0000".to_string() },
        ];
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Comments (2)"));
    }

    #[test]
    fn render_detail_shows_comments_with_author_and_formatted_timestamp() {
        let mut d = detail("PROJ-1");
        d.comments = vec![fenix_jira::Comment {
            author: "Jane Smith".to_string(),
            body: "Looking into it".to_string(),
            created: "2024-01-02T00:00:00.000+0000".to_string(),
        }];
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Jane Smith @ 2024-01-02 00:00"));
        assert!(panel.text.contains("Looking into it"));
        assert!(!panel.text.contains("No comments yet"));
    }

    #[test]
    fn render_detail_with_no_comments_shows_a_placeholder_under_an_uncounted_header() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        assert!(panel.text.contains("Comments\n"));
        assert!(!panel.text.contains("Comments ("));
        assert!(panel.text.contains("No comments yet"));
    }

    #[test]
    fn render_detail_lines_stay_the_same_length_as_text() {
        let d = detail("PROJ-1");
        let panel = render_detail(Some(&d));
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn format_timestamp_trims_seconds_milliseconds_and_timezone() {
        assert_eq!(format_timestamp("2024-01-15T10:30:00.000+0000"), "2024-01-15 10:30");
    }

    #[test]
    fn format_timestamp_falls_back_to_the_raw_string_when_not_jiras_own_format() {
        assert_eq!(format_timestamp("not a timestamp"), "not a timestamp");
        assert_eq!(format_timestamp(""), "");
    }
}
