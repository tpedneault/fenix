# Plan: screen readers

- **Status:** phase 1 built, for the release after 1.5.2 (§5); phases 2
  to 4 not started.
- **Scope:** Fenix tells assistive technology -- Narrator, NVDA and JAWS
  on Windows, Orca on Linux, VoiceOver on macOS, braille displays and
  magnifiers through them -- what's on screen and what has the
  keyboard, through [AccessKit](https://github.com/AccessKit/accesskit)
  and its winit adapter.
- **Lands as:** the phases in §4, each one usable on its own, with the
  tree built only while an assistive technology is listening.

## 1. What it is

Fenix draws everything itself on the GPU: there are no native widgets
for a platform's accessibility API to look into, so a screen reader
today sees one empty window. AccessKit is the layer that lets a
custom-drawn UI describe itself: the application gives it a tree of
nodes (a window, a text area, a list and its items, a status line), each
with a role, a name, a value and, for text, its characters, caret and
selection; AccessKit's platform adapters (UI Automation on Windows,
AT-SPI on Linux, NSAccessibility on macOS) answer the screen reader's
questions from it. `accesskit_winit` attaches that to a winit window.
egui, Bevy, Slint and Xilem use it the same way.

What someone using a screen reader should be able to do, by the end:

- hear the line, word or character the caret moves onto, and what a
  selection covers, in any buffer or page Fenix shows as text;
- hear the mode when it changes (Normal, Insert, Visual), and every
  message the modeline or a toast gives, errors at once;
- read a prompt (`:`, `/`, a rename) as they type in it;
- move through a picker's or completion list's items and hear each,
  with how many there are;
- hear the which-key popup's choices after a leader key;
- find the panes, the sidebar, the terminal and the agents' dock as
  regions, and the tab bar as tabs;
- with a braille display, route the caret by pressing a cell.

## 2. Where Fenix stands (1.5)

- One `App` (`crates/fenix-gui/src/app.rs`) owns every frame (window);
  `Launcher` (`launch.rs`) creates the first window *hidden*, starts the
  GPU and the splash, then shows it -- which is what an AccessKit
  adapter needs (it must exist before the window is first shown).
  Further frames are made by `open_frame`.
- Input goes `window_event` → `handle_key` → `dispatch_keypress` →
  `route_keypress`; background work arrives as `FenixUserEvent`s through
  an `EventLoopProxy`, which is also how `accesskit_winit` delivers its
  requests.
- What's in front is known without drawing: the focused buffer and
  cursor (`open()`, `cursor()`), Vim's mode and Visual anchor (`vim`),
  the modeline (`modeline_pieces`), the open prompt
  (`active_prompt_text`), the picker (`active_picker`), the which-key
  popup (`leader_matcher` and `which_key`), status messages
  (`status_message`) and toasts (`notifications`).
- Text is drawn in a grid of cells: column *n* of a line is cell *n*
  after tabs and decorations (`PaneLayout`, `app/rows.rs`), which is
  what character bounds for a magnifier would come from.

## 3. Design

### The tree

```text
Window "Fenix -- main.rs"                      root
├── Group "Panes"                              phase 3
│   ├── MultilineTextInput "main.rs, Normal"   the focused buffer
│   │   ├── TextRun                            a line each, the lines
│   │   └── ...                                around the caret
│   └── MultilineTextInput "lib.rs"            other panes, phase 3
├── TextInput "Prompt"                         a prompt, when open
│   └── TextRun
├── ListBox "Find file" (n items)              a picker, phase 2
│   └── ListBoxOption "src/main.rs" ...        the shown items
├── Menu "SPC f" / MenuItem "f  find file"     which-key, phase 2
├── Terminal "Terminal"                        the terminal, phase 3
├── Status "NORMAL  main.rs  12:4"             the modeline
└── Label (live)                               announcements
```

- **Ids** are stable: fixed ones for the window, the editor, the
  prompt, the modeline and the announcer; a line's run is `2^32 + line`,
  so moving the window of lines doesn't renumber what stays.
- **Focus** is the prompt when one is open (it has the keyboard), else
  the focused text. Later: the picker's selected item, the terminal.
- **Text.** AccessKit's text model is runs of characters, each with its
  UTF-8 length, the line break counted as one more; the caret and the
  selection are positions in runs. Fenix sends a run per buffer line for
  the lines around the caret (`LINES_AROUND`, 50 each side) -- the
  window VS Code calls its accessibility page size. A screen reader
  reading by line or word never needs more than that between two caret
  moves, and the cost stays the same in a file of a million lines.
  A character is a `char` for now; grapheme clusters (an emoji with a
  modifier, a letter and its combining accent) are phase 2.
- **Vim's selection** takes in the character under the cursor, so the
  selection's far end is one on; Standard keys' isn't.
- **Announcements** go in a live region: a `Label` whose text is what
  was just said, polite for a mode change or a message, assertive for an
  error. Platforms read a live region when its text changes, so the
  same words twice differ by a trailing space. A change of mode and a
  message in the same round are said together ("Normal. Saved").

### How updates are driven

- `Launcher` creates `accesskit_winit::Adapter::with_event_loop_proxy`
  for the first window, after creating it hidden and before showing it,
  when `accessibility.screen_reader` is on (the default), and gives it to
  `App` (`attach_accessibility`).
- Every window event goes to the adapter first (`a11y_window_event`):
  that's how it keeps the window's bounds and focus.
- The adapter's requests arrive as `FenixUserEvent::Accessibility`:
  *InitialTreeRequested* when an assistive technology first asks
  (`active` from then on), *AccessibilityDeactivated* when it lets go,
  and *ActionRequested* for what it asks Fenix to do.
- After each round of events (`about_to_wait`), if `active`, Fenix takes
  a `Snapshot` -- plain data, the lines around the caret included -- and,
  when it differs from the last one sent, builds the tree from it
  (`a11y::tree`) and hands it to `update_if_active`. Snapshots make the
  change test cheap and the tree a pure function, tested against
  `accesskit_consumer` (what every platform adapter is built on).
- The proxy-based adapter can't answer the first request synchronously,
  so the platform has a placeholder tree for one round of the loop; and
  each update is the whole tree. Phase 4 moves to
  `with_mixed_handlers` with incremental updates if either shows.

### Cost

- **When no assistive technology is listening** (almost always): the
  adapter exists and answers whether the window is accessible. On
  Windows that's a subclassed window procedure answering
  `WM_GETOBJECT`; on Linux, `accesskit_unix` starts a thread that
  connects to the session bus and watches whether a screen reader is
  enabled; on macOS, the window's view subclassed to answer the
  accessibility calls. Fenix itself does one
  branch per round of events. Nothing is built.
- **When one is**: a snapshot per round of events (about 100 lines read
  from the rope, the modeline formatted, a comparison) and, when it
  changed, the tree built and sent: under a millisecond for the window
  of lines, well within a frame.
- **Build**: `accesskit_winit` brings `accesskit_windows` (on the
  `windows` crate Fenix already uses), `accesskit_macos`, and on Linux
  `accesskit_unix` with `zbus` and `async-io` -- the largest addition to
  the dependency tree, compiled once.
- **The setting** (`accessibility.screen_reader`, on) turns the adapter
  off entirely, for anyone who'd rather not have the thread and the bus
  connection on Linux. It's on by default because someone who relies on
  a screen reader can't turn on a setting they can't read. It takes
  effect at the next start: the adapter can only attach to a window
  that hasn't been shown.

### Not in scope

- A right-to-left mode, or bidi-aware motions: the tree gives text in
  its logical order, which is what screen readers want anyway.
- Sound cues (VS Code's "accessibility signals") and an
  accessibility help page (`Alt+F1`): separate features, worth doing
  after phase 2.

## 4. Phases

1. **The focused text and announcements** (built). The window; the
   focused buffer or page as a multi-line text area with the lines
   around the caret, the caret and the selection; a prompt as a text
   field with the focus; the modeline as a status; the live region with
   the mode and status messages. `accessibility.screen_reader`.
2. **Lists, menus and actions.** Pickers (`ActivePicker`), completion
   and the command palette as list boxes with the selected item focused
   (or `active_descendant`) and `size_of_set` / `position_in_set`; the
   which-key popup as a menu; toasts announced, critical ones
   assertive. Actions: `SetTextSelection` moves the cursor (braille
   routing), `Focus` on a pane focuses it. Grapheme clusters as
   characters.
3. **Regions.** Every pane as a text area (not only the focused one),
   the sidebar explorer as a tree, the tab bar as tabs, the terminal
   panel (its screen as text, the cursor as the caret), the agents'
   dock, the debug drawer; additional frames each with their own
   adapter (`open_frame` creates them hidden first).
4. **Geometry and polish.** Bounds for each node and character
   positions for the lines on screen (from `PaneLayout`), so magnifiers
   follow the caret and a screen reader can point at things; the mixed
   adapter and incremental updates if a large tree shows; the line's
   diagnostics as descriptions; an accessibility help page.

## 5. What's built (phase 1)

- `crates/fenix-gui/src/a11y.rs`: `Snapshot` and `tree()`, pure, with
  tests that load the tree into `accesskit_consumer` and read it back as
  a screen reader's adapter would: the text, the caret's line, a
  selection's text, the prompt's focus, the live region's politeness and
  repeated announcements.
- `crates/fenix-gui/src/app/a11y_host.rs`: the adapter on `App`, the
  events, the snapshot from what's in front, what's announced and when;
  tested through the real key path (a mode change, an error, a Vim
  selection, a prompt, nothing built while no one's listening).
- `launch.rs`: the adapter is created between the hidden window and its
  first showing.

Not verified, because the container this was built in has no display,
no GPU and no screen reader: that Narrator, NVDA, Orca or VoiceOver
actually read the text area, follow the caret and speak the live region
as the tree says they should. That's the first thing to check by hand,
on each platform, before phase 2.
