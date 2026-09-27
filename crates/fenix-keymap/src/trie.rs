use std::collections::HashMap;

use crate::key::KeyPress;

struct Node<A> {
    leaf: Option<A>,
    label: Option<&'static str>,
    /// Where the which-key menu lists it within its group: "View",
    /// "Branch & remote"... `None` lists it with the rest.
    section: Option<&'static str>,
    children: HashMap<KeyPress, Node<A>>,
}

impl<A> Node<A> {
    fn new() -> Self {
        Self { leaf: None, label: None, section: None, children: HashMap::new() }
    }

    /// Actions reachable under this node.
    fn leaves(&self) -> usize {
        self.children.values().map(|c| usize::from(c.leaf.is_some()) + c.leaves()).sum()
    }
}

/// One key the which-key menu offers from where a sequence has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    pub key: KeyPress,
    pub label: &'static str,
    /// It opens more keys rather than running something.
    pub group: bool,
    /// For a group, how many commands are under it.
    pub count: usize,
    pub section: Option<&'static str>,
}

/// A trie over key sequences. Used both for Vim's multi-key commands
/// (`dd`, `ciw`) and for the `SPC`-leader menu (`SPC f s`) — same
/// underlying problem (match a key sequence, act on completion, report
/// available continuations), one engine.
///
/// If a node has both a leaf action and children (shouldn't happen for any
/// binding set actually in use, but the type doesn't forbid it), the leaf
/// wins immediately: there's no timeout-based disambiguation here, so a key
/// is either a complete action or a prefix, never ambiguously both.
pub struct KeyTrie<A> {
    root: Node<A>,
}

impl<A> Default for KeyTrie<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A> KeyTrie<A> {
    pub fn new() -> Self {
        Self { root: Node::new() }
    }

    /// Binds `seq` to a leaf action. `label` describes this action, shown
    /// in the which-key popup alongside its key.
    pub fn insert(&mut self, seq: &[KeyPress], label: &'static str, action: A) {
        let mut node = &mut self.root;
        for key in seq {
            node = node.children.entry(*key).or_insert_with(Node::new);
        }
        node.leaf = Some(action);
        node.label = Some(label);
    }

    /// Labels an intermediate node (a "group", e.g. `SPC f` = "files")
    /// without giving it a leaf action of its own.
    pub fn label_group(&mut self, seq: &[KeyPress], label: &'static str) {
        let mut node = &mut self.root;
        for key in seq {
            node = node.children.entry(*key).or_insert_with(Node::new);
        }
        node.label = Some(label);
    }

    /// Files the binding (or group) at `seq` under a section of its
    /// group's which-key menu.
    pub fn set_section(&mut self, seq: &[KeyPress], section: &'static str) {
        let mut node = &mut self.root;
        for key in seq {
            node = node.children.entry(*key).or_insert_with(Node::new);
        }
        node.section = Some(section);
    }

    pub fn matcher(&self) -> Matcher<'_, A> {
        Matcher { trie: self, current: None, path: Vec::new() }
    }
}

/// The result of feeding one keypress into a `Matcher`.
pub enum Step<'a, A> {
    /// The sequence resolved to a leaf action.
    Matched(&'a A),
    /// Valid so far but incomplete; here are the possible next keys.
    Pending(Vec<(KeyPress, &'static str)>),
    /// Not a valid continuation from here. The matcher has reset to the root.
    NoMatch,
}

/// Tracks progress through a `KeyTrie` as keys arrive one at a time.
pub struct Matcher<'a, A> {
    trie: &'a KeyTrie<A>,
    current: Option<&'a Node<A>>,
    /// The keys fed since the root, while a sequence is pending.
    path: Vec<KeyPress>,
}

impl<'a, A> Matcher<'a, A> {
    /// The label of the group the pending sequence is in: "git" after
    /// `SPC g`.
    pub fn label(&self) -> Option<&'static str> {
        self.current.and_then(|n| n.label)
    }

    /// The keys pressed so far in the pending sequence.
    pub fn path(&self) -> &[KeyPress] {
        &self.path
    }

    /// Every key on offer from here, with what the which-key menu needs
    /// to draw it.
    pub fn pending_hints(&self) -> Vec<Hint> {
        let node = self.current.unwrap_or(&self.trie.root);
        node.children
            .iter()
            .map(|(k, n)| {
                let group = n.leaf.is_none() && !n.children.is_empty();
                Hint { key: *k, label: n.label.unwrap_or(""), group, count: if group { n.leaves() } else { 0 }, section: n.section }
            })
            .collect()
    }

    /// Goes back one key: `SPC g` becomes `SPC`. Back from the first key
    /// of a sequence ends it. Says whether anything is still pending.
    pub fn back(&mut self) -> bool {
        self.path.pop();
        if self.path.is_empty() {
            self.cancel();
            return false;
        }
        let mut node = &self.trie.root;
        for key in &self.path {
            match node.children.get(key) {
                Some(next) => node = next,
                None => {
                    self.cancel();
                    return false;
                }
            }
        }
        self.current = Some(node);
        true
    }

    pub fn is_pending(&self) -> bool {
        self.current.is_some()
    }

    /// The key/label pairs reachable from the current position, for
    /// re-rendering a which-key popup without needing to feed another key.
    pub fn pending_children(&self) -> Vec<(KeyPress, &'static str)> {
        children_labels(self.current.unwrap_or(&self.trie.root))
    }

    pub fn feed(&mut self, key: KeyPress) -> Step<'a, A> {
        let node = self.current.unwrap_or(&self.trie.root);
        let Some(next) = node.children.get(&key) else {
            self.cancel();
            return Step::NoMatch;
        };
        if let Some(action) = &next.leaf {
            self.cancel();
            return Step::Matched(action);
        }
        self.current = Some(next);
        self.path.push(key);
        Step::Pending(children_labels(next))
    }

    pub fn cancel(&mut self) {
        self.current = None;
        self.path.clear();
    }
}

fn children_labels<A>(node: &Node<A>) -> Vec<(KeyPress, &'static str)> {
    node.children.iter().map(|(k, n)| (*k, n.label.unwrap_or(""))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_key_matches_immediately() {
        let mut trie = KeyTrie::new();
        trie.insert(&[KeyPress::char('x')], "delete char", 1);
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('x')) {
            Step::Matched(&1) => {}
            _ => panic!("expected Matched"),
        }
        assert!(!m.is_pending());
    }

    #[test]
    fn multi_key_sequence_is_pending_then_matches() {
        let mut trie = KeyTrie::new();
        trie.insert(&[KeyPress::char('d'), KeyPress::char('d')], "delete line", 42);
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('d')) {
            Step::Pending(children) => assert_eq!(children.len(), 1),
            _ => panic!("expected Pending"),
        }
        assert!(m.is_pending());
        match m.feed(KeyPress::char('d')) {
            Step::Matched(&42) => {}
            _ => panic!("expected Matched"),
        }
        assert!(!m.is_pending());
    }

    #[test]
    fn unknown_key_resets_to_root() {
        let mut trie = KeyTrie::new();
        trie.insert(&[KeyPress::char('d'), KeyPress::char('d')], "delete line", 42);
        let mut m = trie.matcher();
        m.feed(KeyPress::char('d'));
        match m.feed(KeyPress::char('z')) {
            Step::NoMatch => {}
            _ => panic!("expected NoMatch"),
        }
        assert!(!m.is_pending());
        // matcher is usable again from root after a reset
        m.feed(KeyPress::char('d'));
        match m.feed(KeyPress::char('d')) {
            Step::Matched(&42) => {}
            _ => panic!("expected Matched"),
        }
    }

    #[test]
    fn group_labels_show_up_in_pending_children() {
        let mut trie = KeyTrie::new();
        trie.label_group(&[KeyPress::char(' '), KeyPress::char('f')], "files");
        trie.insert(
            &[KeyPress::char(' '), KeyPress::char('f'), KeyPress::char('s')],
            "save",
            "file.save",
        );
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        let Step::Pending(children) = m.feed(KeyPress::char('f')) else { panic!("expected Pending") };
        assert_eq!(children, vec![(KeyPress::char('s'), "save")]);
    }

    #[test]
    fn hints_tell_groups_from_commands_and_count_whats_inside() {
        let mut trie = KeyTrie::new();
        let spc = KeyPress::char(' ');
        trie.label_group(&[spc, KeyPress::char('g')], "git");
        trie.insert(&[spc, KeyPress::char('g'), KeyPress::char('f')], "fetch", 1);
        trie.insert(&[spc, KeyPress::char('g'), KeyPress::char('p')], "pull", 2);
        trie.insert(&[spc, KeyPress::char(',')], "settings", 3);
        trie.set_section(&[spc, KeyPress::char('g'), KeyPress::char('f')], "Remote");
        let mut m = trie.matcher();
        m.feed(spc);
        let mut hints = m.pending_hints();
        hints.sort_by_key(|h| h.label);
        assert_eq!((hints[0].label, hints[0].group, hints[0].count), ("git", true, 2));
        assert_eq!((hints[1].label, hints[1].group), ("settings", false));
        m.feed(KeyPress::char('g'));
        assert_eq!(m.path(), &[spc, KeyPress::char('g')]);
        let fetch = m.pending_hints().into_iter().find(|h| h.label == "fetch").unwrap();
        assert_eq!(fetch.section, Some("Remote"));
    }

    #[test]
    fn back_goes_up_one_level_and_ends_at_the_root() {
        let mut trie = KeyTrie::new();
        let spc = KeyPress::char(' ');
        trie.insert(&[spc, KeyPress::char('g'), KeyPress::char('f')], "fetch", 1);
        let mut m = trie.matcher();
        m.feed(spc);
        m.feed(KeyPress::char('g'));
        assert!(m.back());
        assert_eq!(m.path(), &[spc]);
        assert!(m.pending_hints().iter().any(|h| h.key == KeyPress::char('g')));
        assert!(!m.back());
        assert!(!m.is_pending());
    }

    #[test]
    fn cancel_resets_mid_sequence() {
        let mut trie = KeyTrie::new();
        trie.insert(&[KeyPress::char('d'), KeyPress::char('d')], "delete line", 42);
        let mut m = trie.matcher();
        m.feed(KeyPress::char('d'));
        m.cancel();
        assert!(!m.is_pending());
    }

    #[test]
    fn pending_children_at_root_lists_top_level_bindings() {
        let mut trie = KeyTrie::new();
        trie.insert(&[KeyPress::char('h')], "left", 1);
        trie.insert(&[KeyPress::char('l')], "right", 2);
        let m = trie.matcher();
        // no feed() yet, i.e. a fresh matcher sitting at the root -- this is
        // the state fenix-vim's operator-pending context starts in, since
        // entering it doesn't itself consume a key.
        assert!(!m.is_pending());
        let children = m.pending_children();
        assert_eq!(children.len(), 2);
        assert!(children.contains(&(KeyPress::char('h'), "left")));
        assert!(children.contains(&(KeyPress::char('l'), "right")));
    }

    #[test]
    fn modifiers_distinguish_otherwise_identical_chords() {
        let mut trie = KeyTrie::new();
        trie.insert(&[KeyPress::char('r').with_ctrl()], "redo", "edit.redo");
        let mut m = trie.matcher();
        match m.feed(KeyPress::char('r')) {
            Step::NoMatch => {}
            _ => panic!("plain 'r' should not match a Ctrl-r binding"),
        }
        match m.feed(KeyPress::char('r').with_ctrl()) {
            Step::Matched(&"edit.redo") => {}
            _ => panic!("expected Matched"),
        }
    }
}
