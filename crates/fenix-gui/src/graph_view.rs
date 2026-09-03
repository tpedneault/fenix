//! The History view's two listings: the commit graph (rails drawn from
//! `fenix_git::assign_lanes`' lane assignment) and the refs tree (local
//! branches with their sync state against origin, remote branches,
//! tags).
//!
//! The graph gets its own per-line metadata rather than reusing
//! `git_panel::GitLine`, because a graph row is four visually distinct
//! spans -- rails, hash, ref decorations, subject -- and `GitLine` can
//! only say one thing per row. The refs tree, being an ordinary badge-
//! plus-label listing, *does* reuse `GitLine`, so it needs no new
//! rendering path at all.

use fenix_git::{Branch, GraphCommit, GraphRow};

use crate::git_panel::{GitBadgeColor, GitEntry, GitLine, GitLineStyle, GitPanel};

/// One span of a graph row, from its start column to the next span's
/// start (or end of line, for the last).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphSpan {
    /// The `│ ●─╮` rail art -- dim, structural.
    Rails,
    Hash,
    /// `(main, origin/main)` -- where each branch actually points, which
    /// is the whole reason to look at a graph.
    Refs,
    Subject,
    /// The placeholder row when there's no history to draw.
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphLine {
    /// `(start column, kind)` pairs in ascending column order.
    pub spans: Vec<(usize, GraphSpan)>,
    /// The commit this row draws, if it draws one.
    pub commit: Option<String>,
}

pub struct GraphPanel {
    pub text: String,
    pub lines: Vec<Option<GraphLine>>,
}

/// Rail glyphs for one row, `width` columns wide.
///
/// Built in a deliberate order -- horizontals first, then the connector
/// endpoints, then the node -- so that later, more specific marks
/// overwrite earlier, more general ones rather than the reverse. A
/// horizontal crossing a rail that's just passing through becomes `┼`
/// instead of erasing it, which is what keeps an unrelated branch's line
/// continuous behind a merge.
fn rails(row: &GraphRow, width: usize) -> String {
    let mut cells = vec![' '; width.max(row.lane + 1)];
    for &c in &row.through {
        if c < cells.len() {
            cells[c] = '│';
        }
    }
    for &end in row.closing.iter().chain(row.branching.iter()) {
        let (lo, hi) = if end > row.lane { (row.lane + 1, end) } else { (end + 1, row.lane) };
        for cell in cells.iter_mut().take(hi).skip(lo) {
            *cell = if *cell == '│' { '┼' } else { '─' };
        }
    }
    for &c in &row.closing {
        if c < cells.len() {
            cells[c] = if c > row.lane { '╯' } else { '╰' };
        }
    }
    for &c in &row.branching {
        if c < cells.len() {
            cells[c] = if c > row.lane { '╮' } else { '╭' };
        }
    }
    cells[row.lane] = if row.is_merge { '◆' } else { '●' };
    cells.into_iter().collect()
}

/// The commit graph: one row per commit, `{rails} {short hash}
/// {(refs)} {subject}`.
///
/// Rail width is fixed for the whole view (the widest row's lane count),
/// so columns line up vertically and a branch's rail reads as one
/// continuous line rather than shifting as you scroll.
pub fn render_graph(commits: &[GraphCommit], rows: &[GraphRow]) -> GraphPanel {
    let mut text = String::new();
    let mut lines = Vec::new();
    if commits.is_empty() {
        text.push_str("    (no commits)\n");
        lines.push(Some(GraphLine { spans: vec![(0, GraphSpan::Meta)], commit: None }));
        return GraphPanel { text, lines };
    }

    let width = rows
        .iter()
        .map(|r| r.lane.max(r.through.iter().chain(r.closing.iter()).chain(r.branching.iter()).copied().max().unwrap_or(0)) + 1)
        .max()
        .unwrap_or(1);

    for (commit, row) in commits.iter().zip(rows) {
        let rail = rails(row, width);
        let mut line = format!("{rail} {} ", commit.short_hash);
        let mut spans = vec![(0, GraphSpan::Rails), (rail.chars().count() + 1, GraphSpan::Hash)];

        if !commit.refs.is_empty() {
            spans.push((line.chars().count(), GraphSpan::Refs));
            line.push_str(&format!("({}) ", commit.refs.join(", ")));
        }
        spans.push((line.chars().count(), GraphSpan::Subject));
        line.push_str(&commit.subject);

        text.push_str(&line);
        text.push('\n');
        lines.push(Some(GraphLine { spans, commit: Some(commit.hash.clone()) }));
    }
    GraphPanel { text, lines }
}

/// A local branch's sync state against its upstream, as a short badge.
///
/// The four cases are genuinely different and a `0/0` count can't tell
/// them apart, which is exactly the confusion this view exists to
/// remove: in sync, diverged (by how much and which way), an upstream
/// that was deleted, and never having had one at all.
fn sync_badge(branch: &Branch) -> (String, GitBadgeColor) {
    if branch.upstream.is_none() {
        return ("[--]".to_string(), GitBadgeColor::Neutral);
    }
    if branch.upstream_gone {
        return ("[gone]".to_string(), GitBadgeColor::Bad);
    }
    match (branch.ahead, branch.behind) {
        (0, 0) => ("[=]".to_string(), GitBadgeColor::Good),
        (a, 0) => (format!("[^{a}]"), GitBadgeColor::Warn),
        (0, b) => (format!("[v{b}]"), GitBadgeColor::Warn),
        (a, b) => (format!("[^{a} v{b}]"), GitBadgeColor::Warn),
    }
}

fn push(text: &mut String, lines: &mut Vec<Option<GitLine>>, row: &str, meta: Option<GitLine>) {
    text.push_str(row);
    text.push('\n');
    lines.push(meta);
}

fn header(text: &mut String, lines: &mut Vec<Option<GitLine>>, title: &str) {
    push(text, lines, title, Some(GitLine { style: GitLineStyle::Header, entry: None, dim_from: None, badge: None }));
}

/// Formats a fetch age as something a person reads at a glance --
/// minutes up to an hour, then hours, then days. The point isn't
/// precision, it's whether the ahead/behind counts below can be trusted.
fn fetch_age_text(seconds: Option<u64>) -> String {
    match seconds {
        None => "never fetched".to_string(),
        Some(s) if s < 90 => "fetched just now".to_string(),
        Some(s) if s < 3600 => format!("fetched {}m ago", s / 60),
        Some(s) if s < 86_400 => format!("fetched {}h ago", s / 3600),
        Some(s) => format!("fetched {}d ago", s / 86_400),
    }
}

/// The refs tree: Local (each branch badged with its sync state against
/// its upstream), Remotes, Tags -- led by how stale the whole picture
/// is, since every sync badge below is only as current as the last
/// fetch.
pub fn render_refs(branches: &[Branch], remotes: &[String], tags: &[String], fetch_age: Option<u64>) -> GitPanel {
    let mut text = String::new();
    let mut lines: Vec<Option<GitLine>> = Vec::new();

    push(
        &mut text,
        &mut lines,
        &format!("  {}", fetch_age_text(fetch_age)),
        Some(GitLine { style: GitLineStyle::Detail, entry: None, dim_from: Some(0), badge: None }),
    );

    header(&mut text, &mut lines, "Local");
    if branches.is_empty() {
        push(&mut text, &mut lines, "    (none)", Some(GitLine { style: GitLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    }
    for branch in branches {
        let (badge, color) = sync_badge(branch);
        let marker = if branch.current { "*" } else { " " };
        let prefix = format!("  {marker}{badge} ");
        let badge_len = prefix.chars().count();
        let upstream = branch.upstream.as_deref().unwrap_or("");
        let row = if upstream.is_empty() {
            format!("{prefix}{}", branch.name)
        } else {
            format!("{prefix}{}  -> {upstream}", branch.name)
        };
        // The `-> upstream` half is supporting detail, dimmed like every
        // other secondary column in these panels.
        let dim_from = upstream.is_empty().then_some(None).unwrap_or_else(|| Some(prefix.chars().count() + branch.name.chars().count()));
        push(
            &mut text,
            &mut lines,
            &row,
            Some(GitLine {
                style: GitLineStyle::Branch,
                entry: Some(GitEntry::Branch(branch.name.clone())),
                dim_from,
                badge: Some((badge_len, color)),
            }),
        );
    }

    header(&mut text, &mut lines, "Remotes");
    if remotes.is_empty() {
        push(&mut text, &mut lines, "    (none)", Some(GitLine { style: GitLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    }
    for name in remotes {
        push(
            &mut text,
            &mut lines,
            &format!("    {name}"),
            Some(GitLine { style: GitLineStyle::Branch, entry: Some(GitEntry::Branch(name.clone())), dim_from: None, badge: None }),
        );
    }

    header(&mut text, &mut lines, "Tags");
    if tags.is_empty() {
        push(&mut text, &mut lines, "    (none)", Some(GitLine { style: GitLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    }
    for name in tags {
        push(
            &mut text,
            &mut lines,
            &format!("    {name}"),
            Some(GitLine { style: GitLineStyle::Branch, entry: Some(GitEntry::Branch(name.clone())), dim_from: None, badge: None }),
        );
    }

    GitPanel { text, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, subject: &str, refs: &[&str], parents: &[&str]) -> GraphCommit {
        GraphCommit {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: refs.iter().map(|r| r.to_string()).collect(),
            author: "Test".to_string(),
            relative_date: "now".to_string(),
            subject: subject.to_string(),
        }
    }

    fn branch(name: &str, upstream: Option<&str>, ahead: usize, behind: usize, gone: bool) -> Branch {
        Branch {
            name: name.to_string(),
            current: false,
            upstream: upstream.map(str::to_string),
            ahead,
            behind,
            upstream_gone: gone,
        }
    }

    #[test]
    fn a_linear_history_draws_one_rail_of_nodes() {
        let commits =
            vec![commit("aaa1111", "third", &[], &["bbb2222"]), commit("bbb2222", "second", &[], &["ccc3333"]), commit("ccc3333", "first", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        for line in panel.text.lines() {
            assert!(line.starts_with('●'), "every row is a node on the single rail: {line:?}");
        }
        assert_eq!(panel.text.lines().count(), 3);
    }

    #[test]
    fn a_merge_row_is_marked_and_opens_a_rail_to_the_side_it_merged() {
        let commits = vec![
            commit("mmm1111", "merge side", &[], &["ppp2222", "sss3333"]),
            commit("ppp2222", "on main", &[], &["aaa4444"]),
            commit("sss3333", "on side", &[], &["aaa4444"]),
            commit("aaa4444", "initial", &[], &[]),
        ];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        let merge_row = panel.text.lines().next().unwrap();
        assert!(merge_row.starts_with("◆"), "a merge reads differently from a plain commit: {merge_row:?}");
        assert!(merge_row.contains('╮'), "and opens a rail toward the branch it merged: {merge_row:?}");
        // The two sides converge again at the root.
        let root_row = panel.text.lines().nth(3).unwrap();
        assert!(root_row.contains('╯'), "the side rail closes back in: {root_row:?}");
    }

    #[test]
    fn a_rail_passing_a_row_it_is_unrelated_to_is_drawn_through_it() {
        let commits =
            vec![commit("xxx1111", "tip x", &[], &["aaa3333"]), commit("yyy2222", "tip y", &[], &["aaa3333"]), commit("aaa3333", "root", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        let second = panel.text.lines().nth(1).unwrap();
        assert!(second.starts_with("│●"), "lane 0 passes by while `y` is drawn: {second:?}");
    }

    #[test]
    fn a_row_shows_its_hash_ref_decorations_and_subject_in_that_order() {
        let commits = vec![commit("abc1234", "Fix the thing", &["HEAD -> main", "origin/main"], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        let row = panel.text.lines().next().unwrap();
        assert!(row.contains("abc1234"));
        assert!(row.contains("(HEAD -> main, origin/main)"));
        assert!(row.ends_with("Fix the thing"));
    }

    #[test]
    fn every_graph_row_anchors_to_its_commit_and_spans_ascend() {
        let commits = vec![commit("abc1234", "Fix the thing", &["main"], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        let meta = panel.lines[0].as_ref().unwrap();
        assert_eq!(meta.commit.as_deref(), Some("abc1234"));
        let kinds: Vec<GraphSpan> = meta.spans.iter().map(|(_, k)| *k).collect();
        assert_eq!(kinds, vec![GraphSpan::Rails, GraphSpan::Hash, GraphSpan::Refs, GraphSpan::Subject]);
        assert!(meta.spans.windows(2).all(|w| w[0].0 < w[1].0), "span starts must ascend: {:?}", meta.spans);
    }

    #[test]
    fn a_row_with_no_refs_has_no_refs_span() {
        let commits = vec![commit("abc1234", "plain", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        let kinds: Vec<GraphSpan> = panel.lines[0].as_ref().unwrap().spans.iter().map(|(_, k)| *k).collect();
        assert_eq!(kinds, vec![GraphSpan::Rails, GraphSpan::Hash, GraphSpan::Subject]);
    }

    #[test]
    fn an_empty_history_says_so() {
        let panel = render_graph(&[], &[]);
        assert!(panel.text.contains("(no commits)"));
        assert_eq!(panel.lines.len(), 1);
    }

    #[test]
    fn text_and_lines_stay_the_same_length() {
        let commits = vec![commit("aaa1111", "a", &[], &["bbb2222"]), commit("bbb2222", "b", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows);
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn the_sync_badge_tells_the_four_upstream_states_apart() {
        assert_eq!(sync_badge(&branch("a", Some("origin/a"), 0, 0, false)).0, "[=]");
        assert_eq!(sync_badge(&branch("a", Some("origin/a"), 2, 0, false)).0, "[^2]");
        assert_eq!(sync_badge(&branch("a", Some("origin/a"), 0, 3, false)).0, "[v3]");
        assert_eq!(sync_badge(&branch("a", Some("origin/a"), 2, 3, false)).0, "[^2 v3]");
        assert_eq!(sync_badge(&branch("a", Some("origin/a"), 0, 0, true)).0, "[gone]");
        assert_eq!(sync_badge(&branch("a", None, 0, 0, false)).0, "[--]");
    }

    #[test]
    fn an_in_sync_branch_and_a_gone_one_are_never_the_same_color() {
        let in_sync = sync_badge(&branch("a", Some("origin/a"), 0, 0, false));
        let gone = sync_badge(&branch("a", Some("origin/a"), 0, 0, true));
        assert_eq!(in_sync.1, GitBadgeColor::Good);
        assert_eq!(gone.1, GitBadgeColor::Bad);
    }

    #[test]
    fn the_refs_tree_lists_all_three_sections_with_their_entries() {
        let branches = vec![branch("main", Some("origin/main"), 0, 0, false)];
        let panel = render_refs(&branches, &["origin/main".to_string()], &["v1.0".to_string()], Some(120));
        assert!(panel.text.contains("Local"));
        assert!(panel.text.contains("Remotes"));
        assert!(panel.text.contains("Tags"));
        assert!(panel.text.contains("main  -> origin/main"));
        assert!(panel.text.contains("v1.0"));
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn a_local_branch_row_carries_a_branch_entry_so_it_can_be_acted_on() {
        let branches = vec![branch("feature", Some("origin/feature"), 1, 0, false)];
        let panel = render_refs(&branches, &[], &[], None);
        let entries: Vec<&GitEntry> = panel.lines.iter().flatten().filter_map(|l| l.entry.as_ref()).collect();
        assert_eq!(entries[0], &GitEntry::Branch("feature".to_string()));
    }

    #[test]
    fn the_current_branch_is_marked() {
        let mut b = branch("main", Some("origin/main"), 0, 0, false);
        b.current = true;
        let panel = render_refs(&[b], &[], &[], None);
        assert!(panel.text.contains("*[=] main"), "got:\n{}", panel.text);
    }

    #[test]
    fn empty_sections_say_none_rather_than_rendering_nothing() {
        let panel = render_refs(&[], &[], &[], None);
        assert_eq!(panel.text.matches("(none)").count(), 3);
    }

    #[test]
    fn the_fetch_age_line_says_how_stale_the_sync_badges_are() {
        assert_eq!(fetch_age_text(None), "never fetched");
        assert_eq!(fetch_age_text(Some(10)), "fetched just now");
        assert_eq!(fetch_age_text(Some(600)), "fetched 10m ago");
        assert_eq!(fetch_age_text(Some(7200)), "fetched 2h ago");
        assert_eq!(fetch_age_text(Some(200_000)), "fetched 2d ago");
    }
}
