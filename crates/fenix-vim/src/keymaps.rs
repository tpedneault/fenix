use std::sync::OnceLock;

use fenix_keymap::{KeyPress, KeyTrie};

use crate::motion::Motion;
use crate::operator::Operator;
use crate::textobject::TextObject;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertEntry {
    Before,
    After,
    LineStart,
    LineEnd,
    NewlineBelow,
    NewlineAbove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimAction {
    Motion(Motion),
    Operator(Operator),
    /// `D`/`C`/`Y`: the operator applied from the cursor to end of line in
    /// one keypress, rather than composing with a separately-typed motion.
    OperatorToLineEnd(Operator),
    /// `S`: change the whole current line (`cc` in one keypress).
    ChangeLine,
    /// `s`: delete the char under the cursor and enter Insert (`cl`).
    SubstituteChar,
    JoinLines,
    ToggleCase,
    ReplaceChar,
    EnterInsert(InsertEntry),
    EnterVisual,
    EnterCommandLine,
    Undo,
    Redo,
    DeleteCharUnder,
    DeleteCharBefore,
    PasteAfter,
    PasteBefore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualAction {
    Motion(Motion),
    Apply(Operator),
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTarget {
    Motion(Motion),
    TextObject(TextObject),
}

/// Inserts the shared motion set into a trie whose leaf type wraps `Motion`
/// somehow (`VimAction::Motion`, `VisualAction::Motion`, or
/// `PendingTarget::Motion` are all valid `wrap` functions here) -- Normal,
/// Visual, and operator-pending all need the same motions bound the same
/// way, just landing in different leaf enums.
fn add_motions<A>(trie: &mut KeyTrie<A>, wrap: impl Fn(Motion) -> A) {
    trie.insert(&[KeyPress::char('h')], "left", wrap(Motion::Left));
    trie.insert(&[KeyPress::char('l')], "right", wrap(Motion::Right));
    trie.insert(&[KeyPress::char('j')], "down", wrap(Motion::Down));
    trie.insert(&[KeyPress::char('k')], "up", wrap(Motion::Up));
    trie.insert(&[KeyPress::char('w')], "word forward", wrap(Motion::WordForward));
    trie.insert(&[KeyPress::char('b')], "word backward", wrap(Motion::WordBackward));
    trie.insert(&[KeyPress::char('e')], "word end", wrap(Motion::WordEndForward));
    trie.insert(&[KeyPress::char('W')], "WORD forward", wrap(Motion::BigWordForward));
    trie.insert(&[KeyPress::char('B')], "WORD backward", wrap(Motion::BigWordBackward));
    trie.insert(&[KeyPress::char('E')], "WORD end", wrap(Motion::BigWordEndForward));
    trie.insert(&[KeyPress::char('0')], "line start", wrap(Motion::LineStart));
    trie.insert(&[KeyPress::char('^')], "first non-blank", wrap(Motion::LineFirstNonBlank));
    trie.insert(&[KeyPress::char('$')], "line end", wrap(Motion::LineEnd));
    trie.insert(&[KeyPress::char('g'), KeyPress::char('g')], "buffer top", wrap(Motion::BufferTop));
    trie.insert(&[KeyPress::char('G')], "buffer bottom", wrap(Motion::BufferBottom));
}

fn build_normal_trie() -> KeyTrie<VimAction> {
    let mut t = KeyTrie::new();
    add_motions(&mut t, VimAction::Motion);

    t.insert(&[Operator::Delete.trigger_key()], "delete...", VimAction::Operator(Operator::Delete));
    t.insert(&[Operator::Change.trigger_key()], "change...", VimAction::Operator(Operator::Change));
    t.insert(&[Operator::Yank.trigger_key()], "yank...", VimAction::Operator(Operator::Yank));

    t.insert(&[KeyPress::char('x')], "delete char", VimAction::DeleteCharUnder);
    t.insert(&[KeyPress::char('X')], "delete char before", VimAction::DeleteCharBefore);
    t.insert(&[KeyPress::char('p')], "paste after", VimAction::PasteAfter);
    t.insert(&[KeyPress::char('P')], "paste before", VimAction::PasteBefore);

    t.insert(&[KeyPress::char('i')], "insert", VimAction::EnterInsert(InsertEntry::Before));
    t.insert(&[KeyPress::char('a')], "append", VimAction::EnterInsert(InsertEntry::After));
    t.insert(&[KeyPress::char('I')], "insert at line start", VimAction::EnterInsert(InsertEntry::LineStart));
    t.insert(&[KeyPress::char('A')], "append at line end", VimAction::EnterInsert(InsertEntry::LineEnd));
    t.insert(&[KeyPress::char('o')], "open line below", VimAction::EnterInsert(InsertEntry::NewlineBelow));
    t.insert(&[KeyPress::char('O')], "open line above", VimAction::EnterInsert(InsertEntry::NewlineAbove));

    t.insert(&[KeyPress::char('v')], "visual", VimAction::EnterVisual);
    t.insert(&[KeyPress::char(':')], "command line", VimAction::EnterCommandLine);

    t.insert(&[KeyPress::char('u')], "undo", VimAction::Undo);
    t.insert(&[KeyPress::char('r').with_ctrl()], "redo", VimAction::Redo);

    t.insert(&[KeyPress::char('D')], "delete to eol", VimAction::OperatorToLineEnd(Operator::Delete));
    t.insert(&[KeyPress::char('C')], "change to eol", VimAction::OperatorToLineEnd(Operator::Change));
    t.insert(&[KeyPress::char('Y')], "yank to eol", VimAction::OperatorToLineEnd(Operator::Yank));
    t.insert(&[KeyPress::char('S')], "change line", VimAction::ChangeLine);
    t.insert(&[KeyPress::char('s')], "substitute char", VimAction::SubstituteChar);
    t.insert(&[KeyPress::char('J')], "join lines", VimAction::JoinLines);
    t.insert(&[KeyPress::char('~')], "toggle case", VimAction::ToggleCase);
    t.insert(&[KeyPress::char('r')], "replace char", VimAction::ReplaceChar);

    t
}

fn build_visual_trie() -> KeyTrie<VisualAction> {
    let mut t = KeyTrie::new();
    add_motions(&mut t, VisualAction::Motion);
    t.insert(&[Operator::Delete.trigger_key()], "delete selection", VisualAction::Apply(Operator::Delete));
    t.insert(&[Operator::Change.trigger_key()], "change selection", VisualAction::Apply(Operator::Change));
    t.insert(&[Operator::Yank.trigger_key()], "yank selection", VisualAction::Apply(Operator::Yank));
    t.insert(&[KeyPress::char('v')], "exit visual", VisualAction::Exit);
    t
}

fn build_pending_trie() -> KeyTrie<PendingTarget> {
    let mut t = KeyTrie::new();
    add_motions(&mut t, PendingTarget::Motion);
    t.label_group(&[KeyPress::char('i')], "inner...");
    t.insert(
        &[KeyPress::char('i'), KeyPress::char('w')],
        "inner word",
        PendingTarget::TextObject(TextObject::InnerWord),
    );
    t.label_group(&[KeyPress::char('a')], "a...");
    t.insert(
        &[KeyPress::char('a'), KeyPress::char('w')],
        "a word",
        PendingTarget::TextObject(TextObject::AWord),
    );
    t
}

pub(crate) fn normal_trie() -> &'static KeyTrie<VimAction> {
    static TRIE: OnceLock<KeyTrie<VimAction>> = OnceLock::new();
    TRIE.get_or_init(build_normal_trie)
}

pub(crate) fn visual_trie() -> &'static KeyTrie<VisualAction> {
    static TRIE: OnceLock<KeyTrie<VisualAction>> = OnceLock::new();
    TRIE.get_or_init(build_visual_trie)
}

pub(crate) fn pending_trie() -> &'static KeyTrie<PendingTarget> {
    static TRIE: OnceLock<KeyTrie<PendingTarget>> = OnceLock::new();
    TRIE.get_or_init(build_pending_trie)
}
