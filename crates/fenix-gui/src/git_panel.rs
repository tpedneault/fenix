use fenix_git::{Branch, Commit, FileEntry, RepoStatus, Stash};

/// What a git-panel line represents -- what the per-pane action keys
/// (see `app.rs`'s Git action routing) act on when the cursor is on
/// this line. Mirrors `docker_panel::DockerEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitEntry {
    File(String),
    Branch(String),
    Commit(String),
    Stash(usize),
}

/// How one generated line should be colored -- same role as
/// `docker_panel::DockerLineStyle`, extended with the three diff-line
/// kinds `render_main` needs (a unified diff's own `+`/`-`/`@@`
/// convention, colored the same way every other diff viewer does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitLineStyle {
    File,
    Branch,
    Commit,
    Stash,
    /// A `label: value` row in the Status pane.
    Detail,
    /// An added line in a unified diff (`+...`, not the `+++` file
    /// header).
    DiffAdd,
    /// A removed line in a unified diff (`-...`, not the `---` file
    /// header).
    DiffDel,
    /// A hunk header (`@@ ... @@`).
    DiffHunk,
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

/// The Files pane's own content -- one row per changed/untracked path,
/// in the same order `git status` itself already reports them (changed
/// paths first, untracked last), each led by its `[XY]` badge.
pub fn render_files(files: &[FileEntry]) -> GitPanel {
    let mut b = Builder::new();
    if files.is_empty() {
        let (text, meta) = empty_line("Nothing to commit, working tree clean");
        b.push(&text, meta);
    } else {
        for f in files {
            let (letters, color) = file_status_badge(f);
            let prefix = format!("  [{letters}] ");
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{}", f.path);
            b.push(
                &line,
                Some(GitLine {
                    style: GitLineStyle::File,
                    entry: Some(GitEntry::File(f.path.clone())),
                    dim_from: None,
                    badge: Some((badge_len, color)),
                }),
            );
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

fn push_detail_line(b: &mut Builder, label: &str, value: &str) {
    let prefix = format!("    {label}: ");
    let dim_from = prefix.chars().count();
    b.push(&format!("{prefix}{value}"), Some(GitLine { style: GitLineStyle::Detail, entry: None, dim_from: Some(dim_from), badge: None }));
}

/// The Main pane's own content: whatever diff/detail text is currently
/// selected in Files/Commits/Stash (`None` while nothing's been
/// navigated to yet, or the selection has no diff to show, e.g. an
/// untracked file -- see `fenix_git::diff::file_diff`'s own doc comment
/// for why that case is handled by the caller, not `fenix-git` itself).
/// Colors unified-diff `+`/`-`/`@@` lines the same way every other diff
/// viewer does; every other line (file headers, context lines) renders
/// in the pane's plain default color.
pub fn render_main(diff: Option<&str>) -> GitPanel {
    let mut b = Builder::new();
    match diff {
        None => {
            let (text, meta) = empty_line("Nothing selected");
            b.push(&text, meta);
        }
        Some("") => {
            let (text, meta) = empty_line("(no changes)");
            b.push(&text, meta);
        }
        Some(text) => {
            for line in text.lines() {
                let style = if line.starts_with("+++") || line.starts_with("---") {
                    GitLineStyle::Empty
                } else if line.starts_with('+') {
                    GitLineStyle::DiffAdd
                } else if line.starts_with('-') {
                    GitLineStyle::DiffDel
                } else if line.starts_with("@@") {
                    GitLineStyle::DiffHunk
                } else {
                    GitLineStyle::Empty
                };
                b.push(line, Some(GitLine { style, entry: None, dim_from: None, badge: None }));
            }
        }
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
        Branch { name: name.to_string(), current, upstream: None, ahead: 0, behind: 0 }
    }

    fn commit(hash: &str, short: &str, message: &str) -> Commit {
        Commit { hash: hash.to_string(), short_hash: short.to_string(), message: message.to_string(), author: "Test".to_string(), relative_date: "1 hour ago".to_string() }
    }

    fn stash(index: usize, message: &str) -> Stash {
        Stash { index, message: message.to_string() }
    }

    #[test]
    fn render_files_lists_entries_with_the_right_entry() {
        let panel = render_files(&[file("a.txt", '.', 'M'), file("b.txt", 'A', '.')]);
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == GitLineStyle::File).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, Some(GitEntry::File("a.txt".to_string())));
    }

    #[test]
    fn render_files_lines_stay_the_same_length_as_text() {
        let panel = render_files(&[file("a.txt", '.', 'M')]);
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn render_files_empty_list_shows_a_placeholder() {
        let panel = render_files(&[]);
        assert!(panel.text.contains("clean"));
    }

    #[test]
    fn render_files_shows_the_raw_status_letters_as_the_badge() {
        let panel = render_files(&[file("a.txt", '.', 'M')]);
        assert!(panel.text.contains("[.M] a.txt"));
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
    fn render_main_none_shows_nothing_selected() {
        assert!(render_main(None).text.contains("Nothing selected"));
    }

    #[test]
    fn render_main_empty_diff_shows_no_changes() {
        assert!(render_main(Some("")).text.contains("no changes"));
    }

    #[test]
    fn render_main_colors_added_and_removed_lines() {
        let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n context\n";
        let panel = render_main(Some(diff));
        let styles: Vec<_> = panel.lines.iter().flatten().map(|l| l.style).collect();
        assert!(styles.contains(&GitLineStyle::DiffHunk));
        assert!(styles.contains(&GitLineStyle::DiffAdd));
        assert!(styles.contains(&GitLineStyle::DiffDel));
    }

    #[test]
    fn render_main_does_not_color_the_file_header_lines_as_diff_lines() {
        let diff = "--- a/f\n+++ b/f\n";
        let panel = render_main(Some(diff));
        let styles: Vec<_> = panel.lines.iter().flatten().map(|l| l.style).collect();
        assert!(!styles.contains(&GitLineStyle::DiffAdd));
        assert!(!styles.contains(&GitLineStyle::DiffDel));
    }

    #[test]
    fn render_main_lines_stay_the_same_length_as_text() {
        let panel = render_main(Some("a\nb\nc"));
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }
}
