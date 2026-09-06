use std::sync::OnceLock;

use fenix_keymap::{KeyPress, KeyTrie};

/// A single explorer command. Distinct from `fenix_vim::VimAction` on
/// purpose -- browsing a directory listing isn't text editing, so this
/// doesn't route through `fenix-vim` at all, the same reasoning that
/// already keeps Insert/Command mode out of Normal's trie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerAction {
    Down,
    Up,
    /// Open the entry at point: visit a file, or navigate into a
    /// directory (replacing the listing with that directory's).
    Open,
    /// Navigate to the parent of `cwd`.
    ParentDir,
    /// Expand/collapse the directory at point in place, without leaving
    /// the current listing.
    ToggleExpand,
    ToggleMark,
    MarkAll,
    UnmarkAll,
    ToggleAllMarks,
    /// Each `Begin*` starts a confirmation or text-input prompt --
    /// handled by the host (it owns the actual prompt UI), not performed
    /// immediately.
    BeginDelete,
    BeginRename,
    BeginCreateFile,
    BeginCreateDir,
    BeginCopy,
    BeginMove,
    /// Narrow the listing to names containing what you type -- the
    /// answer to a folder with four hundred things in it.
    BeginFilter,
    /// Search this directory and everything under it, by name.
    BeginFind,
    /// Pack the marked set (or the entry at point) into an archive.
    BeginArchive,
    /// Unpack the archive at point.
    ExtractArchive,
    /// Everything about the entry at point that does not fit in a
    /// column -- and, for a directory, what it actually contains.
    ShowProperties,
    /// Flip the read-only attribute: the one that stops an ordinary
    /// save, and the usual reason for opening Windows' own properties
    /// dialog.
    ToggleReadOnly,
    ToggleHidden,
    /// Cycle what the listing is ordered by; `ReverseSort` flips the
    /// direction of whatever that currently is. Two keys rather than
    /// one cycle through eight states, which nobody can navigate.
    CycleSort,
    ReverseSort,
    Refresh,
    Quit,
    /// Confirm the directory currently being browsed (`cwd`, not the
    /// entry at point) as the thing this listing was opened to pick --
    /// meaningless during ordinary browsing (`SPC f j`/the sidebar), only
    /// acted on by a host that opened the explorer in a directory-picking
    /// mode (e.g. Fenix's `SPC p a`). Deliberately a different key from
    /// `Open`/`l`/`Enter`, which keeps navigating *into* directories --
    /// mirrors a standard "choose folder" dialog, where you browse deeper
    /// with one action and confirm the folder you're currently in with a
    /// separate one.
    SelectCwd,
}

impl ExplorerAction {
    /// Whether this action is about *moving around* the listing.
    ///
    /// The sidebar has no cursor of its own, so it needs these. The
    /// buffer-backed listing is real text with a real Vim cursor, and
    /// claiming `j`/`k` there would take two of the most-used motions in
    /// the editor away from the editor -- so it claims only the
    /// operations, and lets Vim move. One table, two readings of it,
    /// rather than two tables that drift.
    pub fn is_navigation(self) -> bool {
        matches!(self, ExplorerAction::Down | ExplorerAction::Up)
    }
}

/// Bindings chosen to match evil-collection's real dired keymap (what
/// Doom's dirvish module actually layers on top of), not a new scheme.
pub fn explorer_trie() -> &'static KeyTrie<ExplorerAction> {
    static TRIE: OnceLock<KeyTrie<ExplorerAction>> = OnceLock::new();
    TRIE.get_or_init(|| {
        let mut t = KeyTrie::new();
        t.insert(&[KeyPress::char('j')], "down", ExplorerAction::Down);
        t.insert(&[KeyPress::char('k')], "up", ExplorerAction::Up);
        t.insert(&[KeyPress::named(fenix_keymap::NamedKey::Enter)], "open", ExplorerAction::Open);
        t.insert(&[KeyPress::char('l')], "open", ExplorerAction::Open);
        t.insert(&[KeyPress::char('h')], "parent dir", ExplorerAction::ParentDir);
        t.insert(&[KeyPress::char('-')], "parent dir", ExplorerAction::ParentDir);
        t.insert(&[KeyPress::named(fenix_keymap::NamedKey::Tab)], "expand/collapse", ExplorerAction::ToggleExpand);

        t.insert(&[KeyPress::char('m')], "mark", ExplorerAction::ToggleMark);
        t.insert(&[KeyPress::char('u')], "unmark", ExplorerAction::ToggleMark);
        t.insert(&[KeyPress::char('U')], "unmark all", ExplorerAction::UnmarkAll);
        t.insert(&[KeyPress::char('t')], "toggle all marks", ExplorerAction::ToggleAllMarks);

        t.insert(&[KeyPress::char('D')], "delete", ExplorerAction::BeginDelete);
        t.insert(&[KeyPress::char('R')], "rename", ExplorerAction::BeginRename);
        t.insert(&[KeyPress::char('c')], "create file", ExplorerAction::BeginCreateFile);
        t.insert(&[KeyPress::char('+')], "create dir", ExplorerAction::BeginCreateDir);
        t.insert(&[KeyPress::char('C')], "copy to...", ExplorerAction::BeginCopy);
        t.insert(&[KeyPress::char('M')], "move to...", ExplorerAction::BeginMove);

        t.insert(&[KeyPress::char('z')], "archive...", ExplorerAction::BeginArchive);
        t.insert(&[KeyPress::char('x')], "extract", ExplorerAction::ExtractArchive);
        t.insert(&[KeyPress::char('i')], "properties", ExplorerAction::ShowProperties);
        t.insert(&[KeyPress::char('w')], "toggle read-only", ExplorerAction::ToggleReadOnly);
        t.insert(&[KeyPress::char('f')], "filter", ExplorerAction::BeginFilter);
        t.insert(&[KeyPress::char('F')], "find under here", ExplorerAction::BeginFind);
        t.insert(&[KeyPress::char('.')], "toggle hidden", ExplorerAction::ToggleHidden);
        t.insert(&[KeyPress::char('o')], "sort by...", ExplorerAction::CycleSort);
        t.insert(&[KeyPress::char('O')], "reverse sort", ExplorerAction::ReverseSort);
        t.insert(&[KeyPress::char('g'), KeyPress::char('r')], "refresh", ExplorerAction::Refresh);
        // `r` as well as `gr`. The buffer-backed listing claims single
        // keys only (a `g` prefix there would have to fight `gg`/`G`,
        // which are genuinely useful in a long directory), and the two
        // forms must not disagree about what a key does -- so the alias
        // lives in the one table both of them read.
        t.insert(&[KeyPress::char('r')], "refresh", ExplorerAction::Refresh);
        t.insert(&[KeyPress::char('q')], "quit", ExplorerAction::Quit);
        t.insert(&[KeyPress::named(fenix_keymap::NamedKey::Escape)], "quit", ExplorerAction::Quit);
        t.insert(&[KeyPress::char('S')], "select this directory", ExplorerAction::SelectCwd);

        t
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_keymap::Step;

    #[test]
    fn down_and_up_resolve_immediately() {
        let trie = explorer_trie();
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('j')) {
            Step::Matched(ExplorerAction::Down) => {}
            _ => panic!("expected Down"),
        }
    }

    #[test]
    fn u_unmarks_and_shift_u_unmarks_all_are_distinct() {
        let trie = explorer_trie();
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('u')) {
            Step::Matched(ExplorerAction::ToggleMark) => {}
            _ => panic!("expected ToggleMark for 'u'"),
        }
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('U')) {
            Step::Matched(ExplorerAction::UnmarkAll) => {}
            _ => panic!("expected UnmarkAll for 'U'"),
        }
    }

    #[test]
    fn g_r_refreshes() {
        let trie = explorer_trie();
        let mut m = trie.matcher();
        assert!(matches!(m.feed(KeyPress::char('g')), Step::Pending(_)));
        match m.feed(KeyPress::char('r')) {
            Step::Matched(ExplorerAction::Refresh) => {}
            _ => panic!("expected Refresh"),
        }
    }

    #[test]
    fn shift_s_selects_the_current_directory() {
        let trie = explorer_trie();
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('S')) {
            Step::Matched(ExplorerAction::SelectCwd) => {}
            _ => panic!("expected SelectCwd"),
        }
    }

    #[test]
    fn refresh_is_reachable_as_both_r_and_gr() {
        // The buffer-backed listing can only claim single keys, and the
        // two forms must agree about what a key does.
        let trie = explorer_trie();
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('r')) {
            Step::Matched(ExplorerAction::Refresh) => {}
            _ => panic!("expected Refresh for 'r'"),
        }
    }

    #[test]
    fn sorting_is_two_keys_rather_than_one_long_cycle() {
        let trie = explorer_trie();
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('o')) {
            Step::Matched(ExplorerAction::CycleSort) => {}
            _ => panic!("expected CycleSort"),
        }
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('O')) {
            Step::Matched(ExplorerAction::ReverseSort) => {}
            _ => panic!("expected ReverseSort"),
        }
    }

    #[test]
    fn only_the_cursor_actions_count_as_navigation() {
        // What the buffer-backed listing leaves to Vim.
        assert!(ExplorerAction::Down.is_navigation());
        assert!(ExplorerAction::Up.is_navigation());
        assert!(!ExplorerAction::Open.is_navigation());
        assert!(!ExplorerAction::BeginDelete.is_navigation());
        assert!(!ExplorerAction::ToggleExpand.is_navigation());
    }

    #[test]
    fn q_and_escape_both_quit() {
        let trie = explorer_trie();
        for key in [KeyPress::char('q'), KeyPress::named(fenix_keymap::NamedKey::Escape)] {
            let mut m = trie.matcher();
            match m.feed(key) {
                Step::Matched(ExplorerAction::Quit) => {}
                _ => panic!("expected Quit"),
            }
        }
    }
}
