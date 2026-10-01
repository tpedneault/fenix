# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The notebook (`SPC n`): notes, journal days and Mermaid diagrams Fenix
  keeps for you without asking where -- plain files in the data folder
  (or any folder, an Obsidian vault included), saved as you type with
  earlier versions kept. A page to find, tag, pin, rename and move them;
  templates; `[[links]]` with completion, following and backlinks; a
  daily journal with the agenda and your commits; quick capture from
  anywhere; search with `tag:`, `type:`, `project:` and `is:todo`;
  checkboxes to the agenda; and a Notebook list on Home.
- A reading view for any Markdown file (`SPC m p` beside, `SPC m r` in
  place): tables, highlighted code, callouts, checkboxes you can tick,
  pictures and diagrams.
- Mermaid diagrams drawn as you type, natively: errors on their line,
  the last good drawing kept, zoom, pan and node walking, Mermaid's
  themes plus one from the editor theme and your own, `.mmd`
  highlighting, `K` on a ```` ```mermaid ```` block, and export to SVG, PNG, the
  clipboard or Markdown (optionally through mermaid-cli). Notes export
  to Markdown or one HTML page.
- `Ctrl+Alt+Q` does what `Esc` does, for tools that can't send Escape;
  `FENIX_STANDALONE=1` runs a second Fenix (a test run) beside yours.

- Highlighting for Go, Java, C#, Lua, SQL, HTML and CSS, with comment
  toggling and TODOs; `gopls` and `lua-language-server` as the default
  language servers for Go and Lua.
- Motion and polish: a launch splash, a key menu that opens after a
  pause on `SPC`, `g`, `z`, `]`, `[` or `SPC m`, animations that show
  what changed (jumps, undo, popups, modeline messages, mode colour,
  tabs, themes), rounded popups with shadows, wavy underlines for
  problems, an overview ruler, sticky scroll, dimmed unfocused panes and
  rainbow brackets. `motion.level` and a setting per effect, in a new
  Motion category of `SPC ,`.
- The PDF reader, rebuilt: a PDF opens as a tab like any file and can be
  read in two panes at once; its pages scroll as one continuous column
  and are rendered ahead; a sidebar holds the outline, the search's
  matches and marks; search highlights every match on the page; text
  can be selected and copied, and links followed with `f` or a click;
  pages can be drawn in the theme's colours; each PDF reopens where you
  left it; Home lists what you're reading; the shelf (`SPC r f`) can
  come from the project; PDFs rewritten on disk reload; sessions keep
  PDF panes.
- `gf` opens the file named under the cursor; `spec.pdf#page=38` opens
  the PDF at that page, and `yp` in a PDF copies such a link.
- Enter on the Font setting lists the installed fonts to pick from.
- SCOS-2000 MIBs, reworked (`SPC k`): the MIB page lists telecommands,
  TC parameters, TM packets, TM parameters and calibrations, with search
  and `field:value` filters; each definition has a page where every
  name is a link (back and forward with `Ctrl-o`/`Ctrl-i`), with bit
  layouts, decoded types, calibrations, limits and what uses it;
  telecommands are inserted from a form showing every argument, checked
  against the MIB, and `SPC k e` edits a call already written; `K`, `gd`
  and completion know MIB names in scripts. MIB problems (unreadable
  files, broken references) are listed with their line.
- A project lists its own MIBs and telecommand templates in
  `.fenix/settings.toml`; they take the place of yours in that project,
  and a project that is a MIB uses itself. MIBs load in the background
  and reload when their files change.
- CCSDS and PUS tools (`SPC k`): a packet inspector decodes hex in any
  spelling layer by layer (space packet, PUS-C or PUS-A, time codes,
  the MIB's parameters with calibrations and limits, CRC), and reads
  TM, TC, AOS and USLP frames with Reed-Solomon decoding, CLTUs, CFDP
  PDUs and CLCWs; a time converter (CUC, CDS, UTC, TAI, GPS, leap
  seconds); a view-only hex view, where files that aren't text now
  open; recordings and live sources (TCP, UDP, a file being written,
  NATS) as a packet list with gaps, problems and a parameter followed
  through time; a standards library linking fields to their PDFs.
  Recordings and live sources also read CLTUs (BCH, tail sequence, the
  TC frame's length and FECF, retransmissions, the telecommands inside)
  and check TM frames' FECF; `f` on a packet shows the CADU or CLTU it
  came in. Mission settings live in a new CCSDS & PUS category.
- CLTUs are decoded in error-correcting mode (one wrong bit per code
  block put right; `ccsds.tc_bch` for detecting only), and a randomized
  uplink is de-randomized (`ccsds.tc_randomized`, or recognized from the
  first CLTU).
- A followed parameter lists its limit crossings (`c` steps through
  them, `r` shows raw values); on a live source it keeps up with the
  rows and a crossing is said in the modeline. `t` follows a parameter
  from its MIB page or the inspector.
- Time correlation (`ccsds.time_correlation`) moves the epoch on-board
  times are read from, and the converter shows them uncorrelated too;
  `ccsds.clock` shows the on-board time in the modeline; `K` on an
  on-board time in a file says when it was.
- `/` on the standards page searches every standard's text at once.
- `]d`/`[d` step through a buffer's problems.
- The SCOS-2000 MIB template writes a mission profile and a
  `recordings/` folder, and the doctor checks the MIB against the
  profile (header size, checksum) and the standards.
- A field of a list setting that takes only certain values is picked
  from them in its form (`←`/`→`), not typed: a live source's framing is
  the first.
- `dev/ccsds-sim`: a simulated spacecraft in Docker (TM over TCP, UDP,
  NATS and a file; CADUs with and without an FECF; the uplink's CLTUs)
  with a ready mission project, to try the CCSDS tools against.
- The MIB is checked against the standards, telecommand calls in
  scripts are checked against it, the MIB page gains a services tab,
  and `SPC k g` generates XTCE, a Wireshark dissector, a C header, a
  Python module, an ICD or test vectors from it. XTCE files can be
  listed as MIBs.
- A tab keeps its cursor when you switch away and back.
- Fields typed on a page have a caret: arrows, `Ctrl-←`/`Ctrl-→` a word
  at a time, `Home`/`End`, `Delete`, and `Ctrl-Backspace` (or `Ctrl-W`)
  to delete a word. `Ctrl-O` browses from the name of a list of paths
  and from the project's program and cwd, and the explorer's `S` picks
  a folder for any path setting.
- Search where the mission pages lacked it: `/` on a MIB definition's
  page keeps the rows with the words typed; the MIB page's search
  narrows the services tab and the problems list; the packet inspector
  finds fields (`/`, `n`, `N`); a recording's or a live source's filter
  narrows every tab -- by APID, the followed parameter's samples,
  problems -- and takes ranges (`apid:0x100..0x1FF`), `name:`, `time:`,
  `len:`, `dir:tc`, `check:ok`, `value:`, `limits:out` and `-` to leave
  something out.

### Fixed

- Arduino completion no longer stops after the first edit: Fenix now
  accepts the language server's progress-token request, whose refusal
  crashed `arduino-language-server`, so suggestions come from the
  server instead of falling back to words in the file.
- C++ and Arduino sketches are highlighted properly (keywords, types,
  numbers, comments); the C++ grammar's rules were used without the C
  rules they build on.
- Language servers, debug adapters and the explorer's git status no
  longer flash a console window on Windows.
- Inserting a telecommand no longer freezes Fenix when the project's
  argument template doesn't name the argument before its value
  (`{value}`): such calls are read back by position.
- The Git graph and the Log page open quickly on a large repository:
  git is no longer asked for `--date-order`, which reads the whole
  history first; the History view reads off the UI thread; and Git and
  diff buffers stop copying their line data on every frame.

### Changed

- **Breaking:** the PDF reader's keys follow Vim. `J`/`K` turn the page
  (`n`/`p` did), `gg`/`G`/`{n}G` go to a page (`g` went to page 1),
  `=`, `zw`, `zp` and `z0` fit the page (`0` and `w` did), and `n`/`N`
  now step through search matches. `SPC r` keeps its keys.
- `SPC t a` cycles motion between off, subtle and full instead of
  switching animations on and off.
- The PDF reader's settings (`documents` among them) have a category of
  their own in `SPC ,`; "Documents & workspaces" is now "Workspaces".
  Setting keys are unchanged.
- **Breaking:** the MIB lookups moved from `SPC m` in Tcl files to `SPC
  k` everywhere (Tcl's `SPC m` keeps its letters); the plain-text detail
  buffers and the one-argument-at-a-time insert prompts are gone, and so
  are `SPC m a`/`SPC m d` -- MIBs are listed in the settings (`SPC k ,`).
  The MIB settings have a category of their own in `SPC ,`; their keys
  are unchanged.
- On a MIB definition's page `/` filters the page; `m` still opens the
  MIB page.

## [1.0.0] - 2026-09-26

The first release. Fenix is a keyboard-first, GPU-rendered text editor
with Vim editing and a `SPC` leader layer, for Windows and Linux.

### Editing

- Vim's modes, motions, operators, text objects, counts, registers,
  macros, marks, jumplist, search and `:s` substitute, with the unnamed
  register mirrored to the system clipboard.
- A `SPC` leader menu with a which-key popup, and a per-language
  `SPC m` menu.
- tree-sitter highlighting and structural folding, breadcrumbs,
  highlighted TODO comments, XML editing helpers and Markdown list
  continuation.
- Native snippets with fields, mirrors and transformations, managed
  from Fenix (`SPC i S`).

### Files, projects and workspaces

- A file explorer sidebar and a full file manager: bulk rename by
  editing the listing, archives, the Recycle Bin, and large transfers.
- A project hub, a new-project wizard with 15 templates, a project
  doctor, monorepo support, and fuzzy pickers for files, buffers and
  symbols.
- Project-wide search and replace with a review buffer, powered by
  ripgrep.
- Split windows, workspaces, tabs with a pinned Home in each workspace,
  and preview tabs for jumps.

### Code intelligence and tools

- Language servers (completion, hover, diagnostics, and rename or code
  actions through a multi-file preview), a DAP debugger, and a task
  runner with process-tree supervision.
- Terminals in panes and a popup terminal.
- Arduino sketches: build, upload, serial monitor, and completion.
- Tcl completion, signatures and hover; SCOS-2000 MIB lookups; and a
  table view for tab-separated data.

### Git and code review

- A Git status page, hunk staging, inline blame, a commit graph, a log
  that acts, interactive rebase, conflict resolution, worktrees, and
  undo for Git operations.
- GitHub pull requests and GitLab merge requests: create, review, reply
  to and resolve threads.

### Integrations

- A personal agenda with tasks, a Kanban board, time tracking and a
  Today page, linked and synced with Jira.
- A Docker/Podman panel, a PDF viewer, and VNC console panes.

### Reliability

- Staged, atomic saves, crash recovery for unsaved buffers, and session
  restore of windows, splits, cursors and scroll positions.
- Settings in `settings.toml`, edited by hand or on the settings page
  (`SPC ,`), with a per-project layer.
- On Windows, Fenix runs without a console window; `fenix --console`
  opens one to show what it prints.

[Unreleased]: https://github.com/tpedneault/fenix/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/tpedneault/fenix/releases/tag/v1.0.0
