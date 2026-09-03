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

/// Which characters the rails are drawn with.
///
/// ASCII is the default, and not for nostalgia: the box-drawing glyphs
/// are frequently missing from a monospace font, and when they are, the
/// renderer silently falls back to some *other* font whose advance width
/// doesn't match -- so every row shifts by a different amount depending
/// on how many rails it happens to contain, and the graph stops lining
/// up. ASCII cannot do that, and it's what `git log --graph` itself
/// draws. `unicode` is there for anyone whose font does have the glyphs
/// (`[git] graph_style = unicode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStyle {
    Ascii,
    Unicode,
}

impl GraphStyle {
    /// `(commit, merge, rail, branch-out, close-in)`.
    fn glyphs(self) -> (char, char, char, char, char) {
        match self {
            GraphStyle::Ascii => ('*', '*', '|', '\\', '/'),
            GraphStyle::Unicode => ('●', '◆', '│', '╲', '╱'),
        }
    }

    /// Parses `[git] graph_style`; anything unrecognized (including
    /// nothing configured) is ASCII, since that's the option that can't
    /// render wrong.
    pub fn from_config(name: Option<&str>) -> Self {
        match name.map(str::trim) {
            Some("unicode") => GraphStyle::Unicode,
            _ => GraphStyle::Ascii,
        }
    }
}

/// Two screen columns per lane, matching `git log --graph`.
///
/// One column per lane looked tighter but left no room between rails for
/// a diagonal, so every merge connector had to be crammed onto the
/// commit's own row (`◆┼╮`) where it read as noise. With two, a
/// connector gets its own column and the graph is legible.
fn lane_col(lane: usize) -> usize {
    lane * 2
}

/// A row of rail cells, `lanes` wide, trimmed of trailing blanks.
struct Rails {
    cells: Vec<char>,
}

impl Rails {
    fn new(lanes: usize) -> Self {
        Self { cells: vec![' '; lanes * 2] }
    }

    fn set(&mut self, col: usize, ch: char) {
        if col < self.cells.len() {
            self.cells[col] = ch;
        }
    }

    /// Vertical rails for every lane in `live`.
    fn rails_at(&mut self, live: impl IntoIterator<Item = usize>, rail: char) {
        for lane in live {
            self.set(lane_col(lane), rail);
        }
    }

    fn finish(self, width: usize) -> String {
        let mut out: String = self.cells.into_iter().collect();
        // Padded rather than trimmed: every row has to occupy the same
        // width or the hash column after it wanders.
        while out.chars().count() < width {
            out.push(' ');
        }
        out
    }
}

/// The commit graph, in `git log --graph`'s own shape: a row per commit,
/// plus a connector row above a commit that branches converge on (`|/`)
/// and below one that opens a branch (`|\`).
///
/// Those connector rows are what make a merge readable. Drawing every
/// edge on the commit's own row instead packs three meanings into one
/// line of glyphs; giving each edge its own row is both what git does
/// and what makes the shape of the history obvious at a glance.
///
/// Short hashes are padded to a common width. Git abbreviates them to
/// whatever length keeps each one unambiguous, so in a big repo they are
/// *not* all the same length, and an unpadded column visibly staggers.
pub fn render_graph(commits: &[GraphCommit], rows: &[GraphRow], style: GraphStyle) -> GraphPanel {
    let mut text = String::new();
    let mut lines = Vec::new();
    if commits.is_empty() {
        text.push_str("    (no commits)\n");
        lines.push(Some(GraphLine { spans: vec![(0, GraphSpan::Meta)], commit: None }));
        return GraphPanel { text, lines };
    }

    let (node_g, merge_g, rail_g, branch_g, close_g) = style.glyphs();
    let lane_count = rows
        .iter()
        .map(|r| {
            let widest = r.live_before.iter().chain(r.live_after.iter()).copied().max().unwrap_or(0);
            r.lane.max(widest) + 1
        })
        .max()
        .unwrap_or(1);
    let rail_width = lane_count * 2;
    let hash_width = commits.iter().map(|c| c.short_hash.chars().count()).max().unwrap_or(7);

    let mut push_edge = |text: &mut String, lines: &mut Vec<Option<GraphLine>>, rails: Rails| {
        let row = rails.finish(rail_width);
        // Trailing blanks carry no information on a connector row, but
        // the leading rails must keep their columns.
        text.push_str(row.trim_end());
        text.push('\n');
        lines.push(Some(GraphLine { spans: vec![(0, GraphSpan::Rails)], commit: None }));
    };

    for (commit, row) in commits.iter().zip(rows) {
        // Above the commit: lanes converging into it, drawn collapsing
        // toward its own lane.
        if !row.closing.is_empty() {
            let mut rails = Rails::new(lane_count);
            rails.rails_at(row.live_before.iter().copied().filter(|l| !row.closing.contains(l)), rail_g);
            rails.set(lane_col(row.lane), rail_g);
            for &c in &row.closing {
                let (col, glyph) = if c > row.lane { (lane_col(c) - 1, close_g) } else { (lane_col(c) + 1, branch_g) };
                rails.set(col, glyph);
            }
            push_edge(&mut text, &mut lines, rails);
        }

        // The commit itself: a rail for every lane that was already live
        // and isn't converging here, and the node in its own lane.
        let mut rails = Rails::new(lane_count);
        rails.rails_at(row.live_before.iter().copied().filter(|l| !row.closing.contains(l) && *l != row.lane), rail_g);
        rails.set(lane_col(row.lane), if row.is_merge { merge_g } else { node_g });

        let rail = rails.finish(rail_width);
        let mut line = format!("{rail} {:<hash_width$} ", commit.short_hash);
        let mut spans = vec![(0, GraphSpan::Rails), (rail_width + 1, GraphSpan::Hash)];
        if !commit.refs.is_empty() {
            spans.push((line.chars().count(), GraphSpan::Refs));
            line.push_str(&format!("({}) ", commit.refs.join(", ")));
        }
        spans.push((line.chars().count(), GraphSpan::Subject));
        line.push_str(&commit.subject);
        text.push_str(&line);
        text.push('\n');
        lines.push(Some(GraphLine { spans, commit: Some(commit.hash.clone()) }));

        // Below the commit: the lane(s) a merge opened, fanning out.
        if !row.branching.is_empty() {
            let mut rails = Rails::new(lane_count);
            rails.rails_at(row.live_after.iter().copied().filter(|l| !row.branching.contains(l)), rail_g);
            for &b in &row.branching {
                let (col, glyph) = if b > row.lane { (lane_col(b) - 1, branch_g) } else { (lane_col(b) + 1, close_g) };
                rails.set(col, glyph);
            }
            push_edge(&mut text, &mut lines, rails);
        }
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

    /// Only the rows that draw a commit -- connector rows have no
    /// commit of their own.
    fn commit_rows(panel: &GraphPanel) -> Vec<String> {
        panel
            .text
            .lines()
            .zip(&panel.lines)
            .filter(|(_, meta)| meta.as_ref().is_some_and(|m| m.commit.is_some()))
            .map(|(text, _)| text.to_string())
            .collect()
    }

    #[test]
    fn a_linear_history_draws_one_rail_of_nodes_and_no_connector_rows() {
        let commits =
            vec![commit("aaa1111", "third", &[], &["bbb2222"]), commit("bbb2222", "second", &[], &["ccc3333"]), commit("ccc3333", "first", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        assert_eq!(panel.text.lines().count(), 3, "nothing branches, so there is nothing to connect");
        for line in panel.text.lines() {
            assert!(line.starts_with('*'), "every row is a node on the single rail: {line:?}");
        }
    }

    /// The defect this rendering was rebuilt to fix: with one column per
    /// lane and no connector rows, merge edges were crammed onto the
    /// commit's own row and the hash column wandered from row to row.
    #[test]
    fn every_commit_row_starts_its_hash_in_the_same_column() {
        let commits = vec![
            commit("mmm1111", "merge side", &[], &["ppp2222", "sss3333"]),
            commit("ppp2222", "on main", &[], &["aaa4444"]),
            commit("sss3333", "on side", &[], &["aaa4444"]),
            commit("aaa4444", "initial", &[], &[]),
        ];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);

        let hash_columns: Vec<usize> =
            commit_rows(&panel).iter().zip(&commits).map(|(line, c)| line.find(c.short_hash.as_str()).unwrap()).collect();
        assert!(hash_columns.windows(2).all(|w| w[0] == w[1]), "hash columns must all match: {hash_columns:?}
{}", panel.text);
    }

    #[test]
    fn hashes_of_differing_abbreviated_length_are_padded_to_one_column() {
        // Git abbreviates each hash to whatever keeps it unambiguous, so
        // in a real repo they are not all the same length.
        let mut short = commit("abc1234", "short hash", &[], &["dddddddddd"]);
        short.short_hash = "abc1234".to_string();
        let mut long = commit("dddddddddd", "longer hash", &[], &[]);
        long.short_hash = "dddddddddd".to_string();
        let commits = vec![short, long];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);

        let subject_columns: Vec<usize> =
            commit_rows(&panel).iter().zip(&commits).map(|(line, c)| line.find(c.subject.as_str()).unwrap()).collect();
        assert_eq!(subject_columns[0], subject_columns[1], "subjects must line up despite differing hash lengths:
{}", panel.text);
    }

    #[test]
    fn a_merge_gets_a_connector_row_below_it_opening_the_merged_lane() {
        let commits = vec![
            commit("mmm1111", "merge side", &[], &["ppp2222", "sss3333"]),
            commit("ppp2222", "on main", &[], &["aaa4444"]),
            commit("sss3333", "on side", &[], &["aaa4444"]),
            commit("aaa4444", "initial", &[], &[]),
        ];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        let lines: Vec<&str> = panel.text.lines().collect();

        assert!(lines[0].starts_with('*'), "the merge commit's own row: {:?}", lines[0]);
        assert_eq!(lines[1], "|\\", "a connector row fans the merged branch out, exactly as git draws it");
        assert!(panel.lines[1].as_ref().unwrap().commit.is_none(), "a connector row draws no commit");
    }

    #[test]
    fn converging_branches_get_a_connector_row_above_the_commit_they_meet_at() {
        let commits = vec![
            commit("mmm1111", "merge side", &[], &["ppp2222", "sss3333"]),
            commit("ppp2222", "on main", &[], &["aaa4444"]),
            commit("sss3333", "on side", &[], &["aaa4444"]),
            commit("aaa4444", "initial", &[], &[]),
        ];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        let lines: Vec<&str> = panel.text.lines().collect();
        let root = lines.iter().position(|l| l.contains("initial")).unwrap();

        assert_eq!(lines[root - 1], "|/", "the side lane collapses in just above the shared ancestor");
    }

    #[test]
    fn a_rail_passing_a_row_it_is_unrelated_to_is_drawn_through_it() {
        let commits =
            vec![commit("xxx1111", "tip x", &[], &["aaa3333"]), commit("yyy2222", "tip y", &[], &["aaa3333"]), commit("aaa3333", "root", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        let second = commit_rows(&panel)[1].clone();
        assert!(second.starts_with("| *"), "lane 0 passes by while `y` is drawn: {second:?}");
    }

    #[test]
    fn each_lane_gets_two_columns_so_connectors_have_room() {
        let commits = vec![
            commit("mmm1111", "merge side", &[], &["ppp2222", "sss3333"]),
            commit("ppp2222", "on main", &[], &["aaa4444"]),
            commit("sss3333", "on side", &[], &["aaa4444"]),
            commit("aaa4444", "initial", &[], &[]),
        ];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        // Two lanes -> four rail columns, then a space, so the hash
        // starts at column 5 on every commit row.
        for (line, commit) in commit_rows(&panel).iter().zip(&commits) {
            assert_eq!(line.find(commit.short_hash.as_str()), Some(5), "{line:?}");
        }
    }

    #[test]
    fn the_unicode_style_swaps_the_glyphs_without_changing_the_layout() {
        let commits = vec![commit("mmm1111", "merge", &[], &["ppp2222", "sss3333"]), commit("ppp2222", "a", &[], &[]), commit("sss3333", "b", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let ascii = render_graph(&commits, &rows, GraphStyle::Ascii);
        let unicode = render_graph(&commits, &rows, GraphStyle::Unicode);
        assert!(unicode.text.contains('◆'), "a merge reads differently in unicode:
{}", unicode.text);
        assert_eq!(
            ascii.text.lines().count(),
            unicode.text.lines().count(),
            "the styles differ only in glyphs, never in structure"
        );
        assert_eq!(
            ascii.text.lines().map(|l| l.chars().count()).collect::<Vec<_>>(),
            unicode.text.lines().map(|l| l.chars().count()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_graph_style_defaults_to_ascii_for_anything_unconfigured_or_unknown() {
        assert_eq!(GraphStyle::from_config(None), GraphStyle::Ascii);
        assert_eq!(GraphStyle::from_config(Some("nonsense")), GraphStyle::Ascii);
        assert_eq!(GraphStyle::from_config(Some("unicode")), GraphStyle::Unicode);
        assert_eq!(GraphStyle::from_config(Some(" unicode ")), GraphStyle::Unicode);
    }

    #[test]
    fn a_row_shows_its_hash_ref_decorations_and_subject_in_that_order() {
        let commits = vec![commit("abc1234", "Fix the thing", &["HEAD -> main", "origin/main"], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        let row = panel.text.lines().next().unwrap();
        assert!(row.contains("abc1234"));
        assert!(row.contains("(HEAD -> main, origin/main)"));
        assert!(row.ends_with("Fix the thing"));
    }

    #[test]
    fn every_graph_row_anchors_to_its_commit_and_spans_ascend() {
        let commits = vec![commit("abc1234", "Fix the thing", &["main"], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
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
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
        let kinds: Vec<GraphSpan> = panel.lines[0].as_ref().unwrap().spans.iter().map(|(_, k)| *k).collect();
        assert_eq!(kinds, vec![GraphSpan::Rails, GraphSpan::Hash, GraphSpan::Subject]);
    }

    #[test]
    fn an_empty_history_says_so() {
        let panel = render_graph(&[], &[], GraphStyle::Ascii);
        assert!(panel.text.contains("(no commits)"));
        assert_eq!(panel.lines.len(), 1);
    }

    #[test]
    fn text_and_lines_stay_the_same_length() {
        let commits = vec![commit("aaa1111", "a", &[], &["bbb2222"]), commit("bbb2222", "b", &[], &[])];
        let rows = fenix_git::assign_lanes(&commits);
        let panel = render_graph(&commits, &rows, GraphStyle::Ascii);
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

