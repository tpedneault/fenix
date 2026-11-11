//! What Fenix shows assistive technology -- a screen reader, braille, a
//! magnifier -- through AccessKit: a tree of nodes built from a
//! `Snapshot` of what's in front. Pure: the host (`app/a11y_host.rs`)
//! takes the snapshot and hands the tree to the window's adapter; see
//! [the plan](../../../docs/plans/screen-reader.md).
//!
//! The tree (phase 1):
//!
//! ```text
//! Window "Fenix -- main.rs"
//! ├── MultilineTextInput "main.rs, Normal"   focused, unless a prompt is
//! │   ├── TextRun  line 120                   the lines around the caret,
//! │   ├── TextRun  line 121                   one run each, the caret and
//! │   └── ...                                 the selection on them
//! ├── TextInput "Command"                    a prompt, when one is open:
//! │   └── TextRun                             focused, its text and caret
//! ├── Status "NORMAL  main.rs  12:4 ..."      the modeline
//! └── Label (live)                           what was just announced: a
//!                                             mode, a message
//! ```
//!
//! A character is a `char` (not a grapheme cluster), and a line its
//! characters with the line break counted as one more, as AccessKit
//! wants them.

use accesskit::{Live, Node, NodeId, Role, TextPosition, TextSelection, TreeId, TreeInfo, TreeUpdate};

/// How many lines either side of the caret's are in the tree: what a
/// screen reader can read without the caret moving. The whole buffer
/// would be rebuilt on every change.
pub const LINES_AROUND: usize = 50;

const ROOT: NodeId = NodeId(1);
const EDITOR: NodeId = NodeId(2);
const MODELINE: NodeId = NodeId(3);
const ANNOUNCE: NodeId = NodeId(4);
const PROMPT: NodeId = NodeId(5);
const PROMPT_RUN: NodeId = NodeId(6);
/// A buffer line's run is `LINE_BASE + line`, so it keeps its id as the
/// window of lines moves.
const LINE_BASE: u64 = 1 << 32;

/// What's in front, as the tree says it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The window's name (its title).
    pub title: String,
    /// The focused text: a buffer, or a page's text.
    pub editor: Option<EditorView>,
    /// A prompt open over it (`:`, `/`, a rename), which has the keyboard.
    pub prompt: Option<PromptView>,
    /// The modeline, as it reads.
    pub modeline: String,
    /// The last thing announced.
    pub announcement: Option<Announcement>,
}

/// The focused text, the lines around the caret.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorView {
    /// What it's called: the file's name, and the mode.
    pub label: String,
    /// The buffer line `lines[0]` is.
    pub first_line: usize,
    /// The lines, without their line breaks.
    pub lines: Vec<String>,
    /// Whether the last of `lines` is the buffer's last (it has no break).
    pub ends_buffer: bool,
    /// The caret, as (buffer line, char column).
    pub caret: (usize, usize),
    /// The other end of the selection, when there is one.
    pub anchor: Option<(usize, usize)>,
    pub read_only: bool,
}

/// A one-line prompt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptView {
    /// What it asks (`Command`, `Search`, `Rename to`).
    pub label: String,
    /// What's been typed.
    pub text: String,
    /// The caret, in chars.
    pub caret: usize,
}

/// Something said once, as it happens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Announcement {
    pub text: String,
    /// An error: said at once, over what's being read.
    pub urgent: bool,
    /// Counts announcements, so the same words twice are said twice.
    pub serial: u64,
}

impl EditorView {
    /// `lines` of a buffer of `line_count` lines around `caret`'s, as
    /// `line_text` reads them.
    pub fn around(label: String, line_count: usize, caret: (usize, usize), anchor: Option<(usize, usize)>, line_text: impl Fn(usize) -> String) -> EditorView {
        let line_count = line_count.max(1);
        let caret_line = caret.0.min(line_count - 1);
        let first_line = caret_line.saturating_sub(LINES_AROUND);
        let end = (caret_line + LINES_AROUND + 1).min(line_count);
        EditorView { label, first_line, lines: (first_line..end).map(line_text).collect(), ends_buffer: end == line_count, caret: (caret_line, caret.1), anchor, read_only: false }
    }

    fn run_id(&self, line: usize) -> NodeId {
        NodeId(LINE_BASE + line as u64)
    }

    /// `(line, col)` as a position on the lines in the tree: a line out
    /// of them is held to the first or last, a column to its line's end
    /// (its line break, when it has one).
    fn position(&self, (line, col): (usize, usize)) -> TextPosition {
        let last = self.first_line + self.lines.len().saturating_sub(1);
        let (line, col) = if line < self.first_line {
            (self.first_line, 0)
        } else if line > last {
            (last, usize::MAX)
        } else {
            (line, col)
        };
        let len = self.lines.get(line - self.first_line).map_or(0, |l| l.chars().count());
        TextPosition { node: self.run_id(line), character_index: col.min(len) }
    }
}

/// The whole tree for `snapshot`: every node, the focus on the prompt if
/// there is one, else the text.
pub fn tree(snapshot: &Snapshot) -> TreeUpdate {
    let mut nodes = Vec::new();
    let mut root = Node::new(Role::Window);
    root.set_label(snapshot.title.clone());
    let mut focus = ROOT;

    if let Some(editor) = &snapshot.editor {
        let mut node = Node::new(Role::MultilineTextInput);
        node.set_label(editor.label.clone());
        if editor.read_only {
            node.set_read_only();
        }
        let last = editor.lines.len().saturating_sub(1);
        for (i, line) in editor.lines.iter().enumerate() {
            let id = editor.run_id(editor.first_line + i);
            let breaks = i < last || !editor.ends_buffer;
            nodes.push((id, text_run(line, breaks)));
            node.push_child(id);
        }
        if editor.lines.is_empty() {
            // Nothing to put a caret on: an empty run.
            let id = editor.run_id(editor.first_line);
            nodes.push((id, text_run("", false)));
            node.push_child(id);
        }
        let focus_at = editor.position(editor.caret);
        let anchor = editor.anchor.map_or(focus_at, |a| editor.position(a));
        node.set_text_selection(TextSelection { anchor, focus: focus_at });
        nodes.push((EDITOR, node));
        root.push_child(EDITOR);
        focus = EDITOR;
    }

    if let Some(prompt) = &snapshot.prompt {
        let mut node = Node::new(Role::TextInput);
        node.set_label(prompt.label.clone());
        node.set_value(prompt.text.clone());
        let at = TextPosition { node: PROMPT_RUN, character_index: prompt.caret.min(prompt.text.chars().count()) };
        node.set_text_selection(TextSelection { anchor: at, focus: at });
        node.push_child(PROMPT_RUN);
        nodes.push((PROMPT_RUN, text_run(&prompt.text, false)));
        nodes.push((PROMPT, node));
        root.push_child(PROMPT);
        focus = PROMPT;
    }

    let mut modeline = Node::new(Role::Status);
    modeline.set_label(snapshot.modeline.clone());
    nodes.push((MODELINE, modeline));
    root.push_child(MODELINE);

    let mut announce = Node::new(Role::Label);
    if let Some(a) = &snapshot.announcement {
        // A live region is read when its text changes: the same words
        // again differ by a space no one hears.
        let pad = if a.serial % 2 == 1 { " " } else { "" };
        announce.set_label(format!("{}{pad}", a.text));
        announce.set_live(if a.urgent { Live::Assertive } else { Live::Polite });
    } else {
        announce.set_live(Live::Polite);
    }
    nodes.push((ANNOUNCE, announce));
    root.push_child(ANNOUNCE);

    nodes.push((ROOT, root));
    let mut tree = TreeInfo::new(ROOT);
    tree.toolkit_name = Some("Fenix".into());
    tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
    TreeUpdate { nodes, tree: Some(tree), tree_id: TreeId::ROOT, focus }
}

/// One line as a text run: its characters' lengths, and its line break
/// (`breaks`) as one more.
fn text_run(line: &str, breaks: bool) -> Node {
    let mut node = Node::new(Role::TextRun);
    let mut lengths: Vec<u8> = line.chars().map(|c| c.len_utf8() as u8).collect();
    let mut value = line.to_string();
    if breaks {
        value.push('\n');
        lengths.push(1);
    }
    let words = word_starts(line);
    node.set_value(value);
    node.set_character_lengths(lengths);
    node.set_word_starts(words);
    node
}

/// Where each word of `line` starts, in characters: a word is a run of
/// letters, digits and `_`, or of other non-space characters.
fn word_starts(line: &str) -> Vec<u8> {
    // AccessKit counts word starts in a byte each; past 255 characters a
    // line's words go unmarked rather than wrong.
    let class = |c: char| if c.is_whitespace() { 0 } else if c.is_alphanumeric() || c == '_' { 1 } else { 2 };
    let mut out = Vec::new();
    let mut previous = 0;
    for (i, c) in line.chars().enumerate() {
        let k = class(c);
        if k != 0 && k != previous {
            match u8::try_from(i) {
                Ok(i) => out.push(i),
                Err(_) => break,
            }
        }
        previous = k;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit_consumer::Tree;

    fn editor(lines: &[&str], caret: (usize, usize), anchor: Option<(usize, usize)>) -> Snapshot {
        Snapshot {
            title: "Fenix -- a.txt".into(),
            editor: Some(EditorView::around("a.txt, Normal".into(), lines.len(), caret, anchor, |l| lines[l].to_string())),
            prompt: None,
            modeline: "NORMAL  a.txt".into(),
            announcement: None,
        }
    }

    #[test]
    fn the_focused_text_is_a_text_area_with_its_lines_and_caret() {
        // A consumer (what every platform's adapter is built on) takes it:
        // the tree is whole and its ids line up.
        let tree = Tree::new(tree(&editor(&["fn main() {", "    let x = 1;", "}"], (1, 8), None)), true);
        let state = tree.state();
        let focus = state.focus().expect("the text has the focus");
        assert_eq!(focus.role(), Role::MultilineTextInput);
        assert_eq!(focus.label().as_deref(), Some("a.txt, Normal"));
        assert!(focus.supports_text_ranges());
        assert_eq!(focus.document_range().text(), "fn main() {\n    let x = 1;\n}");
        let caret = focus.text_selection_focus().unwrap();
        assert_eq!(caret.to_line_index(), 1);
        // The caret on `x`: the rest of the line from it, its break with it.
        let mut rest = caret.to_degenerate_range();
        rest.set_end(caret.forward_to_line_end());
        assert_eq!(rest.text(), "x = 1;\n");
        assert!(!focus.has_text_selection() || focus.text_selection().unwrap().is_degenerate());
    }

    #[test]
    fn a_selection_is_the_text_between_its_ends() {
        let tree = Tree::new(tree(&editor(&["one two", "three"], (1, 2), Some((0, 4)))), true);
        let focus = tree.state().focus().unwrap();
        assert_eq!(focus.text_selection().unwrap().text(), "two\nth");
    }

    #[test]
    fn only_the_lines_around_the_caret_are_in_the_tree() {
        let lines: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        let snap = Snapshot { editor: Some(EditorView::around("big".into(), 1000, (500, 0), None, |l| lines[l].clone())), ..Default::default() };
        let view = snap.editor.as_ref().unwrap();
        assert_eq!(view.first_line, 450);
        assert_eq!(view.lines.len(), 2 * LINES_AROUND + 1);
        assert!(!view.ends_buffer);
        let tree = Tree::new(tree(&snap), true);
        let focus = tree.state().focus().unwrap();
        assert_eq!(focus.text_selection_focus().unwrap().to_line_index(), 50);
        // A selection reaching past them is held to them.
        let mut far = snap.clone();
        far.editor.as_mut().unwrap().anchor = Some((0, 3));
        let tree = Tree::new(super::tree(&far), true);
        assert!(tree.state().focus().unwrap().text_selection().unwrap().text().starts_with("line 450\n"));
    }

    #[test]
    fn an_empty_buffer_still_has_somewhere_for_the_caret() {
        let snap = Snapshot { editor: Some(EditorView::around("empty".into(), 0, (0, 0), None, |_| String::new())), ..Default::default() };
        let tree = Tree::new(tree(&snap), true);
        let focus = tree.state().focus().unwrap();
        assert_eq!(focus.document_range().text(), "");
        assert_eq!(focus.text_selection_focus().unwrap().to_line_index(), 0);
    }

    #[test]
    fn a_prompt_has_the_focus_over_the_text() {
        let mut snap = editor(&["x"], (0, 0), None);
        snap.prompt = Some(PromptView { label: "Command".into(), text: "wq".into(), caret: 2 });
        let tree = Tree::new(tree(&snap), true);
        let focus = tree.state().focus().unwrap();
        assert_eq!(focus.role(), Role::TextInput);
        assert_eq!(focus.label().as_deref(), Some("Command"));
        assert_eq!(focus.document_range().text(), "wq");
    }

    #[test]
    fn an_announcement_is_a_live_region_and_said_again_when_repeated() {
        let mut snap = editor(&["x"], (0, 0), None);
        snap.announcement = Some(Announcement { text: "Insert".into(), urgent: false, serial: 1 });
        let first = tree(&snap);
        let label = |u: &TreeUpdate| u.nodes.iter().find(|(id, _)| *id == ANNOUNCE).and_then(|(_, n)| n.label().map(str::to_string));
        let live = |u: &TreeUpdate| u.nodes.iter().find(|(id, _)| *id == ANNOUNCE).map(|(_, n)| n.live());
        assert_eq!(live(&first), Some(Some(Live::Polite)));
        snap.announcement = Some(Announcement { text: "Insert".into(), urgent: false, serial: 2 });
        let again = tree(&snap);
        assert_ne!(label(&first), label(&again), "the same words again are a change");
        snap.announcement = Some(Announcement { text: "E32: No file name".into(), urgent: true, serial: 3 });
        assert_eq!(live(&tree(&snap)), Some(Some(Live::Assertive)));
        Tree::new(tree(&snap), true);
    }

    #[test]
    fn word_starts_are_where_words_begin() {
        assert_eq!(word_starts("  let x = foo(1);"), vec![2, 6, 8, 10, 13, 14, 15]);
        assert_eq!(word_starts(""), Vec::<u8>::new());
    }
}
