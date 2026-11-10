# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.5.0] - 2026-11-09

### Security

- A project's own programs wait to be allowed. `.fenix/tools.json` (its
  language servers, debug adapters and tasks) and `.fenix/lsp/*.json` (a
  server's settings) are used only once you've said yes for that
  project, with `SPC p y` (`project.trust`), which shows the file and
  asks. Before, opening a file in a project started the language server
  the project named, so a repository you had only just cloned could run
  a program of its choosing. The answer is kept outside the project and
  asked again when those files change. Projects you already used keep
  working; what the wizard, the settings page and the doctor write for
  you needs no allowing.

### Added

- Remote projects, a preview: `SPC p R` (`project.open_remote`) opens a
  folder on another machine as a project, over SSH, with the system's
  `ssh` and a small helper Fenix copies there (Linux; no administrator
  rights). The explorer, the sidebar and the file picker list it; files
  open, edit and save there, and aren't written over when they changed
  there; a dropped connection is made again by itself. Terminals, Git,
  search, language servers, tasks and the debugger don't work on a
  remote project yet. See FEATURES.md's Files and projects.
- A welcome for a first run (`SPC h w` brings it back): what Fenix is,
  then a step each for how you'd like to edit (Vim keys, recommended, or
  standard keys), a theme and the text size, the extension packs, and
  where to learn -- the tutorial in the keys you picked, the keys page, a
  folder to open. Every choice is in use as it's made; a click on a
  choice picks it. The packs are on a view of their own
  (`extensions.welcome`).
- Standard keys: a traditional IDE's way to edit, beside Vim's
  (`editor.keys = "standard"`, `SPC t k`, or the welcome's question). A
  file's text is typed into at once; Shift with a movement selects, and
  typing replaces the selection; Ctrl+C/X/V/Z/Y/S/A, Ctrl+F and Ctrl+H on
  the Search page, Ctrl+P, Ctrl+Shift+P, F12, F2, Ctrl+/, Alt+↑/↓ and the
  rest of VS Code's keys on Windows and Linux; undo a word or a line at a
  time. The leader's menu, every extension's keys in it, opens on Ctrl+K
  (`keys.leader_chord`). Home, Git, diffs and listings are browsed with
  their own keys. Compose panes send on Ctrl+Enter; an agent's question is
  answered with a click, or Alt and its key. See KEYBINDINGS.md's Standard
  keys, and ADR 0004.
- More cursors, with standard keys: Alt+click adds one (or takes it away),
  Alt+Shift+drag makes a column; typing, deleting, Enter, Tab, the arrows
  and pasting happen at each, as one undo step, and undo and redo bring
  the cursors back (`editor.multi_cursor_modifier = "ctrl"` for Ctrl+click
  instead).
- More cursors from the keyboard, with standard keys: Ctrl+D selects the
  word under the cursor, then adds a cursor on the selection's next
  occurrence each time; Ctrl+Shift+L puts one on every occurrence;
  Ctrl+Alt+↑/↓ add one on the line above or below. A selected whole word
  finds whole words only. They're commands too
  (`edit.add_next_occurrence`, `edit.select_all_occurrences`,
  `edit.add_cursor_above`/`below`).
- Chords are keymap data: `[[keys]]` in `keybindings.toml` and in
  extensions' keymaps binds or unbinds a key with its modifiers (`C-s`,
  `C-S-p`, `F12`) for a key scheme and a place; Ctrl+S, Ctrl+Z and the
  rest can be rebound with Vim keys too. `SPC h b`, `SPC h k` and `SPC h K`
  cover chords, and the palette's `Tab` binds one.
- Commands for what only Vim keys, Ex or the right-click menu did:
  `code.hover`, `code.goto_definition`, `code.references`,
  `code.toggle_comment`, `edit.copy`/`cut`/`paste`/`select_all`, moving
  and copying lines, the `:sort` variants, `lsp.restart`,
  `nav.goto_line`, the bracket jumps (`nav.next_problem`…, on `SPC ]` and
  `SPC [` too), panes by number, a tab to the next pane, the recent-buffer
  switcher, and a keys page (`SPC h ?`).
- Extra cursors come back with the session (without their selections,
  as the main cursor's isn't kept) when the file's text is the one they
  were left in; a file changed since gets its main cursor back only.
- Auto-save, off unless chosen on the Settings page: `editor.auto_save =
  "after_delay"` saves a file a moment after its last edit
  (`editor.auto_save_delay_ms`, a second), `"on_focus_change"` when you
  leave it for another buffer, pane or application. It never writes over
  a file that changed on disk, a buffer with no file, a read-only one or
  a remote project's, and it doesn't format.
- More of VS Code's keys, with standard keys: Ctrl+Shift+K deletes the
  line, Ctrl+Enter and Ctrl+Shift+Enter open one below or above, Ctrl+L
  selects the line (again, the next), Ctrl+U takes back the last cursor
  move or added cursor, Ctrl+Shift+\ goes to the matching bracket,
  Ctrl+] and Ctrl+[ indent and dedent, Ctrl+Shift+[ and Ctrl+Shift+] fold
  and unfold, Ctrl+\ splits the pane, Ctrl+Shift+G opens Git. The F-keys
  were already the debugger's.
- Every problem in one list: `code.problems` (Ctrl+Shift+M, `SPC c p`),
  errors first, the quickfix list too; and the project's symbols as the
  language servers find them for what's typed: `code.project_symbols`
  (Ctrl+T, `SPC c j`).
- A split button after each pane's tabs.
- Extensions: the welcome offers a Python pack -- the Python language pack, Jupyter
  notebooks (`fenix.jupyter`, `.ipynb` files run on a kernel) and the CSV table.

### Changed

- `fenix/commands` reports the keys that run a command now: a chord
  (`C-s`) over the leader's keys, which standard keys write with their
  chord (`C-k g g`).
- Hints, messages and toasts say the keys of the scheme in use.
- The refactor preview (a rename, a code action) shows each change as its
  own hunk, files named from the project root; `code.undo_refactor`
  (`SPC c R`) undoes a refactor with either keys.
- Rename (`F2`, `SPC c r`) starts from the whole name under the cursor --
  selected, with standard keys, so typing replaces it; Ctrl+Backspace
  deletes a word in one-line prompts.
- A snippet's previous field is Shift+Tab (and Ctrl+K with Vim keys: with
  standard keys Ctrl+K stays the leader).
- The Search page fits a narrow pane: its toggles go under the field and
  its scopes wrap.
- Find references says how many there are and the key for the next.
- The keys page is a buffer of its own, with no file behind it.

### Fixed

- Following the agent (`SPC a f`) goes to the line it changed, not only the
  file, and scrolls there: the pane beside the dock moved its cursor but,
  without the focus, never scrolled to it. When the agent writes -- through
  Fenix, or to disk itself as Claude Code does -- it goes to the first line
  the write changed; before that, to the line it last read there, as an
  agent's edit seldom names one. A later update to the same edit doesn't
  pull the view back. With the dock across the whole window, `SPC a f` says
  there's no pane beside it to follow in, and follows in any pane that isn't
  the dock's.
- A folder opened as the project from outside (`fenix <folder>`,
  `--project`, `fenix/openProject`) gets a workspace of its own, as `SPC p p`
  gives it, and stays the project once a file in it is open -- a folder with
  no `.git` or `Cargo.toml` stopped being the project then.
- A file in the project has its breadcrumb from the project, however the
  project's folder was spelt (`\\?\`, a drive letter's case), not its whole
  path.
- The agents' tool cards show a file in the project from the project, and
  one elsewhere from `~`, not the whole path the agent gave.
- `settings.toml` loses a table once its last setting is unset (no bare
  `[appearance]` left behind); the comment at the top of the file stays
  there.
- Enter on an indented blank line kept no indent for the next.
- Task Output could show a program's stdout before stderr it had written
  earlier (`cargo test`'s `Running …`).
- Completion opened when the cursor merely moved after a word; with
  standard keys it followed every arrow, and Escape didn't close it.
- The Search page went on following F3 after references replaced its list.
- The sidebar's Git badges stayed as they were after a save, a commit or
  a change made in a terminal.
- A problem's underline at the end of a long line could be drawn over the
  pane beside it.
- Save and delete messages named files by their whole path, the question
  cut off at the prompt's edge.
- With standard keys, the debugger's drawer, the agents' dock titles, the
  Git and review popups, notification cards and the modeline's Git
  operation said Vim's keys (`SPC u e`, `K`, `]h`).

## [1.4.0] - 2026-11-08

### Changed

- Pages: a page's tab has its icon and plain name (Settings, Search · HEADER,
  Git · orbit-ground, Log · master) rather than `*settings*`, and a page starts
  right under its tabs, with no breadcrumb row.
- Settings: whole names, switches for what's on or off, a stepper for the value
  being changed, values on fields, and whether each is changed or the default.
  The setting selected opens with what it does and its key in `settings.toml`;
  the categories are a card with their counts, and you and the project are
  chips.
- Git status: the branch, where it stands against its upstream and the changes
  are chips; each section is a card with its count, each file shows its status
  on a badge, its name before its folder and the lines it adds and removes
  (read once a refresh), and an open diff is a well inside its card. With
  changes but nothing staged, the page says how to stage.
- Search: the query, the replacement and the files in input boxes, case, word
  and regex as toggles that show when they're on, the scope as chips, each
  file's results on a card with its count, and matches on a chip.
- Git log: the graph is drawn rather than typed in `*`, `|`, `\` and `/`:
  each branch a line in its own colour, a commit a dot (a merge a ring, HEAD
  ringed), and a merge or a branch's end a rounded bend, a row a commit with
  no rows of connectors between. The lanes run on through a commit opened to
  its files. With many branches at once the lanes close up to fit a fifth of
  the page and the furthest share its edge; each commit's text starts just
  past the lanes in use near it. Branches and tags as chips, the author's
  initials on a badge in a column of their own and the hash quieter; the
  commit the cursor is on opens under it with its author, when, and its whole
  hash.

- Panels: the window is a tray, and the panes, the sidebar, the terminal and
  the docks sit on it as rounded panels a small gap apart, with no lines
  between them. Dragging between two panes still resizes the split. The panel
  with the keyboard has a quiet edge in the mode's colour, when there's more
  than one to tell apart. `appearance.panels = "flat"` keeps the look from
  before: edge to edge, with lines between the panes.
- Themes work out three more surfaces from their own colours: the `tray`
  between panels, a `card` and an `input`. Each steps away from the page
  only as far as keeps the theme's text at its contrast; a theme file can
  set them itself, under `[surfaces]`.
- The agents' dock: tool calls in a row are one card, what you wrote is on a
  card of its own, and a question the agent asks is an outlined card with an
  answer a line, each on its key's cap. The prompt under it is drawn as a
  box you type in, and the state beside the agent's name is a chip.
- A question the agent asks takes `j` and `k` to move between its answers,
  and Enter to give the one they're on, as well as `y`, `Y`, `n` and the
  numbers from anywhere.
- The debug drawer: the step keys are on caps in its header beside the
  session's state, each column is a card -- the one with the keys outlined
  -- and the evaluate line is a box. It's a row taller, and each column
  shows nine rows.
- The values the debugger writes at the ends of lines are on chips, a
  changed one in ember.
- Every page's key strip shows its keys on caps.
- Terminals start PowerShell 7 (`pwsh`) on Windows when it's installed, else
  Windows PowerShell as before, without its banner; `terminal.shell` names
  another shell and its arguments (`bash -l`, a quoted path).
- Terminals draw in the theme's own colours: the 16 ANSI colours come from
  its Git and syntax colours, its text and its page, unless the theme file
  lists its own (`[terminal] ansi`). Bold, dim, italic, underline and inverse
  are drawn, so PowerShell's menu completion shows the item it's on and its
  suggestions read as quieter than what you typed.
- Pickers float: every picker opens as a card over the dimmed window rather
  than taking over the pane, its matches lit in the theme's accent and its
  keys in a footer on caps. Every picker, the command picker and the `SPC`
  menu are the same card, centred in the window, so going from one to
  another doesn't move it; its rows are taller than the editor's lines, with
  a gap before each section, and commands and the menu's groups have their
  area's icon. Enter, Esc, `C-n`/`C-p`, the arrows, Tab and
  `C-v` are as before.
- The file picker shows each file's icon, its name then its folder, the files
  opened lately first (under *Recent* while nothing's typed), and the first
  lines of the selected file beside the list when the window is wide enough
  (read once per selection, 64 KB at most).
- The command picker (`SPC :`) names each command by its area and a short
  name (`Git › Blame`) with what it does beneath and its keys on caps. A name
  or an area is matched before a description, which is matched by whole
  words; the commands run lately come first, then an area's main command
  (`SPC g g` for Git) and those with the fewest keys, and a query that names
  an area shows its commands under it and the rest under *Elsewhere*. A command that
  can't run here -- XML's outside XML, the debugger's with no session -- is
  dimmed with why, last. Beside the list, the selected command's keys, where
  it is in the `SPC` menu and its id; `C-y` copies the id.
- The `SPC` menu (and the local leader's) is the same floating card: the
  chords used lately, the groups with how many commands each holds, and the
  commands, with `:` to search them all by name. Vim's prefixes (`g`, `z`,
  `]`, `[`) show in a slim hint bar above the modeline instead -- or at the
  top when the cursor is near the bottom -- so it never covers the line
  being edited. `which_key.delay_ms`, Backspace and paging are as before.

### Added

- Copy and paste in terminals: drag across the text to select it,
  `Ctrl-Shift-C` copies and `Ctrl-Shift-V` pastes, and a right click copies
  the selection or pastes when there's none. A paste is bracketed when the
  program asked for it.
- Terminals send the keys line editors bind, as xterm does: `Ctrl`, `Shift`
  and `Alt` on the arrows, Home, End, Page Up and Down and Delete (PowerShell's
  `Ctrl-Left` and `Ctrl-Right` by word, `Shift`-arrows selecting),
  `Ctrl-Space` (PowerShell's menu completion), `Shift-Tab`, `Ctrl-Backspace`,
  `Alt` combinations, F1-F12, and the arrows' application mode. While a debug
  session runs, F5, F10 and F11 still step it.
- In a terminal, a double click selects a word (a path or URL whole), a triple
  click the line, and a selection stays on its text as output scrolls.
  Holding `Ctrl` over a link underlines it and shows a hand.
- A terminal in a pane is titled with what it runs and where (`terminal 1 ·
  pwsh · orbit-ground`), and the panel says the same on the modeline; a shell
  that marks its prompts (`OSC 133`) adds `· running` while a command runs.
- `Ctrl`-click on a `path:line:col` in a terminal -- a compiler's error, a
  stack trace, PowerShell's `At C:\x.ps1:12 char:5` -- opens the file at that
  line beside the terminal; on a URL, in the browser. Relative paths are read
  from where the shell says it is (`OSC 7`, `OSC 9;9`), else where it started.

### Fixed

- Text is drawn in the theme's own colours. It used to come out darker than the
  theme file said on every machine (Orbit Dark's `#bb9af7` keywords as
  `#6142b2`), and with less contrast than the themes are tested for.

- A jump lands in the middle of the window rather than on its last row: a
  symbol picked (a language pack's `SPC m s`), a search result, `gd`, the
  jumplist, a path followed from a terminal. One already on screen stays
  where it is; a step past the edge (`j`) still scrolls a line.
- A terminal's ANSI black is a grey that reads on a dark page, and the
  bright colours are the same hues, livelier, rather than washed towards
  white.
- The agents' dock: the thread's title is in its title bar once, not again
  as the page's first line; the agent's state, mode, model and usage go on
  to a second row in a narrow dock instead of off its edge; and the prompt
  box shows only what you've written, without line numbers or `~` rows.
- The debug drawer: a column with nothing in it says why in full, wrapped
  in the column, and the selected row's value, when its column cut it, is
  shown whole on the drawer's bottom line.
- Pages keep inside a narrow pane, down to a third of a laptop's screen:
  their headers' chips, titles and notes are cut or left out rather than
  running off the edge or over each other, and a pane under 40 columns is
  no longer laid out as if it were 40. In a narrow pane Settings shows its
  settings at the whole width, and Tab trades them for the categories;
  Extensions closes up its tabs; Agent usage drops the cache, time and token
  columns; a section heading is cut to its width (the Snippets preview's ran
  off the page at 80 columns).
- A page's modeline names it as its tab does (`Log · fenix`, not
  `*log: fenix*`), without the line and column of a caret it doesn't have,
  and the modeline never ends on a separator. A pane too narrow for its tabs
  is titled the same way.
- Git status shows an untracked folder as `notes/`, lined up with the files,
  rather than a blank name and the folder a cell further on.
- The Log's tab names what it follows -- the branch (`Log · master`), the
  file (`Log · main.rs`, `Log · a.rs:3–9`) or every branch -- rather than
  the repository.
- Settings: stepping a setting back on to its default takes it out of
  `settings.toml` rather than writing the default there and counting it as
  changed; one that overrides an inherited value still writes it.
- `SPC /` with a Search page open selects the last query, so typing
  replaces it rather than adding to it; an arrow keeps it to edit.
- The agents' dock: a question the agent asks takes the cursor when it
  comes, even when you'd moved up the thread, and the answer `j`, `k` and
  Enter are on is lit across its row. A page is never scrolled past its
  end, so a dock that grows shows the cards above rather than blank rows.
  The prompt's title (and any pane's) is cut with a "…" in a narrow dock
  rather than at its edge, an empty thread's hint wraps, and the modeline
  calls the prompt "Agent prompt", not `*agent prompt*`.
- `SPC b b` is the same card as the file picker: each buffer's icon, its
  name then its folder (in the project, else from `~`) or its tab's name,
  and `unsaved` at the right; Home and the empty scratches are listed once.
- The panel terminal, from Home or a page with no project, starts in the
  folder of the file used last rather than Fenix's own.
- The `SPC` menu: its count of commands is the command picker's; *Recent*
  lists a command once, with its area's icon; a section's heading is never
  left at the foot of a page with nothing under it.
- A file outside any project has `C: › Users › …` as its breadcrumb, not
  `C: › \ › Users`.
- Every pane showing Home has the logo, not only the first.
- The debugger's trail ribbon gives the breadcrumb back once the session
  has ended; `[t` still goes back along the trail.
- `fenix <folder>` opens the folder as the project, as `--project` does,
  rather than trying to open it as a file.
- The `SPC` menu's local leader (`m`, mode) has an icon like the rest.

## [1.3.0] - 2026-11-06

### Added

- The window title says which Fenix it is: `orbit-tools — Fenix 1.3.0`.
- A pipeline's step can be a command: `run = "cargo test"`, with no agent in
  it. It passing moves the run on, and `on_failure` sends its output back to
  an agent's step to be fixed, for `max_rounds` rounds.
- Four more built-in pipelines on `SPC a P`: *Diagnose, fix, review*, *Tests
  first, build, review*, *Plan a refactor, carry it out, review*, and *Review
  what's changed*, a review of what isn't committed.
- `fenix-hangs.log` now says which GPU and driver are in use, when the machine
  slept and woke, what happened to the window's surface and device, and any
  panic. A freeze of 30 seconds or more leaves `fenix-hang.dmp` beside it, with
  every thread's stack.
- `WGPU_BACKEND` (`dx12`, `vulkan`) chooses the graphics API.
- Code screenshots (`SPC c s`, `code.screenshot`): the Visual selection's
  lines, or the file, drawn as a picture in the theme's syntax colours and
  your font -- a window with rounded corners, a soft shadow and a title
  bar with the file's name, on a gradient -- at twice the size for a
  sharp retina image. Tabs are expanded, trailing spaces and the shared
  indentation removed, long lines wrapped or cut. It's copied to the
  clipboard and saved as `fenix-<file>-<lines>-<time>.png`; the
  notification opens the image or its folder. `SPC c S` highlights the
  selected lines among those around them. `screenshot.*` settings choose
  the background, line numbers, title bar, scale, padding, width and
  folder.

### Changed

- The agent writes a commit message from the commit box: `A` in Normal mode,
  while the box is empty, fills it from what's staged. `SPC a c` is gone, and
  the message no longer goes to the clipboard. With text in the box `A` stays
  Vim's append; clear the box to ask again.
- Extension API 1.4: `compose_with_ask`, a compose pane that `A` has the agent
  write, over an untouched template too. The Review extension uses it for a
  pull request's description and your review's summary.
- An agent's form is answered a field at a time: a picker for its choices,
  with *something else…* to type your own, and the prompt line for the rest.
  The card shows the choices and each field's description, and offers `Y`
  only when the defaults make an answer.

- A pipeline's gate stays open: write to the step for another round, as many
  as you want, and `y` hands off when you're happy. The built-in pipeline's
  build now stops at a gate too, each time, so a run no longer finishes
  unattended.
- A pipeline's step that was to change files and changed none asks before
  going on, rather than handing the next step nothing to review. A step with
  no mode of its own doesn't stay in the plan mode a session started in.

### Fixed

- A turn on an earlier step's thread no longer counts as the current step
  ending, which moved the pipeline on by mistake.
- Every `fs/write_text_file` showed as failed on agents that read responses
  strictly, Devin among them, though the file was written: Fenix answered
  `null`, not the object ACP says. `terminal/kill` and `terminal/release` too.
- An agent's form whose field names aren't plain words, `Data scope` say,
  could never be sent: it was edited as TOML, and they aren't TOML keys.
- Choices spelled `anyOf`, or titled with `enumNames`, are read as choices.
- A window whose GPU device is lost, as can happen over a long sleep, recovers:
  Fenix sets up everything it draws with again, in about a second, with nothing
  lost. Before, the window never drew again and Fenix had to be killed.
- A language server, agent, debug adapter or shell that stops reading can no
  longer freeze the window. Writing to one waited for it to read, on the UI
  thread; now that happens on a thread of its own.
- Closing a language server no longer waits on the processes it started, which
  froze the window for as long as they ran; a server started through a shell
  or `npm`'s shim now goes with everything it started.
- An extension's call isn't failed for a deadline missed while the machine was
  asleep.

## [1.2.0] - 2026-11-06

The notifications release: toasts and a history, folding, the mouse in
the editor, a thread per workspace and what agents use; extension API
1.3; and the fixes since 1.1.0.

### Added

- The mouse in the editor: a click puts the cursor on the glyph clicked,
  dragging selects (and scrolls past the pane's edge), a double click
  selects a word and a triple the line, Shift-click extends, Alt-drag
  selects a block. The border between splits drags to resize them, and a
  tab drags onto another pane, or a pane's edge to split it. A right
  click opens a menu of what can be done there -- in the text, or on a
  tab -- with each item's key beside it. The pointer resting on a symbol
  shows its hover and its problems (`editor.mouse_hover`,
  `editor.hover_delay_ms`).
- Notifications, in the manner of Dunst: toasts in a corner of the window
  with an urgency, actions and a timeout counted only while Fenix is in
  front; a history (`SPC o n`), do not disturb (`SPC t q`), rules per
  source, and the system's own notifications while Fenix is away
  (`[notifications]` settings, `notifications.test` to see them).
- An agent's finished turn, its question or its login, and a task that
  took a while or failed, notify (`notifications.agents`,
  `notifications.tasks`).
- Extension API 1.3: `notify-with` and `notify-dismiss`.
- `fenix --notify` and the control channel's `fenix/notify`: a
  notification from a script or a tool.
- Folding as Notepad++ has it: fold markers in the gutter to click,
  Vim's `za zo zc zO zC zR zM zr zm zj zk zx`, `⋯ N lines` on a closed
  fold, folds by indentation where there's no syntax tree, and folds kept
  with the session (`editor.fold_markers`).
- Agents can read what a terminal pane shows (`terminal_output`) and
  where you've been (`recent_locations`: the jumplist and your marks).
- Each workspace keeps its own agent thread: the dock follows you and
  shows the thread of the workspace you're in, while the others work in
  the background; `SPC a T` lists every thread at work, anywhere.
- What the agents use: turns, time at work, and the tokens and cost they
  report (Claude Code's tokens and cost, Codex's tokens), per agent and
  thread, on the modeline (`agents.usage.modeline`) and by day on
  `SPC a U`, with a daily budget that notifies at 80% and 100%
  (`agents.usage.daily_budget`).

### Changed

- The arrow keys are `hjkl` in Normal and Visual mode and after an operator.
- The Search page shows a match beside it once the selection has rested on it
  for a second, so going down the results no longer opens every file on the
  way. `Enter` still opens at once.
- CI caches its dependencies, runs the Windows tests and lint as two
  jobs at once, and no longer runs twice for a branch's pull request;
  dependencies get line tables only in debug builds, and there's a
  `release-fast` profile.

### Fixed

- A launch that reached a Fenix on its way out, one just killed say, ran
  without a control channel for as long as it lived, so `fenix file.rs` and
  `fenix --notify` couldn't reach it. It now waits and takes the channel.
- The wheel over a pane without the focus scrolls what's drawn at once,
  not at the next click there.
- A match the Search page shows beside it is scrolled to, not left below
  the top of the file.

## [1.1.0] - 2026-11-05

The agents and MCP release: Fenix's tools for coding agents in toolsets,
with rules for what each may do; pipelines of agents; drafting with
agents; extension API 1.2; and the fixes since 1.0.1.

### Added

- Coding agents can ask more of Fenix through `fenix --mcp`: definitions,
  references, hover and symbols from the language servers for any file,
  an open file's text with its unsaved edits, and where the debugger is
  stopped. With `agents.mcp.fenix_write` they can evaluate in the
  debugger too.
- Agents Fenix starts are told to prefer those tools
  (`agents.mcp.fenix_brief`), and use the ones that only read without
  asking (`agents.mcp.fenix_auto_read`).
- Extensions can offer agents tools (`contributes.agent_tools`).
- `agents.mcp.servers` takes the address of a running MCP server, over
  HTTP or SSE, as well as a command.
- The control channel has what these use: `fenix/buffer.read`,
  `fenix/lsp`, `fenix/debug.state`, `fenix/debug.evaluate`, `fenix/tools`
  and `fenix/tools.call`.
- Fenix's tools come in toolsets (`core`, `lsp`, `refactor`, `git`,
  `search`, `syntax`, `tasks`, `debug`, `commands`, and one per
  extension). An agent is shown those that fit the project, can turn on
  others itself (`fenix_toolsets`, `fenix_call`), and
  `agents.mcp.toolsets` adds or removes some.
- What Fenix's tools may do is decided per class (read, show, edit,
  run, external), toolset or tool (`agents.mcp.allow`: allow, ask or
  deny). A project's rules can only be stricter. When a tool is asked
  about, Fenix asks you itself, as a card in the agent's thread, whatever
  mode the agent is in: once or for the thread. MCP clients Fenix didn't
  start may only read and show unless `client:` rules say otherwise.
- Agents can ask the language servers for implementations, call and
  type hierarchies, signature help and code actions, and have them
  rename, apply a code action or format: every edit goes to Agent
  changes, as the agent's own do.
- Agents can drive the debugger: list launches, start one, set and
  clear breakpoints (with conditions and log messages), continue, step,
  run to a line, pause and stop. Each answers at the program's next
  stop, with its stack and variables. `SPC u A` clears the breakpoints
  agents set.
- Agents get Git through Fenix (`git_status`, `git_diff`, `git_log`,
  `git_show`, `git_blame`, `git_stage`), a project search that sees
  unsaved text and a replace whose edits you review, the syntax tree
  (`syntax_node`, `syntax_query`), and tasks they can wait for:
  `run_task` answers with the task's problems, tests and output
  (`task_output` for the rest).
- `fenix --mcp` offers MCP resources (an open file as it stands, the
  problems, the selection, the debugger's stop, the repository's state,
  the last task's output, and extensions' `agent_resources`), with
  subscriptions that say when one changes, and prompts (`fix_diagnostic`,
  `explain`, `review_changes`, `write_tests`, and extensions'
  `agent_prompts`). An extension's tool tells the agent its progress,
  and an agent's cancelling reaches the extension.
- Fenix's MCP server over HTTP, for clients that only speak HTTP:
  `agents.mcp.http` keeps one running on this machine, or
  `fenix --mcp --http <address>`. Clients present the token
  `fenix mcp token` prints, and follow the `client:` rules.
- Pipelines (`SPC a P`): a prompt run through agents in turn. Fenix's
  own plans (with a gate where you approve the plan), builds, and
  reviews, sending the review's findings back to be fixed. Each step is
  a thread with its own agent, mode and toolsets; their changes are one
  review. Your own go in `pipelines/*.toml`, or a project's
  `.fenix/pipelines/`.
- `SPC a c` with the commit composer open writes the agent's message
  into it as it comes, in the project's style: its recent commits, a
  commit template, and CONTRIBUTING.md's rules on commits go along.
- `SPC h a` lists every call agents made to Fenix's tools, with the rule
  that applied.
- `fenix --mcp` tells its client when its tool list changes, passes
  cancellations on to Fenix, and speaks MCP revisions 2024-11-05 to
  2025-11-25.
- The control channel's `fenix/agent.*` methods: what `fenix --mcp`
  calls, each for the thread its bridge speaks for (`client.thread` in
  `fenix/hello`).
- An agent can ask you for something as it works (ACP's
  `elicitation/create`): a form, shown as a card in its thread. `y` opens
  it in a compose pane, a line per field saying what it takes, checked
  before it's sent; `Y` sends its defaults, `n` declines, `x` cancels.
  One that asks you to sign in somewhere gives a link: `y` opens it in
  your browser, and the card says when the agent hears it's done.
- Pipelines: the dock's title says which step is running
  (`plan ✓ → build ● → review`), `[s` / `]s` go between the steps'
  threads, `e` at a gate edits what's passed on before it goes, and the
  review's findings show beside the hunks they're about in Agent changes.
- `SPC c i` goes to where the trait, interface or method under the
  cursor is implemented (listed when there are several), and `SPC c h`
  lists who calls the function under the cursor, as `gr` lists
  references.
- A breakpoint a coding agent set has a gutter shade of its own, kept
  across restarts, so you can tell it from yours; `SPC u A` clears them.
- The modeline names the MCP clients connected to Fenix over HTTP
  (`agents.mcp.http`) while they are.

### Extension API 1.2

Everything 1.2 adds is a new import, or a new export Fenix calls only
when an extension has it, so an extension built for 1.0 or 1.1 runs as
it did.

- An agent tool says what it may do (`class`: `read`, `show`, `edit`,
  `run`, `external`), how long it may take (`timeout_ms`, 30 seconds
  unless it says, up to 10 minutes) and whether it's offered by default
  (`default`). Its class must fit its permissions; a tool without one is
  `run` when it `writes`, else `read`.
- A tool call's deadline holds, an answer given later included: past it
  the agent gets an error and the extension hears `service-cancelled`.
  An extension that stops answers every call it had.
- A long tool says how far it's got (`service-progress`).
- An extension's pages go before its tool calls and events: a slow tool
  no longer freezes them.
- `fenix/tools` lists only the tools of extensions that are switched on,
  with each one's class, toolset, deadline and whether it's offered by
  default; a stopped extension's tools are marked stopped.
- `agents::ask_with`: an ask with attachments Fenix reads (a diff,
  commits, a file with its unsaved edits, the selection, the problems,
  text), a structured answer held to a JSON Schema, its draft as it's
  written (`agent-ask-progress`), a thread kept or hidden, a timeout, and
  a handle to cancel it. An ask runs in the agent's most restrictive
  mode, and a file its turn changes anyway is put back
  (`answer.reverted`). The first ask that sends code asks you, once per
  extension and agent (`agents.forget_ask_consent` asks again).
  `agents.ask.agent`, `agents.ask.model` and `agents.ask.show` choose the
  agent, the model and whether the thread stays.
- `[[contributes.agent_resources]]` and `[[contributes.agent_prompts]]`:
  what an extension offers agents to read, and prompts.
- A tool's description is at most 300 characters and its schema 2 KiB,
  for an extension built for 1.2.
- `SPC p r` runs the test the cursor is in, alone, in Task Output: a
  Rust `#[test]` (by its whole module path, in its package and test
  binary), a pytest test or test class, or a Go `TestXxx`. `SPC p T` runs
  it again. (#55)
- `SPC q r` restarts Fenix where you are, the session and unsaved text
  kept: what an extension installed or switched on since start, or a
  setting read only then, needs. The Extensions page and its messages
  say so. (#40)

### Changed

- `agents.mcp.fenix_write` and `agents.mcp.fenix_auto_read` are read as
  the rules they meant (`edit` and `run` allowed; `read` asked about),
  and `agents.mcp.allow` takes their place.
- What a coding agent says reads as Markdown rendered, in the agent
  panel: headings, emphasis, inline code, lists, quotes, tables, links
  and highlighted code blocks, laid out as the reading view lays out a
  note. Yanking a message (`y`) still copies the Markdown it said.

### Fixed

- Installing a code extension that asks for more than its sandbox
  (network, programs, secrets, your coding agents...) shows what it
  asks for and waits for your yes (#41).
- A file is open once, however its path is spelled: on Windows a file
  reached as `\\?\C:\a.rs` (from Home, the recent files or a restored
  session) and as `C:\a.rs` (from the finder or a language server) opened
  in two tabs that disagreed. (#56)
- A very long line, such as 8 MB of JSON on one line, no longer hangs the
  window: only what a row can show is highlighted and shaped. (#57)
- `Ctrl-O` in Insert runs one Normal command and comes back to Insert, as
  in Vim, rather than jumping back to another file still in Insert. (#58)
- Visual mode has text objects (`viw`, `vip`, `vi(`), `o` for the other
  end and `J` to join; `A` in a block appends on every row, a block `c`
  reaches every row, and a block yanked or deleted pastes as a block. (#59)
- Ex commands take ranges and line numbers (`:36`, `:.,$d`, `:%sort`),
  and `:d`, `:y`, `:m`, `:t`, `:j`, `:>`, `:<`, `:norm`, `:e`, `:sp`,
  `:vs`, `:bd`, `:bn`, `:bp` and `:only` work. A command Fenix doesn't
  know says "Not an editor command" instead of doing nothing. `:s` and
  `:g` read Vim's patterns (`\(`, `\+`, `\v`), and `&` and `g&` repeat
  the last `:s`. (#60)
- Text in any encoding opens as text and saves as it came: a UTF-16 or
  Latin-1 file no longer opens in the packet decoder or the hex view, and
  a UTF-8 byte order mark is kept rather than shown. `SPC f E` reads a
  file again as another encoding, and the modeline says which it is. (#61)
- The PDF reader works once its extension is installed: PDFium now comes
  with Fenix, in the zip, the installer and the Linux packages, rather
  than a PDF opening on a blank page. (#62)
- In Visual mode a count applies to the motion after it: `v3l` selects
  three more columns and `3G` goes to line 3. (#68)
- One huge unsaved file no longer stops the session being saved: its
  text stays in its recovery file, and Fenix says so. (#69)
- `cc` and `S` keep the line's indent, as `o` and `Enter` do. (#70)
- `Enter` ends the line when there's nothing to complete, rather than a
  completion popup opened on a trigger character taking it. `Tab` and
  `Ctrl-Y` still accept. (#71)
- The debugger says why an adapter refused: lldb-dap's reason for a
  failed `SPC u e` (`error: use of undeclared identifier ...`), from the
  error's details, instead of "the adapter refused, without saying why".
  (#32)
- A debug session that has ended no longer blocks starting one in another
  project ("debug session belongs to another project"): the new one
  replaces it, as it does in the same project. (#31)
- A Rust panic in Task Output is a place to go: `SPC p n` opens the
  file and line it names (`panicked at src\lib.rs:89:9`), not an empty
  buffer named for the whole line, and `Enter` on any line of Task Output
  that names a `path:line:col` goes there. (#19)
- `SPC f f` on a file that doesn't exist yet opens a new file by that
  name, rather than a `[No Name]` buffer with the path lost, and `SPC f s`
  creates it, and any folders it's in. (#18)
- `fenix/open` answers `-32602` with the reason for a path it can't
  open (an empty one, a folder, a file in a folder that doesn't exist),
  and opens nothing, rather than `{}` and a `[No Name]` tab. A new file
  in a folder that exists opens as `SPC f f` opens it. (#39)
- Paths read as you'd write them on Windows, with no `\\?\` and one kind
  of separator: in the refactor preview and its messages, the projects
  Fenix switches to, launch configurations handed to an extension
  (`${file}`, `${project}`), and the control channel (`fenix/state` said
  `...\ledger\src/main.rs`). (#38)
- A language server that never answers `initialize` no longer shows
  "starting" for good: after 30 seconds Fenix stops it and says so, with
  the last thing it wrote. Tool Status (`SPC l m`) lists each server that
  failed, why, and the last lines of its stderr. (#34)
- `fenix --help` lists the options (`--mcp`, the extension options and
  the rest) and `fenix --version` prints the version; both print and
  exit, on Windows too, rather than handing over to a running Fenix. An
  option Fenix doesn't know (`--projet`) is warned of. (#36)
- The commit composer, and any other blank one, opens in Insert, so what
  you type first is the message, not commands (`P` pasted the
  clipboard). One with text to edit (amend, reword) still opens in
  Normal. (#48)
- Finishing a merge (`SPC g x c`, or `c` on the Git page) commits with
  Git's prepared message (`Merge branch 'feature'`), opened in the
  composer to edit first, rather than with "Merge". (#45)
- "no known debug adapter" says where to look: the project's
  `tools.json`, an extension's tool on `PATH` (`SPC p h`), or a restart
  (`SPC q r`) for one installed since Fenix started. (#40)
- Completion no longer offers the word being typed (`clas` beside
  `class`), unless it's written elsewhere in the file too, so `Enter`
  accepts the word you meant. (#73)
- A picker closes once Enter has run what it picked: `SPC :` and a
  command that opens nothing of its own left a blank PICKER page, with
  keys going to the buffer behind it. With nothing matching, Enter keeps
  the query. (#30)
- A first setting saved to a new `settings.toml` goes after its header
  comment, not above it. (#51)
- The `"0` register holds the last yank (`yyjdd"0p` pastes the yanked
  line), and `"_` deletes without touching any register or the
  clipboard. (#26)
- Home opens on the file to resume when there is one, so the `Enter`
  its column shows resumes it rather than opening the file finder. (#49)
- `SPC g g` with a file from outside any repository in front (one left
  open after a project switch) opens the project's repository, or says
  that neither is in one, rather than nothing. (#47)
- In the Merge view, `o` and `t` on the header row, where the cursor
  starts, take the only conflicted file, as the hints above it say,
  rather than doing nothing. (#46)
- `SPC p t` lists each task once: an extension's and Fenix's own with
  the same name or command line are one row, the extension's. In a uv
  project Fenix's `pytest` and `ruff check` run through `uv run`. (#35)
- An idle window no longer copies the focused file's text and looks at
  the disk each frame to tell its language servers what they already
  know. (#74)
- The project wizard shortens a long path in the middle, keeping the
  project's own folder in view, rather than cutting off its end. (#53)
- Fenix's window comes up before its extensions do: one being let go
  while it's still loading -- starting again in another project -- is no
  longer waited for, nor are the language servers being stopped. An
  extension setting its language server starts again only that
  language's, and only when it changes, not every server of the project.
  With 40 extensions the window was held up for seconds. (#33)
- A language pack that starts with Fenix (`on_startup` and a context
  naming markers or languages) waits for a file of its languages, or a
  project of its markers, before it loads. `[activation]` can say so for
  any extension: `languages` and `markers`. (#33)
- Moving to another project starts again at once only the language
  packs for it; another extension that reads the project starts again
  there the next time it's used -- a command, a page, a service, its
  language server -- and carries on as it was if you come back first.
  Every extension used to start again at every move. (#33)
- An agent's `rename` and `apply_code_action` work with language servers
  that name document versions, as rust-analyzer does: every one was
  refused as stale.
- A pipeline's plan step stops at the plan: its agent's offer to carry
  it out no longer has it build everything, only for that to be put
  back and its summary passed on as the plan. Its gate says it's Fenix
  asking.
- A pipeline ends on the thread with its changes, so `SPC a r` finds them.
- There's one agents' dock, and it comes with you between workspaces, so
  a card an agent waits on stays in sight. Opening it in a second
  workspace made a second one that shared its thread: putting either
  away left the other a Home pane and a prompt, and the first lost its
  titles. A new workspace from the dock starts on Home, not on a thread
  with no prompt.
- A file an agent shows, or that's opened from outside, goes beside the
  dock, not over a thread and its cards.
- `SPC c i` and `SPC c h` say so when no language server runs for the
  file, instead of doing nothing.
- A window that hasn't drawn the editor 10 seconds after the splash
  handed over, still showing the splash, makes its surface again and
  asks for a frame, and says on stderr what was in the way (`fenix
  --console` on Windows shows it). (#65)
- Tool Status (`SPC l m`) agrees with the doctor and the sessions: it
  resolves each server and debug adapter as they do, an extension's
  included (the Rust extension's `lldb-dap`); a server installed off
  `PATH` (pyright in `~/.local/bin`) says where, not `[missing]`; one
  that's running is found, not `[missing, running]`; and Go's and Lua's
  servers have install hints. (#43)
- The project doctor names a project Fenix has no kind of its own for
  -- Zig, Ruby, PHP, Swift, Java, Kotlin, C#, Ada, Lua -- as the
  language pack whose markers it has calls it (`zig (Zig)`), not
  `(Project)`, on the page and in the copied report. (#52)
- `q` closes Tool Status (`SPC l m`) and Task Output, as it closes the
  other pages, rather than starting a macro; closing either (`q`,
  `SPC b d`) closes the workspace it opened in, and its title strip. With
  Task Output's workspace removed (`SPC TAB d`), the next task opens it
  again rather than running with its output shown nowhere; the Git,
  History and Merge views keep track of their workspaces as others are
  removed. (#29)
- With more tabs than fit, a pane's tab strip scrolls to keep the
  active tab in view, rather than leaving it past the right edge; and
  `SPC b b` lists a page by its title, as its tab does, not as
  `*scratch*`. (#50)
- While a terminal has the keyboard (`SPC o t`, or one in a pane), the
  modeline's mode says `TERMINAL` with `Ctrl-\ back to the editor`
  beside it, and the editor's caret is hidden behind the panel, so it's
  plain that keys, `SPC` included, go to the shell. (#42)
- An extension's page that couldn't be shown -- a `bad.pdf` that isn't
  a PDF -- says so on the page, with what went wrong, rather than going
  blank once the red message fades. (#75)
- An extension installed, switched on or off, or removed from the
  Extensions page is put in use at once, without a restart: its code
  starts or stops (its pages, language servers, tasks and checks with
  it), and its commands and keys come and go. Only its themes and
  languages wait for the next start, and the page and its messages say
  so. A setting read only at start (`agents.mcp.http`) is marked so, and
  saving it says `SPC q r` restarts now. (#40)
- An idle window stops drawing: the caret blinks for 30 seconds after
  the last key or click, then stays lit, rather than waking the window
  for every blink and fade forever; and an extension's moving layer (the
  Pets extension's pet walking) is drawn at most 30 times a second, not
  every frame the display can show. (#74)
- `SPC p t` says where each task comes from (`[Fenix]`, the extension
  that gave it, or `[tools.json]`) and its command line, so two of one
  name can be told apart. The project doctor (`SPC p h`) lists each tool
  once: where an extension checks what Fenix does (`rust-analyzer`,
  `Rust`), its row takes Fenix's place, and the summary counts it once.
  (#35)
- A language server is looked for and started off the UI thread,
  Windows' search of `PATH` included: a slow `PATH` (a network drive on
  it) or a server slow to start no longer freezes the window as a file
  opens. (#33)
- The explorer's `x` on an archive named in capitals (`Backup.TAR.GZ`)
  unpacks into a folder spelled as it is (`Backup`), not lowercased.

## [1.0.1] - 2026-11-03

### Added

- Linux packages with each release, a `.deb` and an `.rpm` beside the
  archive: Fenix in your applications menu and `fenix` on `PATH`.
- A [Scoop](https://scoop.sh) bucket, `tpedneault/scoop-fenix`:
  `scoop bucket add fenix https://github.com/tpedneault/scoop-fenix`, then
  `scoop install fenix`.
- The project doctor (`SPC l m`) checks that rust-analyzer can read the
  standard library: a `rustc` on `PATH` without its source (a standalone
  install ahead of rustup's) made it report errors that aren't there.

### Extension API 1.1

Everything 1.1 adds is something an extension asks of Fenix, so an
extension built for 1.0 runs as it did.

- `git-state`: the open project's repository (branch, commit, ahead and
  behind its upstream, conflicts), for an extension that asks for `git`
  events.
- `last-task`: the task that ended last, and its tests passed and failed
  when its output gives them (cargo, pytest, Jest, Vitest, `go test -v`).
- An extension may hear of agents' turns (`events = ["agents"]`) without
  `permissions.agents`. It's told a turn ended and how, but not the
  thread's title or what was said.

### Fixed

- The which-key menu no longer lists an extension's keys, with no label,
  when that extension isn't installed (`SPC g P` and `SPC g M`, the
  Review extension's).
- `q` in the Git graph view (`SPC g G`) closes it, as in the other Git
  views, rather than starting a macro recording.
- An extension's layer is drawn when its next frame is due: a model
  drawn a few times a second (the Pets extension's) moved only when
  something else redrew the window.

## [1.0.0] - 2026-10-31

The first release: a keyboard-first, GPU-rendered editor with Vim's
editing grammar and a `SPC` leader for everything else.

### Editing

- **Vim, for real**: motions, operators, text objects, counts,
  registers, macros, marks, the jumplist, `:s` with backreferences, and
  Visual char, line and block modes.
- **A `SPC` leader with which-key**: every command a short mnemonic
  sequence, with a popup of what comes next.
- Tree-sitter highlighting and folding; native snippets with fields,
  mirrors and transformations; structural reindentation.
- Staged, atomic saves; crash recovery for unsaved buffers; the session
  (windows, splits, cursors, scroll positions) restored at the next
  start.

### Projects and files

- A project hub, a new-project wizard with templates, and per-project
  settings, tasks, language servers and debug launches
  (`.fenix/settings.toml`, `.fenix/tools.json`).
- Fuzzy pickers for files, symbols and buffers; a Search page that
  searches and replaces across the project.
- A file explorer and file manager: marks, copy and move, archives, the
  Recycle Bin, hidden files on demand, bookmarks.
- Terminals in panes and a terminal panel, started in the project.

### Code intelligence and debugging

- LSP: completion, hover, diagnostics, code actions, rename with a
  multi-file preview, several servers on one file with their answers
  merged.
- A debugger in the code, over the Debug Adapter Protocol: breakpoints
  and the current line in the gutter, values written inline, a DEBUG
  mode with the usual F-keys, and a dock for the stack, variables,
  watches and the console.
- A task runner that reads the project's own tasks, with output you can
  jump from.

### Git

- A status page with hunk staging, commits and amends, blame, a commit
  graph, side-by-side compare, interactive rebase, conflict resolution,
  worktrees, and undo for Git operations.

### Coding agents

- Claude Code, Codex, Devin, Gemini CLI or any agent that speaks the
  Agent Client Protocol, in a dock beside the code: threads that come
  back with the session, `@` mentions, the commands an agent runs shown
  as it runs them, and every change it makes reviewable hunk by hunk.
- `fenix --mcp`: Fenix as an MCP server for agents, with the language
  servers, diagnostics and the open buffers.

### Extensions

- Sandboxed WebAssembly extensions on extension API 1.0
  (`fenix-extension-api`): pages, canvases, surfaces, decorations,
  language servers and debug adapters, sockets, HTTP, tasks, coding
  agents, each held to the permissions its manifest asks for.
- A signed registry and a store inside Fenix (`SPC h x`), with packs
  offered at the first start; data-only extensions for themes, snippets,
  templates, keymaps and languages.
- Code review for GitHub and GitLab, containers, an agenda that syncs
  with Jira, a notebook with Mermaid diagrams, a PDF reader, VNC
  consoles, language packs, and tools for Arduino, Tcl, SCOS-2000 MIB
  and CCSDS -- all in the store.

### And

- An interactive tutorial (`SPC h T`): editing, Fenix's pages, Git, and
  the extensions you install.
- Every setting on one page (`SPC ,`), in `settings.toml` underneath.
- The control channel: `fenix file:line`, and a JSON-RPC protocol other
  tools drive Fenix with.
- A Windows installer, for you alone or for everyone on the machine,
  and archives for Windows and Linux.

[Unreleased]: https://github.com/tpedneault/fenix/compare/v1.5.0...HEAD
[1.5.0]: https://github.com/tpedneault/fenix/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/tpedneault/fenix/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/tpedneault/fenix/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/tpedneault/fenix/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/tpedneault/fenix/compare/v1.0.1...v1.1.0
[1.0.1]: https://github.com/tpedneault/fenix/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/tpedneault/fenix/releases/tag/v1.0.0
