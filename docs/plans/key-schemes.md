# Plan: key schemes -- Vim, and standard keys

- **Status:** draft 2026-11-09, for review
- **Scope:** a second way to edit in Fenix, for people who want the keys of a
  traditional IDE (no modes, no leader typed as `SPC`), with nothing taken
  away from the Vim and leader experience that stays the default
- **Lands as:** ADR 0004 once accepted, then the milestones below, one PR
  each. The extensions' part (§6) lands in fenix-extensions after M6

## The idea in one paragraph

A **key scheme** is chosen with one setting, `editor.keys = "vim" |
"standard"`. Vim stays the default and works as it does today. "Standard" is
the same editor with a different policy on top: text buffers rest in typing
mode, Shift+arrows select, Ctrl+C/X/V/Z/S/F/P do what people expect, and the
whole leader tree (and every extension's `SPC` menu) sits behind one chord,
`Ctrl+K`, with which-key showing what comes next. Both schemes are bindings
for the **same commands**. Core gets no feature that only one scheme can
reach. Extensions keep working unchanged. API 1.5 adds a small, optional way
for them to ask which scheme is on, and which keys a command has, so their
hints can say `F12` instead of `gd`.

### Ground rules

- **Vim is the product's voice.** The default, the README, the tutorial's
  first lessons and `KEYBINDINGS.md` stay Vim-first. Standard keys are
  documented beside them, not instead of them.
- **Commands, not keys.** Every action either scheme binds is a command id
  (`SPC h c` describes it, the palette runs it). An action that today exists
  only as a Vim key (`K`, `gd`) becomes a command first (M2).
- **One editor engine.** Standard mode is a policy inside `fenix-vim`, not a
  second editing implementation. Undo, registers, selections, text objects,
  snippets, completion and LSP edits are shared.
- **Pages keep their keys.** A page's own keys (`j`/`k`, `s`, `q` on Git, the
  explorer, an extension's page) mean the same in both schemes, the way
  lazygit's do. Letter keys act where you can't type.
- **No `if scheme == …` scattered through `app.rs`.** The scheme is read in
  a few named predicates (§3.3) and in the keymap data. Everything else asks
  those.

---

## 1. What exists today

Facts the plan builds on (paths in `fenix` unless said otherwise):

| Area | Today | Consequence |
|---|---|---|
| Commands | `CommandRegistry` (`crates/fenix-gui/src/commands.rs`), about 170 ids | Most actions are already bindable |
| Keymap data | `default_keymap.toml`, `keybindings.toml`, extensions' `keymaps/*.toml`: `[[leader]]` and `[[local.*]]` only | `apply_user_keys` refuses keys that don't start with `SPC` (`keymap.rs:363`) |
| Global chords | Hard-coded `if/else` in `route_keypress` (`app.rs:16218`): Ctrl+S, Z, Y, Q, O, I, font size, tab moves | Can't be rebound or added to by data |
| Shift | `Mods` has ctrl, alt, super (`crates/fenix-keymap/src/key.rs`) | Shift+arrow and Ctrl+Shift+P can't be expressed; the Ctrl+Shift+Z branch reads live modifiers and doesn't replay in macros |
| Leader | Started by Space in Normal or Visual (`app.rs:16313`) | Unreachable while typing |
| Editing | `VimState` owns all text keys; `handle_key` makes one undo group per key, and an Insert session coalesces into one step (`fenix-vim/src/state.rs:1007`) | A resting-Insert editor would undo a whole session at once without new break rules |
| Mode checks in the GUI | `Mode::Normal` / `is_inserting()`: 18 in `app.rs`, 7 in `app/mouse.rs`, 4 in `tutorial_host.rs`, a few elsewhere | Page behaviours assume Normal mode: compose's "Esc then Enter" (`app.rs:581`), `A` for the agent, `q` on side by side, Enter on Home |
| Vim-only actions | `K` is `VimAction::RequestLsp(Hover)`; `gd`, `gr` likewise. The right-click menu has its own `MenuAction::{Definition, References, Hover, Cut, Copy, Paste, SelectAll}` (`app/mouse.rs:90`) | Implementations exist; they aren't commands |
| Page key order | `page_key` (`app.rs:16180`) runs before the global chord block; a `more_keys` page gets Ctrl chords first (`app/pages.rs:798`) | Hex's and Logs' Ctrl+F/B already win on their pages |
| Extension API | 1.4.0 (`crates/fenix-extensions/src/manifest.rs:14`). No editor mode is exposed. Pages get `page_key` / `page_key_press` | Extensions don't depend on Vim modes; the assumptions are in core |

---

## 2. Decisions to make first (ADR 0004)

### 2.1 A core setting, with the base chord set in core

`editor.keys`, `Kind::Choice(&["vim", "standard"])`, default `vim`, not
project-overridable (it's about the person, not the code). The standard chord
set ships in core's `default_keymap.toml` (entries marked `scheme =
"standard"`), so the setting works without installing anything and is
covered by core's tests. Flavours (a JetBrains-like set, say) can come later
as data-only extensions with their own `keymaps/*.toml`.

This changes the earlier idea of shipping the base set as an extension: a
setting that does nothing until you install something is a poor first run,
and the base set has to move in step with core's commands.

### 2.2 The leader chord: `Ctrl+K`

In standard mode `Ctrl+K` opens the leader exactly as `SPC` does in Normal
mode: same trie, same which-key popup, so `Ctrl+K g g` is `SPC g g`, and
every extension's `[[leader]]` and `SPC m` entry comes with it. VS Code
users already know `Ctrl+K` as a chord prefix. `Ctrl+Space` stays free for
completion. Configurable as `keys.leader_chord`.

### 2.3 Key notation

Keymap files and which-key write chords as `C-` (Ctrl), `M-` (Alt), `s-`
(Super), `S-` (Shift): `C-S-p`, `S-<Down>`, `<F12>`. Today
`describe_keypress` writes Super as `S-`. It moves to `s-` (Emacs's
convention) before Shift is added, so `S-` is never ambiguous.

### 2.4 Selection in standard mode is Visual under the hood

Shift+motion or a mouse drag enters charwise Visual with a standard-mode
flag: typing replaces the selection, Backspace/Delete remove it, an
unshifted arrow collapses it, Esc drops it and stays typing. This is Vim's
own Select mode, so operators, `format_selection`, `SPC c f` and "ask the
agent about the selection" all keep working. The modeline says `SELECT`, or
nothing while just typing (§3.4).

### 2.5 What standard mode doesn't do (v1)

- No `:` command line while typing (`:` types a colon). Ex commands stay in
  the palette (`Ctrl+Shift+P`) and in `Ctrl+K :`. `Ctrl+G` goes to a line.
- No `.` repeat, no macro keys. Recording and playing macros can be commands
  later.
- No multiple cursors. That's a separate feature for both schemes.
- No attempt to match any one IDE exactly. The bar: productive on day one.

---

## 3. Core design

### 3.1 Keys (`fenix-keymap`, `fenix-gui/src/keymap.rs`)

- **Shift in `Mods`.** `to_keypress` normalizes it: for a printable
  character with no Ctrl or Alt, shift is dropped (the character already
  says `A` vs `a`, so every existing binding and Vim's grammar stay as they
  are). With Ctrl/Alt, or on a named key, it's kept. Macro capture records
  it, which fixes the replay gap the comment at `app.rs:16210` describes.
- **A `[[keys]]` table** in every keymap file, beside `[[leader]]`:

  ```toml
  [[keys]]
  keys = "C-S-p"
  command = "commands.palette"
  when = "anywhere"      # anywhere | editor | text | typing | page
  scheme = "standard"    # vim | standard | any (default any)
  ```

  `when` is a short fixed list, not an expression language. `apply_user_keys`
  grows a sibling that reads `[[keys]]`. `keybindings.toml` can bind and
  unbind them, `SPC h b` / `SPC h K` list them, and the palette's
  "give it a key" (`Tab`) can write one (`fenix-config/src/keybindings.rs`).
- **The hard-coded chords become data.** The Ctrl block in `route_keypress`
  turns into a lookup in a `ChordMap` built like the leader trie: core's
  entries with `scheme = "any"` reproduce today's Ctrl+S/Z/Y/Q/O/I and font
  keys exactly. No behaviour change in Vim mode, and Vim users can now
  rebind them.
- **Order stays as it is.** Capturing prompts → terminals → pickers → the
  focused page (which can claim any chord) → the chord map → the leader →
  `fenix-vim`. New standard chords can't take keys from a page that claims
  them. Terminal panes keep every key except reserved chords; ``Ctrl+` `` (the
  terminal toggle) joins `Ctrl+\` there.

### 3.2 Editing policy (`fenix-vim`)

`VimState::set_policy(EditPolicy::Vim | EditPolicy::Standard)`. Under
`Standard`:

- **Resting mode is Insert.** Opening, switching or focusing a text buffer
  lands in Insert. Esc closes popups and drops a selection; it never enters
  Normal. Commands that went through Normal internally (`one_shot_from`, the
  `Ctrl-O` one-shot) still can, invisibly.
- **Movement:** arrows, `Ctrl+←/→` by word, `Home` to the first non-blank
  then column 0, `Ctrl+Home/End`, PageUp/PageDown (already GUI-side).
- **Selection:** the Shift variant of every movement extends (§2.4).
  `Ctrl+A` selects all, `Ctrl+L` the line.
- **Editing:** `Ctrl+Backspace/Delete` by word, `Tab`/`Shift+Tab` indent the
  selection, `Ctrl+/` toggles a comment (new command `code.toggle_comment`,
  which Vim mode gets too as `gc`, if wanted), `Alt+↑/↓` move lines (`:m`).
- **Clipboard:** `Ctrl+C/X/V` use the system clipboard register (`"+`).
  Copy and cut with no selection take the line, as IDEs do.
- **Undo steps.** Typing coalesces into one step until a break: a word
  boundary after whitespace, Enter, a cursor jump (anything but typing
  moving it), a paste, a pause over `editor.undo_pause_ms` (default 1000).
  `begin_undo_group`/`end_undo_group` stay per key, and the policy decides
  where the Insert session's run is cut.
- **Insert-mode Vim chords are off:** `Ctrl-O`, `Ctrl-W`, `Ctrl-R`,
  `Ctrl-N/P`. Completion is `Ctrl+Space`, accept with Enter or Tab.

Vim's behaviour under `EditPolicy::Vim` is bit-for-bit what it is today;
`fenix-vim`'s existing tests run under it unchanged.

### 3.3 The GUI's mode checks (`fenix-gui`)

Every `Mode::Normal` / `is_inserting()` test outside rendering is replaced
by one of a few predicates on `App`, each with one meaning:

| Predicate | Vim | Standard |
|---|---|---|
| `at_rest()` | Normal, idle, no leader pending | No selection, no popup, no pending chord |
| `typing_text()` | Insert / Replace | A writable text buffer is focused |
| `letter_keys_act()` | Normal | The focused buffer is read-only (Home, diffs, Task Output, side by side's branch pane) |
| `leader_starts(key)` | `SPC` in Normal / Visual | the leader chord (§2.2), anywhere outside a terminal or capturing prompt |

Then the places that assumed Normal:

- **Compose panes** (commit messages, Review's comments, `compose()` /
  `compose_with_ask()` in the API): Vim keeps "Esc then Enter sends, `q`
  discards". Standard: `Ctrl+Enter` sends, Esc asks "discard?" when
  something is written, and the agent's draft is `Ctrl+Shift+A` plus a
  button on the pane's title. The hint line (`app.rs:581`) is built from the
  bound keys (`key_for`, §4), not literal text. Extensions call the same
  functions and need no change.
- **The agents' thread prompt** (`agents_composer_key`): `Ctrl+Enter`
  sends, as above.
- **Side by side's `q`, Home's Enter, the refactor review, Task Output:**
  read-only buffers, so `letter_keys_act()` keeps them working in both
  schemes. `Ctrl+W` (`window.close`) closes them too.
- **The mouse** (`app/mouse.rs`): click "Insert stays Insert" is already
  right. A drag enters SELECT instead of Visual. The right-click menu runs
  the commands from M2.
- **The debugger:** F-keys already work in every mode. DEBUG mode's letter
  keys (`n s o c r`) apply while stopped and `at_rest()`.

### 3.4 What it looks like

- Modeline badge: standard mode shows no badge while typing, `SELECT` with a
  selection, `DEBUG` as today. The `[modes]` theme colours gain `select`.
- Which-key shows `C-k` where it shows `SPC` today. Labels are unchanged.
- `SPC h k` / `C-k h k` ("what does this key run") works for chords.

### 3.5 Choosing and switching

- The welcome page asks once, before the packs: "How do you like to edit?"
  -- *Vim keys* (default) / *Standard keys*, each with a three-line preview.
- `SPC t k` / the palette's `view.key_scheme` toggles it at once: the policy
  is set on every `VimState`, the chord map and leader are rebuilt
  (`reload_leader`), extensions get `Event::KeySchemeChanged`.
- `SPC ,` lists it under Editor, with the leader chord beside it.

### 3.6 The tutorial

Lessons (`crates/fenix-gui/tutorial/*.toml`) get `scheme = "vim" |
"standard" | "any"`. The `editing-*` lessons are Vim's. A new
`standard-1-basics.toml` covers typing, selecting, the clipboard, undo and
`Ctrl+K`. The `fenix-*` lessons (leader, find, windows, pages) become `any`,
with the leader written as `{leader}` and expanded for the scheme in use. An
extension's lessons default to `any`.

---

## 4. Extension API 1.5: the key scheme

Small, optional and read-only. An extension that ignores it behaves exactly
as it does on 1.4.

```rust
/// Which keys the person edits with.
#[non_exhaustive]
pub enum KeyScheme { Vim, Standard }

/// The key scheme in use. No permission needed.
pub fn key_scheme() -> KeyScheme;

/// The keys that run `command` now, as which-key writes them (`gd`,
/// `<F12>`, `C-k g g`): yours, then the scheme's. `None` when it has none.
/// For hints: "`{}` on a MIB name opens its page".
pub fn key_for(command: &str) -> Option<String>;

// And a new event, when the setting changes:
Event::KeySchemeChanged(KeyScheme)
```

- `key_for` is the one most extensions want. It also follows the user's own
  rebinding, which `key_scheme` alone can't, and is the same function core
  uses for the compose hint.
- Host side: two imports in `fenix-wasm` (`editor_key_scheme`,
  `editor_key_for`), the event in the dispatch that already sends
  `focus_moved` and the like, `API` to `"1.5.0"` in `manifest.rs`, and the
  table in `docs/WRITING_EXTENSIONS.md`.
- **Keymap files:** `[[keys]]` (§3.1) is accepted in an extension's
  `keymaps/*.toml`, with `scheme` and `when`. An extension can give its
  headline commands a standard chord (Database: `C-Enter` runs the
  statement under the cursor) without touching its code. Validation is the
  same as yours: an unknown command or scheme is reported, not bound.
- **Page keys are not affected.** `Page::key(keys, what)` hints are the
  page's own keys and stay literal.

---

## 5. Milestones

| | Milestone | Size | Behaviour change |
|---|---|---|---|
| M0 | ADR 0004: §2's decisions | S | none |
| M1 | Shift in `Mods`, the notation, `[[keys]]`, the hard-coded chords moved into `default_keymap.toml` | M | none in Vim mode; chords become rebindable |
| M2 | Vim-only actions as commands: `code.hover`, `code.goto_definition`, `code.references`, `edit.cut`/`copy`/`paste`/`select_all`, `nav.goto_line`, `code.toggle_comment`; the right-click menu runs them | S | none |
| M3 | `EditPolicy::Standard` in `fenix-vim` (§3.2), with its own unit tests | L | none until switched on |
| M4 | The GUI predicates and the places in §3.3; modeline and which-key (§3.4); the leader chord | M | none in Vim mode |
| M5 | `editor.keys`, live switching, the welcome question, the standard chord set in `default_keymap.toml`, `KEYBINDINGS.md`'s standard section | M | the feature, opt-in |
| M6 | API 1.5 (§4) and `[[keys]]` in extensions' keymaps | S | none for existing extensions |
| M7 | fenix-extensions (§6) | S–M | hints read right in both schemes |
| M8 | The tutorial (§3.6) | M | standard users get lessons |

M1 and M2 are worth landing whatever happens to the rest. M3 can be written
next to M1/M2. M5 waits for M3 and M4.

---

## 6. fenix-extensions

Nothing breaks: no extension reads Vim's mode, and every `[[leader]]` and
`SPC m` entry is reached through `Ctrl+K`. The work is wording and
convenience:

- **Hints that name editor keys** move to `key_for` (API 1.5) where they're
  drawn in Fenix: MIB (`K` on a name, `gd`), Tcl (`K`, `gd`), CCSDS (`K` on
  hex or a time), Notebook (`gd` on a link), PDF and Images (`gf`).
- **READMEs** that say `K`, `gd`, `gf`, or "`A` in Normal mode" (Review) get
  the standard key beside the Vim one, in the same table cell.
- **Compose users** (Review's comments and summary, any `compose_with_ask`):
  no code change. Check them in both schemes.
- **Pages with `more_keys`** (Hex, Logs, CSV, Parquet, PDF, Database's
  table): already take their Ctrl chords before core. Check that none of
  them uses `Ctrl+K` (the leader chord) or `Ctrl+Shift+P`. Those two are
  reserved like `Ctrl+\`.
- **Optional standard chords** in a few extensions' `keymaps/*.toml` for
  their most used command (Database `C-Enter`, HTTP `C-Enter`, Containers'
  page).
- **`xtask validate`** checks `[[keys]]` entries: a known `scheme` and
  `when`, and a warning when a chord shadows one of core's standard chords.

---

## 7. Testing

- **`fenix-vim`:** the existing suite unchanged under `EditPolicy::Vim`; a
  new suite for `Standard` (selection replace, Shift+motions, clipboard
  line copy, the undo breaks, Esc never leaving Insert).
- **Keymap:** default `[[keys]]` reproduce today's chords exactly (a table
  test against the old `if/else`); Shift normalization (`A` binds as
  before, `C-S-p` and `S-<Down>` round-trip through `parse_keys` /
  `describe_keypress`); macros replay Ctrl+Shift+Z as redo.
- **Coverage:** every command bound in the default leader map is reachable
  in standard mode, bound or through `Ctrl+K`, and every M2 command has a
  standard chord. A test fails when a new `[[leader]]` entry isn't.
- **Conflicts:** core's standard chords against the keys each core page
  claims; `SPC h K` reports none on a clean install.
- **GUI:** the app tests that exercise compose, side by side, Home, the
  agents' prompt and the mouse run once per scheme (a test helper sets the
  policy).
- **Extensions:** fenix-extensions' suites against a host on API 1.5; a
  `key_for` / `KeySchemeChanged` round trip in `fenix-wasm`'s tests.

---

## 8. Risks and open questions

- **Undo granularity** is the part users will notice first. The break rules
  in §3.2 are a starting point. Tune them on real use before M5 ships.
- **`Ctrl+K`.** Fine on every layout we know, but it's a decision for the
  ADR. Alternatives: `Ctrl+;`, `F1`.
- **Ctrl+Alt on Windows** is AltGr on some layouts. Standard chords avoid
  Ctrl+Alt+letter.
- **Two schemes to keep working.** The ground rules (commands first, the
  predicates, the coverage test) are what keep this cheap. A change that
  needs a scheme check outside §3.3's predicates should be questioned in
  review.
- **Pressure to grow standard mode** (multi-cursor, IDE-exact behaviour).
  §2.5 is the line for v1; anything past it is its own plan.
- **Docs drift.** `KEYBINDINGS.md` gets a standard column only for what
  differs, not a second copy of the whole file.
