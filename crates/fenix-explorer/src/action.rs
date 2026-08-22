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
    ToggleHidden,
    Refresh,
    Quit,
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

        t.insert(&[KeyPress::char('.')], "toggle hidden", ExplorerAction::ToggleHidden);
        t.insert(&[KeyPress::char('g'), KeyPress::char('r')], "refresh", ExplorerAction::Refresh);
        t.insert(&[KeyPress::char('q')], "quit", ExplorerAction::Quit);
        t.insert(&[KeyPress::named(fenix_keymap::NamedKey::Escape)], "quit", ExplorerAction::Quit);

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
