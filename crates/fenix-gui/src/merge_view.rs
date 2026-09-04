//! A conflicted file rendered as two aligned columns -- ours on the
//! left, theirs on the right -- with each conflict called out and
//! numbered.
//!
//! Reading raw conflict markers means holding three things in your head
//! at once: which lines belong to which side, which side "ours" even is
//! (during a rebase it's the branch you're rebasing *onto*, not yours),
//! and what the surrounding file looked like before any of this. The
//! two-column form answers all three at a glance: the sides sit next to
//! each other under their real branch names, and shared context spans
//! both columns so it reads as one file rather than two.
//!
//! Two columns in *one* buffer rather than two panes side by side.
//! Alignment is then free and exact -- row N of the left column is row N
//! of the right one by construction -- and so are scrolling, cursor
//! movement and "which conflict am I on", none of which would stay in
//! step across two independently-scrolled panes.
//!
//! Produces the same `{text, lines}` shape as `diff_view`/`git_panel`,
//! with per-row metadata naming the conflict a row belongs to and the
//! side it came from, so acting on "the conflict under the cursor" is
//! one lookup.

use fenix_git::Conflict;

/// Which column a row's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Shared text, outside any conflict -- spans both columns.
    Both,
    Ours,
    Theirs,
    /// The common ancestor, shown only when the file was merged with
    /// `diff3`/`zdiff3` and so actually records one.
    Base,
}

/// How one rendered row should be colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStyle {
    /// The heading above the columns, naming both branches.
    Header,
    /// The `── conflict 2 of 3 ──` separator opening a conflict.
    Separator,
    /// File text -- shared, or one of the two columns. Which column a
    /// row's text came from is `Side`'s job, not this one's: a
    /// two-column row is a single row carrying both, so its style says
    /// "file content" and its `gutter` says where to split the colors.
    Context,
    /// The common ancestor's version, under a `diff3` conflict.
    Base,
    /// The legend and the "no conflicts left" note.
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeViewLine {
    pub style: MergeStyle,
    pub side: Side,
    /// Index into the conflict list this row belongs to, or `None` for
    /// shared text and headings. What "resolve the conflict under the
    /// cursor" reads.
    pub conflict: Option<usize>,
    /// The row's line number in the real file, so `Enter` can open the
    /// file at the line the cursor is on. `None` for rows this view
    /// invented (headings, separators, alignment padding).
    pub file_line: Option<usize>,
    /// Character column the column divider sits in, for a row that has
    /// two columns. The host colors everything left of it as ours and
    /// everything right of it as theirs -- computed once here rather
    /// than re-derived at highlight time, so the colors can never
    /// disagree with the text about where the split is.
    pub gutter: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeView {
    pub text: String,
    pub lines: Vec<Option<MergeViewLine>>,
}

/// Names for the two sides, already resolved to branches by
/// `fenix_git::conflict_sides`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideLabels {
    pub ours: String,
    pub theirs: String,
    pub ours_role: String,
    pub theirs_role: String,
}

impl SideLabels {
    /// The fallback when the repo isn't in a state that names the sides
    /// (a file left conflicted after the operation was concluded, say).
    /// Git's own marker words, rather than a guess at branches.
    pub fn unknown() -> Self {
        SideLabels {
            ours: "ours".to_string(),
            theirs: "theirs".to_string(),
            ours_role: "HEAD, as the markers label it".to_string(),
            theirs_role: "the incoming side".to_string(),
        }
    }
}

impl From<&fenix_git::ConflictSides> for SideLabels {
    fn from(sides: &fenix_git::ConflictSides) -> Self {
        SideLabels {
            ours: sides.ours.clone(),
            theirs: sides.theirs.clone(),
            ours_role: sides.ours_role.to_string(),
            theirs_role: sides.theirs_role.to_string(),
        }
    }
}

/// Total width the two columns share, gutter included.
const WIDTH: usize = 160;
/// The `│` between the columns, plus a space either side.
const GUTTER: &str = " │ ";
/// The divider on its own -- what a row is searched for to find where
/// its two columns split.
const DIVIDER: char = '│';

pub fn column_width() -> usize {
    (WIDTH - GUTTER.chars().count()) / 2
}

/// Renders `text` -- a working-tree file with conflict markers in it --
/// as two aligned columns.
///
/// `conflicts` comes from `fenix_git::find_conflicts` on the same text;
/// passing them in rather than recomputing keeps this view and the
/// resolve actions working from one list, so "conflict 2" means the same
/// thing to both.
pub fn render(path: &str, text: &str, conflicts: &[Conflict], labels: &SideLabels) -> MergeView {
    let mut out = Builder::new();
    let lines: Vec<&str> = text.split('\n').collect();

    out.push(&format!("  {path}"), MergeStyle::Header, Side::Both, None, None);
    if conflicts.is_empty() {
        out.push("  No conflict markers left  --  s stages it as resolved, u puts the conflict back", MergeStyle::Meta, Side::Both, None, None);
        out.push("", MergeStyle::Meta, Side::Both, None, None);
        // The resolved file, not an empty pane: the last thing anyone
        // wants after choosing a side is to have to go somewhere else
        // to check what they actually ended up with.
        for (n, content) in lines.iter().enumerate() {
            if n + 1 == lines.len() && content.is_empty() {
                break;
            }
            out.push(&format!("  {content}"), MergeStyle::Context, Side::Both, None, Some(n));
        }
        return out.finish();
    }

    // The legend is the whole point of the view: it says which branch
    // each column is, and -- for a rebase, where git's own words are
    // actively misleading -- what that branch's role is.
    out.push(&two_columns(&format!("<<< {} ", labels.ours), &format!(">>> {} ", labels.theirs)), MergeStyle::Header, Side::Both, None, None);
    out.push(&two_columns(&labels.ours_role, &labels.theirs_role), MergeStyle::Meta, Side::Both, None, None);
    out.push(
        &format!("  {} conflict{}  --  o keeps the left, t the right, b both; n/p to move between them", conflicts.len(), if conflicts.len() == 1 { "" } else { "s" }),
        MergeStyle::Meta,
        Side::Both,
        None,
        None,
    );
    out.push("", MergeStyle::Meta, Side::Both, None, None);

    let mut line = 0usize;
    for (index, conflict) in conflicts.iter().enumerate() {
        // Everything between the previous conflict and this one is
        // shared: one column spanning the full width, because splitting
        // identical text into two identical columns is pure noise.
        while line < conflict.start {
            out.push(&format!("  {}", lines.get(line).copied().unwrap_or_default()), MergeStyle::Context, Side::Both, None, Some(line));
            line += 1;
        }
        out.push(&separator(index, conflicts.len()), MergeStyle::Separator, Side::Both, Some(index), Some(conflict.start));

        let ours: Vec<&str> = lines.get(conflict.ours.clone()).unwrap_or_default().to_vec();
        let theirs: Vec<&str> = lines.get(conflict.theirs.clone()).unwrap_or_default().to_vec();
        // Both columns get the taller side's height, so the two
        // versions start on the same row and stay comparable even when
        // one side is much longer.
        for row in 0..ours.len().max(theirs.len()) {
            let left = ours.get(row).copied().unwrap_or("");
            let right = theirs.get(row).copied().unwrap_or("");
            // The row belongs to whichever column actually has content
            // on it; where both do, it's shown as ours (the left column
            // is the one the eye anchors on) but either side's own
            // colors come from the per-column highlight ranges the host
            // builds from `Side`.
            let side = if row < ours.len() { Side::Ours } else { Side::Theirs };
            let file_line = if row < ours.len() { conflict.ours.start.checked_add(row) } else { conflict.theirs.start.checked_add(row - ours.len()) };
            out.push(&two_columns(left, right), MergeStyle::Context, side, Some(index), file_line);
        }

        // `diff3`/`zdiff3` records what both sides started from. It's
        // the single most useful thing for deciding which side is
        // right, so it's shown -- below the columns rather than as a
        // third one, which at this width would leave nothing readable.
        if let Some(base) = conflict.base.clone() {
            let base_lines: Vec<&str> = lines.get(base.clone()).unwrap_or_default().to_vec();
            if !base_lines.is_empty() {
                out.push("      both sides started from:", MergeStyle::Meta, Side::Base, Some(index), None);
                for (row, content) in base_lines.iter().enumerate() {
                    out.push(&format!("      {content}"), MergeStyle::Base, Side::Base, Some(index), base.start.checked_add(row));
                }
            }
        }

        line = conflict.end + 1;
    }
    while line < lines.len() {
        // A trailing newline leaves one empty final piece that isn't a
        // line of the file; showing it would add a phantom row.
        if line + 1 == lines.len() && lines[line].is_empty() {
            break;
        }
        out.push(&format!("  {}", lines[line]), MergeStyle::Context, Side::Both, None, Some(line));
        line += 1;
    }

    out.finish()
}

/// `──── conflict 2 of 3 ─────...` -- ASCII-safe box drawing is not
/// used here because the row is decoration only; see `graph_view`'s own
/// note on why glyph width matters for anything that has to line up.
fn separator(index: usize, total: usize) -> String {
    let label = format!(" conflict {} of {total} ", index + 1);
    let dashes = WIDTH.saturating_sub(label.chars().count() + 2);
    format!("  {}{label}{}", "-".repeat(4), "-".repeat(dashes.saturating_sub(4)))
}

/// One row of the two columns, left padded out to the column width so
/// the gutter lands in the same place on every row.
///
/// A line longer than the column is truncated with a `…` rather than
/// wrapped: wrapping would push the two columns out of alignment, which
/// is the one thing this view exists to guarantee. The full text is
/// always a keypress away in the real file.
fn two_columns(left: &str, right: &str) -> String {
    format!("{}{GUTTER}{}", fit(&format!("  {left}"), column_width()), right)
}

fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return format!("{text}{}", " ".repeat(width - count));
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

struct Builder {
    text: String,
    lines: Vec<Option<MergeViewLine>>,
}

impl Builder {
    fn new() -> Self {
        Builder { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, style: MergeStyle, side: Side, conflict: Option<usize>, file_line: Option<usize>) {
        if !self.lines.is_empty() {
            self.text.push('\n');
        }
        let trimmed = text.trim_end();
        // Only a genuine two-column row carries a divider: these rows
        // are built by `two_columns` and nothing else, so finding the
        // glyph is the same as asking whether this is one of them.
        let gutter = trimmed.find(DIVIDER).map(|byte| trimmed[..byte].chars().count());
        self.text.push_str(trimmed);
        self.lines.push(Some(MergeViewLine { style, side, conflict, file_line, gutter }));
    }

    fn finish(mut self) -> MergeView {
        self.text.push('\n');
        MergeView { text: self.text, lines: self.lines }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFLICTED: &str = "shared line\n<<<<<<< HEAD\nfrom develop\n=======\nfrom my branch\n>>>>>>> abc1234 (feat)\nfooter\n";

    fn labels() -> SideLabels {
        SideLabels {
            ours: "develop".to_string(),
            theirs: "myfeature".to_string(),
            ours_role: "the branch you are rebasing onto".to_string(),
            theirs_role: "your own commit being replayed".to_string(),
        }
    }

    fn render_conflicted(text: &str) -> MergeView {
        render("app.conf", text, &fenix_git::find_conflicts(text), &labels())
    }

    #[test]
    fn the_heading_names_both_branches_and_their_roles() {
        let view = render_conflicted(CONFLICTED);
        assert!(view.text.contains("<<< develop"), "got:\n{}", view.text);
        assert!(view.text.contains(">>> myfeature"), "got:\n{}", view.text);
        // The role line is the whole reason this view exists: git's own
        // "ours" is the rebase target, which is the opposite of what
        // the word suggests.
        assert!(view.text.contains("the branch you are rebasing onto"), "got:\n{}", view.text);
        assert!(view.text.contains("your own commit being replayed"), "got:\n{}", view.text);
    }

    #[test]
    fn the_two_sides_sit_on_the_same_row() {
        let view = render_conflicted(CONFLICTED);
        let row = view.text.lines().find(|l| l.contains("from develop")).expect("ours is shown");
        assert!(row.contains("from my branch"), "both sides share a row so they can be compared:\n{row}");
        // And the gutter is where the column width says it is.
        let gutter = row.find('│').expect("a gutter between the columns");
        assert_eq!(row[..gutter].chars().count(), column_width() + 1, "every row's gutter lands in the same column");
    }

    #[test]
    fn shared_text_spans_both_columns_instead_of_being_duplicated() {
        let view = render_conflicted(CONFLICTED);
        assert_eq!(view.text.matches("shared line").count(), 1);
        assert_eq!(view.text.matches("footer").count(), 1);
        let shared = view.lines.iter().flatten().find(|l| l.file_line == Some(0)).unwrap();
        assert_eq!(shared.side, Side::Both);
        assert_eq!(shared.conflict, None);
    }

    #[test]
    fn every_conflict_row_names_the_conflict_it_belongs_to() {
        let two = "a\n<<<<<<< HEAD\nx1\n=======\ny1\n>>>>>>> b\nmid\n<<<<<<< HEAD\nx2\n=======\ny2\n>>>>>>> b\nz\n";
        let view = render_conflicted(two);
        assert!(view.text.contains("conflict 1 of 2"));
        assert!(view.text.contains("conflict 2 of 2"));
        let second = view.lines.iter().flatten().find(|l| l.conflict == Some(1) && l.style == MergeStyle::Context).unwrap();
        assert_eq!(second.conflict, Some(1));
    }

    #[test]
    fn a_longer_side_pads_the_shorter_one_rather_than_shifting_it() {
        let uneven = "a\n<<<<<<< HEAD\np\nq\nr\n=======\nz\n>>>>>>> b\n";
        let view = render_conflicted(uneven);
        let rows: Vec<usize> = view
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| l.conflict.is_some() && l.style == MergeStyle::Context))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(rows.len(), 3, "the taller side sets the height");
        let text: Vec<&str> = view.text.lines().collect();
        // The short side's missing rows are blank, not absent, so the
        // columns stay aligned all the way down.
        for row in &rows {
            let line = text[*row];
            let gutter = line.find('│').expect("every conflict row keeps its gutter");
            assert_eq!(line[..gutter].chars().count(), column_width() + 1);
        }
        assert!(text[rows[1]].trim_end().ends_with('│'), "the right column is empty here: {:?}", text[rows[1]]);
    }

    #[test]
    fn the_common_ancestor_is_shown_when_the_merge_style_recorded_one() {
        let diff3 = "a\n<<<<<<< HEAD\nmine\n||||||| base\nstarted here\n=======\ntheirs\n>>>>>>> b\n";
        let view = render_conflicted(diff3);
        assert!(view.text.contains("both sides started from:"), "got:\n{}", view.text);
        assert!(view.text.contains("started here"));
    }

    #[test]
    fn a_file_with_nothing_left_to_resolve_says_what_to_do_next() {
        let view = render_conflicted("all resolved\n");
        assert!(view.text.contains("No conflict markers left"));
        assert!(view.text.contains("stages it as resolved"), "got:\n{}", view.text);
    }

    #[test]
    fn a_line_too_wide_for_its_column_is_cut_not_wrapped() {
        // Wrapping would push the right column down a row and break the
        // alignment this whole view is for.
        let long = format!("a\n<<<<<<< HEAD\n{}\n=======\nshort\n>>>>>>> b\n", "x".repeat(400));
        let view = render_conflicted(&long);
        let row = view.text.lines().find(|l| l.contains('…')).expect("the long line is truncated");
        let gutter = row.find('│').unwrap();
        assert_eq!(row[..gutter].chars().count(), column_width() + 1);
        assert!(row.ends_with("short"));
    }
}
