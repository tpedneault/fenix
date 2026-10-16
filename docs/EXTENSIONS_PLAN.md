# Extensions: a phased plan for Fenix 2.0

Status: accepted direction; nothing built yet. Work happens on the
`release/2.0` branch (see §2).

Fenix 2.0 turns Fenix into a **core editor plus extensions**. The core
is what any good text editor needs: editing, projects, language servers,
the debugger, Git, terminals. Everything else becomes an extension you
can install, enable and disable from an in-editor browser. That covers
the notebook, the agenda, Jira, PDF, VNC, diagrams, Docker, the mission
tools, Tcl and Arduino. First-party extensions are published from
`tpedneault/fenix-extensions`.

**Decisions taken**

| Question | Decision |
|---|---|
| What becomes an extension | Everything that isn't core (§3), including GitHub/GitLab review |
| What stays core | Git, the project manager, LSP, DAP, every theme, and the rest of §3 |
| Naming | "Extension" for everything, compiled-in or installed. No "modules" |
| Runtime for extension code | WASM components (wasmtime + WIT) |
| Scripting language | Lua, specifically **Luau** (§8): first for user scripting, later powerful enough to write full extensions, as in Neovim |
| Default install | The core only |
| Bundling | Every 2.x release zip carries the first-party extensions, installed but switched off (§7) |
| Version | **2.0** (a major release) |
| Where the work lands | `release/2.0`, merged to `master` when 2.0 ships |

It's written against the code as it stands (`bcdd773`), so it starts
with what is in the way.

---

## 1. Where Fenix is today

The crates are already well split. Almost every feature has a
host-agnostic crate (`fenix-mib`, `fenix-ccsds`, `fenix-jira`,
`fenix-docker`, `fenix-notebook`, ...) with no knowledge of windows or
the event loop. The coupling all lives in `fenix-gui`, and it's a lot:

| Seam | Today | Why it blocks extensions |
|---|---|---|
| `App` (`app.rs`, 41k lines) | 233 fields, about 57 of them owned by one optional feature (`mib_*`, `pdf_*`, `vnc_*`, `docker_*`, `agenda_*`, `notebook`, `tcl_*`, ...) | Feature state can't be added or removed at runtime; every feature edits the same struct |
| `CommandRegistry` (`commands.rs`) | 256 commands; `id: &'static str`, `fn` pointers taking `&mut App` | No commands that are registered at runtime, owned by something, or carry closures |
| Leader keymap (`keymap.rs`) | Built in Rust with `t.insert(&[spc, ...])`; groups hard-claimed (`SPC d` docker, `SPC j` jira, `SPC k` mission, `SPC n` notebook, `SPC a` agenda, `SPC v` vnc, `SPC r` pdf) | No user keybindings, no contributed bindings, no conflict handling |
| `SPC m` contexts (`keymap.rs`) | A fixed enum of buffer/project kinds | Extensions can't add a major-mode menu |
| `BufferKind` (`fenix-buffers`) | Closed enum: `Docker`, `Vnc`, `Pdf`, ... | A new kind of buffer needs a core change |
| `FenixUserEvent` / `PageEvent` | Closed enums with variants per feature (`JiraIssues`, `MibLoaded`, `StreamRows`, `VncFrame`, `NotebookOpened`, `DiagramDrawn`, ...) | Background work can't report back without a core change |
| `VncPipeline`, `PdfPipeline` | Two textured-quad GPU pipelines, one per feature | Extensions can't put pixels on screen |
| `Config` (`fenix-config`) | Typed fields per feature (`jira_base_url`, `mib_roots`, ...) and a static schema | No settings registered at runtime |
| `Secret` (`fenix-config/secrets.rs`) | Closed enum `GitLab \| Jira \| GitHub`; tokens kept in plaintext in `settings.toml` | No secrets namespaced per extension; a third-party extension that can read settings can read every token |
| Themes (`theme.rs`) | Rust `const`s, looked up with `by_name` | Can't be installed as data |
| Control channel (`fenix-control`) | `fenix/notebook.capture`, `fenix/agenda.*` built in | Methods must move with their extension |
| Session restore (`app/session.rs`) | Rebuilds `Docker`/`Jira`/`Vnc`/`Pdf` buffers by kind | Has to degrade gracefully when an extension is missing |
| Settings page, project doctor, tool status, project wizard templates, home dashboard | Each lists features by hand | Each needs to become a contribution point |

Two existing pieces help a lot:

- **Pages are data.** `page::Page` is text, colour spans with a `Role`,
  rules, a focused row, a popup and images, and a page gets decoded `Key`s.
  It's already the right shape for UI an extension sends across a boundary.
  Most feature pages (Jira, agenda, MIB, stream, packet, Docker, Arduino,
  notebook) are built this way.
- **There's already an RPC protocol.** `fenix-rpc` and `fenix-control`
  give a JSON-RPC framing, a command list (`fenix/commands`) and remote
  command execution (`fenix/runCommand`).

---

## 2. Branching and releasing 2.0

- **`release/2.0`** was cut from `master` at `bcdd773`. Every pull request
  for this project targets it, not `master`. When 2.0 is done,
  `release/2.0` is merged into `master` and `v2.0.0` is tagged from
  `master`.
- **Keep `master` flowing in.** Bug fixes land on `master` (so 1.0.x
  patches can still ship) and `master` is merged into `release/2.0` at
  least weekly, and always before a large refactor PR. Use merge commits,
  never rebase: other people's branches sit on top of it.
- **Freeze feature work on `master`** while Phase 0 is in flight. Phase 0
  moves code out of `app.rs`, and new features added there at the same
  time would each mean a painful merge. A new feature should either wait
  or be built as an extension against `release/2.0`.
- **Pre-releases from the branch.** `release.yml` builds any `v*.*.*`
  tag and checks it against the workspace version. Setting the version
  on `release/2.0` to `2.0.0-alpha.1`, `-beta.1`, ... gives testers real
  builds without touching `master`. (Mark those GitHub releases as
  pre-releases.)
- **CI already covers it.** `ci.yml` runs on every push and pull request,
  whatever the branch. Phase 1 adds a second build of the core alone
  (Cargo features off) so the core can't quietly depend on an extension.
- **Test on Windows before merging.** Windows is the primary platform,
  and CI doesn't exercise the GUI. Every PR to `release/2.0` runs
  `scripts/ci.ps1 -Suite Workspace` on Windows and gets a hand smoke test
  of what it touched. The smoke test runs `fenix --console` with its own
  `FENIX_HOME` and `FENIX_STANDALONE=1`, so it never touches real
  settings or a running Fenix. The PR description records what was
  tried.
- **Why 2.0.** The default install loses features and the way Fenix is
  put together changes, which is a major version under `CONTRIBUTING.md`'s
  semver rules. Being a major version doesn't excuse a rough upgrade,
  though: §7 still makes sure a 1.x user keeps every feature they use.
- **Scope:** 2.0 ships Phases 0–4: the platform and every feature moved
  out. Lua scripting (Phase 5) and Lua extensions (Phase 6) add things
  without breaking any, so they ship as 2.x minor releases from `master`.

---

## 3. What's core and what's an extension

**Core (the default install):**

| Area | What's in it |
|---|---|
| Editing | Vim grammar, registers, macros, marks, `:s`; `SPC` leader and which-key; snippets; formatting; table view |
| Buffers and windows | Splits, tabs, workspaces, multiple OS windows |
| Files | Explorer, file manager, fuzzy pickers, recent files, staged saves, crash recovery, session restore |
| Projects | Project hub, new-project wizard (generic templates), per-project settings, ripgrep search and replace, project doctor |
| Code intelligence | tree-sitter highlighting/folding for mainstream languages, LSP (several servers per buffer), DAP debugger, task runner |
| Git | Status, hunk staging, blame, log/graph, compare, rebase, conflicts, worktrees, undo; the diff and merge views |
| Tools | Terminals (panel and in panes), Markdown reading view, control channel |
| UI | Every theme Fenix ships today, settings page, Home |
| New in 2.0 | Command palette, keybinding tools, the Extensions page |

**Extensions (first-party, in `fenix-extensions`):**

| Extension | From today's | Notes |
|---|---|---|
| `fenix.notebook` | `fenix-notebook`, `SPC n`, `fenix/notebook.capture` | Optional dependency on `fenix.diagrams` for embedded diagrams |
| `fenix.diagrams` | `fenix-diagram` | Mermaid rendering as a *service* other extensions call |
| `fenix.agenda` | `fenix-agenda`, `SPC a`, `fenix/agenda.*` | Exposes a sync-provider contribution point |
| `fenix.jira` | `fenix-jira`, `SPC j` | Plugs into the agenda's sync provider when it's installed |
| `fenix.pdf` | `fenix-pdf`, `SPC r` | Needs a renderer that runs in WASM (§5.4) |
| `fenix.vnc` | `fenix-vnc`, `SPC v` | Needs sockets and the surface API (§5.4) |
| `fenix.docker` | `fenix-docker`, `SPC d` | Phase 3 pilot |
| `fenix.arduino` | `fenix-embedded` | |
| `fenix.github`, `fenix.gitlab` | `fenix-github`, `fenix-gitlab`, review pages | Code review; core Git stays. Both use a `forge` provider over `fenix-forge`'s model |
| `fenix.tcl` | Tcl grammar, keywords, ctags completion | A language pack |
| `fenix.mib`, `fenix.ccsds` | `fenix-mib`, `fenix-ccsds`, MIB/packet/stream pages, hex view | |
| `fenix.mission` | Mission template, mission clock | An extension pack: depends on `fenix.mib`, `fenix.ccsds`, `fenix.tcl` |

Git stays core; only GitHub/GitLab review moves out. Every built-in
theme stays core. Extensions and users can *add* themes as data (Phase
2), but none of today's move out. Mainstream tree-sitter grammars stay
compiled in for 2.0. Moving them to WASM language packs is a later
option (§6, Phase 4).

---

## 4. How extension code runs

- **Declarative contributions (no code)**: themes, snippets, keymaps,
  language definitions (file types, comment tokens, LSP server commands),
  project templates, task problem matchers. These are TOML files in the
  extension.
- **WASM components** for code (wasmtime, WIT interfaces, target
  `wasm32-wasip2`). The feature crates are pure Rust, so most of them
  compile with few changes. Extensions get one sandboxed artifact for
  every OS, a versioned interface, and capabilities the host enforces.
- **Lua** (Phases 5–6): first scripting for users, then a second way to
  write full extensions. Lua extensions use the same manifest, package
  format, permissions and host API as WASM ones (§8).
- **Language servers** remain the way to add heavy language smarts. An
  extension can ship or point at a server.

Native plugin libraries are ruled out: Rust has no stable ABI, and a DLL
per OS defeats the point of one artifact.

---

## 5. The extension model

### 5.1 Package layout

An extension is a `.fenixext` file: a zip with a manifest at its root.

```
jira-1.2.0.fenixext
├── extension.toml
├── extension.wasm        # runtime = "wasm"
├── lua/                  # runtime = "lua" (Phase 6)
├── README.md             # shown on the details page
├── CHANGELOG.md
├── icon.png              # optional
├── themes/ snippets/ templates/ languages/ keymaps/   # optional data
```

### 5.2 Manifest

```toml
[extension]
id = "fenix.jira"                 # unique in the registry
name = "Jira"
version = "1.2.0"
description = "Jira issues, search and agenda sync"
authors = ["Thomas Pedneault"]
license = "MIT OR Apache-2.0"
repository = "https://github.com/tpedneault/fenix-extensions"
categories = ["integrations"]
runtime = "wasm"                  # wasm | lua | none (data only)

[engine]
fenix = ">=2.0, <3"               # which Fenix releases it runs in
api = "0.3"                       # the host API version it was built against

[dependencies]                    # required, by id
[optional-dependencies]           # used when present
"fenix.agenda" = "^1"

[activation]                      # lazy: nothing loads until one of these
commands = ["jira.*"]
languages = []
files = []
on_startup = false

[permissions]                     # shown before install; enforced by the host
network = ["${setting:jira.base_url}"]
secrets = ["jira.token"]
process = []                      # e.g. ["docker", "podman"]
sockets = []                      # e.g. ["*:5900-5999"] for VNC
filesystem = "none"               # none | project-read | project-write | user-picked

[[contributes.commands]]
id = "jira.open"
title = "Open Jira"
external = true                   # may run from fenix/runCommand

[[contributes.keybindings]]
keys = "SPC j j"
command = "jira.open"

[contributes.leader_groups]
"SPC j" = "jira"

[[contributes.settings]]
key = "jira.base_url"
kind = "text"
section = "Jira"
description = "Your Jira server, e.g. https://jira.example.com"

[[contributes.secrets]]
key = "jira.token"
env = "FENIX_JIRA_TOKEN"

[[contributes.agenda_sync]]       # a contribution point another extension defines
id = "jira"
```

Setting keys, command ids and default keys stay as they are today
(`jira.base_url`, `jira.open`, `SPC j j`), so existing settings files,
scripts and muscle memory keep working.

### 5.3 Contribution points

| Contribution | Replaces |
|---|---|
| Commands (id, title, category, `when` context, external flag) | `CommandRegistry::with_builtins` entries |
| Keybindings and leader group labels | `keymap.rs` inserts |
| `SPC m` major-mode menus for a language, file pattern or page | The fixed context enum |
| Settings (schema rows, settings page section) | `Config` fields and `schema.rs` rows |
| Secrets | `Secret` enum |
| Pages (an extension-owned `BufferKind::Page`) | `BufferKind::Docker` etc. |
| Surfaces (pixels: RGBA frames with dirty rects) | `VncPipeline`, `PdfPipeline` |
| Language features: hover, definition, completion, diagnostics, code actions, symbols | MIB's `K` / `gd` / completion in Tcl, Tcl ctags completion |
| Languages (grammar as WASM + queries, file types, comment tokens, LSP servers) | `fenix-syntax` `LanguageId` arms, `lsp.rs` defaults |
| File openers (an extension claims `*.pdf`) | PDF wired into file opening |
| Themes, snippets, project templates | `theme.rs` consts, built-in templates |
| Doctor checks / tool status entries | `project_doctor.rs`, `tool_status.rs` |
| Home dashboard tiles, modeline items | Hand-written in `home.rs` / modeline (agenda tiles, mission clock) |
| Services: named calls one extension offers others (`diagrams.render`, `agenda.add_task`) | Direct crate calls (notebook → diagram) |
| Contribution points an extension defines (`agenda_sync`, `forge`) | Jira code in `app/agenda_sync.rs` |
| Session state (serialize/restore a page) | Cases in `app/session.rs` |
| Control channel methods (`fenix/<ext>.<method>`) | `fenix/agenda.*`, `fenix/notebook.*` |

### 5.4 Pixels: PDF and VNC as extensions

Making PDF and VNC extensions means the host must let an extension draw.

- **The surface API.** Merge `VncPipeline` and `PdfPipeline` into one core
  `SurfacePipeline`. An extension owns surfaces and pushes RGBA frames
  or dirty rectangles. The host uploads them to a texture and draws them
  in a pane or over page cells (`PageImage` becomes a surface reference).
  Input (keys, pointer, wheel, clipboard) flows back. Frames go into
  shared memory between WASM and host, so a 1080p VNC update isn't
  copied twice.
- **VNC.** RFB decoding is plain Rust. `vnc-rs` uses tokio, so the
  extension needs a synchronous version reading from host-provided
  sockets (the `sockets` permission). The frame rate is the risk:
  prototype it in the Phase 3 spike.
- **PDF.** PDFium is a native C++ library and can't be loaded from WASM.
  The options, to settle in the spike:
  1. PDFium compiled to WASM (others have shipped this), bundled in the
     extension. It's big, but it's the same engine as today.
  2. A pure-Rust renderer (e.g. `hayro`), which builds for WASM with no
     extra toolchain. Check its fidelity against the PDFs people
     actually open.
  3. A companion process: the extension ships a native PDFium helper per
     OS and talks to it over `fenix-rpc`. It works, but loses
     single-artifact and sandboxing, so it's a last resort.

### 5.5 Runtime design

- **One worker thread per active extension**, each with its own
  `wasmtime::Store` (or Lua state). The UI thread never calls extension
  code. Results come back as one new
  `FenixUserEvent::Extension { id, event }` variant, the same
  `EventLoopProxy` pattern every feature uses today.
- **Pages are cached.** The last page stays on screen while an extension
  works, so a slow extension never freezes the editor.
- **Hot paths have budgets.** Completion, hover and diagnostics requests
  carry deadlines (as `LspDeadline` does today). Epoch interruption
  stops runaway WASM; an instruction-count hook does the same for Lua.
  An extension that keeps crashing or missing deadlines is disabled and
  reported. Fenix never crashes because of one.
- **Capabilities are enforced in the host.** WASI gets no ambient
  filesystem or network. Every `http`, `spawn`, `socket` and `fs` call
  goes through host functions that check the manifest's permissions and
  the user's grants. HTTP goes through the host's client (`ureq`), so
  proxies and TLS behave the same everywhere.
- **Lazy activation.** Nothing is instantiated until an activation event
  fires, so startup time doesn't grow with the number installed.

### 5.6 The host API (sketch)

The host API is written in WIT, `fenix:extension@0.x`. It is the single
definition: the Rust bindings for WASM and the Lua bindings (Phases
5–6) are both generated from it.

```wit
interface host {
  // editor
  notify: func(level: level, message: string);
  run-command: func(id: string, args: option<string>) -> result<_, string>;
  open-file: func(path: string, line: option<u32>);
  active-buffer: func() -> option<buffer-info>;      // path, language, cursor
  buffer-text: func(buf: buffer-id, range: option<range>) -> string;
  apply-edits: func(buf: buffer-id, edits: list<edit>) -> result<_, string>;
  show-picker: func(title: string, items: list<picker-item>) -> u64;
  prompt: func(label: string, initial: string) -> u64;
  // other extensions
  call-service: func(name: string, payload: string) -> result<string, string>;
  // settings and secrets (only the extension's own namespace)
  setting: func(key: string) -> option<value>;
  secret: func(key: string) -> option<string>;
  storage-dir: func() -> string;                     // the extension's own data
  // surfaces
  surface-create: func(width: u32, height: u32) -> surface-id;
  surface-update: func(s: surface-id, rect: rect, rgba: list<u8>);
  // capabilities, each checked against [permissions]
  http-request: func(req: request) -> result<response, string>;
  spawn: func(cmd: command) -> result<process-handle, string>;
  run-in-terminal: func(cmd: command) -> result<buffer-id, string>;
  socket-connect: func(addr: string) -> result<socket-handle, string>;
  read-file: func(path: string) -> result<list<u8>, string>;
  write-file: func(path: string, data: list<u8>) -> result<_, string>;
}

interface extension {
  activate: func();
  run-command: func(id: string, args: option<string>);
  page-render: func(page: page-id, width: u32) -> page;   // mirrors page::Page
  page-key: func(page: page-id, key: key) -> page-update;
  surface-input: func(s: surface-id, event: input-event);
  hover: func(doc: document, pos: position) -> option<hover>;
  complete: func(doc: document, pos: position) -> list<completion-item>;
  diagnostics: func(doc: document) -> list<diagnostic>;
  serve: func(name: string, payload: string) -> result<string, string>;
  session-save: func(page: page-id) -> option<string>;
  session-restore: func(state: string) -> option<page-id>;
}
```

A thin `fenix-extension-api` crate (bindings made with `wit-bindgen`,
plus helpers to build a `Page`) makes writing an extension feel like
writing a `fenix-gui` page today.

---

## 6. The phases

Each phase ships something on its own and has exit criteria. Sizes are
relative: S is about a week of focused work, M a few weeks, L a month or
more. Phases 0–4 are 2.0.

### Phase 0: Find the seams (L, internal only)

Refactor `fenix-gui` so features are units, without changing behaviour.
This is the bulk of the risk and the bulk of the value.

1. **A `Feature` trait inside `fenix-gui`.** Move each feature's fields
   off `App` into a host struct (`MibHost`, `NotebookHost`, `AgendaHost`,
   `PdfHost`, `VncHost`, ...); the `app/*_host.rs` files are halfway
   there. The trait has `id`, `commands`, `keybindings`, `settings`,
   `on_event`, `page_key`, `services`, `session_save/restore`,
   `doctor_checks`, `home_tiles`. `App` keeps a
   `Vec<Box<dyn Feature>>`. This is the in-process draft of the host
   API, so WIT is designed from real seams rather than guesses.
2. **Cross-feature calls become services.** Notebook→diagrams and
   agenda→Jira go through named services and the `agenda_sync`
   contribution point instead of direct calls.
3. **A dynamic `CommandRegistry`.** Make ids `Cow<'static, str>`, handlers
   boxed closures, and add `source` (core or an extension id),
   `category` and a `when` predicate.
4. **Keymaps as data.** Move the default leader map and `SPC m` menus into
   an embedded TOML table, loaded through the same code that will load
   user and extension keymaps. Detect conflicts.
5. **Settings registered at runtime.** `fenix-config`'s schema accepts
   sections registered at runtime, and each feature reads its own table
   (`[jira]`) through it. Keep the keys.
6. **A secrets store.** Replace `Secret` with namespaced keys in the OS
   keychain (Windows Credential Manager, Secret Service on Linux, via
   `keyring`), keeping the env-var override and a plaintext fallback for
   headless Linux. Migrate the existing tokens.
7. **One surface pipeline.** Merge `VncPipeline` and `PdfPipeline` into
   `SurfacePipeline`, with VNC and PDF as its first two users.
8. **A generic event path.** `FenixUserEvent::Feature { id, event }`;
   move feature variants onto it one at a time.

*Exit:* Every feature in the extension list of §3 lives behind `Feature`,
with no feature fields on `App`. CI is green and behaviour is unchanged.

### Phase 1: The core alone, a command palette, your own keys (M)

1. **Built-in extensions you can switch off.** Each `Feature` outside
   the core is already an extension in name, even while it's still
   compiled in: `extensions.disabled = [...]` in `settings.toml`, with
   the ids it will keep (`fenix.jira`, `fenix.pdf`). A disabled extension
   registers no commands, keys, settings sections, `SPC m` entries, home
   tiles, doctor checks or control methods. The Extensions page and the
   settings never change shape when a built-in later becomes a WASM
   extension.
2. **Cargo features.** `cargo build --no-default-features` builds the
   core only. CI builds both the core alone and everything, so the core
   can't depend on an extension by accident.
3. **The Extensions page, v0** (`SPC h x`, `extensions.open`): lists the
   built-in extensions with enable/disable, built on the existing `Page`
   infrastructure.
4. **First-run welcome.** A fresh install starts with the core and offers
   packs ("Personal: notebook, agenda, diagrams", "Mission", "Review:
   GitHub, GitLab"). An upgrade from 1.x keeps everything it used enabled (§7).
5. **Command palette** (`SPC :` and `M-x`, both free). Fuzzy over every
   registered command with its key and where it's from, recent first.
6. **User keybindings.** `keybindings.toml` over the defaults; `SPC h k`
   describe key, `SPC h c` describe command, `SPC h b` bindings here; a
   conflicts view; rebind from the palette.

*Exit:* Fenix runs with only the core and nothing of the others appears
anywhere. The palette lists every command, and any of them can be
rebound.

### Phase 2: Declarative extensions and the store (M)

1. **Themes as data.** Move `theme.rs` consts to a TOML theme format
   and load the built-in themes through it. Every built-in theme stays in
   the core. The format lets extensions and users add their own.
2. **Data-only extensions**: themes, snippets, project templates,
   language definitions, keymaps.
3. **Install/uninstall/enable/disable/update** under
   `<FENIX_HOME>/extensions/<id>/<version>/`, with state in
   `extensions.toml`. Writes go through `fenix-storage`'s staged writes.
4. **The registry** (§9): a signed `index.json` from a configurable URL,
   with packages as GitHub release assets.
5. **The store on the Extensions page**: Installed / Available /
   Updates, fuzzy search, a details page rendering `README.md` in the
   reading view, permissions, version, changelog, dependencies, install
   with progress.
6. **Offline and managed installs**: `fenix --install-extension
   ./x.fenixext`, a machine-wide extensions directory, and
   `extensions.registry` / `extensions.store = false` (§11).
7. **Safe mode**: `fenix --disable-extensions`.

*Exit:* A data-only extension (a snippet or template pack) installs from
`fenix-extensions` through the browser, and from a file with no network.

### Phase 3: The WASM runtime and API v0 (L)

1. **Spike (S–M).** Embed wasmtime and measure binary size, cold start,
   memory and Windows behaviour. Prototype the two hardest cases early:
   a VNC framebuffer at full rate through the surface API, and the PDF
   renderer options (§5.4). Choose Cranelift or the Pulley interpreter.
   Write it up as an ADR.
2. **Host**: worker threads, capability checks, deadlines, crash
   isolation, an output page per extension (`SPC h l`).
3. **WIT v0**: commands, pages, notifications, pickers, prompts,
   settings, secrets, storage, processes, terminals, HTTP, timers,
   services.
4. **`fenix-extension-api` crate** and an extension template.
5. **Pilot: Docker.** It's self-contained (a page plus the CLI), so it
   exercises pages, processes and session restore without the hard parts.
6. **Second: Arduino.** Same shape, plus terminal programs (the serial
   monitor).

*Exit:* Docker and Arduino install from the store, and their code is gone
from `fenix-gui`.

### Phase 4: Port everything else (L, can run in parallel)

Three tracks once API v0 exists:

- **4a, personal tools**: agenda (with the `agenda_sync` point and its
  control methods), Jira (plugs into it), diagrams (the `diagrams.render`
  service), notebook (uses it when present; capture control method).
- **4b, editor-integrated**: language-feature providers in WIT, merged
  with LSP results through `fenix-lsp`'s `fanout`; tree-sitter grammars
  as WASM; then Tcl, MIB, CCSDS (sockets for streams) and the Mission
  pack; GitHub and GitLab review through a `forge` provider.
- **4c, surfaces**: VNC and PDF on the surface API.

*Exit (= 2.0):* The default install is the core. A 1.x user who upgrades
keeps every feature they used. Every non-core crate is gone from
`fenix-gui`'s dependencies.

### Phase 5: Lua scripting (M, 2.1)

Lua for users: `init.lua` in the config directory, personal commands,
keymaps bound to functions, options, hooks (`on_save`, `on_open`,
`on_project_open`, `on_mode_change`, `on_key`), buffer and window access,
pickers and prompts. Bindings are generated from the WIT host API, so
everything an extension can do, a script can do. See §8.

### Phase 6: Lua extensions, Neovim-class (L, 2.x)

Lua becomes a second runtime for full extensions: `runtime = "lua"`,
packaged and permissioned like WASM, with complete API parity (pages,
surfaces, language providers, services, contribution points). An author
can build something as rich as a large Neovim plugin without a Rust
toolchain. Also here: API 1.0, extension packs from third parties, and
opening the registry to external repositories.

---

## 7. Bundling, and upgrading from 1.x without losing anything

**Bundling (decided).** Every 2.x release zip carries all first-party
extensions as `.fenixext` files in `extensions/bundled/` next to
`fenix.exe`. They are **installed but switched off**, so:

- the default install behaves as the core alone;
- turning one on needs no network, which matters on air-gapped mission
  machines;
- an upgrade can turn on what the user had without downloading
  anything.

The store still delivers updates newer than the bundled copy, and Fenix
uses the newer of the two. Bundling is reconsidered for 3.0, once the
store has a track record. (The WASM extensions are small. Only PDF,
with a PDFium build, adds noticeably to the zip.)

**Upgrading from 1.x.** Being a major release doesn't excuse a rough
upgrade:

- **On first launch after an upgrade**, switch on the extensions
  matching what this user used: `[jira]`, `[agenda]`, `mib.*`, `ccsds.*`,
  forge tokens in settings, a notebook directory, agenda files, known
  PDF/VNC places, buffers in the saved session. Show a one-time notice
  listing what moved and how to turn things off.
- **A fresh install** gets the core and the first-run welcome (Phase 1).
- **Session restore** shows a placeholder for a buffer whose extension is
  off ("Needs *PDF*: Enable / Dismiss") instead of dropping it.
- **Names don't change**: command ids, settings keys, default keys and
  control channel methods. `fenix/agenda.clockIn` keeps working while
  the agenda extension is enabled.
- **Data stays where it is**: notebook files, agenda files, PDF places
  and MIB caches aren't moved. Each extension reads them from where 1.x
  wrote them.

---

## 8. Lua

**Decided: Luau**, embedded with `mlua` (vendored, so nothing is needed
on `PATH`, and it builds with MSVC on Windows).

| Option | For | Against |
|---|---|---|
| Lua 5.4 | Current language, integers, good tooling | Slower than LuaJIT; sandboxing is up to us |
| LuaJIT | Fastest; what Neovim uses, so familiar to plugin authors | Lua 5.1 semantics; upstream development is slow; no instruction or memory limits, so untrusted code can't be contained |
| **Luau** | Built for running untrusted scripts: a sandboxed library, interrupts, memory limits; gradual types; fast, with native code generation on x64 | A dialect (5.1-based plus extensions), so slightly less familiar |

Why Luau: Lua is meant to grow into a full extension language (Phase 6),
and those extensions will come from people other than the user. Each one
must be contained the way a WASM extension is, with no exceptions, and
Luau is the only one of the three designed for that. Its types make
large extensions maintainable and give `luau-lsp` real completion from
stubs generated from the host API. It's fast enough for editor work.
Anyone who knows Lua already knows Luau. Neovim plugins wouldn't have
run unchanged anyway, because the editor API is different.

The design rules:

- **One API.** Lua bindings are generated from the WIT host API, so Lua
  and WASM extensions never drift apart, and the Lua API is documented
  once.
- **Sandboxed like WASM.** No `io`, `os.execute`, `package.loadlib` or C
  modules. `require` resolves only inside the extension's own directory.
  Files, processes and network go through the host's checked functions.
  Each extension gets its own Lua state on its own worker thread.
- **The user's own `init.lua` is trusted** but still uses the host API,
  so a script can't block the UI thread.
- **Developer loop:** `:lua` and `:luafile`, a Lua REPL page, reload on
  save for `init.lua` and for an unpacked extension under development,
  and Luau type definitions generated from WIT for `luau-lsp`, added to
  the core's default language servers for `.luau` files.

---

## 9. `tpedneault/fenix-extensions`

```
fenix-extensions/
├── extensions/
│   ├── docker/          extension.toml  Cargo.toml  src/  README.md  CHANGELOG.md
│   ├── arduino/  notebook/  diagrams/  agenda/  jira/  pdf/  vnc/
│   ├── github/   gitlab/   tcl/   mib/   ccsds/
│   ├── mission/         # a pack: depends on mib + ccsds + tcl, adds templates
├── xtask/               # validate manifests, build, pack, sign, update the index
├── Cargo.toml           # workspace; depends on fenix-extension-api
└── .github/workflows/
    ├── ci.yml           # changed extensions: build wasm32-wasip2, test, validate
    └── release.yml      # tag `<id>-v<version>` → pack, sign, release asset, PR to index
```

- **Releases per extension**: tag `jira-v1.2.0`; CI attaches the
  `.fenixext` and its checksum to a GitHub release.
- **Index**: `index.json` on a `registry` branch (or GitHub Pages) with
  each version's engine range, URL, SHA-256 and permissions. It's signed
  with an ed25519 key (minisign format) whose public key is built into
  Fenix. Fenix checks the signature and each package's hash.
- **Trust**: a curated monorepo means every line is reviewed before it
  ships. Phase 6 adds external entries.

**Timing:** create the repo in Phase 2 for the index and the data-only
extensions. Keep the *code* extensions in the main repo
(`extensions/` on `release/2.0`) while the WIT API is still changing,
so an API change and every extension it breaks land in one PR. Move
them over once the API settles, at the latest at API 1.0.

---

## 10. Features that make this usable

1. **Command palette** (Phase 1). Once features are optional, this is
   how people find them. It also offers "Install *Mission Tools*" when
   someone types "mib" and the extension isn't installed.
2. **Keybinding tools** (Phase 1): `keybindings.toml`, describe key and
   command, a conflicts view, rebinding from the palette. A reserved set
   of core leader keys (`f p s b w g c o t i q e l u h`) that extensions
   never get without the user agreeing.
3. **Lua scripting and extensions** (Phases 5–6, §8).
4. **Profiles**: named sets of enabled extensions and settings
   ("Mission", "Personal"), switchable from the palette.
5. **Project-recommended extensions**: `.fenix/extensions.toml` lists
   what a project wants. Opening the project offers to install them,
   or to enable them for that project only.
6. **Extension packs**: a manifest that has only dependencies.
7. **The author's loop**: `fenix ext new`, `fenix ext dev <dir>`
   (reload on rebuild), `fenix ext pack`, and a headless test harness.
   Pages are plain data, so `page-render` output can be asserted as
   text, as `fenix-gui`'s tests already do with `Page::all_text`.
8. **Smaller items**: a notification centre; an output page per
   extension; extension bisect; control methods `fenix/extensions.list`,
   `.install` and `.enable` for scripted provisioning; settings page
   sections generated from contributed schema.

---

## 11. Risks

**Refactoring `fenix-gui` is the real project.** Phase 0 touches a
41k-line `app.rs` whose interactive behaviour CI doesn't test. Do one
feature per PR, add headless tests per host before moving it, and add
no new `App` fields.

**A long-lived branch.** `release/2.0` will live for months while
`master` gets fixes. Merge `master` in weekly, freeze feature work on
`master` during Phase 0, and keep each PR to `release/2.0` small. The
longer a merge waits, the worse it gets.

**PDF and VNC are the hardest ports.** No PDF renderer runs in WASM
today with PDFium's fidelity. VNC needs frame rates that a naive copy
across the WASM boundary won't hit. Both are prototyped in the Phase 3
spike, before anything depends on them. If neither works, PDF falls back
to a companion-process extension (§5.4, option 3), still installed and
switched off like any other.

**Mission environments are often offline and controlled.** Offline
install, a registry mirror, `extensions.store = false`, a machine-wide
extensions directory, pinned versions, and no automatic updates unless
someone opts in. All of these are in Phase 2, not later.

**Tokens are in plaintext today.** Move them to the OS keychain in Phase
0, before any third-party code runs.

**Supply chain.** An update is code execution on every machine that has
it. Use the curated registry, a signed index, package hashes, permissions
enforced by the host, a prompt when an update asks for new permissions,
and maintainer-only release tags.

**Locking in the API.** Design WIT from the Phase 0 seams, keep it
small, version it `0.x` until every first-party extension is ported,
and support one previous minor. Lua parity (Phase 6) doubles the cost of
every API change, which is another reason to generate the Lua bindings.

**Lua sandboxing.** Neovim's Lua isn't sandboxed. Third-party Lua
extensions here must be, which is why Luau was chosen (§8).

**Performance in hot paths.** Pass ranges and incremental changes, not
whole buffers. Use deadlines, cached pages and lazy activation.

**Binary size.** Wasmtime adds megabytes. Moving the features out
removes PDFium bindings, tokio (from VNC), mermaid/resvg and more, which
should more than offset it. The spike measures both.

**Extensions depend on each other.** Notebook→diagrams, Jira→agenda,
Mission→MIB/CCSDS/Tcl, MIB inside Tcl and Python. Declare dependencies,
use services and contribution points rather than direct calls, and test
each extension with its optional dependencies both present and absent.

**Keybinding collisions.** Reserved core keys, conflict detection at
install time, and the user decides.

**Windows specifics.** Install to a fresh versioned directory and switch
over, never overwrite a running extension. WASM adds no new executables
for antivirus to scan. Plan to code-sign the Fenix binary itself.

**Maintenance load.** More release trains and API docs. Keep code
extensions in-tree until the API settles (§9), and let `xtask` automate
packing and indexing.

---

## 12. Still open

Nothing blocks starting Phase 0. These are settled inside a phase:

1. **Wasmtime: Cranelift or the Pulley interpreter**, and the PDF
   renderer (§5.4). Both are settled in the Phase 3 spike.
2. **The 3.0 bundling review** (§7).
