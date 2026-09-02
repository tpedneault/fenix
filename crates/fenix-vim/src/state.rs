use std::collections::HashMap;
use std::ops::Range;

use fenix_core::{Buffer, Cursor};
use fenix_keymap::{KeyCode, KeyPress, Matcher, Mods, NamedKey, Step};

use crate::bracket;
use crate::indent;
use crate::keymaps::{self, CaseChange, InsertEntry, PendingTarget, ScrollTarget, VimAction, VisualAction};
use crate::keynotation;
use crate::mode::{Mode, VisualKind};
use crate::motion::{self, Motion};
use crate::operator::Operator;
use crate::textobject;

/// What `VimState::handle_key` wants the host application to do -- the one
/// escape hatch out of pure buffer/cursor editing, for the handful of `:`
/// ex-commands that need app-level action (save/quit) rather than a buffer
/// edit, plus a visual-feedback hint the host UI can act on however it
/// likes. Everything else stays inside fenix-vim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimEvent {
    None,
    RequestSave,
    /// `:q`/`SPC b k` -- the host should close the *current buffer*,
    /// but only after checking it for unsaved changes first (unlike
    /// `RequestForceCloseBuffer`). Never quits the application --
    /// see `RequestQuitAll` for that.
    RequestCloseBuffer,
    /// `:q!` -- close the current buffer unconditionally, discarding
    /// any changes, bypassing the check `RequestCloseBuffer` triggers
    /// on the host side. Real Vim's own `!` convention for "I know, do
    /// it anyway."
    RequestForceCloseBuffer,
    /// `:wq`/`:x` -- save the current buffer (if it needs it) then
    /// close it.
    RequestSaveAndCloseBuffer,
    /// `:qa`/`:quitall`/`SPC q q` -- quit the whole application, but
    /// only after checking every buffer for unsaved changes first.
    RequestQuitAll,
    /// `:qa!`/`:quitall!` -- quit the whole application unconditionally.
    RequestForceQuitAll,
    /// `:wqa`/`:xa` -- save every buffer that needs it, then quit the
    /// whole application.
    RequestSaveAllAndQuit,
    /// A yank or paste just happened over this char range -- modeled on
    /// orbit-emacs's own yank/paste pulse feature, for the host UI to
    /// briefly highlight and fade. Not raised for Block-mode yank/paste
    /// (no single contiguous range to pulse cleanly) or delete.
    Pulse(Range<usize>),
    /// `:set shiftwidth=N`/`:set sw=N` just changed the indent width to
    /// this value -- a hint for the host app to persist it, the same
    /// "escape hatch" role `RequestSave` plays for `:w`.
    IndentWidthChanged(usize),
    /// A "big jump" motion (`gg`/`G`, `%`, an initial `/`/`?` search, or
    /// `*`/`#`) just moved the cursor -- carries the char index it moved
    /// *from*, for a host-owned jumplist (`Ctrl-O`/`Ctrl-I`) to record.
    /// `fenix-vim` only ever tracks one buffer/cursor at a time, so the
    /// jumplist itself can't live here -- a jump to a symbol definition
    /// or a grep match crosses buffers entirely, which only the host
    /// knows about. Raised only when the motion actually moved the
    /// cursor (a no-op `%` off a bracket, or a search with no match,
    /// raises `None` instead) -- and, deliberately unlike real Vim,
    /// never for `n`/`N` (`VimAction::RepeatSearch`), which just repeats
    /// whatever the last recorded jump already was rather than starting
    /// a new one.
    JumpRecorded(usize),
    /// `m{name}` just recorded a mark named `name` at this char index.
    /// The host owns the actual mark table, the same reason it owns the
    /// jumplist (see `JumpRecorded`'s own doc comment) -- a mark set in
    /// one buffer needs to still mean that buffer if the user switches
    /// away and back, which `fenix-vim`'s single buffer-agnostic
    /// `VimState` has no way to track.
    MarkSet(char, usize),
    /// `` `{name} ``/`'{name}` -- asks the host to look up mark `name`
    /// and jump there; a no-op (silently) if that mark was never set,
    /// same "graceful, not a hard error" posture as a `%` with nothing
    /// to match. `linewise` mirrors real Vim's own split between the two
    /// forms: `` ` `` wants the mark's exact char index, `'` wants the
    /// first non-blank of its line.
    JumpToMark { name: char, linewise: bool },
    /// `q{name}` just started recording into register `name` (case
    /// preserved: uppercase means "append to whatever's already in
    /// there," matching real Vim's own convention) -- the host owns the
    /// actual multi-source key capture (see `App::dispatch_keypress`'s
    /// own doc comment for why: a leader-triggered command never reaches
    /// `VimState::handle_key` at all, so only the host sees every key),
    /// this is purely the "recording just started, into this register"
    /// notification.
    MacroRecordStart(char),
    /// A second bare `q` just stopped recording into `name` -- no second
    /// key needed, unlike the start. The host should finalize whatever
    /// it captured since the matching `MacroRecordStart` and hand it to
    /// `finish_recording`.
    MacroRecordStop(char),
    /// `@{name}` (or `@@`, `name` literally `'@'`, meaning "whichever
    /// register was last played") -- asks the host to decode register
    /// `name`'s text (`keynotation::decode`) and replay it `count` times.
    /// A no-op if the register (or, for `@@`, "last played") is empty/
    /// unknown, same graceful posture as `JumpToMark`.
    MacroPlay { register: char, count: u32 },
    /// A user-facing error the host should surface (invalid search
    /// pattern, `:s` failure) -- `fenix-vim` has no UI of its own (see
    /// this enum's own doc comment), so this is the same "escape hatch"
    /// every other variant here already is, just carrying a message
    /// instead of an instruction. Replaces what used to be a silent
    /// `eprintln!` at each of these two call sites.
    Error(String),
    /// `zz`/`zt`/`zb` -- asks the host to scroll the focused pane so the
    /// cursor's line lands at `target`. Host-resolved since only it
    /// knows pane geometry (visible-line count).
    ScrollWindow(ScrollTarget),
    /// `.` -- asks the host to replay whatever it last recorded as a
    /// repeatable change. The host owns that capture (raw keystrokes
    /// since the last idle point that produced a real edit), the same
    /// reason it owns macro storage/replay (`MacroPlay`'s own doc
    /// comment) -- `fenix-vim` only ever sees one key at a time.
    RepeatLastChange,
    /// `gcc`/`gc{motion}` -- asks the host to toggle line-comments across
    /// `start_line..=end_line` (inclusive). Host-resolved since only it
    /// knows the buffer's language-specific comment token; `fenix-vim`
    /// only computed *which lines* the motion/text-object touched.
    ToggleComment { start_line: usize, end_line: usize },
}

struct Register {
    text: String,
    /// Whether this register holds whole lines (from a linewise operation
    /// like `dd`/`yy`/`dG`) vs. a char span -- determines whether `p`/`P`
    /// paste as new lines or inline at the cursor.
    linewise: bool,
}

/// What `m`/`` ` ``/`'` are waiting on their next raw key (the mark's
/// name) to do -- mirrors `pending_find`'s "one more raw key, not a
/// trie key" shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingMark {
    Set,
    Jump { linewise: bool },
}

/// vim-surround/evil-surround's own three commands (`ys`/`ds`/`cs`),
/// reached via `y`/`d`/`c` (the ordinary operator triggers) followed by
/// `s` -- see `handle_operator_pending_key`'s own entry-point check for
/// how that's detected. Doesn't fit `Operator`/`PendingTarget` (each
/// command needs a different shape of follow-up keys), so it's its own
/// small pending-state machine, same "next raw key(s) are special"
/// posture as `PendingMark`/`pending_find`/`pending_replace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurroundPending {
    /// `ys{motion}` -- awaiting the motion/text-object naming what to
    /// wrap (or a second `s` for `yss`, "the whole line").
    Target,
    /// The target range resolved; awaiting the one char naming what to
    /// wrap it in.
    AddChar { start: usize, end: usize },
    /// `ds{char}` -- awaiting which surround to delete.
    Delete,
    /// `cs{old}` -- awaiting the *old* surround char.
    ChangeOld,
    /// The old pair was found; awaiting the *new* surround char.
    ChangeNew { open_idx: usize, close_idx: usize },
}

/// What `"`/`q`/`@` are waiting on their next raw key to do -- same
/// "one more raw key, not a trie key" shape as `PendingMark`, unified
/// into one enum since only one of the three can ever be pending at
/// once. `"` selects which register the *next* command (`y`/`d`/`c`/`x`/
/// `s`/`p`/`P`) reads or writes; `q` (only reachable when not already
/// recording -- see `handle_normal_key`'s own `q` dispatch, which
/// short-circuits to `VimEvent::MacroRecordStop` without ever setting
/// this) starts recording into the named register; `@` replays it,
/// `count` carrying whatever was typed before it (`3@a`), exactly like
/// every other countable action already does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPrefixKey {
    Register,
    MacroRecord,
    MacroPlay { count: u32 },
}

/// Tracks a Visual-Block `I` session: what's typed on the top line gets
/// replayed at the same column on every other line in the block when
/// `Escape` ends Insert mode. Ragged lines shorter than `col` are skipped
/// rather than padded with spaces -- a deliberate simplification.
struct BlockInsert {
    line_lo: usize,
    line_hi: usize,
    col: usize,
    typed: String,
}

pub struct VimState {
    mode: Mode,
    /// The unnamed register (`""`) -- always mirrors the *newly written*
    /// text of the most recent yank/delete/change, regardless of
    /// whether a named register (`registers`) was also explicitly
    /// targeted via `"{name}`, matching real Vim's own behavior.
    register: Register,
    /// Named registers `a`-`z`, keyed by lowercase -- case (uppercase =
    /// append, matching real Vim's `"Ayy`-style convention) is only ever
    /// a *write-mode* flag, never a distinct key. A macro recorded via
    /// `q{name}` is stored here too (as `keynotation`-encoded text, see
    /// `finish_recording`) -- real Vim itself has no separate "macro
    /// storage": `@a` just interprets whatever text register `a` holds,
    /// whether it got there by recording, yanking, or hand-editing.
    registers: HashMap<char, Register>,
    /// Set by `"`/`q`/`@`; consumed by the very next raw key -- see
    /// `PendingPrefixKey`'s own doc comment.
    pending_prefix: Option<PendingPrefixKey>,
    /// Set once `"`'s register name arrives: `(lowercase name, append)`
    /// -- consulted (and cleared) by the very next register-touching
    /// command (`write_register`/`paste`), or cleared without effect by
    /// any other command, exactly like a count-prefix already resets
    /// after being consumed (see `handle_normal_key`'s `Step::Matched`/
    /// `Step::NoMatch` arms).
    active_register: Option<(char, bool)>,
    /// `Some(name)` while `q{name}` is actively recording -- the single
    /// source of truth `q`'s own dispatch checks to tell "start" from
    /// "stop" apart (see `MacroRecordStart`/`MacroRecordStop`'s own doc
    /// comments). The actual key-by-key *capture* lives on the host
    /// (`App`), not here -- see `VimEvent::MacroRecordStart`'s doc
    /// comment for why.
    recording_register: Option<char>,
    /// Set once `q{name}` resolves (case preserved); consumed by
    /// `handle_key` and turned into `VimEvent::MacroRecordStart`.
    pending_macro_start: Option<char>,
    /// Set by a second bare `q` while recording; consumed by
    /// `handle_key` and turned into `VimEvent::MacroRecordStop`.
    pending_macro_stop: Option<char>,
    /// Set once `@{name}`'s register name arrives; consumed by
    /// `handle_key` and turned into `VimEvent::MacroPlay`.
    pending_macro_play: Option<(char, u32)>,
    pending_op: Option<Operator>,
    /// The count that was accumulated before the operator was entered
    /// (`3dd`); combined multiplicatively with whatever count is typed
    /// between the operator and its motion (`d3w`, or `d3d` for the
    /// doubled form) at resolution time.
    pending_op_count: u32,
    /// Set by `r`: the *next* key replaces the char under the cursor with
    /// itself repeated `count` times, instead of going through the normal
    /// trie at all -- no motion/text object composition, just one raw key.
    pending_replace: Option<u32>,
    /// Set by Visual mode's `r`: the *next* key overwrites every selected
    /// character. Kept separate from `pending_replace` since Visual's `r`
    /// doesn't take a `count` (the selection already says how much) and
    /// is resolved by `handle_visual_key`, not `handle_normal_key`.
    pending_visual_replace: bool,
    /// Digits accumulated so far for a numeric count prefix (`3w`, `2dd`).
    /// Consumed whenever a key sequence actually resolves to an action;
    /// preserved across `Step::Pending` (mid multi-key sequences like
    /// `gg`) so `3gg` doesn't lose the `3` on the first `g`.
    count: Option<u32>,
    visual_anchor: usize,
    visual_kind: VisualKind,
    /// The kind/anchor/cursor of the most recently exited Visual
    /// selection, for `gv` to restore.
    last_visual: Option<(VisualKind, usize, usize)>,
    block_insert: Option<BlockInsert>,
    /// Set by a yank or (non-block) paste; consumed by `handle_key` right
    /// after dispatch and turned into `VimEvent::Pulse`.
    pending_pulse: Option<Range<usize>>,
    /// Set by a "big jump" motion that actually moved the cursor;
    /// consumed by `handle_key` right after dispatch and turned into
    /// `VimEvent::JumpRecorded` -- see that variant's own doc comment.
    pending_jump: Option<usize>,
    /// Set by `jump_to_search` on an invalid pattern; consumed by
    /// `handle_key` and turned into `VimEvent::Error` -- same drain-chain
    /// shape as `pending_pulse`/`pending_jump`.
    pending_error: Option<String>,
    /// Set by `m`/`` ` ``/`'`: the *next* key names the mark, not a trie
    /// key -- same "one more raw key" shape as `pending_find`.
    pending_mark: Option<PendingMark>,
    /// Set once the mark name arrives after `m`; consumed by `handle_key`
    /// and turned into `VimEvent::MarkSet`.
    pending_mark_set: Option<(char, usize)>,
    /// Set once the mark name arrives after `` ` ``/`'`; consumed by
    /// `handle_key` and turned into `VimEvent::JumpToMark`.
    pending_mark_jump: Option<(char, bool)>,
    /// Set by `zz`/`zt`/`zb`; consumed by `handle_key` and turned into
    /// `VimEvent::ScrollWindow` -- same drain-chain shape as
    /// `pending_jump`.
    pending_scroll: Option<ScrollTarget>,
    /// Set by `.`; consumed by `handle_key` and turned into `VimEvent::
    /// RepeatLastChange` -- same drain-chain shape as `pending_scroll`,
    /// just with no payload to carry.
    pending_repeat_last_change: bool,
    /// `y`/`d`/`c` followed by `s` (`ys`/`ds`/`cs`) -- vim-surround's own
    /// pending-state machine, driving `ys{motion}{char}`, `ds{char}`,
    /// `cs{old}{new}`. `Copy`, so read via `if let Some(pending) = self.
    /// pending_surround` without `.take()` -- exactly mirrors how
    /// `pending_op` stays live across multiple keys until explicitly
    /// cleared by whichever phase resolves. See `SurroundPending`'s own
    /// doc comment for each phase.
    pending_surround: Option<SurroundPending>,
    /// Visual mode's `S`: the *next* key is the surround char -- same
    /// "next key is special" shape as `pending_visual_replace`.
    pending_visual_surround: bool,
    /// `g`+`c` (`gcc`/`gc{motion}`): awaiting either a doubled `c` (whole
    /// line(s)) or a motion/text object -- same shape `pending_surround`'s
    /// own `Target` phase has, just without needing an enum of its own
    /// (there's only ever this one phase).
    pending_comment: bool,
    /// The count that was accumulated *before* `gc` itself was entered
    /// (`3gcc`) -- `self.count` is unconditionally cleared the moment
    /// any normal-trie action resolves (including `gc`'s own two-key
    /// resolution), so without capturing it here it would be lost
    /// before `handle_comment_key` ever sees it. Combined
    /// multiplicatively with whatever count is typed *between* `gc`
    /// and the motion/doubled key (`gc3j`, `3gcc`) at resolution time --
    /// exactly mirrors `pending_op_count`'s own reason for existing.
    pending_comment_count: u32,
    /// Set once `pending_comment` resolves to a line range; consumed by
    /// `handle_key` and turned into `VimEvent::ToggleComment`.
    pending_comment_lines: Option<(usize, usize)>,
    command_line: String,
    /// Set by `f`/`F`/`t`/`T`: the *next* key is the target char, not a
    /// trie key -- `(forward, till, count)`, `count` being whatever was
    /// resolved at the point the prompt started (`3fx`, or the combined
    /// `2d3fx`), same "one more raw key" shape as `pending_replace`.
    pending_find: Option<(bool, bool, u32)>,
    /// The resolved motion (already carrying its target char) plus that
    /// char, from the most recent `f`/`F`/`t`/`T` -- what `;`/`,` repeat.
    last_find: Option<(Motion, char)>,
    /// The query being typed while `mode() == Mode::Search`; direction
    /// is `search_forward`. Cleared on confirm/cancel.
    search_query: String,
    search_forward: bool,
    /// Pattern + direction of the most recently *confirmed* search (via
    /// Enter, or `*`/`#`), for `n`/`N` to repeat.
    last_search: Option<(String, bool)>,
    /// Whether the host UI should be drawing `hlsearch`-style persistent
    /// highlighting for `last_search`'s pattern -- set on every confirmed
    /// search (`jump_to_search`), cleared in `handle_key` the moment a
    /// real buffer edit happens (detected via `Buffer::edit_count`,
    /// snapshotted before/after dispatch). Deliberately simpler than real
    /// Vim's `:noh`/autocmd-driven clearing: "any edit clears it," a
    /// disclosed simplification -- `last_search` itself is untouched by
    /// this, so `n`/`N` keep repeating the same pattern even after its
    /// highlighting has been cleared by an edit.
    hlsearch_active: bool,
    /// Spaces per indent level for Tab, `>>`/`<<`, and the bracket-bump
    /// on Enter -- runtime-configurable via `:set shiftwidth=N`/`:set
    /// sw=N` (see `substitute::run_ex_command`), persisted by the host
    /// app the same way it already persists the theme/font size.
    indent_width: usize,

    normal_matcher: Matcher<'static, VimAction>,
    visual_matcher: Matcher<'static, VisualAction>,
    pending_matcher: Matcher<'static, PendingTarget>,
}

impl VimState {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            register: Register { text: String::new(), linewise: false },
            registers: HashMap::new(),
            pending_prefix: None,
            active_register: None,
            recording_register: None,
            pending_macro_start: None,
            pending_macro_stop: None,
            pending_macro_play: None,
            pending_op: None,
            pending_op_count: 1,
            pending_replace: None,
            pending_visual_replace: false,
            pending_visual_surround: false,
            pending_comment: false,
            pending_comment_count: 1,
            pending_comment_lines: None,
            count: None,
            visual_anchor: 0,
            visual_kind: VisualKind::Char,
            last_visual: None,
            block_insert: None,
            pending_pulse: None,
            pending_jump: None,
            pending_error: None,
            pending_mark: None,
            pending_mark_set: None,
            pending_mark_jump: None,
            pending_scroll: None,
            pending_repeat_last_change: false,
            pending_surround: None,
            command_line: String::new(),
            pending_find: None,
            last_find: None,
            search_query: String::new(),
            search_forward: true,
            last_search: None,
            hlsearch_active: false,
            indent_width: indent::DEFAULT_INDENT_WIDTH,
            normal_matcher: keymaps::normal_trie().matcher(),
            visual_matcher: keymaps::visual_trie().matcher(),
            pending_matcher: keymaps::pending_trie().matcher(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Current spaces-per-indent-level, for the host app to seed from a
    /// persisted setting at startup and to re-persist after `:set
    /// shiftwidth=N` changes it (see `VimEvent::IndentWidthChanged`).
    pub fn indent_width(&self) -> usize {
        self.indent_width
    }

    /// Sets the indent width directly -- used by the host app to apply a
    /// persisted setting at startup, mirroring `:set shiftwidth=N`'s own
    /// effect. Zero is rejected (a no-op) since every indent computation
    /// divides by it.
    pub fn set_indent_width(&mut self, width: usize) {
        if width > 0 {
            self.indent_width = width;
        }
    }

    /// Which kind of selection Visual mode is making. Only meaningful
    /// while `mode()` is `Visual`.
    pub fn visual_kind(&self) -> VisualKind {
        self.visual_kind
    }

    pub fn command_line(&self) -> &str {
        &self.command_line
    }

    /// The in-progress `/`/`?` query text -- only meaningful while
    /// `mode()` is `Search`. Paired with `search_forward` for the host
    /// UI to render `/query` or `?query` in place of the modeline.
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// Direction of the search prompt currently being typed (`true` for
    /// `/`, `false` for `?`). Only meaningful while `mode()` is `Search`.
    pub fn search_forward(&self) -> bool {
        self.search_forward
    }

    /// Where the *in-progress* search query (`search_query`/
    /// `search_forward`) would currently jump to, without moving the
    /// cursor or recording anything to `last_search` -- incsearch's live
    /// preview. `None` outside `Mode::Search`, on an empty query, on an
    /// invalid pattern, or when nothing matches.
    pub fn preview_match(&self, buffer: &Buffer, cursor: &Cursor) -> Option<usize> {
        if self.mode != Mode::Search || self.search_query.is_empty() {
            return None;
        }
        crate::search::find_next(buffer, cursor, &self.search_query, self.search_forward).ok().flatten()
    }

    /// Whether the host UI should render `hlsearch`-style persistent
    /// highlighting for `last_search_pattern()`. See the `hlsearch_active`
    /// field's own doc comment for when this turns on/off.
    pub fn hlsearch_active(&self) -> bool {
        self.hlsearch_active
    }

    /// The pattern `hlsearch_active`/`n`/`N` refer to, if any search has
    /// ever been confirmed this session.
    pub fn last_search_pattern(&self) -> Option<&str> {
        self.last_search.as_ref().map(|(pattern, _)| pattern.as_str())
    }

    /// Every occurrence of `last_search_pattern()` within `byte_range`
    /// (the caller's currently-visible line range, converted to bytes) --
    /// `hlsearch`'s persistent match highlighting. Empty when `hlsearch_
    /// active` is false, there's no confirmed search yet, or the pattern
    /// no longer parses (can't happen today since a pattern only ever
    /// gets here after `find_next` already accepted it, but `search::
    /// all_matches_in_range` still returns `Result` since it re-compiles
    /// the pattern -- degrading to "nothing highlighted" rather than
    /// propagating an error keeps this a plain, infallible query for the
    /// host UI, matching the "log and degrade" posture `jump_to_search`
    /// already uses for a bad pattern).
    pub fn hlsearch_matches(&self, buffer: &Buffer, byte_range: Range<usize>) -> Vec<Range<usize>> {
        if !self.hlsearch_active {
            return Vec::new();
        }
        let Some((pattern, _)) = &self.last_search else {
            return Vec::new();
        };
        crate::search::all_matches_in_range(buffer, pattern, byte_range).unwrap_or_default()
    }

    /// The char offset Visual mode's selection is anchored at. Only
    /// meaningful while `mode()` is `Visual`; the host UI uses this
    /// together with the cursor position to render the selection.
    pub fn visual_anchor(&self) -> usize {
        self.visual_anchor
    }

    /// The unnamed register's current contents (text, linewise) -- for a
    /// host UI that wants to mirror it onto the OS clipboard: push this
    /// after any key that may have written the register (yank, delete,
    /// change), and pull the OS clipboard's contents in via
    /// `set_register` before dispatching a paste key, so `y`/`p` round-
    /// trip through the system clipboard instead of Fenix's own private
    /// scratch register.
    pub fn register(&self) -> (&str, bool) {
        (&self.register.text, self.register.linewise)
    }

    /// Overwrites the unnamed register -- see `register`'s own doc
    /// comment for why a host UI would call this (syncing in whatever's
    /// currently on the OS clipboard right before a paste).
    pub fn set_register(&mut self, text: String, linewise: bool) {
        self.register = Register { text, linewise };
    }

    /// Whether `q{name}` recording is currently active -- for a host UI
    /// to show a "recording" indicator, and for `App::dispatch_keypress`
    /// to decide whether to tap the incoming key into its own capture
    /// buffer (see `VimEvent::MacroRecordStart`'s doc comment).
    pub fn is_recording(&self) -> bool {
        self.recording_register.is_some()
    }

    /// The register currently being recorded into, if any.
    pub fn recording_register(&self) -> Option<char> {
        self.recording_register
    }

    /// No command is currently "in progress" -- Normal mode, no pending
    /// operator/register/replace/find/mark/surround, no count prefix
    /// typed, and not mid-way through a multi-key Normal-trie sequence
    /// (`normal_matcher.is_pending()` -- catches a bare `g` waiting for
    /// `g`/`v`/`c`, `z` waiting for `z`/`t`/`b`, etc.: none of those set
    /// any of the other fields checked here, so without this check a
    /// leading key like `g` would look "idle" the instant it's typed
    /// and get dropped from `.`'s own capture -- a real bug, found via
    /// `gcc` specifically since it was the first `g`-prefixed command
    /// that actually edits the buffer; `gg`/`gv` never exposed it since
    /// neither produces an edit for the capture logic to notice). The
    /// boundary a repeatable command's key-capture starts and ends at:
    /// the host (`App::dispatch_keypress`) watches this to know when to
    /// reset its capture buffer and when a just-completed span is a
    /// candidate for `.` to replay -- see that function's own doc
    /// comment. `visual_matcher` is checked too for the same reason,
    /// even though no Visual-trie sequence is currently more than one
    /// key -- cheap insurance against the identical bug the moment one
    /// is added. Doesn't need to check `pending_scroll`/`pending_pulse`/
    /// etc: those are transient output flags, always drained back to
    /// `None` by the end of the very `handle_key` call that set them
    /// (see its own doc comment), never still-`Some` by the time a
    /// caller could observe `is_idle` from outside.
    pub fn is_idle(&self) -> bool {
        self.mode == Mode::Normal
            && self.count.is_none()
            && self.pending_op.is_none()
            && self.pending_prefix.is_none()
            && self.active_register.is_none()
            && self.pending_replace.is_none()
            && self.pending_find.is_none()
            && self.pending_mark.is_none()
            && self.pending_surround.is_none()
            && !self.pending_comment
            && !self.normal_matcher.is_pending()
            && !self.visual_matcher.is_pending()
    }

    /// Register `name`'s content, decoded (`keynotation::decode`) back
    /// into keystrokes to replay -- for a host handling `VimEvent::
    /// MacroPlay`. Works uniformly regardless of how the register got
    /// its content (typed, yanked, or recorded via `q{name}`), matching
    /// real Vim: there's no distinct "macro storage," `@{name}` just
    /// interprets whatever text is there. `None` for a register that's
    /// never been written (or is empty) -- a host should treat this the
    /// same as real Vim's "nothing to repeat."
    pub fn decode_register(&self, name: char) -> Option<Vec<KeyPress>> {
        let text = &self.registers.get(&name.to_ascii_lowercase())?.text;
        if text.is_empty() {
            return None;
        }
        Some(keynotation::decode(text))
    }

    /// Finalizes a `q{register}...q` recording: encodes `keys` (the
    /// host's own captured buffer -- see `VimEvent::MacroRecordStart`'s
    /// doc comment for why the host, not `VimState`, does the capturing)
    /// via `keynotation::encode` and writes the result into `register`,
    /// appending to whatever was already there if `append` (the
    /// recording was started with an uppercase register name).
    pub fn finish_recording(&mut self, register: char, keys: &[KeyPress], append: bool) {
        let text = keynotation::encode(keys);
        self.store_in_register(register.to_ascii_lowercase(), &text, false, append);
    }

    /// The absolute char range the active Visual selection covers, and
    /// whether it's linewise -- `None` outside Visual mode, or for a
    /// Visual-Block selection (a rectangular column selection has no
    /// single contiguous range; `apply_block_operator`'s own per-row
    /// handling is what deals with those). For a host action that wants
    /// to treat "whatever's selected" as plain text -- e.g. running an
    /// external formatter over it -- rather than going through a Vim
    /// operator.
    pub fn visual_selection_range(&self, buffer: &Buffer, cursor: &Cursor) -> Option<(Range<usize>, bool)> {
        if self.mode != Mode::Visual || self.visual_kind == VisualKind::Block {
            return None;
        }
        Some(self.visual_range(buffer, cursor))
    }

    /// Programmatically exits Visual mode back to Normal -- the same
    /// state transition `Escape` performs (see `handle_visual_key`) --
    /// for a host action that replaces the Visual selection's text
    /// itself (bypassing Vim's own operator dispatch) and needs Vim's
    /// mode/`last_visual` to catch up afterward. `cursor` should already
    /// reflect wherever the host left it before calling this. A no-op
    /// outside Visual mode.
    pub fn exit_visual_mode(&mut self, cursor: &Cursor) {
        if self.mode == Mode::Visual {
            self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
            self.mode = Mode::Normal;
        }
    }

    /// Whether a multi-key sequence (operator-pending, `gg`, ...) is
    /// waiting on more input, for a which-key-style hint in the host UI.
    pub fn is_pending(&self) -> bool {
        self.pending_op.is_some() || self.normal_matcher.is_pending() || self.visual_matcher.is_pending()
    }

    /// The key/label pairs reachable from wherever a pending sequence
    /// currently sits, for a which-key-style hint in the host UI. Empty
    /// when nothing is pending.
    pub fn pending_children(&self) -> Vec<(KeyPress, &'static str)> {
        if self.pending_op.is_some() {
            self.pending_matcher.pending_children()
        } else if self.normal_matcher.is_pending() {
            self.normal_matcher.pending_children()
        } else if self.visual_matcher.is_pending() {
            self.visual_matcher.pending_children()
        } else {
            Vec::new()
        }
    }

    pub fn handle_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        let edit_count_before = buffer.edit_count();
        let event = match self.mode {
            Mode::Insert => self.handle_insert_key(buffer, cursor, key, false),
            Mode::Replace => self.handle_insert_key(buffer, cursor, key, true),
            Mode::Command => self.handle_command_key(buffer, cursor, key),
            Mode::Normal => self.handle_normal_key(buffer, cursor, key),
            Mode::Visual => self.handle_visual_key(buffer, cursor, key),
            Mode::Search => self.handle_search_key(buffer, cursor, key),
        };
        // hlsearch's persistent highlighting clears the moment a real
        // edit happens, regardless of what mode/action caused it -- see
        // `hlsearch_active`'s own doc comment for why this is simpler
        // than real Vim's `:noh`-driven clearing, and why it doesn't
        // touch `last_search` (`n`/`N` keep working afterward).
        if buffer.edit_count() != edit_count_before {
            self.hlsearch_active = false;
        }
        // All drained unconditionally (not just whichever's read below)
        // so none of them ever linger stale into the next keystroke,
        // even though nothing today sets more than one in the same call.
        let pulse = self.pending_pulse.take();
        let jump = self.pending_jump.take();
        let error = self.pending_error.take();
        let mark_set = self.pending_mark_set.take();
        let mark_jump = self.pending_mark_jump.take();
        let macro_start = self.pending_macro_start.take();
        let macro_stop = self.pending_macro_stop.take();
        let macro_play = self.pending_macro_play.take();
        let scroll = self.pending_scroll.take();
        let repeat_last_change = std::mem::take(&mut self.pending_repeat_last_change);
        let comment_lines = self.pending_comment_lines.take();
        // A pulse is purely a visual-feedback hint layered on top of
        // whatever else happened; None is the only event a yank/paste
        // keypress would otherwise produce, so this never shadows a real
        // RequestSave/Quit. The rest take the next priority for the same
        // reason -- every key that sets one of these only ever returns
        // `VimEvent::None` itself, so there's nothing real underneath to
        // shadow. Flattened into an `Option::or` priority chain (same
        // order the old nested-match version had) rather than nesting
        // yet another level per new pending kind.
        pulse
            .map(VimEvent::Pulse)
            .or_else(|| jump.map(VimEvent::JumpRecorded))
            .or_else(|| error.map(VimEvent::Error))
            .or_else(|| mark_set.map(|(name, char_idx)| VimEvent::MarkSet(name, char_idx)))
            .or_else(|| mark_jump.map(|(name, linewise)| VimEvent::JumpToMark { name, linewise }))
            .or_else(|| macro_start.map(VimEvent::MacroRecordStart))
            .or_else(|| macro_stop.map(VimEvent::MacroRecordStop))
            .or_else(|| macro_play.map(|(register, count)| VimEvent::MacroPlay { register, count }))
            .or_else(|| scroll.map(VimEvent::ScrollWindow))
            .or_else(|| repeat_last_change.then_some(VimEvent::RepeatLastChange))
            .or_else(|| comment_lines.map(|(start_line, end_line)| VimEvent::ToggleComment { start_line, end_line }))
            .unwrap_or(event)
    }

    fn handle_insert_key(
        &mut self,
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        key: KeyPress,
        replace: bool,
    ) -> VimEvent {
        match key.code {
            KeyCode::Named(NamedKey::Escape) => {
                self.mode = Mode::Normal;
                let (_, col) = buffer.line_col(cursor);
                if col > 0 {
                    buffer.move_left(cursor);
                }
                self.replay_block_insert(buffer);
            }
            KeyCode::Named(NamedKey::Backspace) => {
                // Backspace right between an empty bracket pair (`{|}`)
                // removes both characters at once rather than leaving the
                // close bracket dangling behind -- the companion to
                // auto-pairing's own insert, same "un-type as a unit"
                // posture. Structural, not provenance-tracked: applies
                // whether or not this exact pair was just auto-inserted,
                // same as type-through above. Its own atomic step
                // (`delete_range` flushes pending), same tradeoff as
                // electric dedent.
                let before = cursor.char_idx.checked_sub(1).and_then(|i| buffer.char_at(i));
                let after = buffer.char_at(cursor.char_idx);
                let is_empty_pair = !replace
                    && match (before, after) {
                        (Some(open), Some(close)) => indent::matching_close_bracket(open) == Some(close),
                        _ => false,
                    };
                if is_empty_pair {
                    buffer.delete_range(cursor, cursor.char_idx - 1, cursor.char_idx + 1);
                } else {
                    buffer.delete_backward(cursor);
                }
                if let Some(bi) = &mut self.block_insert {
                    bi.typed.pop();
                }
            }
            KeyCode::Named(NamedKey::Delete) => buffer.delete_forward(cursor),
            KeyCode::Named(NamedKey::Enter) => {
                // Carries over the current line's leading whitespace onto
                // the new line, plus one extra indent level if the char
                // right before the cursor opens a bracket -- inserted as
                // a run of `insert_char` calls (not `insert_str`) so it
                // coalesces into the same undo step as the surrounding
                // Insert-mode session, same as ordinary typed chars do.
                let (line, _) = buffer.line_col(cursor);
                let base_indent = indent::leading_whitespace(buffer, line);
                let bumps = cursor.char_idx > 0
                    && buffer.char_at(cursor.char_idx - 1).is_some_and(indent::is_opening_bracket);

                // A Markdown-style list line continues its own marker
                // onto the new line instead of the plain carried-over
                // indent below -- see `indent::list_continuation_text`'s
                // own doc comment for exactly what "continues" means and
                // why an empty item (nothing typed after the marker yet)
                // deliberately yields `None` here, falling through to
                // the plain path: that path already carries just the
                // bare indent with no marker, which *is* "leave the
                // list," with no separate cleanup needed since Enter
                // never touches text before the cursor to begin with.
                // Skipped entirely when a bracket bump applies -- the
                // two signals are essentially mutually exclusive in
                // practice, and the bracket one is the more established
                // of this file's own two.
                let list_continuation =
                    if bumps { None } else { indent::parse_list_item(buffer, line).and_then(|item| indent::list_continuation_text(&item)) };

                if let Some(next_line) = list_continuation {
                    buffer.insert_char(cursor, '\n');
                    for ch in next_line.chars() {
                        buffer.insert_char(cursor, ch);
                    }
                } else {
                    let mut new_indent = base_indent.clone();
                    if bumps {
                        new_indent.push_str(&" ".repeat(self.indent_width));
                    }
                    // Enter right between an open/close bracket pair
                    // (`{|}`, almost always auto-paired a moment ago)
                    // splits across three lines, not two: without this,
                    // the close bracket was just left trailing on the
                    // cursor's own bumped-indent line (`        }`
                    // sitting a full extra level deep, wherever that
                    // bump happened to land -- not actually related to
                    // any other bracket on the line, it only looked
                    // that way) instead of dropping back onto its own
                    // line at the *open* bracket's own indent.
                    let splits_closing_bracket = bumps
                        && buffer.char_at(cursor.char_idx).is_some_and(|c| {
                            matches!(buffer.char_at(cursor.char_idx - 1), Some(open) if indent::matching_close_bracket(open) == Some(c))
                        });
                    buffer.insert_char(cursor, '\n');
                    for ch in new_indent.chars() {
                        buffer.insert_char(cursor, ch);
                    }
                    if splits_closing_bracket {
                        // Insert the close bracket's own line past the
                        // cursor's real resting spot, then step back --
                        // same "insert past, then pull back" idiom the
                        // auto-pair insert above this uses for the same
                        // reason (`insert_char` always advances the cursor
                        // it's given; stepping back with a manual `char_idx`
                        // assignment instead of `move_left` keeps this whole
                        // sequence one coalesced pending edit).
                        let resting_at = cursor.char_idx;
                        buffer.insert_char(cursor, '\n');
                        for ch in base_indent.chars() {
                            buffer.insert_char(cursor, ch);
                        }
                        cursor.char_idx = resting_at;
                        let (_, col) = buffer.line_col(cursor);
                        cursor.sticky_col = col;
                    }
                }
            }
            KeyCode::Named(NamedKey::Tab) => {
                // Soft-tab: spaces up to the next stop, not a literal
                // '\t' -- the render pipeline has no tab-stop logic (see
                // `indent.rs`'s doc comment), so a raw tab wouldn't align
                // to anything. A char-at-a-time loop, not `insert_str`,
                // so it coalesces with the surrounding Insert-mode run.
                let (_, col) = buffer.line_col(cursor);
                let n = indent::spaces_to_next_stop(col, self.indent_width);
                for _ in 0..n {
                    buffer.insert_char(cursor, ' ');
                }
                if let Some(bi) = &mut self.block_insert {
                    bi.typed.push_str(&" ".repeat(n));
                }
            }
            KeyCode::Named(NamedKey::Left) => buffer.move_left(cursor),
            KeyCode::Named(NamedKey::Right) => buffer.move_right(cursor),
            KeyCode::Named(NamedKey::Up) => buffer.move_up(cursor),
            KeyCode::Named(NamedKey::Down) => buffer.move_down(cursor),
            KeyCode::Named(NamedKey::Home) => buffer.move_home(cursor),
            KeyCode::Named(NamedKey::End) => buffer.move_end(cursor),
            KeyCode::Char(c) if key.mods == Mods::default() => {
                if replace && buffer.char_at(cursor.char_idx).is_some() {
                    buffer.delete_forward(cursor);
                }
                if !replace && indent::is_closing_bracket(c) && buffer.char_at(cursor.char_idx) == Some(c) {
                    // Type-through: the bracket you just typed is already
                    // the very next character (almost always one this
                    // same self-insert arm auto-paired a moment ago) --
                    // move past it instead of inserting a duplicate, the
                    // standard "close over" behavior every auto-pairing
                    // editor has. Checked before electric dedent below:
                    // if the bracket's already there, nothing should be
                    // inserted or dedented, just stepped over.
                    cursor.char_idx += 1;
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                } else {
                    // Electric dedent: typing a closing bracket as the
                    // first non-whitespace char on the line snaps it one
                    // indent level shallower first, the common "type `}`
                    // and watch it jump left" habit. A fixed one-level
                    // heuristic, not real bracket matching -- its own
                    // undo step (dedent_line uses delete_range, which
                    // doesn't coalesce), separate from whatever's typed
                    // next.
                    if !replace && indent::is_closing_bracket(c) && indent::line_blank_before_cursor(buffer, cursor) {
                        let (line, _) = buffer.line_col(cursor);
                        indent::dedent_line(buffer, cursor, line, self.indent_width);
                        // Not `line_first_non_blank` -- the line is
                        // entirely whitespace (that's the trigger
                        // condition), so "first non-blank" degenerates to
                        // the line start, not where the bracket should
                        // land. The line's remaining content is exactly
                        // the un-removed leading whitespace, so its own
                        // end is the right spot to type into.
                        cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
                        let (_, col) = buffer.line_col(cursor);
                        cursor.sticky_col = col;
                    }
                    buffer.insert_char(cursor, c);
                    // Auto-pair: typing an opening bracket also inserts
                    // its close right after, then steps the cursor back
                    // between them -- via `insert_char` (not `insert_str`)
                    // so it coalesces with the opening char into one
                    // pending run (undoing right after a bare `{}` removes
                    // both together); the manual `char_idx -= 1` (not
                    // `Buffer::move_left`, which flushes pending) is what
                    // keeps that coalescing intact. Typing content after
                    // stepping back does end that run at the next
                    // `insert_char` call, since its target position no
                    // longer immediately follows the pending run's end --
                    // a disclosed side effect, and arguably the right one:
                    // undoing what you just typed *inside* the brackets
                    // shouldn't also remove the brackets themselves.
                    if !replace {
                        if let Some(close) = indent::matching_close_bracket(c) {
                            buffer.insert_char(cursor, close);
                            cursor.char_idx -= 1;
                            let (_, col) = buffer.line_col(cursor);
                            cursor.sticky_col = col;
                        }
                    }
                }
                if let Some(bi) = &mut self.block_insert {
                    bi.typed.push(c);
                }
            }
            _ => {}
        }
        VimEvent::None
    }

    /// Replays a Visual-Block `I` session's typed text at the same column
    /// on every other line in the block. Lines too short to reach that
    /// column are skipped rather than padded with spaces.
    fn replay_block_insert(&mut self, buffer: &mut Buffer) {
        let Some(bi) = self.block_insert.take() else { return };
        if bi.typed.is_empty() {
            return;
        }
        for line in (bi.line_lo + 1)..=bi.line_hi {
            if line >= buffer.line_count() || bi.col > buffer.line_len(line) {
                continue;
            }
            let at = buffer.line_start_char(line) + bi.col;
            let mut c = Cursor { char_idx: at, sticky_col: 0 };
            buffer.insert_str(&mut c, &bi.typed);
        }
    }

    fn handle_command_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        match key.code {
            KeyCode::Named(NamedKey::Escape) => {
                self.command_line.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Named(NamedKey::Enter) => {
                let cmd = std::mem::take(&mut self.command_line);
                self.mode = Mode::Normal;
                let last_search = self.last_search.as_ref().map(|(pattern, _)| pattern.as_str());
                return crate::substitute::run_ex_command(&cmd, buffer, cursor, &mut self.indent_width, last_search);
            }
            KeyCode::Named(NamedKey::Backspace) => {
                self.command_line.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => {
                self.command_line.push(c);
            }
            _ => {}
        }
        VimEvent::None
    }

    /// `Mode::Search`'s key handler -- same "next keystrokes are special"
    /// shape as `handle_command_key`, except `Enter` also runs the search
    /// and moves the cursor (recording it as `last_search` for `n`/`N`)
    /// instead of parsing an ex-command.
    fn handle_search_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        match key.code {
            KeyCode::Named(NamedKey::Escape) => {
                self.search_query.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Named(NamedKey::Enter) => {
                let query = std::mem::take(&mut self.search_query);
                self.mode = Mode::Normal;
                if !query.is_empty() {
                    self.last_search = Some((query.clone(), self.search_forward));
                    let before = cursor.char_idx;
                    self.jump_to_search(buffer, cursor, &query, self.search_forward);
                    if cursor.char_idx != before {
                        self.pending_jump = Some(before);
                    }
                }
            }
            KeyCode::Named(NamedKey::Backspace) => {
                self.search_query.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => {
                self.search_query.push(c);
            }
            _ => {}
        }
        VimEvent::None
    }

    /// Runs `search::find_next` and moves the cursor to the result, if
    /// any -- shared by the search prompt's `Enter`, `n`/`N`, and `*`/`#`.
    /// A no-op (silent, matching this project's established "log and
    /// degrade, never crash" posture for a bad user-supplied pattern) on
    /// a regex compile error or no match found.
    fn jump_to_search(&mut self, buffer: &Buffer, cursor: &mut Cursor, pattern: &str, forward: bool) {
        self.hlsearch_active = true;
        match crate::search::find_next(buffer, cursor, pattern, forward) {
            Ok(Some(idx)) => {
                cursor.char_idx = idx;
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            Ok(None) => {}
            Err(err) => self.pending_error = Some(format!("invalid search pattern: {err}")),
        }
    }

    fn handle_normal_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        if let Some(count) = self.pending_replace.take() {
            if key.code != KeyCode::Named(NamedKey::Escape) {
                if let KeyCode::Char(c) = key.code {
                    self.replace_char(buffer, cursor, c, count);
                }
            }
            return VimEvent::None;
        }

        // `f`/`F`/`t`/`T`'s target char -- checked before the
        // `pending_op` dispatch below so this resolves correctly
        // whether the find was started standalone or mid-`d{motion}`
        // (`resolve_find` itself checks `pending_op` to decide which).
        // Set either by `VimAction::FindCharPrompt` (the standalone
        // trie leaf) or directly by `handle_operator_pending_key`
        // (whose own trie has no leaves for these -- the target char
        // isn't known at trie-build time).
        if let Some((forward, till, count)) = self.pending_find.take() {
            match key.code {
                KeyCode::Char(c) if key.mods == Mods::default() => {
                    self.resolve_find(buffer, cursor, forward, till, c, count);
                }
                _ => {
                    // Escape, or anything else -- abort, same as
                    // Escape's own handling in `handle_operator_pending_key`.
                    self.pending_op = None;
                    self.count = None;
                }
            }
            return VimEvent::None;
        }

        // `m`/`` ` ``/`'`'s mark name -- same "one more raw key" shape as
        // `pending_find` above. Not composable with an operator (no
        // `` d`a ``/`d'a`` support -- see `PendingMark`'s own doc comment
        // for why), so unlike `pending_find` this is never set while
        // `pending_op` is also active, and doesn't need to coordinate
        // with `handle_operator_pending_key` the way `pending_find` does.
        if let Some(pending) = self.pending_mark.take() {
            if let KeyCode::Char(name) = key.code {
                if key.mods == Mods::default() {
                    self.resolve_mark(cursor, pending, name);
                }
            }
            // Anything else (Escape, ...) just silently aborts.
            return VimEvent::None;
        }

        // `"`/`q`/`@`'s register name -- same "one more raw key" shape as
        // `pending_mark` above, shared with `handle_visual_key` (only
        // `Register` can ever be pending there) via `resolve_pending_prefix`.
        if let Some(pending) = self.pending_prefix.take() {
            self.resolve_pending_prefix(pending, key);
            return VimEvent::None;
        }

        // vim-surround's `ys`/`ds`/`cs` -- not `.take()`n, since `Target`
        // needs to stay live across multiple keys (a motion/text-object
        // can itself be multi-key, `ysiw"`) until `handle_surround_key`
        // itself explicitly resolves or cancels it, exactly how
        // `pending_op` stays live across `handle_operator_pending_key`.
        if let Some(pending) = self.pending_surround {
            self.handle_surround_key(buffer, cursor, pending, key);
            return VimEvent::None;
        }

        // `gc{motion}`/`gcc` -- same "stays live across multiple keys
        // until explicitly resolved" shape as `pending_surround`.
        if self.pending_comment {
            self.handle_comment_key(buffer, cursor, key);
            return VimEvent::None;
        }

        if let Some(op) = self.pending_op {
            self.handle_operator_pending_key(buffer, cursor, op, key);
            return VimEvent::None;
        }

        // Count prefix (`3w`, `2dd`): digits were never trie leaves, so
        // intercept them here rather than feeding them to the matcher.
        // Only while not already mid-sequence, so e.g. the second `g` of
        // `gg` is never misread as a count digit.
        if !self.normal_matcher.is_pending() {
            if let KeyCode::Char(c) = key.code {
                if key.mods == Mods::default() {
                    if let Some(d) = c.to_digit(10) {
                        if d != 0 || self.count.is_some() {
                            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
                            return VimEvent::None;
                        }
                    }
                }
            }
        }

        // `"`/`q`/`@`: bare, unmodified, top-level triggers -- never trie
        // leaves (mirrors the count-digit interception just above), since
        // none of them compose with anything else. `q` is the one
        // conditional case: it means "start recording" only when nothing
        // is already being recorded, "stop" (immediately, no second key)
        // otherwise -- `VimState` is the sole owner of `recording_
        // register`, so it alone can tell these apart.
        if !self.normal_matcher.is_pending() {
            if let KeyCode::Char(c @ ('"' | 'q' | '@')) = key.code {
                if key.mods == Mods::default() {
                    match c {
                        '"' => {
                            self.count = None;
                            self.pending_prefix = Some(PendingPrefixKey::Register);
                        }
                        'q' => {
                            self.count = None;
                            match self.recording_register.take() {
                                Some(reg) => self.pending_macro_stop = Some(reg),
                                None => self.pending_prefix = Some(PendingPrefixKey::MacroRecord),
                            }
                        }
                        '@' => {
                            let count = self.count.take().unwrap_or(1).max(1);
                            self.pending_prefix = Some(PendingPrefixKey::MacroPlay { count });
                        }
                        _ => unreachable!(),
                    }
                    return VimEvent::None;
                }
            }
        }

        match self.normal_matcher.feed(key) {
            Step::Matched(action) => {
                let raw_count = self.count.take();
                self.apply_normal_action(buffer, cursor, *action, raw_count);
                // A register selected via `"{name}` is one-shot: consumed
                // by `write_register`/`paste` if this action touched a
                // register, discarded here otherwise -- exactly like
                // `count` already resets whether or not it was used.
                // Skipped while an operator is left pending (`"ad2w`):
                // the register is still needed once the motion arrives,
                // resolved through `handle_operator_pending_key` instead
                // of this match arm.
                if self.pending_op.is_none() {
                    self.active_register = None;
                }
            }
            Step::NoMatch => {
                self.count = None;
                self.active_register = None;
            }
            Step::Pending(_) => {}
        }
        VimEvent::None
    }

    /// Resolves a pending `"`/`q`/`@` once its register-name key arrives
    /// -- shared by `handle_normal_key` and `handle_visual_key` (only
    /// `Register` is ever reachable from the latter). Anything other
    /// than a plain, unmodified letter (or, for `MacroPlay`, the literal
    /// `@` sentinel) silently cancels, same posture as an invalid mark
    /// name.
    fn resolve_pending_prefix(&mut self, pending: PendingPrefixKey, key: KeyPress) {
        let KeyCode::Char(name) = key.code else { return };
        if key.mods != Mods::default() {
            return;
        }
        match pending {
            PendingPrefixKey::Register => {
                if name.is_ascii_alphabetic() {
                    self.active_register = Some((name.to_ascii_lowercase(), name.is_ascii_uppercase()));
                }
            }
            PendingPrefixKey::MacroRecord => {
                if name.is_ascii_alphabetic() {
                    self.recording_register = Some(name.to_ascii_lowercase());
                    self.pending_macro_start = Some(name);
                }
            }
            PendingPrefixKey::MacroPlay { count } => {
                if name.is_ascii_alphabetic() || name == '@' {
                    self.pending_macro_play = Some((name, count));
                }
            }
        }
    }

    /// `raw_count` is the count exactly as typed (`None` if no digits
    /// preceded the key) -- most actions below just want it defaulted to
    /// 1 (`count`, computed once up front), but `gg`/`G` need to know
    /// whether a count was typed at all, not just its resolved value:
    /// `1gg` and bare `gg` happen to coincide (both mean "line 1"), but
    /// bare `G` (last line) and `1G` (line 1) very much don't -- see
    /// `motion::buffer_line_target`'s own doc comment.
    fn apply_normal_action(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, action: VimAction, raw_count: Option<u32>) {
        let count = raw_count.unwrap_or(1).max(1);
        match action {
            VimAction::Motion(m @ (Motion::BufferTop | Motion::BufferBottom)) => {
                let before = cursor.char_idx;
                cursor.char_idx = motion::buffer_line_target(buffer, m, raw_count);
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
                if cursor.char_idx != before {
                    self.pending_jump = Some(before);
                }
            }
            VimAction::Motion(m @ Motion::MatchingBracket) => {
                let before = cursor.char_idx;
                apply_motion(buffer, cursor, m, count);
                if cursor.char_idx != before {
                    self.pending_jump = Some(before);
                }
            }
            VimAction::Motion(m) => apply_motion(buffer, cursor, m, count),
            VimAction::Operator(op) => {
                self.pending_op = Some(op);
                self.pending_op_count = count;
            }
            VimAction::EnterInsert(entry) => self.enter_insert(buffer, cursor, entry),
            VimAction::EnterVisual(kind) => {
                self.mode = Mode::Visual;
                self.visual_kind = kind;
                self.visual_anchor = cursor.char_idx;
            }
            VimAction::EnterCommandLine => {
                self.mode = Mode::Command;
                self.command_line.clear();
            }
            VimAction::ReselectVisual => {
                if let Some((kind, anchor, last_cursor)) = self.last_visual {
                    self.mode = Mode::Visual;
                    self.visual_kind = kind;
                    self.visual_anchor = anchor.min(buffer.len_chars());
                    cursor.char_idx = last_cursor.min(buffer.len_chars());
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                }
            }
            VimAction::Undo => {
                for _ in 0..count {
                    buffer.undo(cursor);
                }
            }
            VimAction::Redo => {
                for _ in 0..count {
                    buffer.redo(cursor);
                }
            }
            VimAction::DeleteCharUnder => {
                // Real Vim's `x` never eats the line's own newline, even
                // with a count that overruns the line (`5x` on a
                // 2-char line deletes just those 2 chars, not the `\n`
                // after them) -- clamp to the line's own content end,
                // not the whole buffer's.
                let (line, _) = buffer.line_col(cursor);
                let line_end = buffer.line_start_char(line) + buffer.line_len(line);
                let end = (cursor.char_idx + count as usize).min(line_end);
                if end > cursor.char_idx {
                    let text = buffer.delete_range(cursor, cursor.char_idx, end);
                    self.write_register(text, false);
                }
            }
            VimAction::DeleteCharBefore => {
                let start = cursor.char_idx.saturating_sub(count as usize);
                if start < cursor.char_idx {
                    let text = buffer.delete_range(cursor, start, cursor.char_idx);
                    self.write_register(text, false);
                }
            }
            VimAction::PasteAfter => self.paste(buffer, cursor, true, count),
            VimAction::PasteBefore => self.paste(buffer, cursor, false, count),
            VimAction::IndentLine | VimAction::DedentLine => {
                let (line, _) = buffer.line_col(cursor);
                let end_line = (line + count as usize - 1).min(motion::last_line(buffer));
                for l in line..=end_line {
                    if action == VimAction::IndentLine {
                        indent::indent_line(buffer, cursor, l, self.indent_width);
                    } else {
                        indent::dedent_line(buffer, cursor, l, self.indent_width);
                    }
                }
                cursor.char_idx = motion::line_first_non_blank(buffer, line);
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            VimAction::OperatorToLineEnd(op) => {
                // Count support skipped here (real Vim's "3D" also pulls in
                // the next 2 full lines, a nuance not worth the complexity
                // for this action) -- always behaves as a plain d$/c$/y$.
                let range = range_for_motion(buffer, cursor, Motion::LineEnd, 1);
                self.finish_operator(buffer, cursor, op, range, false);
            }
            VimAction::ChangeLine => {
                let (line, _) = buffer.line_col(cursor);
                let end_line = (line + count as usize - 1).min(motion::last_line(buffer));
                let range = lines_content_range(buffer, line, end_line);
                self.finish_operator(buffer, cursor, Operator::Change, range, true);
            }
            VimAction::SubstituteChar => {
                let end = (cursor.char_idx + count as usize).min(buffer.len_chars());
                if end > cursor.char_idx {
                    let text = buffer.delete_range(cursor, cursor.char_idx, end);
                    self.write_register(text, false);
                }
                self.mode = Mode::Insert;
            }
            VimAction::JoinLines => {
                // "NJ" joins N lines together (N-1 individual joins); bare
                // J (count=1) and 2J both mean "join with just the next line".
                for _ in 0..count.saturating_sub(1).max(1) {
                    self.join_lines(buffer, cursor);
                }
            }
            VimAction::ToggleCase => {
                for _ in 0..count.max(1) {
                    self.toggle_case(buffer, cursor);
                }
            }
            VimAction::ReplaceChar => self.pending_replace = Some(count),
            VimAction::FindCharPrompt { forward, till } => self.pending_find = Some((forward, till, count)),
            VimAction::RepeatFind { reverse } => {
                if let Some((m, _)) = self.last_find {
                    let m = if reverse { reverse_find_motion(m) } else { m };
                    apply_motion(buffer, cursor, m, count);
                }
            }
            VimAction::EnterSearch { forward } => {
                self.mode = Mode::Search;
                self.search_query.clear();
                self.search_forward = forward;
            }
            VimAction::RepeatSearch { reverse } => {
                if let Some((pattern, dir)) = self.last_search.clone() {
                    let forward = if reverse { !dir } else { dir };
                    self.jump_to_search(buffer, cursor, &pattern, forward);
                }
            }
            VimAction::SearchWord { forward } => {
                if let Some(pattern) = crate::search::word_under_cursor_pattern(buffer, cursor) {
                    self.last_search = Some((pattern.clone(), forward));
                    let before = cursor.char_idx;
                    self.jump_to_search(buffer, cursor, &pattern, forward);
                    if cursor.char_idx != before {
                        self.pending_jump = Some(before);
                    }
                }
            }
            VimAction::MarkSetPrompt => self.pending_mark = Some(PendingMark::Set),
            VimAction::MarkJumpPrompt { linewise } => self.pending_mark = Some(PendingMark::Jump { linewise }),
            VimAction::ScrollWindow(target) => self.pending_scroll = Some(target),
            VimAction::IncrementNumber { delta } => self.increment_number(buffer, cursor, delta * count as i64),
            // No buffer mutation here -- replay is host-driven (mirrors
            // `VimEvent::MacroPlay`), since only the host owns the raw-
            // keystroke capture this replays. Plain `.` only for v1: no
            // count-override semantics (`3.` isn't "repeat with count 3
            // instead" the way real Vim's is) -- out of scope for now.
            VimAction::RepeatLastChange => self.pending_repeat_last_change = true,
            VimAction::ToggleCommentPrompt => {
                self.pending_comment = true;
                self.pending_comment_count = count;
            }
        }
    }

    /// Resolves a pending `f`/`F`/`t`/`T` once its target char arrives --
    /// shared by the standalone form (`fx`) and the operator-pending one
    /// (`dfx`), which only differ in what happens with the resolved
    /// motion: move the cursor directly, or compute a range and hand it
    /// to `finish_operator`. `count` is whatever was resolved at the
    /// point `f`/`F`/`t`/`T` itself was pressed (already combined with
    /// `pending_op_count` if there was one before the operator) --
    /// passed in rather than re-read from `self.count`, which by this
    /// point holds whatever (if anything) was typed *after* the prompt
    /// started, not before it.
    fn resolve_find(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, forward: bool, till: bool, c: char, count: u32) {
        let m = match (forward, till) {
            (true, false) => Motion::FindChar(c),
            (false, false) => Motion::FindCharBack(c),
            (true, true) => Motion::TillChar(c),
            (false, true) => Motion::TillCharBack(c),
        };
        self.last_find = Some((m, c));
        if let Some(op) = self.pending_op.take() {
            let total_count = self.pending_op_count.saturating_mul(count);
            let range = range_for_motion(buffer, cursor, m, total_count);
            self.finish_operator(buffer, cursor, op, range, m.is_linewise());
        } else {
            apply_motion(buffer, cursor, m, count);
        }
    }

    /// Resolves a pending `m`/`` ` ``/`'` once its mark name arrives --
    /// just stashes the right pending-event field for `handle_key` to
    /// drain into a `VimEvent` (`MarkSet`/`JumpToMark`); the actual
    /// bookkeeping/cursor movement is entirely the host's, see those
    /// variants' own doc comments.
    fn resolve_mark(&mut self, cursor: &Cursor, pending: PendingMark, name: char) {
        match pending {
            PendingMark::Set => self.pending_mark_set = Some((name, cursor.char_idx)),
            PendingMark::Jump { linewise } => self.pending_mark_jump = Some((name, linewise)),
        }
    }

    /// `J`: joins the current line with the next, trimming the next
    /// line's leading whitespace and separating with a single space.
    /// A no-op on the buffer's last real line.
    fn join_lines(&mut self, buffer: &mut Buffer, cursor: &mut Cursor) {
        let (line, _) = buffer.line_col(cursor);
        if line >= motion::last_line(buffer) {
            return;
        }
        let this_line_end = buffer.line_start_char(line) + buffer.line_len(line);
        let next_start = buffer.line_start_char(line + 1);
        let next_len = buffer.line_len(line + 1);
        let mut skip = 0;
        while skip < next_len && buffer.char_at(next_start + skip).is_some_and(|c| c.is_whitespace()) {
            skip += 1;
        }
        buffer.delete_range(cursor, this_line_end, next_start + skip);
        cursor.char_idx = this_line_end;
        buffer.insert_str(cursor, " ");
        cursor.char_idx = this_line_end;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// `~`: flips the case of the char under the cursor and advances,
    /// like Vim (not full Unicode case-folding -- takes the first char of
    /// `to_uppercase`/`to_lowercase`, which is lossy for the rare
    /// multi-char cases).
    fn toggle_case(&mut self, buffer: &mut Buffer, cursor: &mut Cursor) {
        let Some(c) = buffer.char_at(cursor.char_idx) else { return };
        let toggled = apply_case(c, CaseChange::Toggle);
        let start = cursor.char_idx;
        buffer.delete_range(cursor, start, start + 1);
        buffer.insert_str(cursor, &toggled.to_string());
        cursor.char_idx = (start + 1).min(buffer.len_chars());
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// `Ctrl-a`/`Ctrl-x`: finds the nearest run of ASCII digits (optionally
    /// `-`-prefixed) at or after the cursor on its own line, adds `delta`
    /// to it, and lands the cursor on the new value's last digit --
    /// matches real Vim's own `Ctrl-a`/`Ctrl-x`. A no-op if the line has
    /// no number from the cursor onward, or the found run doesn't parse
    /// (longer than `i64` can hold -- not worth a bigint dependency for
    /// this). Scoped to the current line -- a number spanning a newline
    /// isn't a real case worth handling.
    fn increment_number(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, delta: i64) {
        let (line, _) = buffer.line_col(cursor);
        let line_start = buffer.line_start_char(line);
        let line_text = buffer.text_range(line_start, line_start + buffer.line_len(line));
        let chars: Vec<char> = line_text.chars().collect();
        let cursor_col = cursor.char_idx.saturating_sub(line_start);

        let Some((rel_start, rel_end)) = find_number(&chars, cursor_col) else { return };
        let old_text: String = chars[rel_start..rel_end].iter().collect();
        let Ok(value) = old_text.parse::<i64>() else { return };
        let new_text = value.saturating_add(delta).to_string();

        let start = line_start + rel_start;
        let end = line_start + rel_end;
        buffer.replace_range(cursor, start, end, &new_text);
        cursor.char_idx = start + new_text.chars().count() - 1;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// `r<char>`, with `count` (`3rx` replaces 3 chars with `x`). Vim
    /// rejects the whole thing if there aren't `count` chars available
    /// (no partial replace) -- matched here rather than silently clamping.
    fn replace_char(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, c: char, count: u32) {
        let count = count.max(1) as usize;
        let start = cursor.char_idx;
        let end = start + count;
        if end > buffer.len_chars() {
            return;
        }
        buffer.delete_range(cursor, start, end);
        let replacement: String = std::iter::repeat_n(c, count).collect();
        buffer.insert_str(cursor, &replacement);
        cursor.char_idx = start;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// Visual mode's `r<char>`: overwrites every selected character with
    /// `c`, keeping the buffer's line structure -- any newline inside the
    /// selection (a multi-line charwise span, a linewise span, or just the
    /// ragged tail past a shorter line in Block mode) is left alone rather
    /// than being clobbered, matching real Vim. Exits to Normal mode and
    /// leaves the cursor at the selection's top-left, same as real Vim's
    /// own `r` in Visual mode.
    fn visual_replace_char(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, c: char) {
        self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
        match self.visual_kind {
            VisualKind::Block => {
                let (line_lo, line_hi, col_lo, col_hi) = self.block_bounds(buffer, cursor);
                for line in line_lo..=line_hi {
                    let start = buffer.line_start_char(line);
                    let len = buffer.line_len(line);
                    let lo = col_lo.min(len);
                    let hi = col_hi.min(len);
                    if lo >= hi {
                        continue;
                    }
                    let replacement: String = std::iter::repeat_n(c, hi - lo).collect();
                    buffer.replace_range(cursor, start + lo, start + hi, &replacement);
                }
                cursor.char_idx = buffer.line_start_char(line_lo) + col_lo.min(buffer.line_len(line_lo));
            }
            VisualKind::Char | VisualKind::Line => {
                let (range, _) = self.visual_range(buffer, cursor);
                let start = range.start;
                for idx in range {
                    if buffer.char_at(idx) == Some('\n') {
                        continue;
                    }
                    buffer.replace_range(cursor, idx, idx + 1, &c.to_string());
                }
                cursor.char_idx = start;
            }
        }
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        self.mode = Mode::Normal;
    }

    /// Visual mode's `~`/`u`/`U`: changes the case of every character in
    /// the selection (newlines left alone), leaving the buffer's line
    /// structure untouched -- same three-way `visual_kind` dispatch and
    /// "exit to Normal at the selection's start" shape as `visual_
    /// replace_char`, just applying `apply_case` per-character instead
    /// of overwriting with one literal char.
    fn visual_change_case(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, mode: CaseChange) {
        self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
        match self.visual_kind {
            VisualKind::Block => {
                let (line_lo, line_hi, col_lo, col_hi) = self.block_bounds(buffer, cursor);
                for line in line_lo..=line_hi {
                    let start = buffer.line_start_char(line);
                    let len = buffer.line_len(line);
                    let lo = col_lo.min(len);
                    let hi = col_hi.min(len);
                    if lo >= hi {
                        continue;
                    }
                    let original = buffer.text_range(start + lo, start + hi);
                    let replacement: String = original.chars().map(|c| apply_case(c, mode)).collect();
                    buffer.replace_range(cursor, start + lo, start + hi, &replacement);
                }
                cursor.char_idx = buffer.line_start_char(line_lo) + col_lo.min(buffer.line_len(line_lo));
            }
            VisualKind::Char | VisualKind::Line => {
                let (range, _) = self.visual_range(buffer, cursor);
                let start = range.start;
                for idx in range {
                    let Some(c) = buffer.char_at(idx) else { continue };
                    if c == '\n' {
                        continue;
                    }
                    let changed = apply_case(c, mode);
                    if changed != c {
                        buffer.replace_range(cursor, idx, idx + 1, &changed.to_string());
                    }
                }
                cursor.char_idx = start;
            }
        }
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        self.mode = Mode::Normal;
    }

    fn enter_insert(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, entry: InsertEntry) {
        match entry {
            InsertEntry::Before => {}
            InsertEntry::After => {
                if buffer.char_at(cursor.char_idx).is_some() {
                    cursor.char_idx += 1;
                }
            }
            InsertEntry::LineStart => cursor.char_idx = motion::target(buffer, cursor, Motion::LineFirstNonBlank),
            InsertEntry::LineEnd => {
                let (line, _) = buffer.line_col(cursor);
                cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
            }
            InsertEntry::NewlineBelow => {
                let (line, _) = buffer.line_col(cursor);
                // Same list-continuation behavior as Insert-mode Enter
                // -- see its own doc comment. `O` (`NewlineAbove`, right
                // below) deliberately doesn't get this: opening a new
                // line *above* the current item would need renumbering
                // every ordered item from there down to stay correct,
                // real added complexity for a much rarer motion than
                // `o`/Enter.
                let new_indent = indent::parse_list_item(buffer, line)
                    .and_then(|item| indent::list_continuation_text(&item))
                    .unwrap_or_else(|| indent::leading_whitespace(buffer, line));
                cursor.char_idx = buffer.line_start_char(line) + buffer.line_len(line);
                buffer.insert_char(cursor, '\n');
                for ch in new_indent.chars() {
                    buffer.insert_char(cursor, ch);
                }
            }
            InsertEntry::NewlineAbove => {
                let (line, _) = buffer.line_col(cursor);
                let new_indent = indent::leading_whitespace(buffer, line);
                cursor.char_idx = buffer.line_start_char(line);
                buffer.insert_char(cursor, '\n');
                cursor.char_idx -= 1;
                for ch in new_indent.chars() {
                    buffer.insert_char(cursor, ch);
                }
            }
        }
        self.mode = Mode::Insert;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    fn handle_operator_pending_key(
        &mut self,
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        op: Operator,
        key: KeyPress,
    ) {
        if key.code == KeyCode::Named(NamedKey::Escape) {
            self.pending_op = None;
            self.pending_matcher.cancel();
            self.count = None;
            return;
        }

        // Count typed between the operator and its motion/doubled key
        // (`d3w`, `d3d`) -- combines multiplicatively with pending_op_count
        // (`2d3w` = 6 words) at resolution time below. Same "not mid-
        // sequence" guard as the Normal-mode count check.
        if !self.pending_matcher.is_pending() {
            if let KeyCode::Char(c) = key.code {
                if key.mods == Mods::default() {
                    if let Some(d) = c.to_digit(10) {
                        if d != 0 || self.count.is_some() {
                            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
                            return;
                        }
                    }
                }
            }
        }

        // `f`/`F`/`t`/`T` as an operator's motion (`dfx`, `d3fx`): not a
        // `pending_matcher` leaf (the target char isn't known at
        // trie-build time, same reason it isn't a Normal-trie leaf
        // either) -- recognized directly here, same as the digit check
        // above, and resolved once the target char arrives via the
        // `pending_find` check at the top of `handle_normal_key` (which
        // every key reaches regardless of `pending_op`, `resolve_find`
        // itself branches on it).
        if !self.pending_matcher.is_pending() {
            if let KeyCode::Char(c) = key.code {
                if key.mods == Mods::default() {
                    let find = match c {
                        'f' => Some((true, false)),
                        'F' => Some((false, false)),
                        't' => Some((true, true)),
                        'T' => Some((false, true)),
                        _ => None,
                    };
                    if let Some((forward, till)) = find {
                        let count = self.count.take().unwrap_or(1).max(1);
                        self.pending_find = Some((forward, till, count));
                        return;
                    }
                }
            }
        }

        // vim-surround/evil-surround: `y`/`d`/`c` followed by `s` doesn't
        // mean "yank/delete/change the letter s" (there's no motion/
        // text-object named `s`) -- it starts `ys`/`ds`/`cs` instead.
        // Checked before the doubled-operator check below since `s` can
        // never collide with `op.trigger_key()` (`y`/`d`/`c`).
        if !self.pending_matcher.is_pending() && key == KeyPress::char('s') {
            self.pending_op = None;
            self.pending_surround = Some(match op {
                Operator::Yank => SurroundPending::Target,
                Operator::Delete => SurroundPending::Delete,
                Operator::Change => SurroundPending::ChangeOld,
            });
            return;
        }

        // Doubled operator (dd/cc/yy): linewise, current line(s). Only
        // applies as the very first key after the operator, matching Vim.
        // cc (unlike dd/yy) keeps the line itself -- it only clears the
        // content, leaving a blank line to type into.
        if !self.pending_matcher.is_pending() && key == op.trigger_key() {
            self.pending_op = None;
            let total_count = self.pending_op_count.saturating_mul(self.count.take().unwrap_or(1).max(1));
            let (line, _) = buffer.line_col(cursor);
            let end_line = (line + total_count as usize - 1).min(motion::last_line(buffer));
            let range = if op == Operator::Change {
                lines_content_range(buffer, line, end_line)
            } else {
                linewise_range(buffer, line, end_line)
            };
            self.finish_operator(buffer, cursor, op, range, true);
            return;
        }

        match self.pending_matcher.feed(key) {
            Step::Pending(_) => {}
            Step::NoMatch => {
                self.pending_op = None;
                self.count = None;
            }
            Step::Matched(target) => {
                let target = *target;
                self.pending_op = None;
                let raw_count = self.count.take();
                let total_count = self.pending_op_count.saturating_mul(raw_count.unwrap_or(1).max(1));
                let (range, linewise) = match target {
                    PendingTarget::Motion(m @ (Motion::BufferTop | Motion::BufferBottom)) => {
                        // `d5G`/`5dG`: the count targets an absolute
                        // line, not a repeat count (meaningless for a
                        // jump-to-line motion) -- same distinction
                        // `apply_normal_action` makes for the standalone
                        // form. Prefers a count typed between the
                        // operator and motion (`d5G`) over one typed
                        // before the operator (`5dG`) if both are given.
                        let line_count = raw_count.or((self.pending_op_count != 1).then_some(self.pending_op_count));
                        let dest = motion::buffer_line_target(buffer, m, line_count);
                        let (cur_line, _) = buffer.line_col(cursor);
                        let (dest_line, _) = buffer.line_col(&Cursor { char_idx: dest, sticky_col: 0 });
                        (linewise_range(buffer, cur_line, dest_line), true)
                    }
                    PendingTarget::Motion(m) => {
                        let m = adjust_for_change_word(op, m, buffer, cursor);
                        (range_for_motion(buffer, cursor, m, total_count), m.is_linewise())
                    }
                    // Counts on text objects aren't supported ("2diw" behaves
                    // like "diw") -- a disclosed simplification.
                    PendingTarget::TextObject(obj) => (textobject::span(buffer, cursor, obj), textobject::is_linewise(obj)),
                };
                self.finish_operator(buffer, cursor, op, range, linewise);
            }
        }
    }

    fn finish_operator(
        &mut self,
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        op: Operator,
        range: Range<usize>,
        linewise: bool,
    ) {
        match op {
            Operator::Yank => {
                let text = buffer.text_range(range.start, range.end);
                self.pending_pulse = Some(range.clone());
                self.write_register(text, linewise);
                cursor.char_idx = range.start.min(buffer.len_chars());
                let (_, col) = buffer.line_col(cursor);
                cursor.sticky_col = col;
            }
            Operator::Delete | Operator::Change => {
                let text = buffer.delete_range(cursor, range.start, range.end);
                self.write_register(text, linewise);
                if op == Operator::Change {
                    self.mode = Mode::Insert;
                }
            }
        }
    }

    /// Dispatches a key to whichever `SurroundPending` phase is active --
    /// vim-surround/evil-surround's own `ys`/`ds`/`cs`. See `SurroundPending`'s
    /// own doc comment for what each phase is waiting on.
    fn handle_surround_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, pending: SurroundPending, key: KeyPress) {
        match pending {
            SurroundPending::Target => {
                if key.code == KeyCode::Named(NamedKey::Escape) {
                    self.pending_surround = None;
                    self.pending_matcher.cancel();
                    self.count = None;
                    return;
                }
                // `yss`: doubled form, "surround the whole current line"
                // -- same "doubled key means the current line" shape
                // `dd`/`cc`/`yy` already have, just reached through this
                // pending machine instead of `handle_operator_pending_key`.
                if !self.pending_matcher.is_pending() && key == KeyPress::char('s') {
                    let (line, _) = buffer.line_col(cursor);
                    let start = motion::line_first_non_blank(buffer, line);
                    let end = buffer.line_start_char(line) + buffer.line_len(line);
                    self.pending_surround = Some(SurroundPending::AddChar { start, end });
                    return;
                }
                match self.pending_matcher.feed(key) {
                    Step::Pending(_) => self.pending_surround = Some(SurroundPending::Target),
                    Step::NoMatch => self.pending_surround = None,
                    Step::Matched(target) => {
                        let range = self.surround_target_range(buffer, cursor, *target);
                        self.pending_surround = Some(SurroundPending::AddChar { start: range.start, end: range.end });
                    }
                }
            }
            SurroundPending::AddChar { start, end } => {
                self.pending_surround = None;
                if let KeyCode::Char(c) = key.code {
                    if key.mods == Mods::default() {
                        self.add_surrounding(buffer, cursor, start..end, c);
                    }
                }
            }
            SurroundPending::Delete => {
                self.pending_surround = None;
                if let KeyCode::Char(c) = key.code {
                    if key.mods == Mods::default() {
                        if let Some((open_idx, close_idx)) = self.find_surrounding(buffer, cursor, c) {
                            buffer.delete_range(cursor, close_idx, close_idx + 1);
                            buffer.delete_range(cursor, open_idx, open_idx + 1);
                            cursor.char_idx = open_idx;
                            let (_, col) = buffer.line_col(cursor);
                            cursor.sticky_col = col;
                        }
                    }
                }
            }
            SurroundPending::ChangeOld => {
                self.pending_surround = None;
                if let KeyCode::Char(c) = key.code {
                    if key.mods == Mods::default() {
                        if let Some((open_idx, close_idx)) = self.find_surrounding(buffer, cursor, c) {
                            self.pending_surround = Some(SurroundPending::ChangeNew { open_idx, close_idx });
                        }
                    }
                }
            }
            SurroundPending::ChangeNew { open_idx, close_idx } => {
                self.pending_surround = None;
                if let KeyCode::Char(c) = key.code {
                    if key.mods == Mods::default() {
                        if let Some((open_str, close_str)) = surround_pair_for(c) {
                            buffer.replace_range(cursor, close_idx, close_idx + 1, &close_str);
                            buffer.replace_range(cursor, open_idx, open_idx + 1, &open_str);
                            cursor.char_idx = open_idx;
                            let (_, col) = buffer.line_col(cursor);
                            cursor.sticky_col = col;
                        }
                    }
                }
            }
        }
    }

    /// `ys{motion}`'s target range -- deliberately simpler than the real
    /// operator-pending resolver (`handle_operator_pending_key`'s own
    /// `PendingTarget` match): no `adjust_for_change_word` (a `Change`-
    /// operator-specific nuance for `cw`) and no absolute-line-count
    /// special case for `BufferTop`/`BufferBottom` (a rare target for
    /// surround specifically) -- both skipped as disclosed
    /// simplifications, same posture text objects already have for
    /// counts. `count` is always `1`: `ys3w"` isn't supported, matching
    /// that same simplification.
    fn surround_target_range(&self, buffer: &Buffer, cursor: &Cursor, target: PendingTarget) -> Range<usize> {
        match target {
            PendingTarget::Motion(m) => range_for_motion(buffer, cursor, m, 1),
            PendingTarget::TextObject(obj) => textobject::span(buffer, cursor, obj),
        }
    }

    /// Wraps `range` in whatever `surround_pair_for(c)` resolves to --
    /// shared by `ys{motion}{char}`'s `AddChar` phase and Visual `S`.
    /// Inserts the close delimiter first, then the open one: inserting
    /// at `range.end` first doesn't shift `range.start`, so no offset
    /// math is needed for the second insert. Leaves the cursor at the
    /// (possibly now-shifted-left-by-nothing) start of the wrapped text.
    fn add_surrounding(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, range: Range<usize>, c: char) {
        let Some((open_str, close_str)) = surround_pair_for(c) else { return };
        let mut end_cursor = Cursor { char_idx: range.end, sticky_col: 0 };
        buffer.insert_str(&mut end_cursor, &close_str);
        let mut start_cursor = Cursor { char_idx: range.start, sticky_col: 0 };
        buffer.insert_str(&mut start_cursor, &open_str);
        cursor.char_idx = range.start;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// The (open, close) delimiter positions of the nearest surrounding
    /// pair matching `c` -- `ds`/`cs`'s own lookup, dispatching to
    /// `bracket::enclosing_pair` for a bracket char (or its `b`/`B`
    /// vim-surround alias) or `textobject::quote_positions` for a quote
    /// char. `None` for any other char, or if nothing of that kind
    /// encloses the cursor.
    fn find_surrounding(&self, buffer: &Buffer, cursor: &Cursor, c: char) -> Option<(usize, usize)> {
        match c {
            '(' | ')' | 'b' => bracket::enclosing_pair(buffer, cursor.char_idx, '(', ')'),
            '{' | '}' | 'B' => bracket::enclosing_pair(buffer, cursor.char_idx, '{', '}'),
            '[' | ']' => bracket::enclosing_pair(buffer, cursor.char_idx, '[', ']'),
            '"' | '\'' | '`' => textobject::quote_positions(buffer, cursor, c),
            _ => None,
        }
    }

    /// `gc{motion}`/`gcc`'s own key handler -- resolves to a line range
    /// (`pending_comment_lines`), never touching the buffer directly
    /// (only the host knows the language's comment token). Comment
    /// toggling always operates on whole lines regardless of whether the
    /// motion itself was charwise, matching real Comment.nvim/vim-
    /// commentary (`gcap` comments every line in the paragraph, `gcj`
    /// comments the current+next line) -- so a resolved char range is
    /// converted to `(start_line, end_line)` via `range_to_lines` rather
    /// than kept as-is.
    fn handle_comment_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) {
        if key.code == KeyCode::Named(NamedKey::Escape) {
            self.pending_comment = false;
            self.pending_matcher.cancel();
            self.count = None;
            return;
        }
        // `gcc`: doubled form, current line(s) -- same count-aware shape
        // `dd`/`cc`/`yy`'s own doubled form has (`pending_comment_count`,
        // typed *before* `gc`, combines multiplicatively with whatever's
        // typed between `gc` and this doubled key).
        if !self.pending_matcher.is_pending() && key == KeyPress::char('c') {
            self.pending_comment = false;
            let total_count = self.pending_comment_count.saturating_mul(self.count.take().unwrap_or(1).max(1));
            let (line, _) = buffer.line_col(cursor);
            let end_line = (line + total_count as usize - 1).min(motion::last_line(buffer));
            self.pending_comment_lines = Some((line, end_line));
            return;
        }
        match self.pending_matcher.feed(key) {
            Step::Pending(_) => {}
            Step::NoMatch => {
                self.pending_comment = false;
                self.count = None;
            }
            Step::Matched(target) => {
                let target = *target;
                self.pending_comment = false;
                let total_count = self.pending_comment_count.saturating_mul(self.count.take().unwrap_or(1).max(1));
                let range = match target {
                    PendingTarget::Motion(m) => range_for_motion(buffer, cursor, m, total_count),
                    PendingTarget::TextObject(obj) => textobject::span(buffer, cursor, obj),
                };
                self.pending_comment_lines = Some(range_to_lines(buffer, &range));
            }
        }
    }

    /// Writes `text` into whatever register `"{name}` most recently
    /// selected (consuming that one-shot selection), or the unnamed
    /// register if none was selected -- shared by every yank/delete/
    /// change site above. The unnamed register always ends up holding
    /// this same `text` regardless, matching real Vim: `""` mirrors the
    /// most recent yank/delete even when a named register was also
    /// explicitly targeted.
    fn write_register(&mut self, text: String, linewise: bool) {
        if let Some((name, append)) = self.active_register.take() {
            self.store_in_register(name, &text, linewise, append);
        }
        self.register = Register { text, linewise };
    }

    /// Writes (or, if `append`, appends -- joined by a newline for a
    /// linewise append, matching real Vim's `"Ayy` convention) `text`
    /// into named register `name`. Shared by `write_register` (driven by
    /// a `"{name}` selection) and `finish_recording` (a macro's own
    /// register, already known directly, never going through the `"`-
    /// prefix mechanism at all).
    fn store_in_register(&mut self, name: char, text: &str, linewise: bool, append: bool) {
        let final_text = if append {
            match self.registers.get(&name) {
                Some(existing) if !existing.text.is_empty() => {
                    let mut combined = existing.text.clone();
                    if existing.linewise && !combined.ends_with('\n') {
                        combined.push('\n');
                    }
                    combined.push_str(text);
                    combined
                }
                _ => text.to_string(),
            }
        } else {
            text.to_string()
        };
        self.registers.insert(name, Register { text: final_text, linewise });
    }

    fn paste(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, after: bool, count: u32) {
        let (text, linewise) = match self.active_register.take() {
            Some((name, _append)) => match self.registers.get(&name) {
                Some(r) => (r.text.clone(), r.linewise),
                None => (String::new(), false),
            },
            None => (self.register.text.clone(), self.register.linewise),
        };
        if text.is_empty() {
            return;
        }
        let count = count.max(1) as usize;
        if linewise {
            let mut block = text.clone();
            if !block.ends_with('\n') {
                block.push('\n');
            }
            let block = block.repeat(count);
            let (line, _) = buffer.line_col(cursor);
            let insert_line = if after { line + 1 } else { line };
            let at = if insert_line < buffer.line_count() {
                buffer.line_start_char(insert_line)
            } else {
                buffer.len_chars()
            };
            cursor.char_idx = at;
            buffer.insert_str(cursor, &block);
            self.pending_pulse = Some(at..(at + block.chars().count()));
            // Vim leaves the cursor on the first non-blank of the pasted
            // block's first line, not at column 0 -- matches `^`.
            cursor.char_idx = motion::target(buffer, &Cursor { char_idx: at, sticky_col: 0 }, Motion::LineFirstNonBlank);
        } else {
            let text = text.repeat(count);
            let at = if after { (cursor.char_idx + 1).min(buffer.len_chars()) } else { cursor.char_idx };
            cursor.char_idx = at;
            buffer.insert_str(cursor, &text);
            let inserted_len = text.chars().count();
            self.pending_pulse = Some(at..(at + inserted_len));
            // Charwise paste leaves the cursor on the last pasted char, not
            // the first -- matches Vim (e.g. `yiw` then `p` lands you at
            // the end of the word you just pasted, ready to keep typing).
            cursor.char_idx = at + inserted_len - 1;
        }
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    fn handle_visual_key(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, key: KeyPress) -> VimEvent {
        if self.pending_visual_replace {
            self.pending_visual_replace = false;
            if let KeyCode::Char(c) = key.code {
                self.visual_replace_char(buffer, cursor, c);
            } else if key.code == KeyCode::Named(NamedKey::Escape) {
                // Unlike Normal mode's own pending-replace (already in
                // Normal mode, so Escape just cancels in place), Escape
                // here also exits Visual mode -- matches real Vim, where
                // Escape always returns to Normal mode regardless of what
                // sub-state it interrupts.
                self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                self.mode = Mode::Normal;
            }
            // Any other key (an arrow, a digit, ...) is a silent no-op,
            // same as Normal mode's own pending_replace.
            return VimEvent::None;
        }

        if self.pending_visual_surround {
            self.pending_visual_surround = false;
            if let KeyCode::Char(c) = key.code {
                self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                // Block visual surround isn't supported (matches real
                // vim-surround's own lack of block support) -- `visual_
                // range` itself doesn't handle Block anyway (see its own
                // doc comment), so this guard is required, not optional.
                if self.visual_kind != VisualKind::Block {
                    let (range, _) = self.visual_range(buffer, cursor);
                    self.add_surrounding(buffer, cursor, range, c);
                }
                self.mode = Mode::Normal;
            } else if key.code == KeyCode::Named(NamedKey::Escape) {
                self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                self.mode = Mode::Normal;
            }
            return VimEvent::None;
        }

        if key.code == KeyCode::Named(NamedKey::Escape) {
            self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
            self.mode = Mode::Normal;
            return VimEvent::None;
        }

        if let Some(pending) = self.pending_prefix.take() {
            self.resolve_pending_prefix(pending, key);
            return VimEvent::None;
        }
        // `"{name}` selects which register `d`/`y`/`c` below reads or
        // writes -- a bare, unmodified `"`, never a trie leaf (mirrors
        // `handle_normal_key`'s own treatment of it), since the Visual
        // trie has no notion of "wait for one more key."
        if key.code == KeyCode::Char('"') && key.mods == Mods::default() {
            self.pending_prefix = Some(PendingPrefixKey::Register);
            return VimEvent::None;
        }

        if let Step::Matched(action) = self.visual_matcher.feed(key) {
            match *action {
                // Counts aren't supported in Visual mode -- the selection
                // is already explicit, unlike Normal mode's motions.
                VisualAction::Motion(m) => apply_motion(buffer, cursor, m, 1),
                VisualAction::Apply(op) => {
                    self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                    if self.visual_kind == VisualKind::Block {
                        self.apply_block_operator(buffer, cursor, op);
                    } else {
                        let (range, linewise) = self.visual_range(buffer, cursor);
                        self.finish_operator(buffer, cursor, op, range, linewise);
                    }
                    if self.mode == Mode::Visual {
                        self.mode = Mode::Normal;
                    }
                }
                VisualAction::SetKind(kind) => {
                    if kind == self.visual_kind {
                        self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                        self.mode = Mode::Normal; // same key again: toggle out
                    } else {
                        self.visual_kind = kind; // different kind: switch, keep anchor
                    }
                }
                VisualAction::Indent | VisualAction::Dedent => {
                    self.last_visual = Some((self.visual_kind, self.visual_anchor, cursor.char_idx));
                    // Always linewise, regardless of `visual_kind` --
                    // real Vim's `>`/`<` never restrict to the selected
                    // columns, even in Block mode.
                    let anchor_cursor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
                    let (anchor_line, _) = buffer.line_col(&anchor_cursor);
                    let (cursor_line, _) = buffer.line_col(cursor);
                    let anchor_was_top = anchor_line <= cursor_line;
                    let (line_lo, line_hi) = if anchor_was_top { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) };
                    for l in line_lo..=line_hi {
                        if *action == VisualAction::Indent {
                            indent::indent_line(buffer, cursor, l, self.indent_width);
                        } else {
                            indent::dedent_line(buffer, cursor, l, self.indent_width);
                        }
                    }
                    // Real Vim's own `>`/`<` drop straight back to Normal
                    // mode after one shift -- surprising enough in
                    // practice (a repeated `>>` habit from Normal mode
                    // lands its second press on a bare, incomplete `>`
                    // that does nothing) that most real Vim configs remap
                    // `>`/`<` to `>gv`/`<gv`: reselect the same range
                    // immediately after shifting it, so pressing the key
                    // again stacks another level with the selection
                    // staying visibly highlighted throughout. Built in
                    // here rather than left as the vanilla default.
                    //
                    // Indenting only ever changes a line's *leading*
                    // whitespace, never which lines exist, so the
                    // reselect doesn't need real Vim's saved `'<`/`'>`
                    // marks to reconstruct the range -- `line_lo`/
                    // `line_hi` are still exactly the right line numbers;
                    // only their fresh first-non-blank char offsets need
                    // recomputing; anchor and cursor swap back onto
                    // whichever end they started on, so a second press
                    // keeps extending the same direction rather than
                    // flipping.
                    let lo_first_non_blank = motion::line_first_non_blank(buffer, line_lo);
                    let hi_first_non_blank = motion::line_first_non_blank(buffer, line_hi);
                    let (new_anchor, new_cursor) = if anchor_was_top {
                        (lo_first_non_blank, hi_first_non_blank)
                    } else {
                        (hi_first_non_blank, lo_first_non_blank)
                    };
                    self.visual_anchor = new_anchor;
                    cursor.char_idx = new_cursor;
                    let (_, col) = buffer.line_col(cursor);
                    cursor.sticky_col = col;
                }
                VisualAction::BlockInsertLeft => {
                    if self.visual_kind == VisualKind::Block {
                        let (line_lo, line_hi, col_lo, _) = self.block_bounds(buffer, cursor);
                        cursor.char_idx = buffer.line_start_char(line_lo) + col_lo.min(buffer.line_len(line_lo));
                        self.block_insert = Some(BlockInsert { line_lo, line_hi, col: col_lo, typed: String::new() });
                        self.mode = Mode::Insert;
                        let (_, col) = buffer.line_col(cursor);
                        cursor.sticky_col = col;
                    }
                }
                VisualAction::ReplaceChar => {
                    self.pending_visual_replace = true;
                }
                VisualAction::Surround => {
                    self.pending_visual_surround = true;
                }
                VisualAction::ChangeCase(mode) => {
                    self.visual_change_case(buffer, cursor, mode);
                }
            }
            // Same one-shot reset as `handle_normal_key`'s own
            // `Step::Matched` arm -- a register selected via `"{name}`
            // that this action didn't consume (anything but `Apply`)
            // doesn't linger into the next command. Harmless no-op for
            // `Apply` itself, which already consumed it via
            // `write_register`/`apply_block_operator`.
            self.active_register = None;
        }
        VimEvent::None
    }

    /// The char range (and whether it's linewise) the current selection
    /// covers, for Char/Line kinds. Block has its own per-row handling in
    /// `apply_block_operator` (a single contiguous range can't represent a
    /// column rectangle), so this is never called for it.
    fn visual_range(&self, buffer: &Buffer, cursor: &Cursor) -> (Range<usize>, bool) {
        match self.visual_kind {
            VisualKind::Line => {
                let anchor_cursor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
                let (anchor_line, _) = buffer.line_col(&anchor_cursor);
                let (cursor_line, _) = buffer.line_col(cursor);
                (linewise_range(buffer, anchor_line, cursor_line), true)
            }
            VisualKind::Char | VisualKind::Block => {
                let (lo, hi) = if self.visual_anchor <= cursor.char_idx {
                    (self.visual_anchor, cursor.char_idx + 1)
                } else {
                    (cursor.char_idx, self.visual_anchor + 1)
                };
                (lo..hi.min(buffer.len_chars()), false)
            }
        }
    }

    /// (line_lo, line_hi, col_lo, col_hi_exclusive) of the Block selection
    /// rectangle spanned by the anchor and the cursor.
    fn block_bounds(&self, buffer: &Buffer, cursor: &Cursor) -> (usize, usize, usize, usize) {
        let anchor_cursor = Cursor { char_idx: self.visual_anchor, sticky_col: 0 };
        let (anchor_line, anchor_col) = buffer.line_col(&anchor_cursor);
        let (cursor_line, cursor_col) = buffer.line_col(cursor);
        let (line_lo, line_hi) =
            if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) };
        let (col_lo, col_hi) =
            if anchor_col <= cursor_col { (anchor_col, cursor_col + 1) } else { (cursor_col, anchor_col + 1) };
        (line_lo, line_hi, col_lo, col_hi)
    }

    /// `d`/`y`/`c` on a Block selection: acts on the same column range on
    /// every line in the block, clamped to each (possibly ragged) line's
    /// length. Simplification, disclosed in the plan: this is N separate
    /// undo steps (one delete_range per line), not one atomic block edit,
    /// and the register just joins the per-line pieces with '\n' -- paste
    /// (`p`/`P`) won't reconstruct the block shape, only Delete/Yank
    /// themselves are block-aware.
    fn apply_block_operator(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, op: Operator) {
        let (line_lo, line_hi, col_lo, col_hi) = self.block_bounds(buffer, cursor);
        let mut pieces = vec![String::new(); line_hi - line_lo + 1];

        if op == Operator::Yank {
            for line in line_lo..=line_hi {
                let start = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let seg_start = start + col_lo.min(len);
                let seg_end = start + col_hi.min(len);
                if seg_start < seg_end {
                    pieces[line - line_lo] = buffer.text_range(seg_start, seg_end);
                }
            }
        } else {
            // Bottom to top so deleting a lower line never invalidates the
            // char offsets of lines above it that haven't been processed yet.
            for line in (line_lo..=line_hi).rev() {
                let start = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let seg_start = start + col_lo.min(len);
                let seg_end = start + col_hi.min(len);
                if seg_start < seg_end {
                    pieces[line - line_lo] = buffer.delete_range(cursor, seg_start, seg_end);
                }
            }
        }

        self.write_register(pieces.join("\n"), false);
        cursor.char_idx = buffer.line_start_char(line_lo) + col_lo.min(buffer.line_len(line_lo));
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;

        if op == Operator::Change {
            self.mode = Mode::Insert;
        }
    }
}

impl Default for VimState {
    fn default() -> Self {
        Self::new()
    }
}

/// `,`'s own view of `;`: the opposite-direction counterpart of a
/// resolved find motion, for `RepeatFind { reverse: true }`. Motions
/// other than the `FindChar`-family pass through unchanged (can't
/// happen in practice -- `last_find` only ever stores one of the four).
fn reverse_find_motion(m: Motion) -> Motion {
    match m {
        Motion::FindChar(c) => Motion::FindCharBack(c),
        Motion::FindCharBack(c) => Motion::FindChar(c),
        Motion::TillChar(c) => Motion::TillCharBack(c),
        Motion::TillCharBack(c) => Motion::TillChar(c),
        other => other,
    }
}

fn apply_motion(buffer: &Buffer, cursor: &mut Cursor, m: Motion, count: u32) {
    for _ in 0..count.max(1) {
        let target = motion::target(buffer, cursor, m);
        if target == cursor.char_idx {
            break; // no progress (already at a boundary) -- stop instead of spinning
        }
        cursor.char_idx = target;
        if !matches!(m, Motion::Up | Motion::Down) {
            let (_, col) = buffer.line_col(cursor);
            cursor.sticky_col = col;
        }
    }
}

/// Where `count` repeated applications of `m` from `cursor` would land,
/// without mutating `cursor` -- used to compute an operator's range
/// (`3dw`, `d3w`) without moving the cursor before the delete/change/yank
/// actually happens.
fn motion_target_repeated(buffer: &Buffer, cursor: &Cursor, m: Motion, count: u32) -> usize {
    let mut probe = *cursor;
    apply_motion(buffer, &mut probe, m, count);
    probe.char_idx
}

/// Vim's well-known `cw`-behaves-like-`ce` rule: changing a word from a
/// non-blank char shouldn't also swallow the trailing whitespace the way
/// plain `dw` would, since you're about to type a replacement right after
/// the word.
fn adjust_for_change_word(op: Operator, motion: Motion, buffer: &Buffer, cursor: &Cursor) -> Motion {
    if op == Operator::Change && motion == Motion::WordForward && motion::is_non_blank_at(buffer, cursor.char_idx) {
        Motion::WordEndForward
    } else {
        motion
    }
}

/// The char range (relative to `chars`, a single line's own characters)
/// of the nearest run of ASCII digits at or after `from_col` -- `Ctrl-a`/
/// `Ctrl-x`'s own target-finding. If `from_col` already sits inside a
/// digit run, that run is what's returned (backs up to its start first)
/// rather than the *next* one, matching real Vim: incrementing with the
/// cursor mid-number still targets that number. A `-` immediately before
/// the run is included (a negative number, not a separate token). `None`
/// if there's no digit from `from_col` onward.
fn find_number(chars: &[char], from_col: usize) -> Option<(usize, usize)> {
    let len = chars.len();
    let mut i = from_col.min(len);
    if i < len && chars[i].is_ascii_digit() {
        while i > 0 && chars[i - 1].is_ascii_digit() {
            i -= 1;
        }
    } else {
        while i < len && !chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i >= len || !chars[i].is_ascii_digit() {
        return None;
    }
    let start = if i > 0 && chars[i - 1] == '-' { i - 1 } else { i };
    let mut end = i;
    while end < len && chars[end].is_ascii_digit() {
        end += 1;
    }
    Some((start, end))
}

/// The (open, close) insertion strings vim-surround/evil-surround uses
/// for a given surround char -- `(`/`{`/`[` (the *opening*-bracket
/// spelling) get inner padding, matching real vim-surround's own
/// convention that spells "with a space inside" by typing the open
/// delimiter and "tight, no space" by typing the close one; `b`/`B` are
/// vim-surround's own aliases for `()`/`{}`. `None` for any other char --
/// callers treat that as "not a recognized surround char," a silent
/// no-op same posture as an unrecognized mark name.
fn surround_pair_for(c: char) -> Option<(String, String)> {
    match c {
        '(' => Some(("( ".to_string(), " )".to_string())),
        ')' | 'b' => Some(("(".to_string(), ")".to_string())),
        '{' => Some(("{ ".to_string(), " }".to_string())),
        '}' | 'B' => Some(("{".to_string(), "}".to_string())),
        '[' => Some(("[ ".to_string(), " ]".to_string())),
        ']' => Some(("[".to_string(), "]".to_string())),
        '"' => Some(("\"".to_string(), "\"".to_string())),
        '\'' => Some(("'".to_string(), "'".to_string())),
        '`' => Some(("`".to_string(), "`".to_string())),
        _ => None,
    }
}

/// Which lines `range` (an ordinary char range from a resolved motion or
/// text object) touches -- `gc{motion}`'s own "always whole lines"
/// conversion. Uses `range.end.saturating_sub(1)` (the last *included*
/// char), not `range.end` itself, so an exclusive range that lands
/// exactly on the next line's first char (e.g. a linewise motion, or
/// `w` stopping at the start of a following line) doesn't spuriously
/// pull that next line in as "touched." An empty range still resolves
/// to its own single line.
fn range_to_lines(buffer: &Buffer, range: &Range<usize>) -> (usize, usize) {
    let start_char = range.start.min(buffer.len_chars().saturating_sub(1));
    let end_char = range.end.saturating_sub(1).max(range.start).min(buffer.len_chars().saturating_sub(1));
    let (start_line, _) = buffer.line_col(&Cursor { char_idx: start_char, sticky_col: 0 });
    let (end_line, _) = buffer.line_col(&Cursor { char_idx: end_char, sticky_col: 0 });
    (start_line, end_line.max(start_line))
}

/// What `~`/`u`/`U` (Normal mode's single char and Visual mode's own
/// `ChangeCase`) turn `c` into. Takes only the *first* char of Rust's
/// `to_uppercase`/`to_lowercase` iterator -- lossy for the rare
/// multi-char mappings (German `ß` -> `"SS"`), a disclosed
/// simplification: every call site here replaces exactly one char with
/// the result, so a multi-char mapping would desync buffer indices.
fn apply_case(c: char, mode: CaseChange) -> char {
    match mode {
        CaseChange::Toggle => {
            if c.is_uppercase() {
                c.to_lowercase().next().unwrap_or(c)
            } else if c.is_lowercase() {
                c.to_uppercase().next().unwrap_or(c)
            } else {
                c
            }
        }
        CaseChange::Upper => c.to_uppercase().next().unwrap_or(c),
        CaseChange::Lower => c.to_lowercase().next().unwrap_or(c),
    }
}

fn linewise_range(buffer: &Buffer, line_a: usize, line_b: usize) -> Range<usize> {
    let (lo, hi) = if line_a <= line_b { (line_a, line_b) } else { (line_b, line_a) };
    let start = buffer.line_start_char(lo);
    let end =
        if hi + 1 < buffer.line_count() { buffer.line_start_char(hi + 1) } else { buffer.len_chars() };
    start..end
}

/// `line_a..line_b`'s content, excluding the *last* line's terminator --
/// unlike `linewise_range`, deleting this collapses the lines into one
/// (now empty, if `line_a == line_b`) instead of removing them entirely.
/// `cc`/`S` want this: change replaces content but doesn't remove the
/// line(s), so a blank line remains for the user to type into.
fn lines_content_range(buffer: &Buffer, line_a: usize, line_b: usize) -> Range<usize> {
    let (lo, hi) = if line_a <= line_b { (line_a, line_b) } else { (line_b, line_a) };
    let start = buffer.line_start_char(lo);
    let end = buffer.line_start_char(hi) + buffer.line_len(hi);
    start..end
}

fn range_for_motion(buffer: &Buffer, cursor: &Cursor, motion: Motion, count: u32) -> Range<usize> {
    let target = motion_target_repeated(buffer, cursor, motion, count);
    if motion.is_linewise() {
        let (cur_line, _) = buffer.line_col(cursor);
        let (tgt_line, _) = buffer.line_col(&Cursor { char_idx: target, sticky_col: 0 });
        return linewise_range(buffer, cur_line, tgt_line);
    }
    let cur = cursor.char_idx;
    let (lo, hi) = if cur <= target { (cur, target) } else { (target, cur) };
    let hi = match motion.inclusivity() {
        motion::Inclusivity::Inclusive => (hi + 1).min(buffer.len_chars()),
        motion::Inclusivity::Exclusive => hi,
    };
    lo..hi
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::buf;

    fn keys(s: &mut VimState, b: &mut Buffer, c: &mut Cursor, seq: &str) {
        for ch in seq.chars() {
            s.handle_key(b, c, KeyPress::char(ch));
        }
    }

    fn named(s: &mut VimState, b: &mut Buffer, c: &mut Cursor, n: NamedKey) -> VimEvent {
        s.handle_key(b, c, KeyPress::named(n))
    }

    #[test]
    fn starts_in_normal_mode() {
        let vim = VimState::new();
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn hjkl_move_the_cursor() {
        let mut b = buf("ab\ncd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1);
        keys(&mut vim, &mut b, &mut c, "j");
        assert_eq!(b.line_col(&c), (1, 1));
        keys(&mut vim, &mut b, &mut c, "h");
        assert_eq!(b.line_col(&c), (1, 0));
        keys(&mut vim, &mut b, &mut c, "k");
        assert_eq!(b.line_col(&c), (0, 0));
    }

    #[test]
    fn i_enters_insert_mode_and_typing_inserts() {
        let mut b = buf("bc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "a");
        assert_eq!(b.text(), "abc");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn escape_from_insert_moves_cursor_back_one_column() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "hi");
        assert_eq!(c.char_idx, 2);
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(c.char_idx, 1); // on the 'i', not past it
    }

    #[test]
    fn tab_in_insert_mode_inserts_spaces_to_the_next_stop() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Tab);
        assert_eq!(b.text(), "    "); // column 0 -> full 4 spaces
        keys(&mut vim, &mut b, &mut c, "xy"); // now at column 6
        named(&mut vim, &mut b, &mut c, NamedKey::Tab);
        assert_eq!(b.text(), "    xy  "); // column 6 -> 2 spaces to reach 8
    }

    #[test]
    fn enter_in_insert_mode_carries_over_the_previous_lines_indentation() {
        let mut b = buf("    foo");
        let mut c = Cursor { char_idx: 7, sticky_col: 7 }; // end of "foo"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "    foo\n    ");
        keys(&mut vim, &mut b, &mut c, "bar");
        assert_eq!(b.text(), "    foo\n    bar");
    }

    #[test]
    fn enter_continues_a_bulleted_list() {
        let mut b = buf("- first");
        let mut c = Cursor { char_idx: 7, sticky_col: 7 }; // end of "first"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "- first\n- ");
        keys(&mut vim, &mut b, &mut c, "second");
        assert_eq!(b.text(), "- first\n- second");
    }

    #[test]
    fn enter_continues_an_ordered_list_incrementing_the_number() {
        let mut b = buf("3. third");
        let mut c = Cursor { char_idx: 8, sticky_col: 8 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "3. third\n4. ");
    }

    #[test]
    fn enter_on_a_nested_bullet_preserves_its_indent() {
        let mut b = buf("  - nested");
        let mut c = Cursor { char_idx: 10, sticky_col: 10 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "  - nested\n  - ");
    }

    #[test]
    fn enter_continues_a_checkbox_item_unchecked_regardless_of_the_original_state() {
        let mut b = buf("- [x] done");
        let mut c = Cursor { char_idx: 10, sticky_col: 10 }; // end of "done"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "- [x] done\n- [ ] ");
    }

    #[test]
    fn enter_on_an_empty_list_item_leaves_the_list_instead_of_repeating_the_marker() {
        let mut b = buf("- ");
        let mut c = Cursor { char_idx: 2, sticky_col: 2 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "- \n");
    }

    #[test]
    fn enter_in_the_middle_of_list_text_still_continues_the_marker() {
        // Splitting mid-line (not necessarily at the end) should behave
        // the same as a plain Enter split: the marker continues onto
        // whatever's left after the cursor.
        let mut b = buf("- hello world");
        let mut c = Cursor { char_idx: 7, sticky_col: 7 }; // between "hello" and " world"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "- hello\n-  world");
    }

    #[test]
    fn open_line_below_continues_a_list_the_same_way_enter_does() {
        let mut b = buf("- first");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "o");
        assert_eq!(b.text(), "- first\n- ");
        assert_eq!(vim.mode(), Mode::Insert);
    }

    #[test]
    fn open_line_above_does_not_continue_a_list() {
        // A disclosed scope cut, not an oversight -- see `NewlineBelow`'s
        // own doc comment on why `O` stays out of this.
        let mut b = buf("- first");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "O");
        assert_eq!(b.text(), "\n- first");
    }

    #[test]
    fn enter_on_an_ordinary_line_is_unaffected_by_list_continuation() {
        let mut b = buf("just text");
        let mut c = Cursor { char_idx: 9, sticky_col: 9 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "just text\n");
    }

    #[test]
    fn enter_after_an_opening_brace_bumps_indent_one_level() {
        let mut b = buf("fn main() {");
        let mut c = Cursor { char_idx: 11, sticky_col: 11 }; // right after "{"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "fn main() {\n    ");
    }

    #[test]
    fn enter_after_an_indented_opening_brace_bumps_one_level_further() {
        let mut b = buf("    if x {");
        let mut c = Cursor { char_idx: 10, sticky_col: 10 }; // right after "{"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "    if x {\n        "); // 4 carried + 4 bumped
    }

    #[test]
    fn enter_between_an_empty_bracket_pair_splits_the_close_bracket_onto_its_own_dedented_line() {
        let mut b = buf("    if x {}");
        let mut c = Cursor { char_idx: 10, sticky_col: 10 }; // between "{" and "}"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "    if x {\n        \n    }");
        // Cursor rests on the middle (bumped-indent) line, not on the
        // close bracket's own line.
        assert_eq!(c.char_idx, "    if x {\n        ".chars().count());
    }

    #[test]
    fn enter_between_an_empty_bracket_pair_at_top_level_dedents_the_close_bracket_to_column_zero() {
        let mut b = buf("if x {}");
        let mut c = Cursor { char_idx: 6, sticky_col: 6 }; // between "{" and "}"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "if x {\n    \n}");
    }

    #[test]
    fn enter_between_mismatched_brackets_does_not_split() {
        // The char right after the cursor isn't the *matching* close for
        // the open bracket right before it -- e.g. `{)` -- so this isn't
        // an empty-pair split, just an ordinary bumped-indent Enter.
        let mut b = buf("foo {)");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 }; // between "{" and ")"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "foo {\n    )");
    }

    #[test]
    fn enter_before_a_close_bracket_not_immediately_after_an_open_one_does_not_split() {
        // Cursor sits right before "}" but the char *before* the cursor
        // isn't an opening bracket -- e.g. mid-content -- so this is
        // ordinary Enter, not the empty-pair-split case.
        let mut b = buf("{ foo}");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 }; // between "foo" and "}"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(b.text(), "{ foo\n}");
    }

    #[test]
    fn o_and_shift_o_carry_over_indentation() {
        let mut b = buf("    foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "o");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "bar");
        assert_eq!(b.text(), "    foo\n    bar");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);

        let mut b2 = buf("    foo");
        let mut c2 = Cursor::at_start();
        let mut vim2 = VimState::new();
        keys(&mut vim2, &mut b2, &mut c2, "O");
        keys(&mut vim2, &mut b2, &mut c2, "baz");
        assert_eq!(b2.text(), "    baz\n    foo");
    }

    #[test]
    fn typing_a_closing_brace_as_first_char_dedents_the_line() {
        let mut b = buf("fn main() {\n        ");
        let mut c = Cursor { char_idx: 20, sticky_col: 8 }; // end of the indented blank line
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "}");
        assert_eq!(b.text(), "fn main() {\n    }");
    }

    #[test]
    fn typing_a_closing_brace_mid_line_does_not_dedent() {
        let mut b = buf("    foo");
        let mut c = Cursor { char_idx: 7, sticky_col: 7 }; // end of "foo", real content precedes it
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, ")");
        assert_eq!(b.text(), "    foo)");
    }

    #[test]
    fn typing_an_opening_bracket_auto_inserts_its_close_and_leaves_the_cursor_between() {
        for (open, pair) in [('(', "()"), ('{', "{}"), ('[', "[]")] {
            let mut b = buf("");
            let mut c = Cursor::at_start();
            let mut vim = VimState::new();
            keys(&mut vim, &mut b, &mut c, "i");
            vim.handle_key(&mut b, &mut c, KeyPress::char(open));
            assert_eq!(b.text(), pair);
            assert_eq!(c.char_idx, 1); // right after the opening bracket, not the close
        }
    }

    #[test]
    fn typing_inside_an_auto_paired_bracket_lands_between_the_pair() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "(hi");
        assert_eq!(b.text(), "(hi)");
        assert_eq!(c.char_idx, 3);
    }

    #[test]
    fn typing_the_matching_close_bracket_types_through_instead_of_duplicating() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "(hi)"); // the ')' should type through the auto-inserted one
        assert_eq!(b.text(), "(hi)");
        assert_eq!(c.char_idx, 4);
    }

    #[test]
    fn typing_through_a_bracket_that_was_not_auto_inserted_still_just_steps_over_it() {
        // Structural, not provenance-tracked -- an existing ")" right at
        // the cursor is skipped over the same way an auto-inserted one
        // would be.
        let mut b = buf(")");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, ")");
        assert_eq!(b.text(), ")");
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn backspace_between_an_empty_auto_paired_bracket_removes_both() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        vim.handle_key(&mut b, &mut c, KeyPress::char('{'));
        assert_eq!(b.text(), "{}");
        named(&mut vim, &mut b, &mut c, NamedKey::Backspace);
        assert_eq!(b.text(), "");
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn backspace_with_content_between_the_pair_only_removes_one_char() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "(x");
        assert_eq!(b.text(), "(x)");
        named(&mut vim, &mut b, &mut c, NamedKey::Backspace); // removes "x", not the brackets
        assert_eq!(b.text(), "()");
    }

    #[test]
    fn replace_mode_neither_auto_pairs_nor_types_through_nor_merges_backspace() {
        // `R` doesn't have a Normal-mode binding yet (a pre-existing,
        // separate gap -- `Mode::Replace` exists and `handle_insert_key`
        // already branches on its own `replace` bool, just nothing
        // transitions into it today), so this drives `handle_insert_key`
        // directly with `replace: true` rather than going through
        // `handle_key`/a key sequence.
        let mut b = buf("(a)");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_insert_key(&mut b, &mut c, KeyPress::char('('), true);
        assert_eq!(b.text(), "(a)"); // overwrote '(' with '(', no auto-pair inserted
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn f_moves_to_the_next_occurrence_of_the_char() {
        let mut b = buf("abcXdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "fX");
        assert_eq!(c.char_idx, 3);
    }

    #[test]
    fn t_stops_just_before_the_char() {
        let mut b = buf("abcXdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "tX");
        assert_eq!(c.char_idx, 2);
    }

    #[test]
    fn count_before_f_finds_the_nth_occurrence() {
        let mut b = buf("aXbXcXd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "2fX");
        assert_eq!(c.char_idx, 3); // the second X
    }

    #[test]
    fn semicolon_repeats_the_last_find_and_comma_reverses_it() {
        let mut b = buf("aXbXcXd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "fX"); // first X, index 1
        keys(&mut vim, &mut b, &mut c, ";"); // next X, index 3
        assert_eq!(c.char_idx, 3);
        keys(&mut vim, &mut b, &mut c, ","); // reversed -- back to index 1
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn dfx_deletes_up_to_and_including_the_found_char() {
        let mut b = buf("abcXdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dfX");
        assert_eq!(b.text(), "def");
    }

    #[test]
    fn dtx_deletes_up_to_but_not_including_the_found_char() {
        let mut b = buf("abcXdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dtX");
        assert_eq!(b.text(), "Xdef");
    }

    #[test]
    fn find_with_no_match_on_the_line_is_a_no_op_and_does_not_leave_pending_state() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "fZ");
        assert_eq!(c.char_idx, 0);
        // A follow-up ordinary motion still works -- confirms `pending_find`
        // didn't leak and swallow the next keypress.
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn escape_after_f_cancels_a_pending_operator_too() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "df");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "l"); // proves we're back in plain Normal mode
        assert_eq!(b.text(), "abc");
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn percent_jumps_to_the_matching_bracket_and_composes_with_delete() {
        let mut b = buf("(hello)");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "%");
        assert_eq!(c.char_idx, 6);

        let mut b2 = buf("(hello) world");
        let mut c2 = Cursor::at_start();
        let mut vim2 = VimState::new();
        keys(&mut vim2, &mut b2, &mut c2, "d%");
        assert_eq!(b2.text(), " world");
    }

    #[test]
    fn paragraph_motions_navigate_between_blank_lines() {
        let mut b = buf("a\nb\n\nc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "}");
        assert_eq!(b.line_col(&c).0, 2);
        keys(&mut vim, &mut b, &mut c, "{");
        assert_eq!(b.line_col(&c).0, 0);
    }

    #[test]
    fn slash_search_jumps_to_the_next_match_and_enters_normal_mode_on_enter() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        assert_eq!(vim.mode(), Mode::Search);
        keys(&mut vim, &mut b, &mut c, "foo");
        assert_eq!(vim.search_query(), "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(c.char_idx, 8); // the second "foo", not the one under the cursor
    }

    #[test]
    fn question_mark_search_goes_backward() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor { char_idx: 10, sticky_col: 10 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "?");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(c.char_idx, 8);
    }

    #[test]
    fn escape_cancels_the_search_prompt_without_moving_the_cursor() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "bar");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn n_and_shift_n_repeat_the_last_search_same_and_reversed() {
        let mut b = buf("foo bar foo baz foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(c.char_idx, 8); // second "foo"

        keys(&mut vim, &mut b, &mut c, "n");
        assert_eq!(c.char_idx, 16); // third "foo"

        keys(&mut vim, &mut b, &mut c, "N"); // reversed -- back to the second
        assert_eq!(c.char_idx, 8);
    }

    #[test]
    fn a_confirmed_search_that_moves_the_cursor_records_a_jump() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/foo");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(ev, VimEvent::JumpRecorded(0));
    }

    #[test]
    fn n_and_shift_n_repeat_a_search_without_recording_a_new_jump() {
        // Real Vim doesn't add `n`/`N` to the jumplist either -- they
        // repeat whatever the last recorded jump already was.
        let mut b = buf("foo bar foo baz foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);

        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('n'));
        assert_eq!(ev, VimEvent::None);
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('N'));
        assert_eq!(ev, VimEvent::None);
    }

    #[test]
    fn star_searches_the_word_under_the_cursor_forward() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "*");
        assert_eq!(c.char_idx, 8);
    }

    #[test]
    fn star_that_moves_the_cursor_records_a_jump() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('*'));
        assert_eq!(ev, VimEvent::JumpRecorded(0));
    }

    #[test]
    fn hash_searches_the_word_under_the_cursor_backward() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor { char_idx: 8, sticky_col: 8 }; // on the second "foo"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "#");
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn star_on_whitespace_is_a_no_op() {
        let mut b = buf("   ");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "*");
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn star_on_whitespace_records_no_jump() {
        let mut b = buf("   ");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('*'));
        assert_eq!(ev, VimEvent::None);
    }

    #[test]
    fn preview_match_finds_the_next_match_without_moving_the_cursor_or_recording_last_search() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        assert_eq!(vim.preview_match(&b, &c), Some(8));
        // Neither the real cursor nor last_search were touched yet.
        assert_eq!(c.char_idx, 0);
        assert_eq!(vim.last_search_pattern(), None);
        assert!(!vim.hlsearch_active());
    }

    #[test]
    fn preview_match_is_none_outside_search_mode_or_with_an_empty_query() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.preview_match(&b, &c), None); // Normal mode
        keys(&mut vim, &mut b, &mut c, "/");
        assert_eq!(vim.preview_match(&b, &c), None); // Search mode, empty query
    }

    #[test]
    fn confirming_a_search_activates_hlsearch_and_records_the_pattern() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(!vim.hlsearch_active());
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert!(vim.hlsearch_active());
        assert_eq!(vim.last_search_pattern(), Some("foo"));
    }

    #[test]
    fn an_edit_clears_hlsearch_but_not_last_search_pattern() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert!(vim.hlsearch_active());

        keys(&mut vim, &mut b, &mut c, "x"); // a real edit
        assert!(!vim.hlsearch_active());
        assert_eq!(vim.last_search_pattern(), Some("foo")); // n/N still work afterward
    }

    #[test]
    fn hlsearch_matches_lists_every_occurrence_in_the_given_byte_range_once_active() {
        let mut b = buf("foo bar foo baz foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);

        let matches = vim.hlsearch_matches(&b, 0..b.text().len());
        assert_eq!(matches, vec![0..3, 8..11, 16..19]);
    }

    #[test]
    fn hlsearch_matches_is_empty_before_any_search_or_after_hlsearch_is_cleared() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(vim.hlsearch_matches(&b, 0..b.text().len()).is_empty());

        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        keys(&mut vim, &mut b, &mut c, "x"); // clears hlsearch_active
        assert!(vim.hlsearch_matches(&b, 0..b.text().len()).is_empty());
    }

    #[test]
    fn a_non_editing_motion_does_not_clear_hlsearch() {
        let mut b = buf("foo bar foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "/");
        keys(&mut vim, &mut b, &mut c, "foo");
        named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert!(vim.hlsearch_active());

        keys(&mut vim, &mut b, &mut c, "l"); // plain motion, no edit
        assert!(vim.hlsearch_active());
    }

    #[test]
    fn dw_deletes_word_forward() {
        let mut b = buf("foo bar baz");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dw");
        assert_eq!(b.text(), "bar baz");
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn dd_deletes_the_whole_line_including_newline() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dd");
        assert_eq!(b.text(), "one\nthree");
    }

    #[test]
    fn cc_clears_the_line_but_keeps_it_unlike_dd() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "cc");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "one\n\nthree");
    }

    #[test]
    fn diw_deletes_inner_word() {
        let mut b = buf("foo bar baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 }; // inside "bar"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "diw");
        assert_eq!(b.text(), "foo  baz");
    }

    #[test]
    fn daw_deletes_a_word_with_trailing_space() {
        let mut b = buf("foo bar baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "daw");
        assert_eq!(b.text(), "foo baz");
    }

    #[test]
    fn di_paren_deletes_inside_brackets_from_anywhere_inside() {
        let mut b = buf("foo(bar)baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 }; // on 'a' inside "bar"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "di(");
        assert_eq!(b.text(), "foo()baz");
    }

    #[test]
    fn ci_quote_changes_the_quoted_text() {
        let mut b = buf(r#"say "hello" now"#);
        let mut c = Cursor { char_idx: 6, sticky_col: 6 }; // inside "hello"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "ci\"");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), r#"say "" now"#);
    }

    #[test]
    fn i_brace_alias_b_and_shift_b_reach_the_same_text_object() {
        let mut b1 = buf("x{y}z");
        let mut c1 = Cursor { char_idx: 2, sticky_col: 2 };
        let mut vim1 = VimState::new();
        keys(&mut vim1, &mut b1, &mut c1, "diB");
        assert_eq!(b1.text(), "x{}z");
    }

    #[test]
    fn dap_removes_a_paragraph_and_its_trailing_blank_line() {
        let mut b = buf("a\nb\n\nc\n");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dap");
        assert_eq!(b.text(), "c\n");
    }

    // -- Surround (`ys`/`ds`/`cs`, Visual `S`) --------------------------

    #[test]
    fn ysiw_quote_wraps_the_inner_word_in_quotes() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start(); // on "hello"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "ysiw\"");
        assert_eq!(b.text(), "\"hello\" world");
        assert_eq!(vim.mode(), Mode::Normal); // ys never enters Insert
    }

    #[test]
    fn ysiw_open_paren_adds_inner_padding_close_paren_does_not() {
        let mut open = buf("hello world");
        let mut oc = Cursor::at_start();
        let mut ovim = VimState::new();
        keys(&mut ovim, &mut open, &mut oc, "ysiw(");
        assert_eq!(open.text(), "( hello ) world");

        let mut close = buf("hello world");
        let mut cc = Cursor::at_start();
        let mut cvim = VimState::new();
        keys(&mut cvim, &mut close, &mut cc, "ysiw)");
        assert_eq!(close.text(), "(hello) world");
    }

    #[test]
    fn yss_surrounds_the_whole_line() {
        let mut b = buf("  hello world  ");
        let mut c = Cursor { char_idx: 4, sticky_col: 4 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yss)");
        assert_eq!(b.text(), "  (hello world  )");
    }

    #[test]
    fn ds_quote_removes_the_nearest_quote_pair() {
        let mut b = buf(r#"say "hello" now"#);
        let mut c = Cursor { char_idx: 6, sticky_col: 6 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "ds\"");
        assert_eq!(b.text(), "say hello now");
    }

    #[test]
    fn ds_paren_removes_the_enclosing_parens() {
        let mut b = buf("foo(bar)baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "ds(");
        assert_eq!(b.text(), "foobarbaz");
    }

    #[test]
    fn cs_quote_to_single_quote_swaps_the_delimiters() {
        let mut b = buf(r#"say "hello" now"#);
        let mut c = Cursor { char_idx: 6, sticky_col: 6 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "cs\"'");
        assert_eq!(b.text(), "say 'hello' now");
    }

    #[test]
    fn cs_paren_to_brace_swaps_bracket_kind() {
        let mut b = buf("foo(bar)baz");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "cs({");
        assert_eq!(b.text(), "foo{ bar }baz");
    }

    #[test]
    fn ds_with_no_enclosing_pair_is_a_no_op() {
        let mut b = buf("plain text");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "ds\"");
        assert_eq!(b.text(), "plain text");
    }

    #[test]
    fn visual_shift_s_surrounds_the_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vll"); // selects "hel"
        vim.handle_key(&mut b, &mut c, KeyPress::char('S'));
        vim.handle_key(&mut b, &mut c, KeyPress::char('"'));
        assert_eq!(b.text(), "\"hel\"lo world");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_tilde_toggles_case_of_the_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vll~"); // selects+toggles "hel"
        assert_eq!(b.text(), "HELlo world");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_shift_u_uppercases_the_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vllU");
        assert_eq!(b.text(), "HELlo world");
    }

    #[test]
    fn visual_lowercase_u_lowercases_the_selection() {
        let mut b = buf("HELLO WORLD");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vllu");
        assert_eq!(b.text(), "helLO WORLD");
    }

    #[test]
    fn visual_line_change_case_leaves_the_newline_alone() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "VU");
        assert_eq!(b.text(), "ABC\ndef");
    }

    #[test]
    fn visual_block_change_case_only_touches_the_rectangle() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "lj");
        keys(&mut vim, &mut b, &mut c, "U");
        assert_eq!(b.text(), "ABc\nDEf");
    }

    // -- `gcc`/`gc{motion}` (comment toggling) ---------------------------

    #[test]
    fn gcc_resolves_the_doubled_form_to_the_current_line() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let event = { keys(&mut vim, &mut b, &mut c, "gc"); vim.handle_key(&mut b, &mut c, KeyPress::char('c')) };
        assert_eq!(event, VimEvent::ToggleComment { start_line: 0, end_line: 0 });
    }

    #[test]
    fn gcc_with_a_count_covers_that_many_lines() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3gc");
        let event = vim.handle_key(&mut b, &mut c, KeyPress::char('c'));
        assert_eq!(event, VimEvent::ToggleComment { start_line: 0, end_line: 2 });
    }

    #[test]
    fn gc_with_a_motion_resolves_via_the_motion_path_not_just_doubled() {
        // `gcw` -- a plain charwise motion, distinct code path from
        // `gcc`'s doubled form and `gcap`'s text-object form (both
        // covered by their own tests). `Motion::Down`/`Up` aren't
        // linewise in this codebase (a disclosed simplification, see
        // `Motion::is_linewise`), so a motion that stays within one
        // line is the representative, unambiguous case here.
        let mut b = buf("one two three");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "gc");
        let event = vim.handle_key(&mut b, &mut c, KeyPress::char('w'));
        assert_eq!(event, VimEvent::ToggleComment { start_line: 0, end_line: 0 });
    }

    #[test]
    fn gc_with_a_text_object_resolves_to_the_lines_it_touches() {
        let mut b = buf("a\nb\n\nc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "gc");
        vim.handle_key(&mut b, &mut c, KeyPress::char('a'));
        let event = vim.handle_key(&mut b, &mut c, KeyPress::char('p')); // gcap: whole paragraph
        assert_eq!(event, VimEvent::ToggleComment { start_line: 0, end_line: 2 }); // includes the trailing blank line
    }

    #[test]
    fn gc_escape_cancels_without_raising_an_event() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "gc");
        let event = vim.handle_key(&mut b, &mut c, KeyPress::named(NamedKey::Escape));
        assert_eq!(event, VimEvent::None);
        assert!(vim.is_idle());
    }

    // -- `zz`/`zt`/`zb`, `Ctrl-a`/`Ctrl-x` -------------------------------

    #[test]
    fn zz_zt_zb_raise_the_expected_scroll_events() {
        let mut b = buf("a");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('z')), VimEvent::None);
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('z')), VimEvent::ScrollWindow(keymaps::ScrollTarget::Center));
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('z')), VimEvent::None);
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('t')), VimEvent::ScrollWindow(keymaps::ScrollTarget::Top));
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('z')), VimEvent::None);
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('b')), VimEvent::ScrollWindow(keymaps::ScrollTarget::Bottom));
    }

    #[test]
    fn ctrl_a_increments_the_number_under_the_cursor() {
        let mut b = buf("count: 41");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('a').with_ctrl());
        assert_eq!(b.text(), "count: 42");
        assert_eq!(c.char_idx, b.len_chars() - 1); // on the last digit
    }

    #[test]
    fn ctrl_x_decrements_the_number_under_the_cursor() {
        let mut b = buf("count: 41");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('x').with_ctrl());
        assert_eq!(b.text(), "count: 40");
    }

    #[test]
    fn ctrl_a_finds_the_next_number_when_the_cursor_isnt_on_one() {
        let mut b = buf("x = 9");
        let mut c = Cursor::at_start(); // on 'x', not the digit
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('a').with_ctrl());
        assert_eq!(b.text(), "x = 10");
    }

    #[test]
    fn ctrl_a_with_a_count_adds_that_many() {
        let mut b = buf("5");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3");
        vim.handle_key(&mut b, &mut c, KeyPress::char('a').with_ctrl());
        assert_eq!(b.text(), "8");
    }

    #[test]
    fn ctrl_a_with_no_number_on_the_line_is_a_no_op() {
        let mut b = buf("no digits here");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('a').with_ctrl());
        assert_eq!(b.text(), "no digits here");
    }

    // -- `is_idle` --------------------------------------------------------

    #[test]
    fn is_idle_is_true_at_rest_and_false_mid_sequence() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(vim.is_idle());
        vim.handle_key(&mut b, &mut c, KeyPress::char('d')); // pending_op set
        assert!(!vim.is_idle());
        vim.handle_key(&mut b, &mut c, KeyPress::char('w'));
        assert!(vim.is_idle()); // dw resolved, back to rest
    }

    #[test]
    fn is_idle_is_false_while_in_insert_mode() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        assert!(!vim.is_idle());
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert!(vim.is_idle());
    }

    #[test]
    fn cw_on_a_word_behaves_like_ce_not_swallowing_trailing_space() {
        let mut b = buf("foo bar");
        let mut c = Cursor::at_start(); // on "foo"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "cw");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), " bar"); // "foo" gone, the space survives
        keys(&mut vim, &mut b, &mut c, "X");
        assert_eq!(b.text(), "X bar");
    }

    #[test]
    fn yy_then_p_duplicates_the_line_below() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yy");
        assert_eq!(b.text(), "one\ntwo"); // yank doesn't delete
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "one\none\ntwo");
    }

    #[test]
    fn yank_produces_a_pulse_event_over_the_yanked_range() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "y"); // operator-pending, no pulse yet
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('w'));
        assert_eq!(ev, VimEvent::Pulse(0..6)); // "hello " (word-forward, exclusive)
    }

    #[test]
    fn paste_produces_a_pulse_event_over_the_inserted_range() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yl"); // yank "a"
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('p'));
        assert_eq!(ev, VimEvent::Pulse(1..2)); // pasted "a" right after the cursor
    }

    #[test]
    fn doubled_yy_also_produces_a_pulse() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "y");
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('y'));
        assert_eq!(ev, VimEvent::Pulse(0..4)); // "one\n"
    }

    #[test]
    fn ordinary_keys_produce_no_pulse() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('l')), VimEvent::None);
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('x')); // delete, not yank
        assert_eq!(ev, VimEvent::None);
    }

    #[test]
    fn x_deletes_char_under_cursor_into_register_and_p_pastes_it() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "x");
        assert_eq!(b.text(), "bc");
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "bac");
    }

    #[test]
    fn x_at_the_end_of_a_line_never_deletes_the_newline() {
        let mut b = buf("ab\ncd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "llx"); // cursor on 'b', the last char of line 1
        assert_eq!(b.text(), "a\ncd");
    }

    #[test]
    fn x_with_a_count_overrunning_the_line_stops_at_the_newline() {
        let mut b = buf("ab\ncd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "5x"); // only 2 chars ("ab") exist before the newline
        assert_eq!(b.text(), "\ncd");
    }

    #[test]
    fn register_reflects_the_most_recent_yank() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yl");
        assert_eq!(vim.register(), ("a", false));
    }

    #[test]
    fn set_register_is_what_the_next_paste_uses() {
        let mut b = buf("x");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.set_register("hi".to_string(), false);
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "xhi");
    }

    // -- Named registers (`"a`-`"z`/`"A`-`"Z`) ------------------------

    #[test]
    fn named_register_yank_and_paste_round_trip_and_mirror_the_unnamed_register() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "\"ayl"); // yank 'a' into register a
        assert_eq!(vim.registers.get(&'a').map(|r| r.text.as_str()), Some("a"));
        assert_eq!(vim.register(), ("a", false)); // "" mirrors it too

        // An intervening plain (unnamed-only) yank must not disturb
        // register a.
        keys(&mut vim, &mut b, &mut c, "l\"byl");
        assert_eq!(vim.registers.get(&'a').map(|r| r.text.as_str()), Some("a"));
        assert_eq!(vim.registers.get(&'b').map(|r| r.text.as_str()), Some("b"));

        keys(&mut vim, &mut b, &mut c, "\"ap");
        assert_eq!(b.text(), "abac\ndef");
    }

    #[test]
    fn uppercase_register_name_appends_to_existing_content() {
        // "abc", not "ab" -- `l` at the buffer's very last character
        // yanks an empty range even inside `yl` (the same clamp a bare
        // `l` motion already has), which isn't what this test wants to
        // exercise; the middle char keeps `l` meaningful both times.
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "\"ayl"); // register a = "a"
        keys(&mut vim, &mut b, &mut c, "l\"Ayl"); // append -> register a = "ab"
        assert_eq!(vim.registers.get(&'a').map(|r| r.text.as_str()), Some("ab"));
    }

    #[test]
    fn an_invalid_register_name_cancels_the_pending_prefix_without_effect() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('"')), VimEvent::None);
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('5')), VimEvent::None); // not a-z/A-Z
        // Confirms nothing was left pending -- an ordinary motion right
        // after still behaves like an ordinary motion.
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn active_register_does_not_linger_past_a_command_that_did_not_use_it() {
        let mut b = buf("abcabc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "\"a"); // select register a...
        keys(&mut vim, &mut b, &mut c, "l"); // ...then an ordinary motion, not a register command
        keys(&mut vim, &mut b, &mut c, "yl"); // plain yank, no "-prefix this time
        assert_eq!(vim.register(), ("b", false)); // the unnamed register got the plain yank...
        assert!(vim.registers.get(&'a').is_none()); // ...register a was never actually written
    }

    #[test]
    fn visual_mode_register_prefix_targets_a_named_register_too() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v"); // enter Visual, char at 0 anchored
        keys(&mut vim, &mut b, &mut c, "l"); // extend to cover "ab"
        keys(&mut vim, &mut b, &mut c, "\"ay");
        assert_eq!(vim.registers.get(&'a').map(|r| r.text.as_str()), Some("ab"));
    }

    // -- Macros (`q{reg}`/`@{reg}`/`@@`) -------------------------------

    #[test]
    fn q_then_a_letter_starts_recording_and_returns_macro_record_start() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('q')), VimEvent::None);
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('a')), VimEvent::MacroRecordStart('a'));
        assert!(vim.is_recording());
        assert_eq!(vim.recording_register(), Some('a'));
    }

    #[test]
    fn an_uppercase_start_register_preserves_case_for_append_detection() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('q'));
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('A')), VimEvent::MacroRecordStart('A'));
        assert_eq!(vim.recording_register(), Some('a')); // storage itself is always lowercase
    }

    #[test]
    fn a_second_bare_q_stops_recording_immediately_no_second_key_needed() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "qa");
        assert!(vim.is_recording());
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('q')), VimEvent::MacroRecordStop('a'));
        assert!(!vim.is_recording());
    }

    #[test]
    fn finish_recording_encodes_and_stores_the_keys_in_the_target_register() {
        let mut vim = VimState::new();
        let typed = vec![KeyPress::char('d'), KeyPress::char('w')];
        vim.finish_recording('a', &typed, false);
        assert_eq!(vim.decode_register('a'), Some(typed));
    }

    #[test]
    fn finish_recording_with_append_true_appends_to_existing_register_content() {
        let mut vim = VimState::new();
        vim.finish_recording('a', &[KeyPress::char('d')], false);
        vim.finish_recording('a', &[KeyPress::char('w')], true);
        assert_eq!(vim.decode_register('a'), Some(vec![KeyPress::char('d'), KeyPress::char('w')]));
    }

    #[test]
    fn decode_register_is_none_for_a_register_that_was_never_written() {
        let vim = VimState::new();
        assert_eq!(vim.decode_register('z'), None);
    }

    #[test]
    fn at_sign_with_a_leading_count_resolves_to_macro_play_with_that_count() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3"); // accumulate a count first, like every other countable action
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('@')), VimEvent::None); // waiting on the register name
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('a')), VimEvent::MacroPlay { register: 'a', count: 3 });
    }

    #[test]
    fn bare_at_sign_defaults_to_a_count_of_one() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('@'));
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('a')), VimEvent::MacroPlay { register: 'a', count: 1 });
    }

    #[test]
    fn at_at_resolves_with_the_literal_at_sign_as_the_repeat_last_sentinel() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('@'));
        assert_eq!(vim.handle_key(&mut b, &mut c, KeyPress::char('@')), VimEvent::MacroPlay { register: '@', count: 1 });
    }

    #[test]
    fn escape_cancels_a_pending_register_macro_record_or_macro_play_prefix() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();

        vim.handle_key(&mut b, &mut c, KeyPress::char('"'));
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "yl");
        assert!(vim.registers.is_empty(), "register prefix should have been cancelled");

        vim.handle_key(&mut b, &mut c, KeyPress::char('q'));
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert!(!vim.is_recording(), "macro-record prefix should have been cancelled");

        vim.handle_key(&mut b, &mut c, KeyPress::char('@'));
        assert_eq!(named(&mut vim, &mut b, &mut c, NamedKey::Escape), VimEvent::None);
    }

    #[test]
    fn charwise_paste_after_lands_on_last_pasted_char() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.register = Register { text: "xyz".to_string(), linewise: false };
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "axyzb");
        assert_eq!(c.char_idx, 3); // on 'z', not back at the pasted 'x'
    }

    #[test]
    fn charwise_paste_before_lands_on_last_pasted_char() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.register = Register { text: "xyz".to_string(), linewise: false };
        keys(&mut vim, &mut b, &mut c, "P");
        assert_eq!(b.text(), "xyzab");
        assert_eq!(c.char_idx, 2); // on 'z'
    }

    #[test]
    fn linewise_paste_lands_on_first_non_blank_of_pasted_line() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.register = Register { text: "  hi\n".to_string(), linewise: true };
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "one\n  hi\ntwo");
        assert_eq!(b.line_col(&c), (1, 2)); // first non-blank of "  hi", not column 0
    }

    #[test]
    fn undo_and_redo_via_u_and_ctrl_r() {
        let mut b = buf("");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "i");
        keys(&mut vim, &mut b, &mut c, "hi");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "hi");

        keys(&mut vim, &mut b, &mut c, "u");
        assert_eq!(b.text(), "");

        vim.handle_key(&mut b, &mut c, KeyPress::char('r').with_ctrl());
        assert_eq!(b.text(), "hi");
    }

    #[test]
    fn visual_mode_delete_removes_the_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        assert_eq!(vim.mode(), Mode::Visual);
        keys(&mut vim, &mut b, &mut c, "llll"); // select "hello"
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), " world");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_escape_cancels_without_editing() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn shift_v_enters_visual_line_mode() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Line);
    }

    #[test]
    fn visual_line_delete_removes_whole_lines_regardless_of_column() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 5, sticky_col: 1 }; // column 1 of "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), "one\nthree");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_char_indent_acts_linewise_despite_the_selection_being_charwise() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 5, sticky_col: 1 }; // column 1 of "two", charwise selection
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(b.text(), "one\n    two\nthree");
        // `>`/`<` reselect the same range afterward (see the Indent/
        // Dedent arm's own doc comment) rather than dropping to Normal
        // mode the way real vanilla Vim's does.
        assert_eq!(vim.mode(), Mode::Visual);
    }

    #[test]
    fn visual_indent_stacks_with_a_second_press_because_the_selection_stays_active() {
        // The actual bug report this fixes: pressing `>` twice out of
        // Normal-mode `>>` habit used to indent once and then silently
        // swallow the second press (real Vim's own vanilla behavior --
        // `>` alone exits Visual mode, so the second `>` just starts an
        // incomplete Normal-mode `>_` sequence). Reselecting after the
        // first shift means the second press lands on the same trie
        // entry again instead.
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, ">");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(b.text(), "        one\n        two\nthree\nfour");
        assert_eq!(vim.mode(), Mode::Visual);
    }

    #[test]
    fn visual_dedent_also_stacks_with_a_second_press() {
        let mut b = buf("            one\n            two\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, "<");
        keys(&mut vim, &mut b, &mut c, "<");
        assert_eq!(b.text(), "    one\n    two\nthree");
    }

    #[test]
    fn visual_indent_reselect_preserves_which_end_the_anchor_was_on() {
        // Selection made bottom-to-top (anchor on the later line) --
        // the reselect after a shift must keep the anchor on that same
        // later line, not silently flip the selection's direction.
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // start of "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "k"); // move up to "one" -- anchor ("two") is now below the cursor
        keys(&mut vim, &mut b, &mut c, ">");
        // A further "jj" from here should extend onto "three" while
        // leaving "one" alone -- only sensible if the anchor really
        // did stay on "two" rather than jumping to "one".
        keys(&mut vim, &mut b, &mut c, "jj");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(b.text(), "    one\n        two\n    three");
    }

    #[test]
    fn escape_still_exits_visual_mode_after_an_indent() {
        let mut b = buf("one\ntwo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(vim.mode(), Mode::Visual);
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_indent_spans_every_line_the_selection_touches() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, ">");
        assert_eq!(b.text(), "    one\n    two\nthree\nfour");
    }

    #[test]
    fn visual_dedent_removes_a_level_from_every_selected_line() {
        let mut b = buf("    one\n    two\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, "<");
        assert_eq!(b.text(), "one\ntwo\nthree");
    }

    #[test]
    fn visual_line_extends_by_whole_lines_with_motions() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "j"); // extend to line 1
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), "three\nfour");
    }

    #[test]
    fn v_then_v_again_toggles_back_to_normal() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vv");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn v_then_shift_v_switches_kind_without_exiting() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        assert_eq!(vim.visual_kind(), VisualKind::Char);
        keys(&mut vim, &mut b, &mut c, "V");
        assert_eq!(vim.mode(), Mode::Visual); // still in Visual, not exited
        assert_eq!(vim.visual_kind(), VisualKind::Line);
    }

    #[test]
    fn visual_line_yank_then_paste_duplicates_lines() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "V");
        keys(&mut vim, &mut b, &mut c, "y");
        assert_eq!(b.text(), "one\ntwo\nthree"); // yank doesn't delete
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "one\none\ntwo\nthree");
    }

    #[test]
    fn command_line_w_and_q_produce_events() {
        let mut b = buf("x");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ":");
        assert_eq!(vim.mode(), Mode::Command);
        keys(&mut vim, &mut b, &mut c, "w");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(ev, VimEvent::RequestSave);
        assert_eq!(vim.mode(), Mode::Normal);

        keys(&mut vim, &mut b, &mut c, ":wq");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(ev, VimEvent::RequestSaveAndCloseBuffer);
    }

    #[test]
    fn set_shiftwidth_reconfigures_tab_and_gtgt_ltlt() {
        let mut b = buf("foo\n");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert_eq!(vim.indent_width(), 4);

        keys(&mut vim, &mut b, &mut c, ":set shiftwidth=3");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Enter);
        assert_eq!(ev, VimEvent::IndentWidthChanged(3));
        assert_eq!(vim.indent_width(), 3);
        assert_eq!(vim.mode(), Mode::Normal);

        // Tab now inserts 3 spaces, not 4.
        keys(&mut vim, &mut b, &mut c, "i");
        named(&mut vim, &mut b, &mut c, NamedKey::Tab);
        assert_eq!(b.text(), "   foo\n");

        // >> now indents by 3 spaces too.
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "0");
        keys(&mut vim, &mut b, &mut c, ">>");
        assert_eq!(b.text(), "      foo\n");
    }

    #[test]
    fn set_indent_width_applies_a_persisted_setting_directly() {
        let mut vim = VimState::new();
        vim.set_indent_width(2);
        assert_eq!(vim.indent_width(), 2);
        vim.set_indent_width(0); // rejected, division-by-zero guard
        assert_eq!(vim.indent_width(), 2);
    }

    #[test]
    fn command_line_escape_cancels() {
        let mut b = buf("x");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ":q");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(vim.command_line(), "");
    }

    #[test]
    fn operator_pending_escape_cancels_without_editing() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "d");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1); // 'l' was treated as a fresh motion, not consumed by 'd'
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn o_and_shift_o_open_lines_and_enter_insert() {
        let mut b = buf("mid");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "o");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "below");
        assert_eq!(b.text(), "mid\nbelow");

        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        keys(&mut vim, &mut b, &mut c, "gg"); // back to line 0
        keys(&mut vim, &mut b, &mut c, "O");
        keys(&mut vim, &mut b, &mut c, "above");
        assert_eq!(b.text(), "above\nmid\nbelow");
    }

    #[test]
    fn is_pending_reflects_partial_sequences() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(!vim.is_pending());
        keys(&mut vim, &mut b, &mut c, "d");
        assert!(vim.is_pending());
        keys(&mut vim, &mut b, &mut c, "w");
        assert!(!vim.is_pending());
    }

    #[test]
    fn pending_children_lists_operator_pending_continuations() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        assert!(vim.pending_children().is_empty());
        keys(&mut vim, &mut b, &mut c, "d");
        let children = vim.pending_children();
        // top-level: motions directly, text objects behind their "i"/"a" prefix
        assert!(children.iter().any(|(_, label)| *label == "word forward"));
        assert!(children.iter().any(|(_, label)| *label == "inner..."));

        keys(&mut vim, &mut b, &mut c, "i");
        let children = vim.pending_children();
        assert!(children.iter().any(|(_, label)| *label == "inner word"));
    }

    #[test]
    fn big_word_motions_are_available_in_normal_mode() {
        let mut b = buf("foo.bar baz");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "W");
        assert_eq!(c.char_idx, 8); // straight to "baz", not stopping at the "."
    }

    #[test]
    fn shift_d_c_y_act_to_end_of_line() {
        let mut b = buf("hello world");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "D");
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn shift_c_enters_insert_after_deleting_to_eol() {
        let mut b = buf("hello world");
        let mut c = Cursor { char_idx: 5, sticky_col: 5 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "C");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn shift_y_yanks_to_eol_without_deleting() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "Y");
        assert_eq!(b.text(), "hello world");
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "hhello worldello world");
    }

    #[test]
    fn shift_s_changes_the_whole_line() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "S");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "one\n\nthree");
    }

    #[test]
    fn lowercase_s_substitutes_one_char() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "s");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "bc");
    }

    #[test]
    fn j_joins_lines_trimming_leading_whitespace() {
        let mut b = buf("hello\n   world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "J");
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn j_on_last_line_is_a_no_op() {
        let mut b = buf("only");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "J");
        assert_eq!(b.text(), "only");
    }

    #[test]
    fn tilde_toggles_case_and_advances() {
        let mut b = buf("aB");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "~");
        assert_eq!(b.text(), "AB");
        assert_eq!(c.char_idx, 1);
        keys(&mut vim, &mut b, &mut c, "~");
        assert_eq!(b.text(), "Ab");
    }

    #[test]
    fn r_replaces_the_char_under_cursor_and_stays_normal() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "r");
        keys(&mut vim, &mut b, &mut c, "x");
        assert_eq!(b.text(), "xbc");
        assert_eq!(c.char_idx, 0);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn r_then_escape_cancels_without_editing() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "r");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "abc");
    }

    #[test]
    fn count_repeats_a_plain_motion() {
        let mut b = buf("one two three four");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3w");
        assert_eq!(c.char_idx, 14); // start of "four", three words forward
    }

    #[test]
    fn count_before_doubled_operator_spans_n_lines() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "2dd");
        assert_eq!(b.text(), "three\nfour");
    }

    #[test]
    fn count_after_operator_and_before_doubled_key_also_works() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "d2d");
        assert_eq!(b.text(), "three\nfour");
    }

    #[test]
    fn greater_greater_indents_the_current_line() {
        let mut b = buf("one\ntwo\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ">>");
        assert_eq!(b.text(), "one\n    two\nthree");
    }

    #[test]
    fn less_less_dedents_the_current_line() {
        let mut b = buf("one\n        two\nthree");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "two"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "<<");
        assert_eq!(b.text(), "one\n    two\nthree");
    }

    #[test]
    fn less_less_removes_only_whats_there_when_less_than_indent_width() {
        let mut b = buf("  two");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "<<");
        assert_eq!(b.text(), "two");
    }

    #[test]
    fn count_before_greater_greater_indents_n_lines() {
        let mut b = buf("one\ntwo\nthree\nfour");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3>>");
        assert_eq!(b.text(), "    one\n    two\n    three\nfour");
    }

    #[test]
    fn indent_dedent_move_cursor_to_the_first_non_blank() {
        let mut b = buf("foo");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, ">>");
        assert_eq!(c.char_idx, 4); // right after the 4 inserted spaces, on "f"
    }

    #[test]
    fn counts_before_operator_and_before_motion_multiply() {
        let mut b = buf("one two three four five six seven");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "2d3w"); // delete 6 words
        assert_eq!(b.text(), "seven");
    }

    #[test]
    fn count_survives_a_multi_key_motion_like_gg() {
        let mut b = buf("a\nb\nc\nd\ne");
        let mut c = Cursor { char_idx: 8, sticky_col: 0 }; // on "e"
        let mut vim = VimState::new();
        // Exercises that the count typed before a two-key sequence isn't
        // dropped partway through the "g" then "g" feed -- 3gg goes to
        // line 3 (index 2, "c"), not line 0 (bare gg's default).
        keys(&mut vim, &mut b, &mut c, "3gg");
        assert_eq!(b.line_col(&c).0, 2);
    }

    #[test]
    fn gg_with_no_count_goes_to_the_first_line() {
        let mut b = buf("a\nb\nc");
        let mut c = Cursor { char_idx: 4, sticky_col: 0 }; // on "c"
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "gg");
        assert_eq!(b.line_col(&c).0, 0);
    }

    #[test]
    fn g_with_a_count_goes_to_that_line_but_bare_g_goes_to_the_last_line() {
        let mut b = buf("a\nb\nc\nd\ne");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "2G");
        assert_eq!(b.line_col(&c).0, 1); // line 2, index 1

        keys(&mut vim, &mut b, &mut c, "G"); // no count -- last line, not line 1
        assert_eq!(b.line_col(&c).0, 4);
    }

    #[test]
    fn a_count_past_the_last_line_clamps_to_it() {
        let mut b = buf("a\nb\nc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "500gg");
        assert_eq!(b.line_col(&c).0, 2);
    }

    #[test]
    fn dg_with_a_count_deletes_linewise_up_to_that_line() {
        let mut b = buf("a\nb\nc\nd\ne");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "d3G"); // delete lines 1-3
        assert_eq!(b.text(), "d\ne");
    }

    #[test]
    fn shift_g_that_actually_moves_the_cursor_records_a_jump() {
        let mut b = buf("a\nb\nc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('G'));
        assert_eq!(ev, VimEvent::JumpRecorded(0));
    }

    #[test]
    fn gg_that_does_not_move_the_cursor_records_no_jump() {
        let mut b = buf("a\nb\nc");
        let mut c = Cursor::at_start(); // already on line 1
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "g");
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('g'));
        assert_eq!(ev, VimEvent::None);
    }

    #[test]
    fn matching_bracket_that_moves_the_cursor_records_a_jump() {
        let mut b = buf("(abc)");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('%'));
        assert_eq!(ev, VimEvent::JumpRecorded(0));
        assert_eq!(c.char_idx, 4);
    }

    #[test]
    fn matching_bracket_with_nothing_to_match_records_no_jump() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('%'));
        assert_eq!(ev, VimEvent::None);
    }

    #[test]
    fn m_then_a_char_emits_mark_set_at_the_cursors_position_without_moving_it() {
        let mut b = buf("abcdef");
        let mut c = Cursor { char_idx: 3, sticky_col: 3 };
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "m");
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('a'));
        assert_eq!(ev, VimEvent::MarkSet('a', 3));
        assert_eq!(c.char_idx, 3); // setting a mark never moves the cursor
    }

    #[test]
    fn backtick_then_a_char_emits_a_charwise_jump_to_mark() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('`'));
        assert_eq!(ev, VimEvent::None); // waiting on the mark name
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('a'));
        assert_eq!(ev, VimEvent::JumpToMark { name: 'a', linewise: false });
    }

    #[test]
    fn quote_then_a_char_emits_a_linewise_jump_to_mark() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('\''));
        let ev = vim.handle_key(&mut b, &mut c, KeyPress::char('z'));
        assert_eq!(ev, VimEvent::JumpToMark { name: 'z', linewise: true });
    }

    #[test]
    fn escape_after_m_aborts_without_setting_a_mark() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "m");
        let ev = named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(ev, VimEvent::None);
        // Confirms nothing was left pending: an ordinary motion right
        // after still behaves like an ordinary motion, not a stale
        // mark-name resolution.
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1);
    }

    #[test]
    fn dg_with_no_count_deletes_linewise_to_the_last_line() {
        let mut b = buf("a\nb\nc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "dG");
        assert_eq!(b.text(), "");
    }

    #[test]
    fn count_before_the_operator_also_targets_an_absolute_line_for_g() {
        let mut b = buf("a\nb\nc\nd\ne");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3dG"); // 3dG, not d3G -- count before the operator
        assert_eq!(b.text(), "d\ne");
    }

    #[test]
    fn count_on_x_deletes_n_chars_into_one_register() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3x");
        assert_eq!(b.text(), "def");
        keys(&mut vim, &mut b, &mut c, "p");
        assert_eq!(b.text(), "dabcef");
    }

    #[test]
    fn count_on_paste_repeats_the_register() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "yl"); // yank "a"
        keys(&mut vim, &mut b, &mut c, "3p");
        assert_eq!(b.text(), "aaaab");
    }

    #[test]
    fn count_on_replace_needs_that_many_chars_available() {
        let mut b = buf("ab");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        // like every other count, it precedes the trigger key: "3rx", not "r3x"
        keys(&mut vim, &mut b, &mut c, "3rx");
        // only 2 chars available, so this should refuse rather than partially replace
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn count_on_join_joins_n_lines() {
        let mut b = buf("a\nb\nc\nd");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "3J");
        assert_eq!(b.text(), "a b c\nd");
    }

    #[test]
    fn no_match_during_count_resets_it() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        // an unbound key after a count should abort the count, not leave
        // it silently applied to whatever comes next
        vim.handle_key(&mut b, &mut c, KeyPress::char('2'));
        vim.handle_key(&mut b, &mut c, KeyPress::named(NamedKey::Tab)); // unbound in Normal mode
        keys(&mut vim, &mut b, &mut c, "l");
        assert_eq!(c.char_idx, 1); // single step, not 2
    }

    #[test]
    fn ctrl_v_enters_visual_block_mode() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Block);
    }

    #[test]
    fn block_delete_removes_the_column_rectangle_from_every_line() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl"); // rectangle: lines 0-2, cols 0-1
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), "c\nf\ni");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn block_yank_does_not_modify_the_buffer() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl");
        keys(&mut vim, &mut b, &mut c, "y");
        assert_eq!(b.text(), "abc\ndef\nghi");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn block_yank_register_joins_column_pieces_with_newline() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl");
        keys(&mut vim, &mut b, &mut c, "y");
        // paste into a separate empty buffer to inspect the register's
        // exact content without the arithmetic of pasting back into the
        // same (now-shifted) buffer
        let mut b2 = buf("");
        let mut c2 = Cursor::at_start();
        keys(&mut vim, &mut b2, &mut c2, "P");
        assert_eq!(b2.text(), "ab\nde\ngh");
    }

    #[test]
    fn block_delete_on_ragged_lines_clamps_per_line() {
        let mut b = buf("abcdef\nx\nghijkl");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // down to line 2, still col 0
        keys(&mut vim, &mut b, &mut c, "lll"); // extend to col 3
        keys(&mut vim, &mut b, &mut c, "d");
        // middle line "x" is shorter than the rectangle -- clamped, not padded
        assert_eq!(b.text(), "ef\n\nkl");
    }

    #[test]
    fn block_change_deletes_column_and_enters_insert_without_propagating() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "j");
        keys(&mut vim, &mut b, &mut c, "c");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(b.text(), "bc\nef"); // deleted on both lines, no I-style propagation
    }

    #[test]
    fn block_insert_left_propagates_typed_text_to_other_lines_on_escape() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // column 0 across all 3 lines
        keys(&mut vim, &mut b, &mut c, "I");
        assert_eq!(vim.mode(), Mode::Insert);
        keys(&mut vim, &mut b, &mut c, "X");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "Xabc\nXdef\nXghi");
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn block_insert_left_propagates_a_tab_to_other_lines_on_escape() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // column 0 across all 3 lines
        keys(&mut vim, &mut b, &mut c, "I");
        named(&mut vim, &mut b, &mut c, NamedKey::Tab);
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        let indent = " ".repeat(indent::DEFAULT_INDENT_WIDTH);
        assert_eq!(b.text(), format!("{indent}abc\n{indent}def\n{indent}ghi"));
    }

    #[test]
    fn block_insert_left_skips_lines_too_short_to_reach_the_column() {
        let mut b = buf("abcdef\nxy\nghijkl");
        let mut c = Cursor { char_idx: 3, sticky_col: 3 }; // col 3 of line 0
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // down to line 2, keeping col 3
        keys(&mut vim, &mut b, &mut c, "I");
        keys(&mut vim, &mut b, &mut c, "Z");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        // line 1 ("xy") is only 2 chars -- too short for column 3, skipped
        assert_eq!(b.text(), "abcZdef\nxy\nghiZjkl");
    }

    #[test]
    fn block_r_replaces_the_selected_rectangle_and_exits_to_normal() {
        let mut b = buf("abc\ndef\nghi");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jjl"); // column 0..=1 across all 3 lines
        keys(&mut vim, &mut b, &mut c, "rx");
        assert_eq!(b.text(), "xxc\nxxf\nxxi");
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(c.char_idx, 0); // cursor lands at the block's top-left
    }

    #[test]
    fn block_r_clamps_per_line_on_ragged_lines() {
        let mut b = buf("abcdef\nx\nghijkl");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        keys(&mut vim, &mut b, &mut c, "jj"); // down to line 2, still col 0
        keys(&mut vim, &mut b, &mut c, "lll"); // extend to col 3
        keys(&mut vim, &mut b, &mut c, "rz");
        // middle line "x" is shorter than the rectangle -- its one char (at
        // column 0, inside the selected 0..4) is replaced, but the
        // rectangle isn't padded out to the other lines' width
        assert_eq!(b.text(), "zzzzef\nz\nzzzzkl");
    }

    #[test]
    fn charwise_r_replaces_only_the_selected_span() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vll"); // select "abc"
        keys(&mut vim, &mut b, &mut c, "rz");
        assert_eq!(b.text(), "zzzdef");
        assert_eq!(vim.mode(), Mode::Normal);
        assert_eq!(c.char_idx, 0);
    }

    #[test]
    fn charwise_r_spanning_lines_leaves_the_newline_alone() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        keys(&mut vim, &mut b, &mut c, "jl"); // select through (and including) "e" on line 2
        keys(&mut vim, &mut b, &mut c, "rz");
        assert_eq!(b.text(), "zzz\nzzf");
    }

    #[test]
    fn linewise_r_replaces_every_non_newline_char_on_each_selected_line() {
        let mut b = buf("abc\nde\nfgh");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "Vj"); // lines 0..=1
        keys(&mut vim, &mut b, &mut c, "rz");
        assert_eq!(b.text(), "zzz\nzz\nfgh");
    }

    #[test]
    fn escape_while_visual_r_is_pending_cancels_without_replacing_and_exits_visual() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vl");
        keys(&mut vim, &mut b, &mut c, "r");
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(b.text(), "abc");
        assert_eq!(vim.mode(), Mode::Normal); // Escape cancels the replace AND exits Visual mode
    }

    #[test]
    fn visual_selection_range_is_none_outside_visual_mode() {
        let b = buf("abc");
        let c = Cursor::at_start();
        let vim = VimState::new();
        assert_eq!(vim.visual_selection_range(&b, &c), None);
    }

    #[test]
    fn visual_selection_range_is_none_for_a_block_selection() {
        let mut b = buf("abc\ndef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        assert_eq!(vim.visual_selection_range(&b, &c), None);
    }

    #[test]
    fn visual_selection_range_covers_a_char_selection() {
        let mut b = buf("abcdef");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "vll"); // selects "abc"
        assert_eq!(vim.visual_selection_range(&b, &c), Some((0..3, false)));
    }

    #[test]
    fn exit_visual_mode_returns_to_normal_and_is_a_noop_otherwise() {
        let mut b = buf("abc");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        vim.exit_visual_mode(&c); // outside Visual: no-op, doesn't panic
        assert_eq!(vim.mode(), Mode::Normal);
        keys(&mut vim, &mut b, &mut c, "v");
        assert_eq!(vim.mode(), Mode::Visual);
        vim.exit_visual_mode(&c);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn v_then_ctrl_v_switches_to_block_without_exiting() {
        let mut b = buf("hello");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        vim.handle_key(&mut b, &mut c, KeyPress::char('v').with_ctrl());
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Block);
    }

    #[test]
    fn gv_reselects_the_last_visual_selection() {
        let mut b = buf("hello world");
        let mut c = Cursor::at_start();
        let mut vim = VimState::new();
        keys(&mut vim, &mut b, &mut c, "v");
        keys(&mut vim, &mut b, &mut c, "llll"); // select "hello"
        named(&mut vim, &mut b, &mut c, NamedKey::Escape);
        assert_eq!(vim.mode(), Mode::Normal);

        keys(&mut vim, &mut b, &mut c, "$"); // move away
        keys(&mut vim, &mut b, &mut c, "gv");
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(vim.visual_kind(), VisualKind::Char);
        keys(&mut vim, &mut b, &mut c, "d");
        assert_eq!(b.text(), " world");
    }
}
