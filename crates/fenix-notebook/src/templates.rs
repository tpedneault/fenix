//! What a new note starts as. The built-in templates live here; your own
//! are Markdown snippets whose trigger starts with `note-` (the GUI
//! offers those beside these, rendered by the snippet engine).
//!
//! A template's text may use `{{title}}`, `{{date}}` (2026-09-29),
//! `{{day}}` (Tuesday 29 September 2026), `{{time}}` (10:41),
//! `{{project}}` and `{{cursor}}`, where the cursor lands.

/// One built-in template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub about: &'static str,
    pub body: &'static str,
}

pub const BUILT_IN: &[Template] = &[
    Template { name: "Blank", about: "a title and nothing else", body: "# {{title}}\n\n{{cursor}}\n" },
    Template {
        name: "Bench session",
        about: "setup, steps, results, anomalies",
        body: "# {{title}}\n\n{{date}} {{time}}\n\n## Setup\n\n- {{cursor}}\n\n## Steps\n\n1. \n\n## Results\n\n| Step | Expected | Observed |\n|------|----------|----------|\n|      |          |          |\n\n## Anomalies\n\n- [ ] \n",
    },
    Template {
        name: "Meeting",
        about: "attendees, notes, actions",
        body: "# {{title}}\n\n{{day}}\n\n**Attendees:** {{cursor}}\n\n## Notes\n\n- \n\n## Actions\n\n- [ ] \n",
    },
    Template {
        name: "Decision record",
        about: "context, options, decision, consequences",
        body: "---\ntags: [decision]\n---\n# {{title}}\n\nStatus: proposed · {{date}}\n\n## Context\n\n{{cursor}}\n\n## Options\n\n1. \n\n## Decision\n\n\n## Consequences\n\n- \n",
    },
    Template {
        name: "Anomaly report",
        about: "observed, expected, packets, cause",
        body: "---\ntags: [anomaly]\n---\n# {{title}}\n\nSeen {{date}} {{time}}\n\n## Observed\n\n{{cursor}}\n\n## Expected\n\n\n## Packets\n\n```\n\n```\n\n## Cause\n\n\n## Actions\n\n- [ ] \n",
    },
    Template { name: "How-to", about: "goal, steps, gotchas", body: "# {{title}}\n\n**Goal:** {{cursor}}\n\n## Steps\n\n1. \n\n## Gotchas\n\n- \n" },
];

/// The values a template is filled with.
#[derive(Debug, Clone, Default)]
pub struct Fill {
    pub title: String,
    pub project: Option<String>,
    pub now: Option<chrono::NaiveDateTime>,
}

/// `body` filled in, and where `{{cursor}}` was: (line, column in chars).
pub fn render(body: &str, fill: &Fill) -> (String, (usize, usize)) {
    let now = fill.now.unwrap_or_else(|| chrono::Local::now().naive_local());
    let mut text = body
        .replace("{{title}}", &fill.title)
        .replace("{{date}}", &now.format("%Y-%m-%d").to_string())
        .replace("{{day}}", &now.format("%A %-d %B %Y").to_string())
        .replace("{{time}}", &now.format("%H:%M").to_string())
        .replace("{{project}}", fill.project.as_deref().unwrap_or(""));
    // A note made in a project is about it.
    if let Some(project) = fill.project.as_deref().filter(|p| !p.is_empty()) {
        text = crate::meta::set_front_matter(&text, "about", Some(project));
    }
    let at = match text.find("{{cursor}}") {
        Some(byte) => {
            let before = &text[..byte];
            let line = before.matches('\n').count();
            let col = before.rsplit('\n').next().unwrap_or("").chars().count();
            text.replace_range(byte..byte + "{{cursor}}".len(), "");
            (line, col)
        }
        None => (text.lines().count().saturating_sub(1), 0),
    };
    (text, at)
}

/// What goes into a new journal day.
#[derive(Debug, Clone, Default)]
pub struct DayFill {
    /// Agenda tasks due today or overdue, as their lines (`FSW-212 Uplink
    /// retry limit · due today`).
    pub agenda: Vec<String>,
    /// Yesterday's (or the last day's) open checkboxes.
    pub carried: Vec<String>,
}

/// A journal day's text.
pub fn journal(date: chrono::NaiveDate, fill: &DayFill) -> String {
    let mut out = format!("# {}\n", date.format("%A %-d %B %Y"));
    if !fill.agenda.is_empty() {
        out.push_str("\n## Agenda\n\n");
        for a in &fill.agenda {
            out.push_str(&format!("- [ ] {a}\n"));
        }
    }
    if !fill.carried.is_empty() {
        out.push_str("\n## Carried over\n\n");
        for c in &fill.carried {
            out.push_str(&format!("- [ ] {c}\n"));
        }
    }
    out.push_str("\n## Log\n\n");
    out
}

/// A line captured into a note: a timestamped bullet, or an open task.
pub fn capture_line(text: &str, task: bool, link: Option<&str>, now: chrono::NaiveTime) -> String {
    let link = link.map(|l| format!(" · {l}")).unwrap_or_default();
    if task {
        format!("- [ ] {text}{link}")
    } else {
        format!("- `{}` {text}{link}", now.format("%H:%M"))
    }
}

/// `text` with `line` added under its `## Log` heading (or at the end).
pub fn append_to_log(text: &str, line: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let log = lines.iter().position(|l| l.trim().eq_ignore_ascii_case("## log"));
    match log {
        Some(at) => {
            // After the last line of that section's content.
            let mut end = at + 1;
            let mut last_content = at;
            while end < lines.len() && !lines[end].starts_with("## ") {
                if !lines[end].trim().is_empty() {
                    last_content = end;
                }
                end += 1;
            }
            let insert = if last_content == at { at + 2 } else { last_content + 1 };
            let insert = insert.min(lines.len());
            if last_content == at && lines.get(at + 1).map(|l| !l.trim().is_empty()).unwrap_or(true) {
                lines.insert(at + 1, "");
                lines.insert((at + 2).min(lines.len()), line);
            } else {
                lines.insert(insert, line);
            }
        }
        None => {
            while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                lines.pop();
            }
            if !lines.is_empty() {
                lines.push("");
            }
            lines.push(line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32) -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(10, 41, 0).unwrap()
    }

    #[test]
    fn a_template_is_filled_and_says_where_the_cursor_goes() {
        let fill = Fill { title: "Bench".into(), project: None, now: Some(at(2026, 9, 29)) };
        let (text, cursor) = render("# {{title}}\n\n{{date}} {{time}}: {{cursor}}\n", &fill);
        assert_eq!(text, "# Bench\n\n2026-09-29 10:41: \n");
        assert_eq!(cursor, (2, 18));
    }

    #[test]
    fn a_note_made_in_a_project_is_about_it() {
        let fill = Fill { title: "T".into(), project: Some("fenix-sat".into()), now: Some(at(2026, 9, 29)) };
        let (text, cursor) = render(BUILT_IN[0].body, &fill);
        assert_eq!(text, "---\nabout: fenix-sat\n---\n# T\n\n\n");
        assert_eq!(cursor, (5, 0));
    }

    #[test]
    fn a_journal_day_lists_the_agenda_and_whats_carried_over() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        let text = journal(d, &DayFill { agenda: vec!["FSW-212 retry limit".into()], carried: vec!["call OBSW".into()] });
        assert_eq!(text, "# Tuesday 29 September 2026\n\n## Agenda\n\n- [ ] FSW-212 retry limit\n\n## Carried over\n\n- [ ] call OBSW\n\n## Log\n\n");
        assert_eq!(journal(d, &DayFill::default()), "# Tuesday 29 September 2026\n\n## Log\n\n");
    }

    #[test]
    fn captures_go_under_the_log() {
        let t = chrono::NaiveTime::from_hms_opt(10, 41, 0).unwrap();
        let line = capture_line("retry is 3", false, Some("[[fenix:src/a.c:1]]"), t);
        assert_eq!(line, "- `10:41` retry is 3 · [[fenix:src/a.c:1]]");
        let day = "# Day\n\n## Log\n\n";
        let once = append_to_log(day, "- a");
        assert_eq!(once, "# Day\n\n## Log\n\n- a\n");
        let twice = append_to_log(&once, "- b");
        assert_eq!(twice, "# Day\n\n## Log\n\n- a\n- b\n");
        let before_next = append_to_log("## Log\n- a\n\n## Later\nx\n", "- b");
        assert_eq!(before_next, "## Log\n- a\n- b\n\n## Later\nx\n");
        assert_eq!(append_to_log("# Inbox\n", "- x"), "# Inbox\n\n- x\n");
        assert_eq!(capture_line("do it", true, None, t), "- [ ] do it");
    }
}
