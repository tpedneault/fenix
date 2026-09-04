use std::collections::{BTreeMap, HashSet};

use fenix_git::{Branch, Commit, FileEntry, RepoStatus, Stash};

/// What a git-panel line represents -- what the per-pane action keys
/// (see `app.rs`'s Git action routing) act on when the cursor is on
/// this line. Mirrors `docker_panel::DockerEntry`. `Dir` targets a
/// whole directory (its full repo-relative path, no trailing slash) --
/// stage/unstage/discard on it act on every file underneath, not just
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitEntry {
    File(String),
    Dir(String),
    Branch(String),
    Commit(String),
    Stash(usize),
}

/// How one generated line should be colored -- same role as
/// `docker_panel::DockerLineStyle`. Diff lines aren't in here: the Main
/// pane renders through `diff_view` (its own `BufferKind::Diff`, with
/// hunk-aware metadata) rather than as prefix-colored text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitLineStyle {
    File,
    /// A directory row in the Files pane's tree (`> src/` collapsed,
    /// `v src/` expanded) -- plain, undecorated text; unlike `File`,
    /// there's no per-row git status badge to color (a directory
    /// aggregates an unknown mix of statuses underneath it).
    Dir,
    Branch,
    Commit,
    Stash,
    /// A `label: value` row in the Status pane.
    Detail,
    /// A section title in a listing that has several ("Local",
    /// "Remotes", "Tags" in the History view's refs tree) -- the same
    /// role `jira_panel::JiraLineStyle::SectionHeader` already plays.
    Header,
    /// Shown when a list comes back empty, or the Main pane has nothing
    /// selected/no changes to show.
    Empty,
}

/// A coarse "how's this doing" bucket for a row's badge -- kept
/// theme-agnostic here, same "tag the meaning, let the host pick the
/// color" split `docker_panel::DockerBadgeColor` already uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitBadgeColor {
    Good,
    Warn,
    Bad,
    Neutral,
}

/// Per-line metadata for one line of `GitPanel::text`, at the matching
/// index in `GitPanel::lines` -- mirrors `docker_panel::DockerLine`.
#[derive(Debug, Clone)]
pub struct GitLine {
    pub style: GitLineStyle,
    /// `Some` only for a `File`/`Branch`/`Commit`/`Stash` row -- what the
    /// action keys target when the cursor is on this line.
    pub entry: Option<GitEntry>,
    /// Char column where the dim portion of the line begins. `None` for
    /// every row -- unlike the Docker panel, every Git row's secondary
    /// info already lives inside its badge (git status letters, a short
    /// hash, a stash index), so there's no separate dim suffix to mark.
    /// Kept for shape-parity with `DockerLine` and in case a future row
    /// kind needs it.
    pub dim_from: Option<usize>,
    /// A row's `[X]` prefix: its char length (so the range `0..len` can
    /// be colored) and which color bucket to use. Every list row
    /// (File/Branch/Commit/Stash) has one -- unlike the Docker panel,
    /// where only Containers rows did -- since every Git list kind has
    /// *some* short, meaningful badge to lead with (a status code, a
    /// current-branch marker, a short hash, a stash index).
    pub badge: Option<(usize, GitBadgeColor)>,
}

/// The generated git panel: `text` is real content for a real
/// `fenix_core::Buffer` (via `BufferList::open_git`); `lines[i]`
/// describes `text`'s line `i`. Mirrors `docker_panel::DockerPanel`.
pub struct GitPanel {
    pub text: String,
    pub lines: Vec<Option<GitLine>>,
}

struct Builder {
    text: String,
    lines: Vec<Option<GitLine>>,
}

impl Builder {
    fn new() -> Self {
        Self { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, meta: Option<GitLine>) {
        self.text.push_str(text);
        self.text.push('\n');
        self.lines.push(meta);
    }

    fn finish(self) -> GitPanel {
        GitPanel { text: self.text, lines: self.lines }
    }
}

fn empty_line(message: &str) -> (String, Option<GitLine>) {
    (format!("    {message}"), Some(GitLine { style: GitLineStyle::Empty, entry: None, dim_from: None, badge: None }))
}

/// `[XY]` using git's own porcelain status letters verbatim (e.g. `.M`
/// unstaged-modified, `M.` staged-modified, `??` untracked, `UU`
/// conflict) -- colored by whichever half is "the interesting one": a
/// conflict always wins, else the staged (index) half if it's changed,
/// else the unstaged (worktree) half. Verified against git's own
/// documented porcelain v2 status-letter meanings (`M` modified, `A`
/// added, `D` deleted, `R` renamed, `C` copied, `T` type-changed, `U`
/// unmerged), not guessed.
fn file_status_badge(entry: &FileEntry) -> (String, GitBadgeColor) {
    let badge = format!("{}{}", entry.index_status, entry.worktree_status);
    if entry.index_status == 'U' || entry.worktree_status == 'U' {
        return (badge, GitBadgeColor::Bad);
    }
    let color = if entry.index_status != '.' { status_color(entry.index_status) } else { status_color(entry.worktree_status) };
    (badge, color)
}

fn status_color(c: char) -> GitBadgeColor {
    match c {
        'A' | 'R' | 'C' => GitBadgeColor::Good,
        'M' | 'T' => GitBadgeColor::Warn,
        'D' => GitBadgeColor::Bad,
        _ => GitBadgeColor::Neutral,
    }
}

/// A directory in the Files pane's tree, built from the flat list of
/// changed paths `git status` reports (there's no real directory
/// listing involved -- only directories that contain at least one
/// changed/untracked file ever appear).
#[derive(Default)]
struct DirNode {
    children: BTreeMap<String, DirNode>,
    files: Vec<FileEntry>,
}

/// Groups `files` by their path components into a directory tree --
/// `a/b/c.txt` walks/creates `a`, then `a/b`, then records `c.txt` as a
/// leaf of `a/b`. A file directly at the repo root (no `/` in its path)
/// is a leaf of the root node itself.
fn build_file_tree(files: &[FileEntry]) -> DirNode {
    let mut root = DirNode::default();
    for f in files {
        let segments: Vec<&str> = f.path.split('/').collect();
        let mut node = &mut root;
        for seg in &segments[..segments.len().saturating_sub(1)] {
            node = node.children.entry((*seg).to_string()).or_default();
        }
        node.files.push(f.clone());
    }
    root
}

/// One flattened row of the Files pane's tree -- `file` is `Some` only
/// for a file row (`is_dir` false).
struct FileTreeRow {
    depth: usize,
    is_dir: bool,
    path: String,
    name: String,
    file: Option<FileEntry>,
}

/// Depth-first flatten of `node`'s children into display rows,
/// directories-first-then-alphabetical (case-insensitive) at each level
/// -- mirrors `fenix_explorer`'s own directory-listing sort convention,
/// so the two trees this editor shows feel consistent. Descends into a
/// directory's own children only when its full path is in `expanded`,
/// so a collapsed directory renders as exactly one row no matter how
/// much it contains underneath.
fn flatten_file_tree(node: &DirNode, prefix: &str, depth: usize, expanded: &HashSet<String>, out: &mut Vec<FileTreeRow>) {
    let mut dir_names: Vec<&String> = node.children.keys().collect();
    dir_names.sort_by_key(|n| n.to_lowercase());
    for name in dir_names {
        let child = &node.children[name];
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        out.push(FileTreeRow { depth, is_dir: true, path: path.clone(), name: name.clone(), file: None });
        if expanded.contains(&path) {
            flatten_file_tree(child, &path, depth + 1, expanded, out);
        }
    }
    let mut files = node.files.clone();
    files.sort_by_key(|f| f.path.to_lowercase());
    for f in files {
        let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
        out.push(FileTreeRow { depth, is_dir: false, path: f.path.clone(), name, file: Some(f) });
    }
}

/// The Staged pane's own content -- every entry with a real index
/// (staged) half, same predicate `app.rs`'s `file_counts` helper already
/// establishes. A file that's both staged *and* further modified
/// (`MM`) appears here too (and in `render_unstaged`) -- `FileEntry`
/// already carries both halves independently, there's nothing to
/// reconcile.
pub fn render_staged(files: &[FileEntry], expanded_dirs: &HashSet<String>) -> GitPanel {
    let staged: Vec<FileEntry> = files.iter().filter(|f| f.index_status != '.' && f.index_status != '?').cloned().collect();
    render_file_tree(&staged, expanded_dirs, "Nothing staged")
}

/// The Unstaged pane's own content -- every entry with a real worktree
/// half, which covers both a plain unstaged modification *and* an
/// untracked file (synthesized `worktree_status: '?'`, also `!= '.'`).
pub fn render_unstaged(files: &[FileEntry], expanded_dirs: &HashSet<String>) -> GitPanel {
    let unstaged: Vec<FileEntry> = files.iter().filter(|f| f.worktree_status != '.').cloned().collect();
    render_file_tree(&unstaged, expanded_dirs, "Nothing to commit, working tree clean")
}

/// Shared by `render_staged`/`render_unstaged`: `files` grouped into a
/// collapsible directory tree (`Tab` toggles a directory under the
/// cursor, see `app.rs`'s `git_toggle_dir_expand`), each file row led
/// by its `[XY]` badge. `expanded_dirs` is the pane's own persisted set
/// of expanded directory paths -- a fresh/never-toggled directory starts
/// collapsed, showing just its name.
fn render_file_tree(files: &[FileEntry], expanded_dirs: &HashSet<String>, empty_message: &str) -> GitPanel {
    let mut b = Builder::new();
    if files.is_empty() {
        let (text, meta) = empty_line(empty_message);
        b.push(&text, meta);
    } else {
        let tree = build_file_tree(files);
        let mut rows = Vec::new();
        flatten_file_tree(&tree, "", 0, expanded_dirs, &mut rows);
        for row in rows {
            let margin = "  ".repeat(row.depth + 1);
            if row.is_dir {
                let marker = if expanded_dirs.contains(&row.path) { "v" } else { ">" };
                let line = format!("{margin}{marker} {}/", row.name);
                b.push(
                    &line,
                    Some(GitLine { style: GitLineStyle::Dir, entry: Some(GitEntry::Dir(row.path)), dim_from: None, badge: None }),
                );
            } else {
                let f = row.file.expect("file rows always carry a FileEntry");
                let (letters, color) = file_status_badge(&f);
                let prefix = format!("{margin}[{letters}] ");
                let badge_len = prefix.chars().count();
                let line = format!("{prefix}{}", row.name);
                b.push(
                    &line,
                    Some(GitLine {
                        style: GitLineStyle::File,
                        entry: Some(GitEntry::File(row.path)),
                        dim_from: None,
                        badge: Some((badge_len, color)),
                    }),
                );
            }
        }
    }
    b.finish()
}

/// The Branches pane's own content -- the checked-out branch marked
/// with a `*` badge, every other with a blank one; `+N`/`-M` after the
/// name when the branch is ahead/behind its upstream.
pub fn render_branches(branches: &[Branch]) -> GitPanel {
    let mut b = Builder::new();
    if branches.is_empty() {
        let (text, meta) = empty_line("No branches found");
        b.push(&text, meta);
    } else {
        for br in branches {
            let marker = if br.current { "*" } else { " " };
            let prefix = format!("  [{marker}] ");
            let badge_len = prefix.chars().count();
            let tracking = match (br.ahead, br.behind) {
                (0, 0) => String::new(),
                (a, 0) => format!("  +{a}"),
                (0, be) => format!("  -{be}"),
                (a, be) => format!("  +{a} -{be}"),
            };
            let line = format!("{prefix}{}{tracking}", br.name);
            let color = if br.current { GitBadgeColor::Good } else { GitBadgeColor::Neutral };
            b.push(
                &line,
                Some(GitLine {
                    style: GitLineStyle::Branch,
                    entry: Some(GitEntry::Branch(br.name.clone())),
                    dim_from: None,
                    badge: Some((badge_len, color)),
                }),
            );
        }
    }
    b.finish()
}

/// The Commits pane's own content -- most-recent-first (`git log`'s own
/// order, unchanged), each row led by its short hash as the badge.
pub fn render_commits(commits: &[Commit]) -> GitPanel {
    let mut b = Builder::new();
    if commits.is_empty() {
        let (text, meta) = empty_line("No commits yet");
        b.push(&text, meta);
    } else {
        for c in commits {
            let prefix = format!("  [{}] ", c.short_hash);
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{}", c.message);
            b.push(
                &line,
                Some(GitLine {
                    style: GitLineStyle::Commit,
                    entry: Some(GitEntry::Commit(c.hash.clone())),
                    dim_from: None,
                    badge: Some((badge_len, GitBadgeColor::Neutral)),
                }),
            );
        }
    }
    b.finish()
}

/// The Stash pane's own content -- each row led by its `stash@{N}`
/// index as the badge.
pub fn render_stash(stashes: &[Stash]) -> GitPanel {
    let mut b = Builder::new();
    if stashes.is_empty() {
        let (text, meta) = empty_line("No stash entries");
        b.push(&text, meta);
    } else {
        for s in stashes {
            let prefix = format!("  [{}] ", s.index);
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{}", s.message);
            b.push(
                &line,
                Some(GitLine {
                    style: GitLineStyle::Stash,
                    entry: Some(GitEntry::Stash(s.index)),
                    dim_from: None,
                    badge: Some((badge_len, GitBadgeColor::Neutral)),
                }),
            );
        }
    }
    b.finish()
}

/// The Status pane's own content: a fixed small repo-overview summary
/// (branch/upstream/ahead-behind, live-updated by the status poller),
/// not tied to whatever's selected elsewhere -- unlike the Docker
/// panel's own Status pane, which follows the cursor in the left
/// column. Real lazygit's own Status panel plays the same "always the
/// repo overview" role (see its `Status`-panel keybindings: switch
/// recent repo, cycle branch logs -- overview actions, not per-item
/// detail), which is what this mirrors.
pub fn render_status(status: Option<&RepoStatus>, staged: usize, unstaged: usize, untracked: usize) -> GitPanel {
    let mut b = Builder::new();
    match status {
        None => {
            let (text, meta) = empty_line("Not a git repository");
            b.push(&text, meta);
        }
        Some(s) => {
            push_detail_line(&mut b, "Branch", &s.branch);
            if let Some(upstream) = &s.upstream {
                push_detail_line(&mut b, "Upstream", upstream);
                push_detail_line(&mut b, "Ahead/Behind", &format!("+{} -{}", s.ahead, s.behind));
            }
            push_detail_line(&mut b, "Staged", &staged.to_string());
            push_detail_line(&mut b, "Unstaged", &unstaged.to_string());
            push_detail_line(&mut b, "Untracked", &untracked.to_string());
        }
    }
    b.finish()
}

/// Word-wraps `value` to `crate::wrap::DEFAULT_WRAP_WIDTH`, indenting
/// continuation lines under the value's own start column -- see
/// `docker_panel::push_detail_line`'s own doc comment, which this
/// mirrors exactly (down to the `dim_from` handling: `GitLine::dim_from`
/// plays the identical "everything from here on is the dimmed value"
/// role `DockerLine::dim_from` does).
fn push_detail_line(b: &mut Builder, label: &str, value: &str) {
    let prefix = format!("    {label}: ");
    let dim_from = prefix.chars().count();
    let indent = " ".repeat(dim_from);
    let wrap_width = crate::wrap::DEFAULT_WRAP_WIDTH.saturating_sub(dim_from).max(20);
    let mut wrapped = crate::wrap::wrap_text(value, wrap_width).into_iter();
    let first = wrapped.next().unwrap_or_default();
    b.push(&format!("{prefix}{first}"), Some(GitLine { style: GitLineStyle::Detail, entry: None, dim_from: Some(dim_from), badge: None }));
    for continuation in wrapped {
        b.push(&format!("{indent}{continuation}"), Some(GitLine { style: GitLineStyle::Detail, entry: None, dim_from: Some(0), badge: None }));
    }
}

/// The Compare view's commit list: which comparison is being shown,
/// then one row per commit `head` has that `base` doesn't. Reuses the
/// Commits pane's own row shape (`[short hash] subject`) so the two
/// listings read identically.
pub fn render_compare(range: &str, commits: &[Commit], truncated: bool) -> GitPanel {
    let mut b = Builder::new();
    // The count belongs in the header: without it a truncated list looks
    // like the whole answer, and "how far apart are these two" is half
    // the reason to run a comparison at all.
    let count = if truncated { format!("{}+ commits", commits.len()) } else { format!("{} commits", commits.len()) };
    b.push(
        &format!("  {range}   ({count})"),
        Some(GitLine { style: GitLineStyle::Header, entry: None, dim_from: None, badge: None }),
    );
    if commits.is_empty() {
        let (text, meta) = empty_line("No commits between these refs");
        b.push(&text, meta);
        return b.finish();
    }
    for commit in commits {
        let prefix = format!("  [{}] ", commit.short_hash);
        let badge_len = prefix.chars().count();
        b.push(
            &format!("{prefix}{}", commit.message),
            Some(GitLine {
                style: GitLineStyle::Commit,
                entry: Some(GitEntry::Commit(commit.hash.clone())),
                dim_from: None,
                badge: Some((badge_len, GitBadgeColor::Neutral)),
            }),
        );
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, index: char, worktree: char) -> FileEntry {
        FileEntry { path: path.to_string(), index_status: index, worktree_status: worktree }
    }

    fn branch(name: &str, current: bool) -> Branch {
        Branch { name: name.to_string(), current, upstream: None, ahead: 0, behind: 0, upstream_gone: false }
    }

    fn commit(hash: &str, short: &str, message: &str) -> Commit {
        Commit { hash: hash.to_string(), short_hash: short.to_string(), message: message.to_string(), author: "Test".to_string(), relative_date: "1 hour ago".to_string() }
    }

    fn stash(index: usize, message: &str) -> Stash {
        Stash { index, message: message.to_string() }
    }

    #[test]
    fn render_unstaged_lists_entries_with_the_right_entry() {
        let panel = render_unstaged(&[file("a.txt", '.', 'M'), file("b.txt", '?', '?')], &HashSet::new());
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::File).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, Some(GitEntry::File("a.txt".to_string())));
    }

    #[test]
    fn render_staged_lists_entries_with_the_right_entry() {
        let panel = render_staged(&[file("a.txt", 'M', '.'), file("b.txt", 'A', '.')], &HashSet::new());
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::File).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, Some(GitEntry::File("a.txt".to_string())));
    }

    #[test]
    fn render_unstaged_excludes_a_staged_only_file() {
        let panel = render_unstaged(&[file("a.txt", 'A', '.')], &HashSet::new());
        assert!(panel.text.contains("clean"));
    }

    #[test]
    fn render_staged_excludes_an_unstaged_only_file() {
        let panel = render_staged(&[file("a.txt", '.', 'M')], &HashSet::new());
        assert!(panel.text.contains("Nothing staged"));
    }

    #[test]
    fn a_partially_staged_file_appears_in_both_panes() {
        let files = [file("a.txt", 'M', 'M')]; // staged AND further modified
        let staged = render_staged(&files, &HashSet::new());
        let unstaged = render_unstaged(&files, &HashSet::new());
        assert_eq!(staged.lines.iter().flatten().filter(|l| l.style == GitLineStyle::File).count(), 1);
        assert_eq!(unstaged.lines.iter().flatten().filter(|l| l.style == GitLineStyle::File).count(), 1);
    }

    #[test]
    fn render_unstaged_lines_stay_the_same_length_as_text() {
        let panel = render_unstaged(&[file("a.txt", '.', 'M')], &HashSet::new());
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn render_unstaged_empty_list_shows_a_placeholder() {
        let panel = render_unstaged(&[], &HashSet::new());
        assert!(panel.text.contains("clean"));
    }

    #[test]
    fn render_staged_empty_list_shows_a_placeholder() {
        let panel = render_staged(&[], &HashSet::new());
        assert!(panel.text.contains("Nothing staged"));
    }

    #[test]
    fn render_unstaged_shows_the_raw_status_letters_as_the_badge() {
        let panel = render_unstaged(&[file("a.txt", '.', 'M')], &HashSet::new());
        assert!(panel.text.contains("[.M] a.txt"));
    }

    #[test]
    fn render_unstaged_groups_a_subdirectorys_files_under_one_collapsed_row() {
        let panel = render_unstaged(&[file("src/a.txt", '.', 'M'), file("src/b.txt", '?', '?')], &HashSet::new());
        let lines: Vec<_> = panel.lines.iter().flatten().collect();
        // Collapsed: just the one directory row, no file rows underneath.
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].style, GitLineStyle::Dir);
        assert_eq!(lines[0].entry, Some(GitEntry::Dir("src".to_string())));
        assert!(panel.text.contains("> src/"));
    }

    #[test]
    fn render_unstaged_expanding_a_directory_reveals_its_files_indented_and_by_basename() {
        let mut expanded = HashSet::new();
        expanded.insert("src".to_string());
        let panel = render_unstaged(&[file("src/a.txt", '.', 'M'), file("root.txt", '.', 'D')], &expanded);

        let lines: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(lines.len(), 3); // src/ dir row + a.txt + root.txt

        let dir_row = lines.iter().find(|l| l.style == GitLineStyle::Dir).unwrap();
        assert_eq!(dir_row.entry, Some(GitEntry::Dir("src".to_string())));

        let file_row = lines.iter().find(|l| l.entry == Some(GitEntry::File("src/a.txt".to_string()))).unwrap();
        assert_eq!(file_row.style, GitLineStyle::File);

        assert!(panel.text.contains("v src/"));
        // The file row shows only its basename, indented deeper than
        // its parent directory row, not the full "src/a.txt" path.
        assert!(panel.text.contains("    [.M] a.txt"));
        assert!(!panel.text.contains("src/a.txt"));
        // A root-level file (no directory) renders exactly as before.
        assert!(panel.text.contains("  [.D] root.txt"));
    }

    #[test]
    fn render_unstaged_nested_subdirectories_only_expand_one_level_at_a_time() {
        let mut expanded = HashSet::new();
        expanded.insert("src".to_string());
        // "src/nested" itself is not in `expanded`, so its file stays
        // collapsed behind its own directory row even though "src" is.
        let panel = render_unstaged(&[file("src/nested/deep.txt", '.', 'M')], &expanded);
        let lines: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(lines.len(), 2); // "src/" row, "src/nested/" row -- no file row yet
        assert!(lines.iter().all(|l| l.style == GitLineStyle::Dir));
        assert_eq!(lines[1].entry, Some(GitEntry::Dir("src/nested".to_string())));
    }

    #[test]
    fn render_unstaged_directories_sort_before_files_alphabetically_within_each_level() {
        let panel = render_unstaged(&[file("z.txt", '.', 'M'), file("a_dir/x.txt", '.', 'M')], &HashSet::new());
        // "a_dir/" (a directory) sorts before "z.txt" despite the
        // alphabetically-later name, matching `fenix_explorer`'s own
        // directories-first convention.
        let first_line = panel.text.lines().next().unwrap();
        assert!(first_line.contains("a_dir/"), "expected the directory row first, got: {first_line:?}");
    }

    #[test]
    fn file_status_badge_prioritizes_staged_over_unstaged_for_color() {
        let (letters, color) = file_status_badge(&file("a.txt", 'M', 'D'));
        assert_eq!(letters, "MD");
        assert_eq!(color, GitBadgeColor::Warn); // staged 'M' wins over unstaged 'D'
    }

    #[test]
    fn file_status_badge_falls_back_to_unstaged_when_index_is_unchanged() {
        let (_, color) = file_status_badge(&file("a.txt", '.', 'D'));
        assert_eq!(color, GitBadgeColor::Bad);
    }

    #[test]
    fn file_status_badge_marks_untracked_neutral() {
        let (letters, color) = file_status_badge(&file("a.txt", '?', '?'));
        assert_eq!(letters, "??");
        assert_eq!(color, GitBadgeColor::Neutral);
    }

    #[test]
    fn file_status_badge_marks_a_conflict_bad_regardless_of_which_side() {
        assert_eq!(file_status_badge(&file("a.txt", 'U', 'U')).1, GitBadgeColor::Bad);
        assert_eq!(file_status_badge(&file("a.txt", 'A', 'U')).1, GitBadgeColor::Bad);
    }

    #[test]
    fn render_branches_marks_the_current_branch() {
        let panel = render_branches(&[branch("main", true), branch("feature", false)]);
        assert!(panel.text.contains("[*] main"));
        assert!(panel.text.contains("[ ] feature"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].badge.map(|(_, c)| c), Some(GitBadgeColor::Good));
        assert_eq!(entries[1].badge.map(|(_, c)| c), Some(GitBadgeColor::Neutral));
    }

    #[test]
    fn render_branches_shows_ahead_and_behind_counts() {
        let mut b = branch("main", true);
        b.ahead = 2;
        b.behind = 1;
        let panel = render_branches(&[b]);
        assert!(panel.text.contains("+2 -1"));
    }

    #[test]
    fn render_branches_empty_list_shows_a_placeholder() {
        assert!(render_branches(&[]).text.contains("No branches found"));
    }

    #[test]
    fn render_commits_lists_entries_with_the_right_entry() {
        let panel = render_commits(&[commit("abc123full", "abc123", "Fix the thing")]);
        assert!(panel.text.contains("[abc123] Fix the thing"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].entry, Some(GitEntry::Commit("abc123full".to_string())));
    }

    #[test]
    fn render_commits_empty_list_shows_a_placeholder() {
        assert!(render_commits(&[]).text.contains("No commits yet"));
    }

    #[test]
    fn render_stash_lists_entries_with_the_right_entry() {
        let panel = render_stash(&[stash(0, "WIP on main: abc first")]);
        assert!(panel.text.contains("[0] WIP on main: abc first"));
        let entries: Vec<_> = panel.lines.iter().flatten().collect();
        assert_eq!(entries[0].entry, Some(GitEntry::Stash(0)));
    }

    #[test]
    fn render_stash_empty_list_shows_a_placeholder() {
        assert!(render_stash(&[]).text.contains("No stash entries"));
    }

    #[test]
    fn render_compare_heads_the_list_with_the_range_and_lists_each_commit() {
        let panel = render_compare("main...side", &[commit("abcdef123", "abcdef1", "on side")], false);
        assert!(panel.text.starts_with("  main...side"));
        assert!(panel.text.contains("(1 commits)"), "the header carries the count:
{}", panel.text);
        assert!(panel.text.contains("[abcdef1] on side"));
        let entries: Vec<_> = panel.lines.iter().flatten().filter_map(|l| l.entry.as_ref()).collect();
        assert_eq!(entries[0], &GitEntry::Commit("abcdef123".to_string()));
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn render_compare_with_nothing_between_the_refs_says_so() {
        let panel = render_compare("main...main", &[], false);
        assert!(panel.text.contains("No commits between these refs"));
        assert!(panel.text.contains("(0 commits)"));
    }

    #[test]
    fn render_compare_marks_a_truncated_list_so_it_is_not_read_as_the_whole_answer() {
        let commits: Vec<Commit> = (0..3).map(|i| commit(&format!("hash{i}"), &format!("h{i}"), "work")).collect();
        let panel = render_compare("main...side", &commits, true);
        assert!(panel.text.contains("(3+ commits)"), "got:
{}", panel.text);
    }

    #[test]
    fn render_status_none_shows_not_a_repo() {
        assert!(render_status(None, 0, 0, 0).text.contains("Not a git repository"));
    }

    #[test]
    fn render_status_shows_branch_and_file_counts() {
        let s = RepoStatus { branch: "main".to_string(), upstream: None, ahead: 0, behind: 0 };
        let panel = render_status(Some(&s), 2, 1, 3);
        assert!(panel.text.contains("Branch: main"));
        assert!(panel.text.contains("Staged: 2"));
        assert!(panel.text.contains("Unstaged: 1"));
        assert!(panel.text.contains("Untracked: 3"));
        assert!(!panel.text.contains("Upstream:"));
    }

    #[test]
    fn render_status_shows_upstream_and_ahead_behind_when_tracking() {
        let s = RepoStatus { branch: "main".to_string(), upstream: Some("origin/main".to_string()), ahead: 2, behind: 1 };
        let panel = render_status(Some(&s), 0, 0, 0);
        assert!(panel.text.contains("Upstream: origin/main"));
        assert!(panel.text.contains("Ahead/Behind: +2 -1"));
    }

    #[test]
    fn a_long_status_detail_value_word_wraps_onto_continuation_lines() {
        let long_branch = format!("feature/{}", "a-very-descriptive-segment-".repeat(6));
        let s = RepoStatus { branch: long_branch.clone(), upstream: None, ahead: 0, behind: 0 };
        let panel = render_status(Some(&s), 0, 0, 0);
        let detail_rows = panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::Detail).count();
        assert!(detail_rows > 4, "expected the long branch name to wrap onto more than one line, got {detail_rows} detail rows:\n{}", panel.text);
        assert_eq!(panel.text.lines().count(), panel.lines.len(), "text and lines must stay in lockstep after wrapping");
    }

    #[test]
    fn a_wrapped_status_continuation_line_is_dimmed_in_full() {
        let long_branch = format!("feature/{}", "a-very-descriptive-segment-".repeat(6));
        let s = RepoStatus { branch: long_branch, upstream: None, ahead: 0, behind: 0 };
        let panel = render_status(Some(&s), 0, 0, 0);
        let detail_lines: Vec<&GitLine> = panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::Detail).collect();
        assert!(detail_lines.iter().any(|l| l.dim_from == Some(0)), "expected a continuation line dimmed from column 0");
    }

    #[test]
    fn a_short_status_detail_value_never_wraps() {
        let s = RepoStatus { branch: "main".to_string(), upstream: None, ahead: 0, behind: 0 };
        let panel = render_status(Some(&s), 0, 0, 0);
        assert!(!panel.text.contains("\nmain\n"), "a short value must not spill onto its own continuation line");
    }





}
