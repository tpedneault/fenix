# Extensions: a phased plan

Status: proposal. Nothing here is built yet.

Fenix ships a lot of specialist functionality that not everyone wants:
Tcl support, the mission tools (SCOS-2000 MIB, CCSDS/PUS packets and
streams), Jira, Docker, Arduino, VNC, PDF. This plan turns Fenix into a
core editor plus extensions you can install, enable and disable from an
in-editor browser, with first-party extensions published from
`tpedneault/fenix-extensions`.

It's written against the code as it stands (`bcdd773`), so it starts
with what is in the way.

---

## 1. Where Fenix is today

The crates are already well split. Almost every feature has a
host-agnostic crate (`fenix-mib`, `fenix-ccsds`, `fenix-jira`,
`fenix-docker`, `fenix-embedded`, ...) with no knowledge of windows or
the event loop. The coupling all lives in `fenix-gui`, and it's a lot:

| Seam | Today | Why it blocks extensions |
|---|---|---|
| `App` (`app.rs`, 41k lines) | 233 fields, about 57 of them owned by one optional feature (`mib_*`, `pdf_*`, `vnc_*`, `docker_*`, `agenda_*`, `tcl_*`, ...) | Feature state can't be added or removed at runtime; every feature edits the same struct |
| `CommandRegistry` (`commands.rs`) | 256 commands; `id: &'static str`, `fn` pointers taking `&mut App` | No commands that are registered at runtime, owned by something, or carry closures |
| Leader keymap (`keymap.rs`) | Built in Rust with `t.insert(&[spc, ...])`; groups hard-claimed (`SPC d` docker, `SPC j` jira, `SPC k` mission, `SPC v` vnc) | No user keybindings, no contributed bindings, no conflict handling |
| `SPC m` contexts (`keymap.rs`) | A fixed enum of buffer/project kinds | Extensions can't add a major-mode menu |
| `BufferKind` (`fenix-buffers`) | Closed enum: `Docker`, `Vnc`, `Pdf`, `Debug`, ... | A new kind of buffer needs a core change |
| `FenixUserEvent` / `PageEvent` | Closed enums with variants per feature (`JiraIssues`, `MibLoaded`, `StreamRows`, `VncFrame`, ...) | Background work can't report back without a core change |
| `Config` (`fenix-config`) | Typed fields per feature (`jira_base_url`, `mib_roots`, ...) and a static schema | No settings registered at runtime |
| `Secret` (`fenix-config/secrets.rs`) | Closed enum `GitLab \| Jira \| GitHub`; tokens kept in plaintext in `settings.toml` | No secrets namespaced per extension; a third-party extension that can read settings can read every token |
| Themes (`theme.rs`) | Rust `const`s, looked up with `by_name` | Can't be installed as data |
| Session restore (`app/session.rs`) | Rebuilds `Docker`/`Jira`/`Vnc`/`Pdf` buffers by kind | Has to degrade gracefully when an extension is missing |
| Settings page, project doctor, tool status, project wizard templates, home dashboard | Each lists features by hand | Each needs to become a contribution point |

Two existing pieces help a lot:

- **Pages are data.** `page::Page` is text, colour spans with a `Role`,
  rules, a focused row, a popup and images, and a page gets decoded `Key`s.
  It's already the right shape for UI an extension sends across a boundary.
  Most of the feature pages (Jira, MIB, stream, packet, Docker, Arduino)
  are built this way.
- **There's already an RPC protocol.** `fenix-rpc` and `fenix-control`
  give a JSON-RPC framing, a command list (`fenix/commands`) and remote
  command execution (`fenix/runCommand`). The command palette and a
  scripting host can both build on them.

---

## 2. Goals and non-goals

**Goals**

1. A user can run Fenix without Jira, Docker, the mission tools, Tcl,
   etc.: none of their commands, keys, settings or pages are present.
2. They can browse, install, update, enable, disable and uninstall
   extensions from inside Fenix, and offline from a file.
3. Third-party code runs sandboxed, with declared permissions the user
   can see.
4. One extension artifact works on Windows and Linux.
5. Existing users don't lose features on upgrade.

**Non-goals (for now)**

- Extensions that draw on the GPU directly. VNC and PDF stay native
  (see §4.4).
- Binary compatibility for native Rust plugins (`dylib`). Rust has no
  stable ABI, and this route has burned every Rust editor that tried it.
- An open marketplace with self-service publishing. The first registry
  is curated (§7).

---

## 3. The key decision: how extension code runs

| Option | Sandboxed | One artifact, every OS | Stable ABI | Fits Rust crates we have | Cost |
|---|---|---|---|---|---|
| Cargo features only | n/a | n/a | n/a | yes | Lean builds, but no store |
| Native `cdylib` plugins | no | no (DLL per OS) | no | yes | Unsafe, lockstep compiler versions |
| Out-of-process (JSON-RPC over stdio, like VS Code's extension host) | process-level | no (a binary per OS, or a bundled runtime) | yes (protocol) | yes | Process per extension; we'd ship or require a runtime |
| Embedded script (Lua/Rhai) | partly | yes | yes | no (rewrite 5–10k lines of Rust per feature) | Slow for parsers |
| **WASM components (wasmtime + WIT)** | **yes (capabilities)** | **yes** | **yes (WIT, versioned)** | **yes: compile to `wasm32-wasip2`** | Bigger binary; host API to design |

**Recommendation: a tiered model.**

1. **Declarative contributions (no code)**: themes, snippets, keymaps,
   language definitions (file types, comment tokens, LSP server
   commands), project templates, task problem matchers. These are TOML
   files in the extension. They're safe, easy to review, and ship first.
2. **WASM extensions** for code. Zed has proven this works for a Rust
   editor. Our feature crates are pure Rust with no GUI dependencies, so
   `fenix-mib`, `fenix-ccsds`, `fenix-jira` and `fenix-docker` compile to
   `wasm32-wasip2` mostly unchanged. The host provides HTTP, processes,
   files and sockets, so policy and proxy/TLS handling stay in one place.
3. **Language servers** stay the way to add heavy language smarts. Since
   #61 a buffer can have several servers, so an extension can also ship
   or point at an LSP server.
4. **A scripting language** for users, not extension authors: personal
   commands, hooks and macros in an `init` file (§8.3). It's bound to the
   same API as WASM extensions, so it isn't a second API to maintain.

The out-of-process host stays a fallback if the WASM spike (Phase 3)
fails on binary size or Windows.

---

## 4. The extension model

### 4.1 Package layout

An extension is a `.fenixext` file: a zip with a manifest at its root.

```
jira-1.2.0.fenixext
├── extension.toml
├── extension.wasm        # optional: present for code extensions
├── README.md             # shown on the details page
├── CHANGELOG.md
├── icon.png              # optional
├── themes/ snippets/ templates/ languages/ keymaps/   # optional data
```

### 4.2 Manifest

```toml
[extension]
id = "fenix.jira"                 # reverse-DNS-ish, unique in the registry
name = "Jira"
version = "1.2.0"
description = "Jira issues, search and agenda sync"
authors = ["Thomas Pedneault"]
license = "MIT OR Apache-2.0"
repository = "https://github.com/tpedneault/fenix-extensions"
categories = ["integrations"]

[engine]
fenix = ">=1.4, <2"               # which Fenix releases it runs in
api = "0.3"                       # the WIT API version it was built against

[dependencies]                     # other extensions, by id
# "fenix.mib" = "^1"

[activation]                      # lazy: nothing loads until one of these
commands = ["jira.*"]
languages = []
files = []
on_startup = false

[permissions]                     # shown before install; enforced by the host
network = ["${setting:jira.base_url}"]
secrets = ["jira.token"]
process = []                      # e.g. ["docker", "podman"] for Docker
filesystem = "none"               # none | project-read | project-write

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
```

The setting keys stay as they are today (`jira.base_url`, `mib.roots`),
so existing `settings.toml` files keep working when a feature moves out.

### 4.3 Contribution points

Each row replaces something hard-coded in the table in §1.

| Contribution | Replaces |
|---|---|
| Commands (id, title, category, `when` context, external flag) | `CommandRegistry::with_builtins` entries |
| Keybindings and leader group labels | `keymap.rs` inserts |
| `SPC m` major-mode menus for a language, file pattern or page | The fixed context enum |
| Settings (schema rows, settings page section) | `Config` fields and `schema.rs` rows |
| Secrets | `Secret` enum |
| Pages (an extension-owned `BufferKind::Page`) | `BufferKind::Docker` etc. |
| Language features: hover, definition, completion, diagnostics, code actions, symbols | MIB's `K` / `gd` / completion in Tcl, Tcl ctags completion |
| Languages (tree-sitter grammar as WASM + queries, file types, comment tokens, LSP servers) | `fenix-syntax` `LanguageId` arms, `lsp.rs` defaults |
| Themes, snippets, project templates (wizard) | `theme.rs` consts, built-in templates |
| Doctor checks / tool status entries | `project_doctor.rs`, `tool_status.rs` |
| Home dashboard tiles, modeline items | Hand-written in `home.rs` / modeline |
| Agenda sync providers | Jira-specific code in `app/agenda_sync.rs` |
| Session state (serialize/restore a page) | Cases in `app/session.rs` |
| Control channel methods (`fenix/<ext>.<method>`) | `fenix/agenda.*`, `fenix/notebook.*` |

### 4.4 What stays native

Some features render straight to the GPU or need native libraries:

- **VNC**: a live framebuffer through its own `VncPipeline`, with high
  frame rates.
- **PDF**: PDFium is a native library, rendered through `PdfPipeline`.
- **Terminal, LSP, DAP, Git**: core editor features.

VNC and PDF become **built-in modules**: compiled in behind Cargo
features, and enable/disable-able at runtime like an extension, listed
on the same page. A later "image surface" API (an RGBA buffer, which
`PageImage` almost is already) could move PDF out. VNC's frame rate makes
it a poor candidate.

### 4.5 Runtime design

- **One worker thread per active extension**, each with its own
  `wasmtime::Store`. The UI thread never calls into WASM. It sends
  messages and gets results back as a single new
  `FenixUserEvent::Extension { id, event }` variant, the same pattern
  every feature uses today with `EventLoopProxy`.
- **Pages are cached.** An extension returns a `Page`, which App draws
  like any other page. A keypress goes to the extension and the new page
  comes back. The last page stays on screen while it works, so a slow
  extension never freezes the editor.
- **Hot paths have budgets.** Completion, hover and diagnostics requests
  carry deadlines (as `LspDeadline` does today). Epoch interruption stops
  runaway code. An extension that keeps missing deadlines or crashing is
  disabled and reported with a notification, never a Fenix crash.
- **Capabilities are enforced in the host.** WASI gets no ambient
  filesystem or network. Every `http`, `spawn`, `socket` and `fs` call
  goes through host functions that check the manifest's permissions and
  the user's grants.
- **Lazy activation.** Nothing is instantiated until an activation event
  fires, so startup time doesn't grow with the number installed.

### 4.6 The WIT API (sketch)

A versioned package, `fenix:extension@0.x`, kept small and grown on
demand:

```wit
interface host {
  // editor
  notify: func(level: level, message: string);
  run-command: func(id: string) -> result<_, string>;
  open-file: func(path: string, line: option<u32>);
  active-buffer: func() -> option<buffer-info>;      // path, language, cursor
  buffer-text: func(range: option<range>) -> string;
  insert-at-cursor: func(text: string);               // MIB's "insert telecommand"
  show-picker: func(title: string, items: list<picker-item>) -> u64;
  // settings and secrets (only the extension's own namespace)
  setting: func(key: string) -> option<value>;
  secret: func(key: string) -> option<string>;
  // capabilities, each checked against [permissions]
  http-request: func(req: request) -> result<response, string>;
  spawn: func(cmd: command) -> result<process-handle, string>;
  socket-connect: func(addr: string) -> result<socket-handle, string>;
  read-file: func(path: string) -> result<list<u8>, string>;
}

interface extension {
  activate: func();
  run-command: func(id: string, args: option<string>);
  page-render: func(page: page-id, width: u32) -> page;   // mirrors page::Page
  page-key: func(page: page-id, key: key) -> page-update;
  hover: func(doc: document, pos: position) -> option<hover>;
  complete: func(doc: document, pos: position) -> list<completion-item>;
  diagnostics: func(doc: document) -> list<diagnostic>;
  session-save: func(page: page-id) -> option<string>;
  session-restore: func(state: string) -> option<page-id>;
}
```

The Rust side gets a thin `fenix-extension-api` crate (bindings made
with `wit-bindgen`, plus helpers to build a `Page`), so writing an
extension feels like writing a `fenix-gui` page today.

---

## 5. The phases

Each phase ships something on its own and has exit criteria. Sizes are
relative: S is about a week of focused work, M a few weeks, L a month or
more.

### Phase 0: Find the seams (L, internal only)

Refactor `fenix-gui` so features are units, without changing behaviour.
This is the bulk of the risk and the bulk of the value. Every later
phase depends on it.

1. **A `Feature` trait inside `fenix-gui`.** Move each feature's fields
   off `App` into a host struct (`MibHost`, `DockerHost`, `JiraHost`,
   `PdfHost`, ...); the `app/*_host.rs` files are halfway there. The
   trait has `id`, `commands`, `keybindings`, `settings`,
   `on_event`, `page_key`, `session_save/restore`, `doctor_checks`.
   `App` keeps a `Vec<Box<dyn Feature>>`. This is the in-process
   precursor of the extension API, so the WIT API is designed from real
   seams, not guesses.
2. **A dynamic `CommandRegistry`.** Make ids `Cow<'static, str>`, handlers
   boxed closures, and add `source` (core / module / extension id),
   `category` and a `when` predicate. Keep `with_builtins` as the core
   set.
3. **Keymaps as data.** Move the default leader map and `SPC m` menus into
   an embedded TOML table, loaded through the same code that will load
   user and extension keymaps. Detect conflicts. Tie which-key labels to
   that data.
4. **Settings registered at runtime.** Let `fenix-config`'s schema take
   sections registered at runtime, so each feature reads its own table
   (`[jira]`) through the schema instead of fields on `Config`. Keep the
   keys. `legacy.rs` shows migrations are already routine.
5. **A secrets store.** Replace `Secret` with namespaced keys. Store
   tokens in the OS keychain (Windows Credential Manager, Secret
   Service on Linux, via the `keyring` crate), keeping the env-var
   override and a plaintext fallback for headless Linux. Migrate the
   existing tokens.
6. **A generic event path.** Add `FenixUserEvent::Feature { id, event:
   Box<dyn Any + Send> }` and move feature variants onto it one at a
   time.
7. **Split the Config/App god objects as you go.** Each feature moved
   out shrinks `app.rs`. Track the line count as a health metric.

*Exit:* MIB, CCSDS, Jira, Docker, Arduino, PDF and VNC each live behind
`Feature`, with no new `App` fields. CI is green and behaviour is
unchanged.

### Phase 1: Modules you can switch off, a command palette, your own keys (M)

The first user-visible win, and it doesn't need a store yet.

1. **Built-in modules.** Each `Feature` is a module: `modules.disabled =
   ["jira", "mib"]` in `settings.toml`. A disabled module registers no
   commands, keys, settings sections, `SPC m` entries, home tiles or
   doctor checks. Needing a restart is fine at first.
2. **Cargo features** for lean builds: `cargo build --no-default-features
   --features git,lsp,terminal`. CI builds the minimal and full feature
   sets so neither rots. This alone solves "I don't want Jira in my
   binary" for anyone who builds from source.
3. **An Extensions page, v0** (`SPC h x`, command `extensions.open`). It
   lists the built-in modules with enable/disable, built on the existing
   `Page` infrastructure. The same page gets the store in Phase 2.
4. **Command palette** (`SPC :` and `M-x`, both free). A fuzzy picker over
   every registered command, showing the title, its keybinding and where
   it comes from, and remembering recent ones. This is cheap after Phase
   0 and becomes how people find things once features stop being
   hard-wired.
5. **User keybindings.** A `keybindings.toml` layered over the defaults,
   with `SPC h k` (describe key), `SPC h c` (describe command) and a
   keybindings page that shows conflicts and lets you rebind from the
   palette.

*Exit:* With Jira, Docker, the mission tools and Tcl disabled, nothing
about them appears anywhere. The palette lists every command, and a user
can rebind any of them.

### Phase 2: Declarative extensions and the store (M)

1. **Themes as data.** Move `theme.rs` consts to a TOML theme format and
   load built-in themes through it. Catppuccin becomes the first theme
   extension.
2. **Data-only extensions**: themes, snippets, project templates (the
   wizard's *Mission* template), language definitions (file types,
   comments, LSP server commands), keymaps.
3. **Install/uninstall/enable/disable/update.** Installed under
   `<FENIX_HOME>/extensions/<id>/<version>/`, with state in
   `extensions.toml`. Writes go through `fenix-storage`'s staged writes,
   so an interrupted install can't leave a half-written extension.
4. **The registry** (§7): a signed `index.json` read from a configurable
   URL, with packages as GitHub release assets.
5. **The store on the Extensions page.** Tabs for Installed, Available
   and Updates; fuzzy search; a details page that renders `README.md`
   with the reading view; permissions, version, changelog and
   dependencies; install with progress.
6. **Offline and managed installs.** `fenix --install-extension
   ./x.fenixext`, a machine-wide extensions directory IT can manage, and
   `extensions.registry` / `extensions.store = false` settings (§9).
7. **Safe mode.** `fenix --disable-extensions` to recover from a bad one.

*Exit:* A user installs a theme pack from `fenix-extensions` through the
browser, and someone with no network installs it from a file.

### Phase 3: The WASM runtime and API v0 (L)

1. **Spike first (S).** Embed wasmtime and measure binary size, cold
   start, memory and Windows behaviour (AV scanning, MSVC build). Decide
   between wasmtime with Cranelift, wasmtime's Pulley interpreter, or
   the out-of-process fallback. Write the result up as an ADR.
2. **Host.** Worker threads, capability checks, deadlines, crash
   isolation, logs per extension (an output page under `SPC h l`).
3. **WIT v0**: commands, pages, notifications, pickers, settings,
   secrets, processes, HTTP, timers.
4. **`fenix-extension-api` crate** plus a template:
   `cargo generate tpedneault/fenix-extension-template`.
5. **Pilot: Docker.** It's self-contained (a page plus the CLI, with no
   editor hooks), so it exercises pages, processes and session restore
   without the hard parts. Ship it as a first-party extension and remove
   the built-in.
6. **Second: Arduino (`fenix-embedded`).** Same shape, plus terminal
   programs (the serial monitor), which forces a "run in a terminal
   pane" host call.

*Exit:* Docker and Arduino are installed from the store, and their code
is gone from `fenix-gui`.

### Phase 4: Extensions that reach into the editor (L)

1. **Language-feature providers in WIT** (hover, definition, completion,
   diagnostics, code actions, symbols), merged with LSP results through
   `fenix-lsp`'s existing `fanout` merging.
2. **Tree-sitter grammars as WASM** (tree-sitter's `wasm` feature), so a
   language pack carries its grammar without native code.
3. **Tcl pack**: grammar, highlight queries, keywords, ctags-based
   completion. `ctags` scanning becomes a `process` permission.
4. **Mission Tools** (`fenix.mission`, depending on `fenix.mib` and
   `fenix.ccsds`): MIB pages, forms, `SPC m` menus, hover/gd/completion
   in Tcl and Python, diagnostics, CCSDS packet and stream pages (needs a
   `socket` permission), the hex view, generators, the mission clock
   modeline item, and the project template.
5. **Jira**, once the agenda has a **sync provider** contribution point,
   so the agenda stays in core and Jira plugs into it.
6. Then decide case by case: the GitHub/GitLab forges (a `forge`
   provider, behind `fenix-forge`'s neutral model), the notebook and
   diagrams.

*Exit:* A default install has none of the specialist tools. Mission
users get everything back with one click (or with a project's
recommendation, §8.5).

### Phase 5: Scripting and an ecosystem (M, open-ended)

User scripting (§8.3), an extension SDK and dev loop (§8.6), extension
packs, project-recommended extensions, and API 1.0. That's the point to
open the registry to third-party repositories.

---

## 6. Moving existing users

When Phase 3 and 4 remove a feature from the core:

- **Bundle first-party extensions** in the release zip for at least one
  minor release, installed and enabled, so an upgrade changes nothing.
- **Detect usage.** If `settings.toml` has `[jira]`, `mib.roots` or
  `ccsds.*`, or a project has a `[ccsds]` table, keep or offer the matching
  extension.
- **Session restore** shows a placeholder page for a buffer whose
  extension is missing or disabled ("Restoring needs the *Mission Tools*
  extension: Install / Enable / Dismiss") instead of dropping it.
- **Commands, settings and keys keep their names**, so muscle memory,
  `fenix/runCommand` scripts and settings files carry over.
- **CHANGELOG and a one-time notice** say what moved and where.

---

## 7. `tpedneault/fenix-extensions`

```
fenix-extensions/
├── extensions/
│   ├── docker/          extension.toml  Cargo.toml  src/  README.md  CHANGELOG.md
│   ├── arduino/
│   ├── jira/
│   ├── tcl/
│   ├── mib/
│   ├── ccsds/
│   ├── mission/         # meta: depends on mib + ccsds, adds templates
│   └── theme-catppuccin/
├── xtask/               # validate manifests, build, pack, sign, update the index
├── Cargo.toml           # workspace; depends on fenix-extension-api
└── .github/workflows/
    ├── ci.yml           # changed extensions: build wasm32-wasip2, test, validate
    └── release.yml      # tag `<id>-v<version>` → pack, sign, release asset, PR to index
```

- **Releases per extension**: tag `jira-v1.2.0`, built by CI, with the
  `.fenixext` and its checksum attached to a GitHub release.
- **Index**: `index.json` on a `registry` branch (or GitHub Pages),
  listing each extension's versions, engine ranges, download URL,
  SHA-256 and permissions. It's signed with an ed25519 key (minisign
  format) whose public key is built into Fenix. Fenix checks the index
  signature and each package's hash.
- **Trust**: a curated monorepo means every line is reviewed before it
  ships, which is the right posture while the API and sandbox are new. It
  will become a bottleneck. Phase 5 adds external entries (pointing at
  another repo's release, still signed through the index).

**Recommendation on timing:** create the repo in Phase 2 for the index
and data-only extensions. Keep *code* extensions in the main repo
(`fenix/extensions/`) through Phases 3–4, while the WIT API is still
changing, so an API change and every extension it breaks land in one PR.
Move them to `fenix-extensions` when the API reaches 1.0. Two repos from
day one mean every API change is a cross-repo dance with broken windows
in between.

---

## 8. Features that make this usable

Ranked by value for the effort.

### 8.1 Command palette (Phase 1)
`SPC :` / `M-x`: fuzzy over every command with its key and where it's
from, recent first, and arguments asked for inline. Once features are
optional, this is how people find them. `fenix/commands` on the control
channel already exposes the same list.

### 8.2 Keybinding tools (Phase 1)
`keybindings.toml`; `SPC h k` describe key, `SPC h c` describe command,
`SPC h b` list bindings in the current context; a conflicts view; rebind
from the palette; extensions can't take an existing binding without the
user agreeing; a reserved set of core leader keys (`f p s b w g c o t i
q e l u h`) that extensions never get by default.

### 8.3 Scripting (Phase 5)
**Lua** via `mlua` (Lua 5.4, vendored so nothing is needed on `PATH`):
the `commands.rs` comment already planned for it. It's what Neovim users
know, it's small, and it embeds well. Use cases: an `init.lua` with
personal commands, hooks (`on_save`, `on_open`, `on_project_open`,
`on_mode_change`), small text transforms, and binding keys to functions.
It's bound to the **same** host API as WIT (generated from one
description), so scripts and extensions can't drift apart. Keep
scripting for users. Extensions in the store are WASM, so they're
sandboxed and typed.

*Alternative considered:* Rhai (pure Rust, smaller, sandboxed by default).
It's less familiar, and its ecosystem is too small to matter. Steel
(Scheme) suits Emacs users but is niche.

### 8.4 Profiles
Named sets of enabled extensions and settings ("Mission", "Personal"),
switchable from the palette. A mission machine and a home machine can
share one Fenix install.

### 8.5 Project-recommended extensions
`.fenix/extensions.toml` in a project lists the extensions it wants.
Opening the project offers to install or enable them, and can enable them
for that project only. This fits the existing project settings layer and
the Mission project template.

### 8.6 The extension author's loop
`fenix ext new`, `fenix ext dev <dir>` (load an unpacked extension and
reload it on rebuild), `fenix ext pack`, and a headless test harness:
pages are plain data, so `page-render` output can be asserted as text,
the same way `fenix-gui`'s tests read `Page::all_text`.

### 8.7 Smaller items
- **Extension packs**: a manifest with only dependencies ("Space Ops").
- **Notification centre**: a history of notifications, since extensions
  will raise more of them.
- **Output page for each extension**: logs, crashes, missed deadlines.
- **Extension bisect**: disable half, ask, repeat, to find a misbehaving
  one.
- **Control channel methods**: `fenix/extensions.list`, `.install`,
  `.enable`, so IT can script provisioning.
- **Settings page sections** generated from contributed schema, with a
  link to the extension's details page.

---

## 9. Risks you may not have considered

**Refactoring `fenix-gui` is the real project.** The store is the easy
part. Phase 0 touches a 41k-line `app.rs` whose interactive behaviour
CI doesn't test (docs/CI.md says interactive GUI behaviour is checked by
hand). *Mitigation:* move one feature per PR, add headless tests per
host before moving it, and don't let a new feature add an `App` field in
the meantime.

**Mission environments are often offline and controlled.** The users
most likely to want Mission Tools are on air-gapped or restricted
networks with change control. *Mitigation:* install from a file, a
mirror URL for the registry, `extensions.store = false`, a machine-wide
extensions directory, pinned versions, and no automatic updates unless
someone opts in. Offline install has to be a first-class path, not an
afterthought.

**Tokens are in plaintext today.** `settings.toml` holds the GitLab,
Jira and GitHub tokens. That's fine for a single trusted binary and not
fine once third-party code runs. *Mitigation:* the OS keychain and
namespaced secrets in Phase 0, before any third-party code exists.

**Supply chain.** An extension update is code execution on every machine
that has it. *Mitigation:* the curated registry, a signed index, package
hashes, permissions enforced by the host rather than trusted, a prompt
when an update asks for new permissions, and maintainer-only release
tags.

**Locking in the API.** Whatever v0 exposes, extensions will depend on.
*Mitigation:* design WIT from the Phase 0 `Feature` seams; keep it
small; mark it `0.x` (unstable) until Docker, Arduino, Tcl, Mission and
Jira have all been ported; support one previous API minor version.

**Performance in hot paths.** Completion and hover run on nearly every
keystroke. A WASM call is cheap, but copying a buffer across isn't.
*Mitigation:* pass ranges and incremental changes, not whole buffers;
deadlines; render cached pages; lazy activation so startup doesn't grow.

**Binary size and build time.** Wasmtime with Cranelift adds megabytes
and compile time. *Mitigation:* the Phase 3 spike measures it; the Pulley
interpreter or out-of-process fallback are the alternatives. Removing
the features that move out shrinks the core, which may offset it.

**Extensions depend on each other.** MIB works inside Tcl and Python
files (`mib.editor_files`), CCSDS uses MIB definitions, the Mission
template writes `[ccsds]`, and the agenda syncs with Jira. *Mitigation:*
`[dependencies]` in the manifest, and contribution points (sync
providers, language-feature providers) instead of extensions calling
each other directly.

**Keybinding collisions.** Today's groups are claimed by hand (`SPC d`
docker, `SPC j` jira, `SPC k` mission). Two extensions will want the
same letter. *Mitigation:* reserved core keys, conflict detection at
install time, and the user decides.

**Windows specifics.** Windows is the primary platform. Downloaded files
get scanned by Defender; unsigned files trigger SmartScreen; a running
extension's files can be locked during update. *Mitigation:* install to a
new versioned directory and switch over, never overwrite in place; WASM
means no new executables to scan; plan for code-signing the Fenix
binary itself.

**Maintenance load.** More repos, more CI, more release trains, and docs
for an API. *Mitigation:* code extensions stay in-tree until API 1.0
(§7); `xtask` automates packing and indexing.

**Fewer users get the specialist features.** Discoverability drops when
things become opt-in. *Mitigation:* the store, project recommendations,
and the palette offering "Install *Mission Tools*" when someone types
"mib".

---

## 10. Decisions for you

1. **What to call them**: "extensions" for things from the store and
   "modules" for built-ins you can switch off (VNC, PDF), or one word
   for both? (This plan uses both.)
2. **Runtime**: confirm WASM components as the target, pending the
   Phase 3 spike.
3. **Code extensions in-tree until API 1.0** (recommended), or in
   `fenix-extensions` from the start?
4. **Scripting language**: Lua (recommended) or Rhai?
5. **What a default install contains**: which of the notebook, agenda,
   diagrams, forges, VNC and PDF stay in the core download.
6. **Bundling window**: how many releases ship the first-party extensions
   pre-installed after they move out.
