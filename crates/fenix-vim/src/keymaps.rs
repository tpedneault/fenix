use std::sync::OnceLock;

use fenix_keymap::{KeyPress, KeyTrie};

use crate::mode::VisualKind;
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
    EnterVisual(VisualKind),
    EnterCommandLine,
    /// `gv`: re-enter Visual mode with the most recently exited selection.
    ReselectVisual,
    Undo,
    Redo,
    DeleteCharUnder,
    DeleteCharBefore,
    PasteAfter,
    PasteBefore,
    /// `>>`: indent the current (or, with a count, the next N) line(s)
    /// by one level. Doubled-key form only, mirroring `dd`/`cc`/`yy` --
    /// bound as a direct two-key trie leaf rather than going through
    /// `Operator`/`pending_op`, since indent doesn't fit that machinery's
    /// "compute a range, then delete/yank/change it" shape.
    IndentLine,
    /// `<<`: the dedent counterpart of `IndentLine`.
    DedentLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualAction {
    Motion(Motion),
    Apply(Operator),
    /// Pressing a visual-entry key (`v`/`V`/`Ctrl-v`) while already in
    /// Visual mode: same kind as the current selection exits to Normal
    /// (toggle), a different kind switches to it in place (anchor kept).
    SetKind(VisualKind),
    /// `I` in Visual Block: insert at the block's left column, replaying
    /// what's typed onto the other lines when Insert mode ends. A no-op
    /// outside Block mode (bound unconditionally since the trie is shared
    /// across all three Visual kinds).
    BlockInsertLeft,
    /// `>`/`<`: indent/dedent every line the selection touches, always
    /// linewise regardless of `visual_kind` -- matches real Vim, which
    /// never restricts Visual indent to the selected columns.
    Indent,
    Dedent,
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

    t.insert(&[KeyPress::char('v')], "visual", VimAction::EnterVisual(VisualKind::Char));
    t.insert(&[KeyPress::char('V')], "visual line", VimAction::EnterVisual(VisualKind::Line));
    t.insert(&[KeyPress::char('v').with_ctrl()], "visual block", VimAction::EnterVisual(VisualKind::Block));
    t.insert(&[KeyPress::char(':')], "command line", VimAction::EnterCommandLine);

    t.insert(&[KeyPress::char('g'), KeyPress::char('v')], "reselect visual", VimAction::ReselectVisual);

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

    t.insert(&[KeyPress::char('>'), KeyPress::char('>')], "indent line", VimAction::IndentLine);
    t.insert(&[KeyPress::char('<'), KeyPress::char('<')], "dedent line", VimAction::DedentLine);

    t
}

fn build_visual_trie() -> KeyTrie<VisualAction> {
    let mut t = KeyTrie::new();
    add_motions(&mut t, VisualAction::Motion);
    t.insert(&[Operator::Delete.trigger_key()], "delete selection", VisualAction::Apply(Operator::Delete));
    t.insert(&[Operator::Change.trigger_key()], "change selection", VisualAction::Apply(Operator::Change));
    t.insert(&[Operator::Yank.trigger_key()], "yank selection", VisualAction::Apply(Operator::Yank));
    t.insert(&[KeyPress::char('v')], "visual (char)", VisualAction::SetKind(VisualKind::Char));
    t.insert(&[KeyPress::char('V')], "visual (line)", VisualAction::SetKind(VisualKind::Line));
    t.insert(&[KeyPress::char('v').with_ctrl()], "visual (block)", VisualAction::SetKind(VisualKind::Block));
    t.insert(&[KeyPress::char('I')], "insert at block left", VisualAction::BlockInsertLeft);
    t.insert(&[KeyPress::char('>')], "indent selection", VisualAction::Indent);
    t.insert(&[KeyPress::char('<')], "dedent selection", VisualAction::Dedent);
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
