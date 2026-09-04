//! The Merge Requests view's two panes, rendered.
//!
//! Same `{text, lines}` contract as `git_panel`/`docker_panel`/
//! `jira_panel`, and reusing `git_panel`'s own line metadata so the
//! Git theme colors apply unchanged -- a merge request list is a list
//! of things with status badges, which is exactly what that already
//! renders.
//!
//! Written against `fenix_forge`'s neutral model rather than GitLab's
//! JSON, so a second forge is a second client, not a second panel.

use fenix_forge::{Approvals, ChangedFile, MergeRequest, MrFilter};

use crate::git_panel::{GitBadgeColor, GitEntry, GitLine, GitLineStyle, GitPanel};

/// The merge request list: one row each, led by its `!number`.
///
/// The badge carries the two things that decide whether a request is
/// worth opening -- whether CI is unhappy and whether it conflicts --
/// because those are the reasons a request that looks ready isn't.
pub fn render_list(project: &str, filter: MrFilter, requests: &[MergeRequest], error: Option<&str>, width: usize) -> GitPanel {
    let mut b = Builder::new(width);
    b.header(&format!("  {project}"));
    b.detail(&format!("    showing: {}  --  f cycles, u refreshes", filter.label()));
    b.blank();

    if let Some(error) = error {
        b.bad("  couldn't reach GitLab");
        for line in error.trim().lines() {
            b.detail(&format!("    {line}"));
        }
        return b.finish();
    }
    if requests.is_empty() {
        b.detail(&format!("  No open merge requests ({})", filter.label()));
        return b.finish();
    }

    for mr in requests {
        // The badge is the row's own identity (`!42`), colored by what
        // would stop it merging -- so a scan down the column finds the
        // blocked ones without reading a word of the titles.
        let badge = format!("  [{}] ", mr.reference());
        let color = if mr.has_conflicts || mr.pipeline.as_ref().is_some_and(|p| p.is_bad()) {
            GitBadgeColor::Bad
        } else if mr.draft {
            GitBadgeColor::Neutral
        } else {
            GitBadgeColor::Good
        };
        b.wrapped_row(
            &badge,
            &draft_prefixed(mr),
            GitLineStyle::Commit,
            // Reuses the `Commit` entry kind to carry the number: the
            // action keys read it back the same way every other pane
            // reads its own selection, without a new `GitEntry` variant
            // that would need matching everywhere `GitEntry` is.
            Some(GitEntry::Commit(mr.number.to_string())),
            color,
        );
    }
    b.finish()
}

/// The selected merge request in full: who wrote it, where it's going,
/// what's blocking it, and what it changes.
pub fn render_detail(
    request: Option<&MergeRequest>,
    approvals: Option<&Approvals>,
    files: &[ChangedFile],
    error: Option<&str>,
    width: usize,
) -> GitPanel {
    let mut b = Builder::new(width);
    let Some(mr) = request else {
        b.detail("  Select a merge request to see it here.");
        return b.finish();
    };

    b.header(&format!("  {} {}", mr.reference(), draft_prefixed(mr)));
    b.blank();
    b.field("Author", &mr.author);
    b.field("Branches", &format!("{} -> {}", mr.source_branch, mr.target_branch));
    b.field("State", mr.state.label());
    b.field("Updated", &mr.updated_at);

    // Everything that would stop this merging, grouped -- the question
    // anyone opening a merge request is actually asking.
    match &mr.pipeline {
        Some(status) => b.field("Pipeline", status.label()),
        None => b.field("Pipeline", "none"),
    }
    match approvals {
        Some(a) if a.required > 0 => {
            b.field("Approvals", &format!("{} of {} -- {}", a.required.saturating_sub(a.left), a.required, approver_list(a)))
        }
        // No approval rule configured (or a Free-tier instance, which
        // doesn't have them): say who approved without claiming a rule
        // that doesn't exist.
        Some(a) if !a.approved_by.is_empty() => b.field("Approved by", &approver_list(a)),
        Some(_) => b.field("Approvals", "none yet"),
        None => {}
    }
    if mr.has_conflicts {
        b.bad("    Conflicts: this can't merge as it stands");
    }
    b.field("Comments", &mr.comment_count.to_string());
    b.field("URL", &mr.web_url);

    if !mr.description.trim().is_empty() {
        b.blank();
        b.header("  Description");
        // Per source line: a description is prose *with* newlines in
        // it, and wrapping the whole blob at once produces a "line"
        // with a newline inside it -- which puts the text and its own
        // per-line metadata permanently out of step.
        for source_line in mr.description.trim().lines() {
            if source_line.trim().is_empty() {
                b.blank();
                continue;
            }
            for line in crate::wrap::wrap_text(source_line, b.width.saturating_sub(4).max(20)) {
                b.detail(&format!("    {line}"));
            }
        }
    }

    b.blank();
    if let Some(error) = error {
        b.bad("  couldn't load the changed files");
        for line in error.trim().lines() {
            b.detail(&format!("    {line}"));
        }
        return b.finish();
    }
    b.header(&format!("  Changed files ({})", files.len()));
    if files.is_empty() {
        b.detail("    none");
    }
    for file in files {
        let badge = format!("    [{}] ", file.change.letter());
        b.wrapped_row(&badge, file.display_path(), GitLineStyle::File, Some(GitEntry::File(file.display_path().to_string())), badge_for(file));
    }
    b.finish()
}

/// The title, with `Draft:` in front of it -- unless it's already
/// there, which it usually is: GitLab marks a draft by putting the word
/// in the title itself and setting the flag, so prefixing
/// unconditionally reads as `Draft: Draft: ...`.
fn draft_prefixed(mr: &MergeRequest) -> String {
    let title = mr.title.trim();
    if !mr.draft || title.to_ascii_lowercase().starts_with("draft:") {
        return title.to_string();
    }
    format!("Draft: {title}")
}

fn badge_for(file: &ChangedFile) -> GitBadgeColor {
    match file.change {
        fenix_forge::FileChange::Added => GitBadgeColor::Good,
        fenix_forge::FileChange::Deleted => GitBadgeColor::Bad,
        _ => GitBadgeColor::Neutral,
    }
}

fn approver_list(approvals: &Approvals) -> String {
    if approvals.approved_by.is_empty() {
        "nobody yet".to_string()
    } else {
        approvals.approved_by.join(", ")
    }
}

/// Mirrors `git_panel`'s own builder, with the few line shapes this
/// panel uses named for what they mean here.
struct Builder {
    text: String,
    lines: Vec<Option<GitLine>>,
    /// The pane's own character width, so rows wrap to where they'll
    /// actually be shown rather than to a fixed guess.
    width: usize,
}

impl Builder {
    fn new(width: usize) -> Self {
        Builder { text: String::new(), lines: Vec::new(), width: width.max(24) }
    }

    /// A `[badge] text` row, wrapped to the pane with continuation
    /// lines indented under the text rather than under the badge.
    ///
    /// Every line of the row carries the same `entry`, so `Enter` and
    /// the action keys work with the cursor anywhere in it -- landing on
    /// the second line of a wrapped title and finding the keys dead
    /// would be worse than not wrapping at all. Only the first line
    /// carries the badge, since that's the only line the badge's
    /// characters are actually on.
    fn wrapped_row(&mut self, badge: &str, text: &str, style: GitLineStyle, entry: Option<GitEntry>, color: GitBadgeColor) {
        let badge_len = badge.chars().count();
        let available = self.width.saturating_sub(badge_len).max(16);
        let mut wrapped = crate::wrap::wrap_text(text, available).into_iter();
        let first = wrapped.next().unwrap_or_default();
        self.push(&format!("{badge}{first}"), style, entry.clone(), Some((badge_len, color)));
        let indent = " ".repeat(badge_len);
        for rest in wrapped {
            self.push(&format!("{indent}{rest}"), style, entry.clone(), None);
        }
    }

    fn push(&mut self, text: &str, style: GitLineStyle, entry: Option<GitEntry>, badge: Option<(usize, GitBadgeColor)>) {
        if !self.lines.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(text);
        self.lines.push(Some(GitLine { style, entry, dim_from: None, badge }));
    }

    fn header(&mut self, text: &str) {
        self.push(text, GitLineStyle::Header, None, None);
    }

    fn detail(&mut self, text: &str) {
        self.push(text, GitLineStyle::Detail, None, None);
    }

    fn bad(&mut self, text: &str) {
        // A leading zero-length badge would color nothing, so the badge
        // covers the whole line -- this is a warning, not a list row.
        self.push(text, GitLineStyle::Detail, None, Some((text.chars().count(), GitBadgeColor::Bad)));
    }

    fn blank(&mut self) {
        self.push("", GitLineStyle::Detail, None, None);
    }

    /// A `    Label: value` row, wrapped and dimmed past the label the
    /// same way `git_panel`'s own detail rows are.
    fn field(&mut self, label: &str, value: &str) {
        let prefix = format!("    {label}: ");
        let dim_from = prefix.chars().count();
        let indent = " ".repeat(dim_from);
        let width = self.width.saturating_sub(dim_from).max(20);
        let mut wrapped = crate::wrap::wrap_text(value, width).into_iter();
        let first = wrapped.next().unwrap_or_default();
        self.push(&format!("{prefix}{first}"), GitLineStyle::Detail, None, None);
        if let Some(line) = self.lines.last_mut().and_then(Option::as_mut) {
            line.dim_from = Some(dim_from);
        }
        for rest in wrapped {
            self.push(&format!("{indent}{rest}"), GitLineStyle::Detail, None, None);
            if let Some(line) = self.lines.last_mut().and_then(Option::as_mut) {
                line.dim_from = Some(dim_from);
            }
        }
    }

    fn finish(mut self) -> GitPanel {
        self.text.push('\n');
        GitPanel { text: self.text, lines: self.lines }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_forge::{DiffRefs, FileChange, MrState, PipelineStatus};

    #[test]
    fn a_long_title_wraps_and_every_line_of_it_still_answers_the_action_keys() {
        // Landing on the second line of a wrapped title and finding
        // Enter dead would be worse than not wrapping at all.
        let long = mr(42, "Refactor the widget pipeline so that the configuration loader stops reading the environment twice");
        let panel = render_list("g/p", MrFilter::AllOpen, &[long], None, 48);

        let rows: Vec<&str> = panel.text.lines().filter(|l| !l.trim().is_empty()).skip(2).collect();
        assert!(rows.len() >= 2, "the title should have wrapped:\n{}", panel.text);
        assert!(rows.iter().all(|l| l.chars().count() <= 48), "a row ran past the pane:\n{}", panel.text);
        // Continuation lines line up under the text, not under the
        // badge, and carry the same entry.
        assert!(rows[1].starts_with("       "), "got: {:?}", rows[1]);
        let entries: Vec<Option<GitEntry>> =
            panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::Commit).map(|l| l.entry.clone()).collect();
        assert!(entries.len() >= 2);
        assert!(entries.iter().all(|e| e == &Some(GitEntry::Commit("42".to_string()))), "got: {entries:?}");
        // Only the first line carries the badge -- it's the only line
        // the badge's characters are on.
        let badges: Vec<bool> =
            panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::Commit).map(|l| l.badge.is_some()).collect();
        assert_eq!(badges[0], true);
        assert!(badges[1..].iter().all(|b| !b), "got: {badges:?}");
    }

    #[test]
    fn a_long_file_path_wraps_to_the_pane_too() {
        let deep = file("src/very/deeply/nested/module/with/a/long/name/implementation_detail.rs", FileChange::Modified);
        let panel = render_detail(Some(&mr(1, "t")), None, &[deep], None, 40);
        let rows: Vec<&str> = panel.text.lines().collect();
        assert!(rows.iter().all(|l| l.chars().count() <= 40), "a row ran past the pane:\n{}", panel.text);
    }

    #[test]
    fn a_narrow_pane_wraps_the_fields_and_the_description_as_well() {
        let mut wordy = mr(1, "t");
        wordy.description = "A description long enough that it has to wrap more than once at this width.".to_string();
        wordy.web_url = "https://gitlab.example.com/some/deeply/nested/group/project/-/merge_requests/1".to_string();
        let panel = render_detail(Some(&wordy), None, &[], None, 44);
        assert!(panel.text.lines().all(|l| l.chars().count() <= 44), "a row ran past the pane:\n{}", panel.text);
        // Still one metadata row per text row after all that wrapping.
        assert_eq!(panel.text.trim_end_matches('\n').split('\n').count(), panel.lines.len());
    }

    #[test]
    fn a_draft_is_not_labelled_twice() {
        // GitLab marks a draft by putting the word in the title *and*
        // setting the flag, so an unconditional prefix reads
        // "Draft: Draft: ...".
        let mut already = mr(1, "Draft: work in progress");
        already.draft = true;
        let panel = render_list("g/p", MrFilter::AllOpen, &[already], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("[!1] Draft: work in progress"), "got:
{}", panel.text);
        assert!(!panel.text.contains("Draft: Draft:"), "got:
{}", panel.text);
    }

    #[test]
    fn a_draft_whose_title_does_not_say_so_still_gets_the_label() {
        let mut plain = mr(1, "Untitled work");
        plain.draft = true;
        let panel = render_list("g/p", MrFilter::AllOpen, &[plain], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("[!1] Draft: Untitled work"), "got:
{}", panel.text);
    }

    #[test]
    fn every_rendered_row_is_exactly_one_line_of_metadata() {
        // A multi-line description used to be wrapped as one blob,
        // producing a "row" with a newline inside it -- which puts the
        // buffer text and its own per-line metadata permanently out of
        // step, so every color below it lands on the wrong row.
        let mut multi = mr(1, "t");
        multi.description = "First paragraph.\n\nSecond one, quite a lot longer so that it also has to wrap somewhere.".to_string();
        let panel = render_detail(Some(&multi), None, &[], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert_eq!(
            panel.text.trim_end_matches('\n').split('\n').count(),
            panel.lines.len(),
            "text rows and metadata rows must match:
{}",
            panel.text
        );
        assert!(panel.text.contains("    First paragraph."), "got:
{}", panel.text);
        assert!(panel.text.contains("    Second one,"), "got:
{}", panel.text);
    }

    #[test]
    fn a_list_row_count_matches_its_metadata_too() {
        let panel = render_list("g/p", MrFilter::AllOpen, &[mr(1, "a"), mr(2, "b")], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert_eq!(panel.text.trim_end_matches('\n').split('\n').count(), panel.lines.len());
    }

    fn mr(number: u64, title: &str) -> MergeRequest {
        MergeRequest {
            number,
            title: title.to_string(),
            description: "Does the thing.".to_string(),
            state: MrState::Open,
            draft: false,
            source_branch: "feature/thing".to_string(),
            target_branch: "develop".to_string(),
            author: "Thomas Pedneault".to_string(),
            web_url: "https://gitlab.example.com/g/p/-/merge_requests/1".to_string(),
            has_conflicts: false,
            sha: "abc1234".to_string(),
            diff_refs: DiffRefs::default(),
            comment_count: 2,
            pipeline: Some(PipelineStatus::Success),
            updated_at: "2026-09-04T10:00:00Z".to_string(),
        }
    }

    fn file(path: &str, change: FileChange) -> ChangedFile {
        ChangedFile { old_path: path.to_string(), new_path: path.to_string(), change, diff: String::new() }
    }

    #[test]
    fn the_list_leads_each_row_with_the_number_people_actually_say() {
        let panel = render_list("group/project", MrFilter::AllOpen, &[mr(42, "Add the thing")], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("[!42] Add the thing"), "got:\n{}", panel.text);
        assert!(panel.text.contains("group/project"));
        assert!(panel.text.contains("showing: all open"), "got:\n{}", panel.text);
    }

    #[test]
    fn a_blocked_request_gets_a_bad_badge_so_a_scan_finds_it() {
        let mut failing = mr(1, "Broken CI");
        failing.pipeline = Some(PipelineStatus::Failed);
        let mut conflicted = mr(2, "Conflicts");
        conflicted.has_conflicts = true;
        let mut draft = mr(3, "Not ready");
        draft.draft = true;

        let panel = render_list("g/p", MrFilter::AllOpen, &[failing, conflicted, draft, mr(4, "Ready")], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        let colors: Vec<GitBadgeColor> = panel.lines.iter().flatten().filter_map(|l| l.badge.map(|(_, c)| c)).collect();
        assert_eq!(colors, vec![GitBadgeColor::Bad, GitBadgeColor::Bad, GitBadgeColor::Neutral, GitBadgeColor::Good]);
        assert!(panel.text.contains("Draft: Not ready"), "got:\n{}", panel.text);
    }

    #[test]
    fn a_row_carries_its_number_so_the_action_keys_can_find_it() {
        let panel = render_list("g/p", MrFilter::Mine, &[mr(42, "t")], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        let entry = panel.lines.iter().flatten().find_map(|l| l.entry.clone()).unwrap();
        assert_eq!(entry, GitEntry::Commit("42".to_string()));
    }

    #[test]
    fn an_unreachable_instance_says_so_in_the_pane_not_just_the_modeline() {
        // An empty list beside an error that has already scrolled past
        // reads as "no merge requests", which is a different answer.
        let panel = render_list("g/p", MrFilter::Mine, &[], Some("HTTP 401 (Unauthorized)"), crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("couldn't reach GitLab"), "got:\n{}", panel.text);
        assert!(panel.text.contains("HTTP 401"), "got:\n{}", panel.text);
        assert!(!panel.text.contains("No open merge requests"), "got:\n{}", panel.text);
    }

    #[test]
    fn the_detail_pane_answers_whether_this_can_merge() {
        let approvals = Approvals { approved: false, required: 2, left: 1, approved_by: vec!["Alice".to_string()] };
        let panel = render_detail(Some(&mr(42, "Add the thing")), Some(&approvals), &[file("src/a.rs", FileChange::Modified)], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("!42 Add the thing"), "got:\n{}", panel.text);
        assert!(panel.text.contains("feature/thing -> develop"), "got:\n{}", panel.text);
        assert!(panel.text.contains("Pipeline: passed"), "got:\n{}", panel.text);
        assert!(panel.text.contains("Approvals: 1 of 2 -- Alice"), "got:\n{}", panel.text);
        assert!(panel.text.contains("[M] src/a.rs"), "got:\n{}", panel.text);
    }

    #[test]
    fn a_conflicted_request_says_so_where_it_cannot_be_missed() {
        let mut conflicted = mr(1, "t");
        conflicted.has_conflicts = true;
        let panel = render_detail(Some(&conflicted), None, &[], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("this can't merge as it stands"), "got:\n{}", panel.text);
    }

    #[test]
    fn an_instance_with_no_approval_rules_reports_approvers_not_a_rule() {
        // `approvals_required` is a Premium field; on Free it's absent,
        // and claiming "0 of 0" would be inventing a rule.
        let approvals = Approvals { approved: true, required: 0, left: 0, approved_by: vec!["Bob".to_string()] };
        let panel = render_detail(Some(&mr(1, "t")), Some(&approvals), &[], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("Approved by: Bob"), "got:\n{}", panel.text);
        assert!(!panel.text.contains("of 0"), "got:\n{}", panel.text);
    }

    #[test]
    fn a_long_description_is_wrapped_rather_than_running_off_the_pane() {
        let mut long = mr(1, "t");
        long.description = "word ".repeat(60);
        let panel = render_detail(Some(&long), None, &[], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(
            panel.text.lines().all(|l| l.chars().count() <= crate::wrap::DEFAULT_WRAP_WIDTH + 8),
            "a line ran long:\n{}",
            panel.text
        );
    }

    #[test]
    fn nothing_selected_says_what_to_do_rather_than_rendering_blank() {
        let panel = render_detail(None, None, &[], None, crate::wrap::DEFAULT_WRAP_WIDTH);
        assert!(panel.text.contains("Select a merge request"), "got:\n{}", panel.text);
    }
}
