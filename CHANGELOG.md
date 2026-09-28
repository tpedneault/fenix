# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
