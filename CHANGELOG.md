# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/tpedneault/fenix/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/tpedneault/fenix/releases/tag/v1.0.0
