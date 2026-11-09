# ADR 0004: Key schemes -- Vim, and standard keys

- **Status:** accepted 2026-11-09
- **Date:** 2026-11-09
- **Plan:** [`docs/plans/key-schemes.md`](../plans/key-schemes.md) (its §10
  lists every decision reviewed)

## Context

Fenix is a modal editor: Vim's grammar for text, a `SPC` leader for
everything else. People who'd use Fenix's pages, Git, agents and debugger
ask for the keys of a traditional IDE instead: no modes, Ctrl shortcuts,
Shift+arrows to select, several cursors with a click. The worry is that
serving them blunts the experience Fenix is built around.

What the code has today decides what's possible:

- Actions are mostly commands (`CommandRegistry`, about 170), and keymaps
  are data, but only `[[leader]]` (keys starting with `SPC`) and `SPC m`
  menus. Ctrl chords are an `if/else` in `route_keypress`.
- Key presses carry no Shift, and F-keys never reach the keymap.
- `VimState` owns every text key. Some actions exist only as Vim keys
  (`K`, `gd`, `]d`) or Ex commands (`:sort`). About 35 checks in the GUI
  assume Normal mode.
- There is one cursor per pane.
- The extension API exposes no editor mode. Extensions' keys are their
  pages' own, and their keymaps add leader entries. The assumptions about
  Vim are in core.

## Decision

1. **Two key schemes, one setting.** `editor.keys = "vim" | "standard"`,
   default `vim`, per person (a project can't set it). Vim mode is
   unchanged, and stays the voice of the README, the docs and the
   tutorial's first lessons.
2. **One editor, two policies.** Standard mode is `EditPolicy::Standard`
   inside `fenix-vim`, not a second editing engine. A buffer you type in
   rests in Insert; Esc never enters Normal; a selection is Visual
   underneath, shown as `SELECT`; undo steps break on words, Enter, jumps,
   pastes and pauses (`editor.undo_pause_ms`). Undo, registers, snippets,
   completion and LSP edits are shared.
3. **Commands, not keys.** Both schemes bind the same commands. Every
   action that exists only as a Vim key or an Ex command becomes a command
   before standard mode binds it. Core gets no feature only one scheme can
   reach.
4. **Keymaps take chords.** Key presses carry Shift (dropped for a plain
   printable character, so existing bindings are untouched) and F-keys. A
   `[[keys]]` table, beside `[[leader]]`, binds chords with a `scheme`
   (`vim`, `standard`, `any`) and a fixed `when` (`anywhere`, `editor`,
   `text`, `typing`, `page`, `terminal`). Today's hard-coded Ctrl chords
   move into it unchanged, so Vim users can rebind them too. Notation:
   `C-`, `M-`, `s-` (Super, formerly written `S-`), `S-` (Shift).
5. **The leader is `Ctrl+K` in standard mode.** The same trie and
   which-key popup: `Ctrl+K g g` is `SPC g g`, every extension's menus
   included. `keys.leader_chord` changes it.
6. **The standard shortcuts are VS Code's Windows/Linux defaults**, where
   Fenix has the command, and ship in core's `default_keymap.toml`.
   Flavours (JetBrains-like, say) can come as data-only extensions. Find is
   the Search page (`Ctrl+F`, `Ctrl+H`, `F3`). Splitting and closing panes
   stay on the leader (`Ctrl+K w …`); `Ctrl+\` keeps leaving the terminal.
7. **Multiple cursors in standard mode.** Alt+click adds or removes one,
   Alt+Shift+drag makes a column (at most 10,000), Esc goes back to one;
   `editor.multi_cursor_modifier = "alt" | "ctrl"` for desktops that take
   Alt+click. A pane's cursors are a primary plus extras, so code that
   knows one cursor reads the primary and is unaffected. Edits apply at
   every cursor in one undo step. Vim mode has no extra cursors.
8. **Buffers you don't type in browse.** Home, Git views, diffs, the
   explorer listing, Task Output: Normal underneath, so their own keys
   work as today, with arrows and Shift+arrows as standard users expect.
   Letter keys act where you can't type.
9. **The GUI reads the scheme in a few named predicates** (`at_rest`,
   `typing_text`, `letter_keys_act`, `leader_starts`) and in the keymap
   data, nowhere else. Pages that assumed Normal get standard behaviour:
   compose sends on `Ctrl+Enter`; an agent's question card is clickable
   and answers with Alt from the prompt; DEBUG's letter keys are off;
   chosen chords skip a focused terminal.
10. **Hints say the keys in use.** Every hint that names a key is built
    from the keymap (`key_for`), checked by a test against new hand-written
    key text. The control protocol reports the same keys.
11. **Extension API 1.5, optional and read-only:** `key_scheme()`,
    `key_for(command)`, `Event::KeySchemeChanged`, and `[[keys]]` in
    extensions' keymaps. `Ctrl+K`, `Ctrl+Shift+P` and `Ctrl+\` are
    reserved from pages. An extension that ignores 1.5 works as on 1.4.
12. **Out of scope for standard mode:** the `:` line, `.` repeat, macro
    keys, a menu bar (a modeline hint and a keys page instead), keyboard
    ways to add cursors (a follow-up), macOS's Cmd.

## Rejected

- **A second editing engine** for standard mode. Two implementations of
  undo, selections, completion and every LSP edit would drift; Vim's Select
  mode already is the model standard editing needs.
- **Standard keys as an extension alone.** Extensions can't reach the
  editing layer (they get page keys, not buffer keys), and the policy has
  to be in core. Shipping only the base shortcuts as an extension was
  rejected too: a setting that does nothing until something is installed
  is a poor first run, and the shortcuts must move in step with core's
  commands.
- **Scheme checks wherever they're needed.** The ~35 Normal-mode checks
  would become ~70; the predicates keep the scheme in one place.
- **`Ctrl+\` to split** (VS Code's). It means "leave the terminal" in Fenix;
  splitting stays on the leader.
- **An inline find bar.** The Search page already finds in a file and steps
  through matches; a second way to find would split the feature.
- **A menu bar.** The palette and which-key cover discovery; a menu bar is
  a second command tree to keep in step.

## Consequences

- Vim users see the new commands (in the palette, `SPC ]` / `SPC [`, arrow
  aliases under `SPC w`), rebindable Ctrl chords, and a × and split button
  on tabs when the pointer is over them. Nothing else changes for them.
- Every new action must be a command, and every new hint must come from
  the keymap. The coverage, conflict and hint tests enforce it.
- The hint audit touches most of `fenix-gui` (~250 strings), once.
- Two schemes are tested: the app tests that cover pages, compose, agents,
  the debugger, terminals and the mouse run once per scheme.
- Extensions need no change. Those that name editor keys in their hints
  (MIB, Tcl, CCSDS, Notebook, PDF, Images) move to `key_for` and API 1.5.
- The order of work is the plan's §8: M1 (keys as data) and M2 (commands)
  are worth having whatever happens to the rest; standard mode ships at M6,
  with multiple cursors and generated hints, after a week of use.
