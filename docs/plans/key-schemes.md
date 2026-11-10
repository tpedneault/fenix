# Plan: key schemes -- Vim, and standard keys

- **Status:** decisions reviewed 2026-11-09 (§10 lists every one), recorded
  in [ADR 0004](../adr/0004-key-schemes.md). Implemented for Fenix 1.5;
  §12 lists where the implementation differs from this plan
- **Scope:** a second way to edit in Fenix, for people who want the keys of a
  traditional IDE (no modes, no leader typed as `SPC`), with nothing taken
  away from the Vim and leader experience that stays the default
- **Lands as:** [ADR 0004](../adr/0004-key-schemes.md), then the milestones in §8, one PR each. The
  extensions' part (§7) lands in fenix-extensions after API 1.5

## The idea in one paragraph

A **key scheme** is chosen with one setting, `editor.keys = "vim" |
"standard"`. Vim stays the default and works as it does today. "Standard" is
the same editor with a different policy on top: text buffers rest in typing
mode, Shift+arrows select, Alt+click adds a cursor, and the usual IDE
shortcuts (modelled on VS Code's Windows/Linux defaults) run Fenix's
commands. The whole leader tree, every extension's `SPC` menu included, sits
behind one chord, `Ctrl+K`, with which-key showing what comes next. Both
schemes are bindings for the **same commands**: core gets no feature only
one scheme can reach. Extensions keep working unchanged. API 1.5 adds a
small, optional way for them to ask which scheme is on and which keys a
command has, so their hints can say `F12` instead of `gd`.

### Ground rules

- **Vim is the product's voice.** The default, the README, the tutorial's
  first lessons and `KEYBINDINGS.md` stay Vim-first. Standard keys are
  documented beside them, not instead of them.
- **Commands, not keys.** Every action either scheme binds is a command id
  (`SPC h c` describes it, the palette runs it). An action that today exists
  only as a Vim key (`K`, `gd`, `]d`) or only as an Ex command (`:sort`)
  becomes a command first (M2).
- **One editor engine.** Standard mode is a policy inside `fenix-vim`, not a
  second editing implementation. Undo, registers, selections, text objects,
  snippets, completion and LSP edits are shared.
- **Pages keep their keys.** A page's own keys (`j`/`k`, `s`, `q` on Git, the
  explorer, an extension's page) mean the same in both schemes, the way
  lazygit's do. Letter keys act where you can't type.
- **No `if scheme == …` scattered through `app.rs`.** The scheme is read in
  a few named predicates (§4.3) and in the keymap data. Everything else asks
  those.
- **No key text written by hand.** A hint that names a key gets it from the
  keymap (§4.6), so it's right in either scheme and after your rebinding.

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
| Cursors | One `Cursor { char_idx, sticky_col }` per pane (`crates/fenix-core/src/cursor.rs`). V-BLOCK's `I`/`A` replay what's typed on Esc (`BlockInsert`, `fenix-vim/src/state.rs:294`). Alt-drag selects a block (`app/mouse.rs:286`) | Multiple cursors are new |
| Mode checks in the GUI | `Mode::Normal` / `is_inserting()`: 18 in `app.rs`, 7 in `app/mouse.rs`, 4 in `tutorial_host.rs`, a few elsewhere | Page behaviours assume Normal: compose's "Esc then Enter" (`app.rs:581`), `A` for the agent, `q` on side by side, Enter on Home, DEBUG's letter keys |
| Vim-only actions | `K`, `gd`, `gr` are `VimAction::RequestLsp`. Bracket jumps (`]d ]h ]c ]t ]a`) are `VimAction::BracketJump` (`fenix-vim/src/keymaps.rs:277`). The right-click menu has its own `MenuAction::{Definition, References, Hover, Cut, Copy, Paste, SelectAll}` (`app/mouse.rs:90`) | Implementations exist; they aren't commands |
| Ex-only actions | `:sort`, `:lsp-restart`, `:[range]m`/`t`, `:norm` | Unreachable without `:` |
| Buffers you don't type in | No read-only flag: `BufferKind` (`Dashboard`, `Explorer`, `Git`, `WorkspaceEdit`, …) and the pages' own handlers. They're moved around with Normal-mode keys | Standard mode needs a way to browse them (§4.3) |
| Finding in a file | `SPC s s` opens the Search page on this file; `SPC p n`/`N` step through the last search's matches | Standard's `Ctrl+F` uses it (§2.7) |
| The mouse in text | Click, drag, double/triple click, Shift-click, Alt-drag, right-click menu, hover on rest. No Ctrl+click | Ctrl+click to go to definition is new |
| Tabs | A pane's last tab closing falls back to Home: a pane never empties. Tabs have no × button | §2.9 |
| Page key order | `page_key` (`app.rs:16180`) runs before the global chord block; a `more_keys` page gets Ctrl chords first (`app/pages.rs:798`) | Hex's and Logs' Ctrl+F/B already win on their pages |
| Terminals | A focused terminal takes every key but `Ctrl+\` | Standard needs a few chords to skip the shell (§4.5) |
| Hint text | About 250 string literals in `fenix-gui` name keys (`SPC …`, "Esc then Enter", "q discards"). The control protocol's `CommandInfo.keys` gives outside tools the leader keys (`fenix-control/src/protocol.rs:259`) | Wrong in standard mode unless generated (§4.6) |
| Extension API | 1.4.0 (`crates/fenix-extensions/src/manifest.rs:14`). No editor mode is exposed. Pages get `page_key` / `page_key_press` | Extensions don't depend on Vim modes; the assumptions are in core |

---

## 2. Decisions (ADR 0004)

### 2.1 A core setting, with the base shortcuts in core

`editor.keys`, `Kind::Choice(&["vim", "standard"])`, default `vim`, not
project-overridable (it's about the person, not the code). The standard
shortcuts ship in core's `default_keymap.toml` (entries marked `scheme =
"standard"`), so the setting works without installing anything and is
covered by core's tests. Flavours (a JetBrains-like set, say) can come later
as data-only extensions with their own `keymaps/*.toml`.

### 2.2 The leader chord: `Ctrl+K`

In standard mode `Ctrl+K` opens the leader exactly as `SPC` does in Normal
mode: same trie, same which-key popup, so `Ctrl+K g g` is `SPC g g`, and
every extension's `[[leader]]` and `SPC m` entry comes with it. VS Code
users already know `Ctrl+K` as a chord prefix, and a leader entry can take a
Ctrl key next (`SPC C-i`), so VS Code's `Ctrl+K Ctrl+I` works as written.
`Ctrl+Space` stays free for completion. Configurable as `keys.leader_chord`.

### 2.3 Key notation

Keymap files and which-key write chords as `C-` (Ctrl), `M-` (Alt), `s-`
(Super), `S-` (Shift): `C-S-p`, `S-<Down>`, `<F12>`. Today
`describe_keypress` writes Super as `S-`. It moves to `s-` before Shift is
added, so `S-` is never ambiguous.

### 2.4 Selection in standard mode is Visual under the hood

Shift+motion or a mouse drag enters charwise Visual with a standard-mode
flag: typing replaces the selection, Backspace/Delete remove it, an
unshifted arrow collapses it, Esc drops it and stays typing. This is Vim's
own Select mode, so operators, `format_selection`, `Ctrl+K c f` and "ask the
agent about the selection" all keep working. The modeline says `SELECT`
(§4.4).

### 2.5 What standard mode doesn't do

- No `:` command line (`:` types a colon). What only `:` could do gets a
  command: `edit.sort_lines` (the selection or the buffer; reverse, ignore
  case, numeric and unique asked in a picker) and `lsp.restart` (a picker
  of the project's servers). Moving and copying lines are `Alt+↑/↓` and
  `Shift+Alt+↑/↓`. `:norm`, ranges and `:g` stay Vim's.
- No `.` repeat and no macro keys.
- No attempt to match any one IDE exactly. The bar: productive on day one.
- No macOS keys (Cmd for Ctrl): Fenix doesn't ship there today.

### 2.6 Multiple cursors, with the usual click

| Input | Does |
|---|---|
| Alt+click | Add a cursor there; on an existing cursor, remove it |
| Alt+Shift+drag | A column of cursors, one per line (column selection), at most 10,000 |
| Esc | Back to the one primary cursor |
| A plain click | One cursor, there |

`editor.multi_cursor_modifier = "alt" | "ctrl"` (default `alt`) is for
desktops whose window manager takes Alt+click (KDE, some X11 setups). With
`ctrl`, cursors are Ctrl+click and go to definition moves to Alt+click.

Vim mode doesn't change: Alt-drag still selects a block and V-BLOCK works as
it does. Keyboard ways to add cursors (`Ctrl+D` for the next occurrence,
`Ctrl+Alt+↑/↓`) are a follow-up on the same cursor set, not v1. Extra
cursors aren't restored with the session; agents and outside tools see the
primary cursor's selection.

### 2.7 The standard shortcuts

Modelled on VS Code's Windows/Linux defaults. "new" marks a command M2 adds.
Everything not listed is reached through `Ctrl+K` (the leader), the palette,
or the mouse.

**Files and tabs**

| Keys | Command |
|---|---|
| `Ctrl+P` | `project.find_file` |
| `Ctrl+O` | `file.find` (open by path) |
| `Ctrl+N` | `buffer.scratch` |
| `Ctrl+S` | `file.save` (as today) |
| `Ctrl+W` | close the tab (`SPC b d`'s command) |
| `Ctrl+Shift+T` | reopen the closed tab (`SPC b u`'s) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | a recently-used tab switcher (new `buffer.switch_recent`): held Ctrl, Tab steps, release picks |
| `Ctrl+PageDown` / `Ctrl+PageUp` | next / previous tab (as today) |
| `Ctrl+E` | `file.recent` |

**Editing** (most are the policy's, §4.2)

| Keys | Does |
|---|---|
| `Ctrl+Z`, `Ctrl+Y` / `Ctrl+Shift+Z` | undo, redo (as today) |
| `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | copy, cut, paste (new `edit.copy`/`cut`/`paste`); with nothing selected, the line |
| `Ctrl+A`, `Ctrl+L` | select all (new), the line |
| `Ctrl+/` | toggle a comment (new `code.toggle_comment`; no new Vim key) |
| `Alt+↑/↓`, `Shift+Alt+↑/↓` | move lines, copy lines (new `edit.move_lines_*`, `edit.copy_lines_*`) |
| `Tab` | in order: a snippet's next stop, accept completion, indent the selection, insert an indent. `Shift+Tab` dedents |
| `Ctrl+Space` | completion |
| `Ctrl+G` | go to a line (new `nav.goto_line`) |

**Finding** -- the Search page, as `SPC s …` uses it

| Keys | Command |
|---|---|
| `Ctrl+F` | `search.file` (this file, or the selection) |
| `Ctrl+H` | `search.replace_buffer` |
| `Ctrl+Shift+F` / `Ctrl+Shift+H` | `search.project` / `search.replace_project` |
| `F3` / `Shift+F3` | `project.quickfix_next` / `_prev`: the last search's next / previous match |

**Code**

| Keys | Command |
|---|---|
| `F12`, `Ctrl+click` | new `code.goto_definition` |
| `Shift+F12` | new `code.references` |
| `Ctrl+F12` | `code.implementations` |
| `Ctrl+K Ctrl+I` | new `code.hover` (the debugger's lens while stopped) |
| `F2` | `code.lsp_rename` |
| `Ctrl+.` | `code.lsp_code_action` |
| `Shift+Alt+F` | `code.format_buffer` (`Ctrl+K c f` the selection) |
| `Ctrl+Shift+O` | `code.outline` |
| `F8` / `Shift+F8` | new `nav.next_problem` / `nav.prev_problem` |
| `Alt+F5` / `Shift+Alt+F5` | new `nav.next_hunk` / `nav.prev_hunk` |
| `Alt+←` / `Alt+→` | `nav.jump_back` / `nav.jump_forward` (Ctrl+O/I in Vim mode) |

The other bracket jumps -- an agent's next change, the next TODO, side by
side's next change, the debugger's trail -- become commands too and get
leader keys under new `SPC ]` / `SPC [` groups (`SPC ] a`, `SPC [ t`, …),
which Vim mode has as well.

**The workbench**

| Keys | Command |
|---|---|
| `Ctrl+Shift+P`, `F1` | `commands.palette` (`Alt+X` as today) |
| `Ctrl+K` | the leader |
| `Ctrl+B` | `explorer.toggle_sidebar` |
| `Ctrl+0` | focus the sidebar (new `window.focus_sidebar`) |
| `Ctrl+Shift+E` | `explorer.reveal_current` |
| `Ctrl+J` | show or hide the terminal panel (`terminal.toggle`) |
| ``Ctrl+` `` | to the terminal panel and back (new `terminal.focus`) |
| `Ctrl+,` | the settings page (`SPC ,`'s) |
| `Ctrl+=`, `Ctrl+-`, `Ctrl+0`… | font size as today, except `Ctrl+0`, which standard gives the sidebar; reset stays on `Ctrl+K t 0` |
| `Ctrl+Q` | quit (as today) |

**The debugger:** F5, Shift+F5, Ctrl+Shift+F5, F6, F9, F10, F11, Shift+F11
as today, in every mode.

### 2.8 Windows and panes

Splitting, closing and moving between panes stay on the leader: `Ctrl+\` is
not taken (it stays "leave the terminal").

| Keys | Does |
|---|---|
| `Ctrl+K w v` / `Ctrl+K w s` | split right / down |
| `Ctrl+K w q` / `w o` / `w =` | close the pane / the others / balance |
| `Ctrl+K w ←/→/↑/↓` | to the pane in that direction (new arrow aliases of `h/j/k/l`, in both schemes) |
| `Ctrl+1` … `Ctrl+9` | to pane N (new `window.focus_nth`) |
| `Ctrl+Alt+→` / `Ctrl+Alt+←` | move the tab to the next / previous pane (new `window.move_tab_next` / `_prev`) |
| `Ctrl+Shift+N` | another OS window (`SPC w n`'s) |
| The mouse | as today: drag a tab to a pane's edge to split, right-click a tab, drag a border to resize |

- **Closing a split pane's last tab closes the pane** in standard mode. The
  last pane left shows Home. Vim mode keeps Home in its place.
- **Tabs get a × button, and the tab strip a split button**, shown on
  hover, in both schemes.
- Workspaces stay on the leader (`Ctrl+K Tab …`). Panes are resized with the
  mouse; no resize keys in v1.

### 2.9 Discoverability: no menu bar

Standard users find things through the palette (`Ctrl+Shift+P`) and the
leader's which-key. v1 adds two helps and no menu bar:

- The modeline shows `Ctrl+K commands` until the leader has been used a few
  times (`keys.leader_hint = true | false`).
- A keys page: every standard shortcut, by area, generated from the keymap
  (so your rebinding shows), on a new `help.keys_sheet` under `Ctrl+K h`.

---

## 3. Who's affected

| | Vim users | Standard users | Extensions |
|---|---|---|---|
| M1 (keys as data) | can rebind Ctrl chords; nothing else changes | -- | can bind chords in `[[keys]]` (with API 1.5) |
| New commands (M2) | in the palette, `SPC ]`/`SPC [`, arrow aliases under `SPC w` | bound per §2.7 | can call them |
| × and split buttons on tabs | appear on hover | appear on hover | -- |
| Everything else | unchanged | the feature | hints via `key_for`, optional |

---

## 4. Core design

### 4.1 Keys (`fenix-keymap`, `fenix-gui/src/keymap.rs`)

- **Shift in `Mods`.** `to_keypress` normalizes it: for a printable
  character with no Ctrl or Alt, shift is dropped (the character already
  says `A` vs `a`, so every existing binding and Vim's grammar stay as they
  are). With Ctrl/Alt, or on a named key, it's kept. Macro capture records
  it, which fixes the replay gap the comment at `app.rs:16210` describes.
  F-keys get `NamedKey` variants (today they're handled before
  `to_keypress`, which returns `None` for them).
- **A `[[keys]]` table** in every keymap file, beside `[[leader]]`:

  ```toml
  [[keys]]
  keys = "C-S-p"
  command = "commands.palette"
  when = "anywhere"      # anywhere | editor | text | typing | page | terminal
  scheme = "standard"    # vim | standard | any (default any)
  ```

  `when` is a short fixed list, not an expression language. `terminal`
  marks a chord that skips the shell (§4.5). `apply_user_keys` grows a
  sibling that reads `[[keys]]`. `keybindings.toml` can bind and unbind
  them, `SPC h b` / `SPC h K` list them, and the palette's "give it a key"
  (`Tab`) can write one (`fenix-config/src/keybindings.rs`).
- **The hard-coded chords become data.** The Ctrl block in `route_keypress`
  turns into a lookup in a `ChordMap` built like the leader trie: core's
  entries with `scheme = "any"` (or `"vim"`, for Ctrl+O/I) reproduce today's
  chords exactly. No behaviour change in Vim mode, and Vim users can now
  rebind them.
- **Order stays as it is.** Capturing prompts → terminals (but for their
  skip list) → pickers → the focused page (which can claim any chord) → the
  chord map → the leader → `fenix-vim`. New standard chords can't take keys
  from a page that claims them.

### 4.2 Editing policy (`fenix-vim`)

`VimState::set_policy(EditPolicy::Vim | EditPolicy::Standard)`. Under
`Standard`:

- **In a buffer you type in, the resting mode is Insert.** Opening,
  switching or focusing one lands in Insert. Esc closes popups and drops a
  selection or the extra cursors; it never enters Normal. Commands that
  went through Normal internally (`one_shot_from`) still can, invisibly.
- **Movement:** arrows, `Ctrl+←/→` by word, `Home` to the first non-blank
  then column 0, `Ctrl+Home/End`, PageUp/PageDown (already GUI-side).
- **Selection:** the Shift variant of every movement extends (§2.4).
- **Editing:** `Ctrl+Backspace/Delete` by word; the rest as §2.7.
- **Clipboard:** the system clipboard register (`"+`). Copy and cut with no
  selection take the line.
- **Undo steps.** Typing coalesces into one step until a break: whitespace
  after a word, Enter, a cursor jump (anything but typing moving it), a
  paste, a pause over `editor.undo_pause_ms` (default 1000). Tuned on real
  use before M6 ships.
- **Insert-mode Vim chords are off:** `Ctrl-O`, `Ctrl-W`, `Ctrl-R`,
  `Ctrl-N/P`.

Vim's behaviour under `EditPolicy::Vim` is bit-for-bit what it is today;
`fenix-vim`'s existing tests run under it unchanged.

### 4.2a Multiple cursors (`fenix-core`, `fenix-vim`, `fenix-gui`)

- **The cursor set.** A pane holds `Cursors { primary: Cursor, extra:
  Vec<Caret> }`, where a `Caret` is a cursor plus an optional selection
  anchor. `primary` stays the plain `Cursor` it is today, so code that only
  knows one cursor (the viewport, LSP requests, the jumplist, session
  restore, every Vim path) reads it and is unaffected. `extra` is empty
  except in standard mode.
- **Edits are applied at every caret, live.** Each typed character,
  Backspace/Delete, Enter (with its auto-indent), paste and movement runs
  once per caret, from the last in the buffer to the first, inside the one
  undo group `handle_key` already opens. One undo takes back the keystroke
  at every caret.
- **Carets stay sane.** After each key, carets that meet or whose
  selections overlap merge. Edits that aren't the carets' own (an LSP edit,
  an agent's hunk, a format, a reload) map them the way marks are mapped
  today; a caret whose text is deleted collapses to the edit's start.
- **Clipboard.** Copy/cut with N carets puts N pieces on the clipboard, one
  per line. Paste with the same N gives each caret its piece; any other
  count pastes the whole text at every caret.
- **Primary only:** completion's popup (the accepted text is then typed at
  every caret), snippets' tab stops, hover, signature help, rename, and
  every command that isn't plain text editing. A command that moves the
  primary elsewhere (go to definition, a picker) first drops the extras.
- **Drawing.** Extra carets are drawn like the primary, without the motion
  glide. The modeline shows `3 cursors` while there's more than one.
- **The mouse** (`app/mouse.rs`): Alt+click and Alt+Shift+drag as §2.6.

### 4.3 The GUI's mode checks (`fenix-gui`)

**Typing or browsing.** Each `BufferKind` (and each pane-resident page)
says whether you type in it. `Text` and `Table` do; Home, the explorer
listing (except while its names are edited, `SPC e w`), Git views, diffs,
side by side's branch pane, the refactor review and Task Output don't. In
standard mode, a buffer you don't type in **browses**: Normal mode
underneath, so its Vim motions and its own keys (Enter, `q`, `s`, `d`, …)
work as they do today, and arrows, PageUp/PageDown, Home/End,
Shift+arrows and `Ctrl+C` work as standard users expect. No badge is shown.

Every `Mode::Normal` / `is_inserting()` test outside rendering is replaced
by one of a few predicates on `App`:

| Predicate | Vim | Standard |
|---|---|---|
| `at_rest()` | Normal, idle, no leader pending | Browsing; or typing with no selection, popup or pending chord |
| `typing_text()` | Insert / Replace | A buffer you type in is focused |
| `letter_keys_act()` | Normal | Browsing |
| `leader_starts(key)` | `SPC` in Normal / Visual | the leader chord, anywhere outside a terminal or capturing prompt |

Then the places that assumed Normal:

- **Compose panes** (commit messages, Review's comments, `compose()` /
  `compose_with_ask()`): Vim keeps "Esc then Enter sends, `q` discards".
  Standard: `Ctrl+Enter` sends, Esc asks "discard?" when something is
  written, `Ctrl+Shift+A` has the agent write it (plus a button on the
  pane's title). Extensions call the same functions and need no change.
- **The agents' prompt and question cards:** `Ctrl+Enter` sends, as it
  already does from Insert. Today `y`/`Y`/`n` answer a question card "from
  an empty prompt", and the thread takes no clicks (a click only moves focus
  between the prompt and the thread, `app/agents/dock.rs:253`). In standard
  mode:
  - **Each answer on a card is a button.** It's drawn as the card's answer
    row is today, on its key's cap, lit under the pointer, and a click gives
    that answer. A form's fields open their picker or the prompt line on a
    click; a link card's "open" is a button too.
  - **The keys still work:** plain (`y`, `Y`, `n`, …) with the thread
    focused, and with Alt (`Alt+Y`, `Alt+N`, …) from the prompt, where the
    plain letters are typed text.
  - A pipeline's gate is a card like the others: its `y` / `e` / `n` are
    buttons, and the same keys with Alt from the prompt.
- **Side by side's `q`, Home's Enter, the refactor review, Task Output:**
  browsing, so `letter_keys_act()` keeps them. `Ctrl+W` closes them too.
- **The file explorer** (browsing) also takes `F2` to rename, `Delete` to
  delete (with its confirm), Enter to open, beside its letter keys.
- **The mouse** (`app/mouse.rs`): a click keeps typing; a drag enters
  SELECT; `Ctrl+click` goes to definition (§2.6 for the modifier); the
  right-click menu runs the M2 commands.
- **The debugger:** the F-keys only. DEBUG mode's letter keys (`n s o c r`)
  are off in standard mode, where they'd be typed text.
- **Tabs and panes:** §2.8's last-tab rule; the × and split buttons.

### 4.4 What it looks like

- Modeline: no mode badge while typing or browsing; `SELECT` with a
  selection, `3 cursors`, `DEBUG` as today. The `[modes]` theme colours gain
  `select`. The mode rail stays the neutral colour unless SELECT or DEBUG.
- The caret is a bar, always (Vim mode's block in Normal doesn't apply).
- Line numbers default to absolute.
- These are defaults: your own settings win.
- Which-key shows `C-k` where it shows `SPC` today. Labels are unchanged.
- `SPC h k` / `Ctrl+K h k` ("what does this key run") works for chords.

### 4.5 Terminals

A focused terminal still takes every key, but for the chords marked `when
= "terminal"`. In standard mode those are ``Ctrl+` ``, `Ctrl+J`, `Ctrl+P`,
`Ctrl+Shift+P`, `Ctrl+Tab` and `Ctrl+B`. `Ctrl+K` stays the shell's
(readline's kill-line), and `Ctrl+\` leaves the terminal in both schemes.
Vim mode's list is empty, as today.

### 4.6 Hints say the keys in use

- `keymap::key_for(command) -> Option<String>`: the keys that run a command
  now, yours first, then the scheme's (`F12`, `C-k g g`, `gd`). Core's hint
  text uses it: the compose line (`app.rs:581`), messages, notifications,
  empty pages, the welcome, the about-to-do confirmations -- the ~250
  literals in `fenix-gui` that name keys today.
- A test fails on new string literals in `fenix-gui` that look like key text
  (`SPC x`, "Esc then", "press q") outside the keymap and tests.
- The control protocol's `CommandInfo.keys` (`fenix-control/src/
  protocol.rs:259`) is `key_for`'s answer, so outside tools and agents are
  told the keys in use.

### 4.7 Choosing and switching

- The welcome page asks once, before the packs: "How do you like to edit?"
  -- *Vim keys* (default) / *Standard keys*, each with a three-line preview.
  On Linux, the standard choice mentions `editor.multi_cursor_modifier`.
- `SPC t k` / the palette's `view.key_scheme` switches at once: the policy
  is set on every `VimState`, the chord map and leader are rebuilt
  (`reload_leader`), extensions get `Event::KeySchemeChanged`.
- `SPC ,` lists it under Editor, with the leader chord beside it.

### 4.8 The tutorial

Lessons (`crates/fenix-gui/tutorial/*.toml`) get `scheme = "vim" |
"standard" | "any"`. The `editing-*` lessons are Vim's. A new
`standard-1-basics.toml` covers typing, selecting, multiple cursors, the
clipboard, undo, find and `Ctrl+K`. The `fenix-*` lessons (leader, find,
windows, pages) become `any`, with keys written as `{leader}` or
`{key:command.id}` and expanded through `key_for`. An extension's lessons
default to `any`.

---

## 5. Extension API 1.5: the key scheme

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

- `key_for` is the one most extensions want: it also follows your own
  rebinding, and it's the same function core's hints use (§4.6).
- Host side: two imports in `fenix-wasm` (`editor_key_scheme`,
  `editor_key_for`), the event in the dispatch that already sends
  `focus_moved` and the like, `API` to `"1.5.0"` in `manifest.rs`, and the
  table in `docs/WRITING_EXTENSIONS.md`.
- **Keymap files:** `[[keys]]` (§4.1) is accepted in an extension's
  `keymaps/*.toml`, with `scheme` and `when` (but not `terminal`, which is
  core's and yours). An extension can give its headline commands a standard
  chord (Database: `C-Enter` runs the statement under the cursor) without
  touching its code. An unknown command or scheme is reported, not bound.
- **Reserved:** pages can't claim `Ctrl+K`, `Ctrl+Shift+P` or `Ctrl+\`.
- **Page keys are not affected.** `Page::key(keys, what)` hints are the
  page's own keys and stay literal.

---

## 6. Docs

- `KEYBINDINGS.md`: a "Standard keys" section with §2.7 and §2.8's tables,
  and a standard column in the leader table only where a row differs (it
  rarely does: `Ctrl+K` is `SPC`).
- `AGENTS.md`: the prompt and question cards in standard mode (§4.3).
- `CONFIGURATION.md`: `editor.keys`, `keys.leader_chord`,
  `editor.multi_cursor_modifier`, `editor.undo_pause_ms`, `keys.leader_hint`.
- `WRITING_EXTENSIONS.md`: API 1.5 and `[[keys]]`.
- README: one line under Highlights that standard keys exist.

---

## 7. fenix-extensions

Nothing breaks: no extension reads Vim's mode, and every `[[leader]]` and
`SPC m` entry is reached through `Ctrl+K`. The work is wording and
convenience:

- **Hints that name editor keys** move to `key_for` where they're drawn in
  Fenix: MIB (`K` on a name, `gd`), Tcl (`K`, `gd`), CCSDS (`K` on hex or a
  time), Notebook (`gd` on a link), PDF and Images (`gf`).
- **READMEs** that say `K`, `gd`, `gf`, or "`A` in Normal mode" (Review) get
  the standard key beside the Vim one, in the same table cell.
- **Compose users** (Review's comments and summary, any `compose_with_ask`):
  no code change. Check them in both schemes.
- **Pages with `more_keys`** (Hex, Logs, CSV, Parquet, PDF, Database's
  table): already take their Ctrl chords before core. Check that none
  claims a reserved chord (§5).
- **Optional standard chords** in a few extensions' `keymaps/*.toml` for
  their most used command (Database `C-Enter`, HTTP `C-Enter`).
- **`xtask validate`** checks `[[keys]]` entries: a known `scheme` and
  `when`, and a warning when a chord shadows one of core's standard ones.

---

## 8. Milestones

Sizes: S is a few days, M one to two weeks, L more. Each milestone is one PR
or a few, and each leaves `master` releasable: nothing a Vim user sees
changes until M6 switches the feature on, except the new commands (M2) and
the tab buttons (M4b).

| | Milestone | Size | Needs | Behaviour change |
|---|---|---|---|---|
| M0 | ADR 0004 | S | -- | none |
| M1 | Keys as data | M | M0 | none in Vim mode; Ctrl chords become rebindable |
| M2 | Commands for every action | M | M1 | new commands; Vim keys unchanged |
| M3 | The standard editing policy | L | M1 | none until switched on |
| M3b | Multiple cursors | M–L | M3 | none until switched on |
| M4a | The GUI's foundations | M | M1, M3 | none in Vim mode |
| M4b | Pages, panes and the mouse | L | M2, M4a | tab buttons for everyone |
| M5 | Hints from the keymap | M | M1 | none in Vim mode |
| M6 | Standard mode ships | M | M3b, M4b, M5 | the feature, opt-in |
| M7 | Extension API 1.5 | S | M5 | none for existing extensions |
| M8 | fenix-extensions | S–M | M7 | hints read right in both schemes |
| M9 | The tutorial | M | M6 | standard users get lessons |

```text
M0 ─ M1 ─┬─ M2 ───────────────┐
         ├─ M3 ─┬─ M3b ───────┤
         │      └─ M4a ─ M4b ─┼─ M6 ─ M9
         └─ M5 ─┬─────────────┘
                └─ M7 ─ M8
```

M1 and M2 are worth landing whatever happens to the rest. M2, M3 and M5 can
go in parallel once M1 is in. M6 waits for M3b, M4b and M5: standard mode
ships with multiple cursors and with hints that say its keys.

### M0 — ADR 0004 (S)

- `docs/adr/0004-key-schemes.md`: the problem, §2's decisions, the decision
  record (§10), what was rejected (a separate editing engine, a pure
  extension, the base shortcuts as an extension, a menu bar).
- **Done when:** the ADR is accepted.

### M1 — Keys as data (M)

- **`fenix-keymap`:** `Mods.shift`; `NamedKey::F(1..=12)`; `KeyPress`
  helpers (`with_shift`).
- **`keymap.rs`:** `to_keypress` normalizes Shift (§4.1) and reports F-keys
  (the terminal's and the debugger's F-key handling move behind it, same
  order). `describe_keypress` writes `s-` for Super, `S-` for Shift, `<F12>`.
  `parse_keys` reads `C-`, `M-`, `s-`, `S-` and `<Name>`, so a leader entry
  can take `SPC C-i`.
- **`[[keys]]`:** `KeymapData.keys` with `keys`, `command`, `label`, `when`,
  `scheme`, `unbind`. A `ChordMap` built from core's, the extensions' and
  yours, like the leader trie, rebuilt by `reload_leader`. Problems reported
  like `[[leader]]`'s.
- **`route_keypress`:** the Ctrl block (`app.rs:16218`) becomes a
  `ChordMap` lookup, in the same place in the order. Core's entries
  reproduce it: Ctrl+S, Ctrl+Z, Ctrl+Shift+Z, Ctrl+Y, Ctrl+Q, font size,
  Ctrl+PageUp/PageDown/Tab with `scheme = "any"`; Ctrl+O/Ctrl+I with
  `scheme = "vim"` and `when = "editor"` (not while typing, as today).
- **Yours:** `keybindings.toml` takes `[[keys]]`; `write_binding`
  (`fenix-config/src/keybindings.rs`) writes one when the palette's `Tab`
  is given a chord; `SPC h b`, `SPC h k` and `SPC h K` show chords.
- **Done when:** the table test (today's `if/else` against the map) passes;
  every existing test passes unchanged; Ctrl+S rebound in a test
  `keybindings.toml` runs the new command; a macro replays Ctrl+Shift+Z as
  redo.

### M2 — Commands for every action (M)

- **From Vim keys and the right-click menu:** `code.hover`,
  `code.goto_definition`, `code.references`, `edit.cut`, `edit.copy`,
  `edit.paste`, `edit.select_all`. `MenuAction`'s arms run them.
- **The bracket jumps** (`VimAction::BracketJump`): `nav.next_problem` /
  `prev_problem`, `nav.next_hunk` / `prev_hunk`, `nav.next_agent_change` /
  `prev_…`, `nav.next_todo` / `prev_…`, `nav.next_side_change` / `prev_…`,
  `debug.trail_back` / `trail_forward`. Leader groups `SPC ]` and `SPC [`.
- **From Ex:** `edit.sort_lines` (options in a picker), `lsp.restart`
  (servers in a picker).
- **New:** `nav.goto_line`, `code.toggle_comment` (the language's line
  comment, from its tree-sitter config, else its block comment),
  `edit.move_lines_up` / `down`, `edit.copy_lines_up` / `down`,
  `window.focus_nth` (1–9), `window.move_tab_next` / `prev`,
  `window.focus_sidebar`, `terminal.focus`, `buffer.switch_recent` (the
  held-Ctrl switcher), `help.keys_sheet`, `view.key_scheme`.
- **Leader:** `SPC C-i` → `code.hover`; `SPC w <Left/Down/Up/Right>` beside
  `h/j/k/l`.
- **Done when:** each command has a test; `SPC h c` describes every one;
  Vim's own keys for the same actions are unchanged (their tests pass).

### M3 — The standard editing policy (L)

- **`fenix-vim`:** `EditPolicy` and `set_policy`. Under `Standard`: resting
  in Insert, Esc's meaning, the movement keys with Ctrl and Shift, SELECT
  (Visual with the standard flag: replace on type, collapse on an unshifted
  arrow), `Ctrl+Backspace/Delete`, `Tab`'s order (snippet, completion,
  indent the selection, indent), Insert-mode Vim chords off.
- **Clipboard:** a `VimEvent` asks the GUI for the system clipboard (as
  paste does today); copy and cut with no selection take the line.
- **Undo:** `fenix-core`'s buffer gets a way to cut the Insert session's
  coalesced run (`break_undo_run`); the policy calls it on §4.2's breaks,
  with `editor.undo_pause_ms`.
- **Done when:** the standard suite (§9) passes; the Vim suite passes
  unchanged under `EditPolicy::Vim`.

### M3b — Multiple cursors (M–L)

- **`fenix-core`:** `Cursors { primary, extra }` and `Caret`; carets mapped
  through every edit as marks are; merging.
- **`fenix-vim`:** under `Standard`, typing, deleting, Enter, paste and
  movement applied at each caret, last first, in one undo group; the
  clipboard's pieces; what stays primary-only (§4.2a).
- **`fenix-gui`:** drawing the extra carets and their selections; `3
  cursors` on the modeline; Alt+click (add, remove), Alt+Shift+drag (a
  column, at most 10,000, with a message past it); Esc back to one;
  `editor.multi_cursor_modifier`, with Ctrl+click's go-to-definition moving
  to Alt+click under `ctrl`.
- **Done when:** the multiple-cursor tests (§9) pass, agents' change ledger
  and decorations included; Vim mode never has an extra caret.

### M4a — The GUI's foundations (M)

- **Typing or browsing:** a `writable` property per `BufferKind` and
  pane-resident page; browsing under `Standard` is Normal underneath, with
  arrows, PageUp/PageDown, Home/End, Shift+arrows and Ctrl+C.
- **The predicates** (§4.3): `at_rest`, `typing_text`, `letter_keys_act`,
  `leader_starts`, replacing the ~35 `Mode::Normal` / `is_inserting()`
  checks outside rendering (`app.rs`, `app/mouse.rs`, `tutorial_host.rs`,
  `motion_host.rs`, …).
- **The leader chord:** `keys.leader_chord` starts the leader matcher in
  standard mode; which-key writes it as `C-k`.
- **Looks:** the modeline badge (none, `SELECT`, `DEBUG`), `[modes] select`,
  the quiet rail, the bar caret, absolute line numbers as the default.
- **Done when:** every predicate has a test per scheme; the app tests pass
  unchanged in Vim mode; in a test App on `Standard`, `Ctrl+K g g` opens
  Git and Home browses with arrows and Enter.

### M4b — Pages, panes and the mouse (L)

- **Compose panes:** `Ctrl+Enter`, Esc asks, `Ctrl+Shift+A` and the title's
  button.
- **The agents' thread:** clickable answers on every kind of card (a
  question, a form's fields, a link, a pipeline's gate), lit under the
  pointer; plain keys from the thread, Alt+key from the prompt.
- **The debugger:** DEBUG's letter keys off in standard mode.
- **The explorer:** F2, Delete, Enter while browsing.
- **Terminals:** the skip list (`when = "terminal"`).
- **The mouse:** a drag enters SELECT; Ctrl+click goes to definition.
- **Panes and tabs:** a split pane closes with its last tab (standard); the
  tab's × and the strip's split button, on hover, in both schemes.
- **Done when:** the GUI tests in §9 pass in both schemes; the tab buttons
  have screenshot tests in both themes.

### M5 — Hints from the keymap (M)

- `keymap::key_for(command)`: yours, then the scheme's, as which-key writes
  keys.
- **The strings** (~250 in `fenix-gui`): built with `key_for`, in a PR per
  area -- the agents, Git, the explorer and file manager, search, the
  debugger, projects and tasks, pages and pickers, Home, the welcome and
  notifications.
- **The lint test** (§4.6), with an allow-list for the keymap and tests.
- **The control protocol:** `CommandInfo.keys` from `key_for`.
- **Done when:** the lint test passes with an empty allow-list outside the
  keymap; every compose, confirm and empty-page hint reads right in a test
  App on each scheme.

### M6 — Standard mode ships (M)

- `editor.keys` in `fenix-config`'s schema (Editor), live switching
  (§4.7), `SPC t k`.
- §2.7's shortcuts in `default_keymap.toml` with `scheme = "standard"`.
- The welcome question; the modeline's `Ctrl+K commands` hint
  (`keys.leader_hint`); the keys page.
- The docs (§6).
- A week of use on standard mode by people who'd use it, then the undo
  breaks tuned.
- **Done when:** the coverage and conflict tests (§9) pass; `SPC h K` is
  clean on a fresh install; the docs are in; the week's findings are fixed
  or written down.

### M7 — Extension API 1.5 (S)

- `fenix-extension-api`: `KeyScheme`, `key_scheme()`, `key_for()`,
  `Event::KeySchemeChanged`.
- `fenix-wasm`: `editor_key_scheme`, `editor_key_for`, the event's dispatch.
- `manifest.rs`: `API = "1.5.0"`. Extensions' `keymaps/*.toml` take
  `[[keys]]`, refusing reserved chords and `when = "terminal"`.
- `docs/WRITING_EXTENSIONS.md`.
- **Done when:** a test extension's round trip (`key_for` before and after
  a switch, the event) passes in `fenix-wasm`; every extension in
  fenix-extensions still loads and passes its tests on the new host.

### M8 — fenix-extensions (S–M)

- MIB, Tcl, CCSDS, Notebook, PDF, Images: hints through `key_for`; these
  move to `api = "1.5"` in their manifests and get a release each.
- READMEs with Vim keys get the standard key beside them (Review's `A`
  included).
- Review and every `compose` user checked in both schemes.
- The `more_keys` pages checked against the reserved chords.
- Database and HTTP: `C-Enter` in `[[keys]]`.
- `xtask validate`: `[[keys]]` checks.
- **Done when:** CI is green; each changed extension's hints read right in
  both schemes.

### M9 — The tutorial (M)

- `scheme` on lessons; `{leader}` and `{key:command.id}` expanded through
  `key_for`.
- `standard-1-basics.toml`: typing, selecting, multiple cursors, the
  clipboard, undo, find, `Ctrl+K`.
- The `fenix-*` lessons moved to `any`; the `editing-*` lessons kept Vim's.
- **Done when:** every lesson runs in a test App on its schemes;
  `SPC h T` in standard mode lists only lessons that apply.

---

## 9. Testing

- **`fenix-vim`:** the existing suite unchanged under `EditPolicy::Vim`; a
  new suite for `Standard` (selection replace, Shift+motions, clipboard
  line copy, the undo breaks, Esc never leaving Insert, Tab's order).
- **Multiple cursors:** typing, Backspace, Enter with auto-indent and paste
  at 2 and 3 carets on the same and different lines; carets merging; one
  undo per keystroke across all carets; carets mapped through an outside
  edit (an LSP `WorkspaceEdit`, an agent's hunk); copy/paste with matching
  and mismatched counts; the 10,000 limit; `extra` always empty under
  `EditPolicy::Vim`.
- **Keymap:** default `[[keys]]` reproduce today's chords exactly (a table
  test against the old `if/else`); Shift normalization (`A` binds as
  before, `C-S-p` and `S-<Down>` round-trip through `parse_keys` /
  `describe_keypress`); macros replay Ctrl+Shift+Z as redo.
- **Coverage:** every command bound in the default leader map is reachable
  in standard mode, bound or through `Ctrl+K`; every command in §2.7 exists.
  A test fails when a new `[[leader]]` entry isn't reachable.
- **Conflicts:** core's standard chords against the keys each core page
  claims and the reserved ones; `SPC h K` reports none on a clean install.
- **Hints:** the lint test (§4.6); `key_for` answers per scheme and after a
  rebinding.
- **GUI:** the app tests that exercise compose, side by side, Home, the
  explorer, the agents' prompt and cards (a click on each kind of answer,
  `Alt+Y` from the prompt, `y` typed into it), the debugger, terminals and
  the mouse run once per scheme (a test helper sets the policy); browsing
  buffers take arrows and their letter keys; a split pane closes with its
  last tab in standard mode only.
- **Extensions:** fenix-extensions' suites against a host on API 1.5; a
  `key_for` / `KeySchemeChanged` round trip in `fenix-wasm`'s tests.

---

## 10. Decision record

Settled in review on 2026-11-09.

| # | Decision | Where |
|---|---|---|
| 1 | Leader chord `Ctrl+K` in standard mode | §2.2 |
| 2 | Notation `C-` `M-` `s-` `S-` | §2.3 |
| 3 | No `:`, `.` or macro keys in standard mode | §2.5 |
| 4 | Multiple cursors: Alt+click, Alt+Shift+drag, Esc back to one | §2.6 |
| 5 | `editor.keys`, default Vim, per person | §2.1 |
| 6 | Standard shortcuts in core; flavours as data extensions | §2.1 |
| 7 | Selection is Visual underneath, shown as SELECT | §2.4 |
| 8 | `editor.multi_cursor_modifier = alt \| ctrl` | §2.6 |
| 9 | Keyboard cursor adding is a follow-up | §2.6 |
| 10 | No multiple cursors in Vim mode for now | §2.6 |
| 11 | Undo breaks, `editor.undo_pause_ms` 1000, tuned before shipping | §4.2 |
| 12 | Copy/cut with nothing selected takes the line | §4.2 |
| 13 | Insert-mode Vim chords off; `Ctrl+Space` completes | §4.2 |
| 14 | Compose: `Ctrl+Enter`, Esc asks, `Ctrl+Shift+A` | §4.3 |
| 15 | The welcome question | §4.7 |
| 16 | No mode badge while typing | §4.4 |
| 17 | Tutorial lessons tagged by scheme, keys expanded | §4.8 |
| 18 | `SPC t k` / `view.key_scheme` switches live | §4.7 |
| 19 | `code.toggle_comment` on `Ctrl+/`; no new Vim key | §2.7 |
| 20 | API 1.5: `key_scheme`, `key_for`, `KeySchemeChanged` | §5 |
| 21 | `[[keys]]` with `scheme` and a fixed `when` list | §4.1 |
| 22 | `Ctrl+K`, `Ctrl+Shift+P`, `Ctrl+\` reserved from pages | §5 |
| 23 | Shortcuts modelled on VS Code's Windows/Linux defaults | §2.7 |
| 24 | `Ctrl+F` / `Ctrl+H` use the Search page | §2.7 |
| 25 | No menu bar; the modeline hint and a keys page | §2.9 |
| 26 | Terminal skip list | §4.5 |
| 27 | Buffers you don't type in browse | §4.3 |
| 28 | DEBUG's letter keys off in standard mode | §4.3 |
| 29 | Commands for `:sort` and `:lsp-restart`; `Alt+↑/↓` move lines | §2.5 |
| 30 | `Ctrl+O` opens a file; jumps on `Alt+←/→` | §2.7 |
| 31 | `Ctrl+Tab` is a recently-used switcher | §2.7 |
| 32 | Tab's order | §2.7 |
| 33 | Standard defaults: bar caret, absolute numbers, quiet rail | §4.4 |
| 34 | Hints from the keymap, with a lint test | §4.6 |
| 35 | `CommandInfo.keys` follows the scheme | §4.6 |
| 36 | Ctrl+click goes to definition | §2.7, §4.3 |
| 37 | The explorer's F2, Delete, Enter | §4.3 |
| 38 | `Ctrl+B`, `Ctrl+J`, `Ctrl+0`, `Ctrl+Shift+E` | §2.7 |
| 39 | At most 10,000 cursors from a column drag | §2.6 |
| 40 | Extra cursors not restored; agents see the primary | §2.6 |
| 41 | No macOS keys | §2.5 |
| 42 | Splits on the leader; `Ctrl+\` untouched | §2.8 |
| 43 | `Ctrl+1`–`9`; arrow aliases under `SPC w` | §2.8 |
| 44 | `Ctrl+Alt+←/→` move the tab to a pane | §2.8 |
| 45 | `Ctrl+W`, `Ctrl+Shift+T`, `Ctrl+Shift+N` | §2.7, §2.8 |
| 46 | A split pane closes with its last tab, standard only | §2.8 |
| 47 | × and split buttons on hover, both schemes | §2.8 |
| 48 | Workspaces on the leader | §2.8 |
| 49 | No resize keys in v1 | §2.8 |
| 50 | Agents' question cards are clickable in standard mode, and answer with their keys from the thread or with Alt from the prompt | §4.3 |

---

## 11. Risks

- **Undo granularity** is the part users will notice first. Tune the break
  rules on real use before M6 ships.
- **Alt+click and window managers.** KDE and some X11 setups move windows on
  Alt+drag. `editor.multi_cursor_modifier` is the answer, and the welcome
  says so on Linux.
- **Multiple cursors and the rest of the editor.** Everything that holds a
  position into a buffer (decorations, agents' change ledger, the
  debugger's lines) already maps through edits; edits at N carets in one
  undo group is a new pattern for them. The tests apply edits at several
  places and check those positions too.
- **`Ctrl+0` and `Ctrl+Alt`.** Standard mode moves font reset off `Ctrl+0`
  (to the leader). `Ctrl+Alt+letter` is AltGr on some layouts, so standard
  chords use `Ctrl+Alt` only with arrows.
- **The hint audit is wide** (~250 strings). It's mechanical, and the lint
  test keeps it from coming back, but it touches most of `fenix-gui`; land
  it in a few PRs by area.
- **Two schemes to keep working.** The ground rules (commands first, the
  predicates, the coverage and hint tests) are what keep this cheap. A
  change that needs a scheme check outside §4.3's predicates should be
  questioned in review.
- **Pressure to grow standard mode** (IDE-exact behaviour, keyboard cursor
  adding, multiple cursors in Vim mode, a menu bar). §2.5, §2.6 and §2.9 are
  the line; anything past them is its own plan.

---

## 12. As built (Fenix 1.5)

Where the implementation differs from the plan above, and why:

- **`when`'s list** is `anywhere`, `text`, `typing`, `not_typing`, `page`,
  `terminal`: `editor` (§3.1) became `text`, and `not_typing` was needed
  for Vim's Ctrl+O and Ctrl+I, which Insert keeps for its own.
- **Extra cursors aren't mapped through an outside edit** (§4.2a): an edit
  made elsewhere -- a language server's, an agent's, a reload, an undo --
  goes back to one cursor. Mapping them would have meant draining the
  buffer's anchor edits, which extensions' decorations already drain.
- **Keyboard cursors** (§2.6's follow-up) are built: `Ctrl+D` selects
  the word, then adds the next occurrence (the newest is the primary, so
  the view follows it); `Ctrl+Shift+L` every occurrence;
  `Ctrl+Alt+↑/↓` a cursor above or below. A selection that is a whole
  word finds whole words, as VS Code does when `Ctrl+D` picked the word;
  other text matches anywhere, case and all. Where to put them is
  `fenix-vim`'s `occurrence` module; Vim keys get a message, since only
  standard keys have extra cursors.
- **Extra cursors are kept with the session** (§2.6 said they
  weren't): each pane saves its extras and the text's length. They come
  back when the document's text is the one they were saved in -- the
  session's own unsaved text, or a file whose size and time on disk are
  as they were -- and the length agrees; otherwise the primary alone,
  as before. Only with standard keys.
- **The rest of §2.7's keys** came after: Ctrl+Shift+K, Ctrl+Enter
  and Ctrl+Shift+Enter, Ctrl+L, Ctrl+U (cursor undo: the cursors before
  each key that moved them without an edit, a hundred deep; an edit
  forgets them), Ctrl+Shift+\, Ctrl+] and Ctrl+[, Ctrl+Shift+[ and ]
  (bound as `C-{` and `C-}`, what a US keyboard sends), Ctrl+\,
  Ctrl+T (`workspace/symbol`, asked again as the query changes),
  Ctrl+Shift+M and Ctrl+Shift+G. The line commands are `fenix-vim`'s
  with standard keys and Vim's own keys (`dd`, `o`, `V`, `>>`, `%`,
  `zc`) with Vim keys. F5, F9, F10 and F11 were already the debugger's,
  ahead of the keymap and in every mode, so they stay out of it.
- **Tabs already had a ×** (§2.8); the split button is the new part. It
  sits right after the tabs.
- **The keys page** is `SPC h ?` (`help.keys_sheet`), a Markdown page of
  the chords in use and the leader's groups.
- **Hints** (§4.6) are said for the scheme where they're shown --
  messages, errors, toasts, compose titles, menus, Home and pages --
  through one table of Vim phrases and their standard-keys forms, rather
  than each string built with `key_for`. The test scans the source for
  hints that name Vim's keys and fails on one the table doesn't translate.
  On pages and Home only `SPC` is rewritten (to the leader chord, when it
  is as wide), so a page's cells stay where they were laid.
- **Compose's agent button** on the pane's title isn't there: Ctrl+Shift+A
  is. The title says so.
- **Key routing** takes the event loop as optional, so tests drive the
  real `dispatch_keypress` (`app/test_keys.rs`); the standard-keys tests
  are in `app/standard_tests.rs`, `app/carets.rs` and `fenix-vim`'s
  `state/standard.rs`.

- **The welcome question** (§4.7) is a step of a welcome of its own
  (`welcome_page.rs`), not a line above the packs: Hello, Keys (Vim keys
  marked recommended), Look, Extensions, Learn. The packs are a view of
  the Extensions page the Extensions step opens.
