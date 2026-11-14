# Plan: project kinds from extensions

- **Status:** steps 1 and 2 built, 14 November 2026, on `kinds/impl`
  (not merged), reviewed, and the review's findings and three decisions
  (§2.4, §2.6, §5; §13.3) built; for the maintainer to review. Recorded in
  [ADR 0006](../adr/0006-project-kinds.md). §13 says what was built
  differently from this plan; §6.2 states the rules as built, which the
  store's `xtask validate` holds packs to.
- **Scope:** a language pack says what kind of project it knows -- an
  id, a name, a short tag, a colour, the files that mark one, a file to
  open first -- and Fenix names, colours and detects that kind as it
  does its own. A Java project is a `JAVA` project in the hub, on Home,
  in the modeline, in the doctor and on the settings page, and `kind =
  "java"` in `.fenix/settings.toml` is kept and shown even where the Java
  pack isn't installed. Fenix keeps its own kinds and what they do
  (Cargo, uv and the rest) with no extension installed.
- **Lands as:** two Fenix pull requests (§11), then the packs' manifests
  in fenix-extensions. No extension API change: the manifest section is
  data, Fenix 1.5 and every earlier release ignore it (§6.3), and no WIT
  record changes (§6.4).
- **Why:** the maintainer's question, "Shouldn't project kinds be defined
  in extensions and not in Fenix core? Does that mean that a Java project
  couldn't get recognized?" -- and then, "plan the extension-defined
  project kinds and implement it."

Paths are in this repository unless they say `fenix-extensions`. Every
claim about what Fenix does today cites `master` at `3e77131`. The
branch `mib/not-a-project` (`3229e6e`, not merged) removes the `Mib`
kind; this plan is written as if it has landed, and says where the two
meet (§2.4).

## 1. What a project kind is today

`ProjectKind` is a closed, `Copy` enum of ten cases (nine once the MIB
branch lands): Python, Arduino, Rust, Tcl, Cpp, Node, Go, Monorepo,
Other (`crates/fenix-project/src/kind.rs:14-29`). Each has an id for
files (`id`, `kind.rs:57-70`), a tag (`tag`, `kind.rs:86-99`: `PY`,
`INO`, `RS`, `TCL`, `C++`, `JS`, `GO`, `MONO`, `DIR`) and a name
(`label`, `kind.rs:101-114`). `from_id` lowercases and takes aliases
(`py`, `uv`, `ino`, `sketch`, `rs`, `c++`, `c`, `cmake`, `js`, `ts`,
`javascript`, `typescript`; the MIB branch adds `mib` meaning Other),
and returns `None` for anything else (`kind.rs:72-82`).

A root's kind is the one its project declares, else the first
detection that matches (`detect_kind`, `kind.rs:120-122`):

1. **Declared**: `[project] kind = "..."` in `.fenix/settings.toml`, or
   the old `.fenix/project.ini` (`declared_kind`, `kind.rs:194-200`). An
   id `from_id` doesn't know is `None`, so the declaration is **ignored**
   and detection runs: `kind = "java"` today shows the project as
   whatever its files say, usually `DIR`.
2. **Detected** (`detect_kind_from_files`, `kind.rs:126-150`), in this
   order: Arduino (a sketch, `root.rs:14-21`, or `library.properties`);
   Rust (`Cargo.toml`); Python (`pyproject.toml`, `setup.py`,
   `requirements.txt`, `uv.lock`); Go (`go.mod`); Node (`package.json`);
   C/C++ (`CMakeLists.txt`); Tcl (`pkgIndex.tcl`, or any `*.tcl` at the
   top, read from the folder's listing, `kind.rs:183-189`); else Other.
   Monorepo is never detected, only declared (`kind.rs:24-26`).

Nothing about a kind is stored by Fenix. `state/projects.json` holds the
known roots (`known.rs:4-15`, key `known`) and the pins and groups
(`meta.rs:1-27`); no kind. The kind is worked out again each run and
kept in memory only, in `App::project_kinds`
(`fenix-gui/src/app.rs:5158-5163`), filled on first use
(`project_kind_of`, `app.rs:7300-7302`) and cleared only when a project
is created or its declared kind is changed (`app/pages.rs:1475`,
`1832`). It is never cleared when extensions change.

Where the kind is used is listed in full in Appendix A. In short:

- **Presentation** -- the tag and its colour in the modeline
  (`app/projects.rs:28-34`, `app.rs:21562`), Home's project rows
  (`dashboard.rs:442-445`), the hub's rows and kind filter
  (`project_hub.rs:172-181`, `357-366`, `390`, `438`), the wizard's
  template list and review (`project_wizard.rs:783-803`, `910`, `946`,
  `1000`), the doctor's title (`project_doctor.rs:63-68`, `204-209`),
  the explorer's header (`explorer_page.rs:437`), the settings page's
  kind cycler (`project_settings.rs:265-272`, `540-548`). The colour is a
  theme role per kind (`app/projects.rs:11-24`).
- **Behaviour** -- the doctor's core checks by kind: uv and the venv,
  pyright and debugpy for Python; cargo, rust-analyzer and the
  standard library's source for Rust; cmake and clangd for C/C++; go and
  gopls; node and typescript-language-server
  (`fenix-project/src/doctor.rs:366-389`). The workspace a new project
  joins (`workspace.rs:30-37`, `64-66`, used by the wizard for Rust and
  Python only, `project_wizard.rs:445-451`, `1036`). The file a project
  opens on when you've had none of it open (`main_file`,
  `kind.rs:155-181`, `app/pages.rs:1281-1288`).
- **Templates** -- a template must name a kind `from_id` knows, or it is
  refused with "unknown kind" (`template.rs:332`). That is why 21 of the
  store's 33 pack templates say `kind = "other"`: Java's four, .NET's
  five, Dart's three, Elixir's two, Lua's two, Haskell's, OCaml's,
  PHP's, Swift's and Terraform's (fenix-extensions
  `extensions/*/templates/*/template.toml`).

### 1.1 Project roots, and where the packs' markers come in

`find_project_root` (`root.rs:39-59`) walks up from a file and stops at
the nearest folder holding any of Fenix's own markers (`MARKERS`,
`root.rs:8`: `.git`, `.projectile`, `Cargo.toml`, `package.json`,
`pyproject.toml`, `go.mod`, `CMakeLists.txt`, `library.properties`,
`pkgIndex.tcl`), a `.fenix/` settings file, or a sketch. Only if that
finds nothing all the way up does it try the packs' markers
(`EXTENSION_MARKERS`, `root.rs:26-31`), with `{folder}` replaced by
each folder's own name. The packs' markers are every
`[[contributes.contexts]]` `markers` entry of every switched-on code
extension, set at startup (`app/wasm_host.rs:780-786`) and again when
extensions change (`wasm_host.rs:810-811`, from
`extensions_host.rs:279-292`).

So a Java project **is found** today when it's a folder with `pom.xml`
and nothing of Fenix's above it, and when it's the top of a git
repository (found by `.git`). Its kind is Other, `DIR`. But a Gradle
project in a subfolder of a repository is **not** a project of its own:
the repository's `.git` is found first, by design
(`root.rs:104-125`, "In a repository, the repository is still the
project"), so it's not one of the hub's subprojects either
(`workspace.rs:103-128` asks `find_project_root`). A Cargo crate in the
same place is its own project. This plan doesn't change that (§9, Q6).

The doctor already names a `DIR` project by the extension whose context
markers it holds -- "Java and Kotlin" -- through
`WasmHost::project_language` (`wasm_host.rs:591-595`, used at
`app/pages.rs:1697` and `project_doctor.rs:63-68`). That's the one place
a pack's identity shows today.

### 1.2 Corrections to the brief

- **"About 90 uses of specific variants across `fenix-gui`."** There are
  39 (Python 13, Rust 9, Arduino 5, Other 4, Tcl 2, Mib 2, Go 1, Node 1,
  Cpp 1, Monorepo 1), and 24 of them are in tests. The 15 in shipped
  code are the colour table (`app/projects.rs:13-22`, 10), the doctor
  page's "Other means ask the pack" (`project_doctor.rs:65`, `207`), and
  the wizard's Cargo/uv workspace line (`project_wizard.rs:447`,
  `1036`). What `fenix-gui` does with a kind is almost all presentation.
  The behaviour is in `fenix-project`: `doctor.rs:366-389` (10 uses),
  `workspace.rs:30-53` (8), and `kind.rs` itself.
- **"Core behaviour that switches on Python/Rust/Arduino."** Python and
  Rust do have core behaviour (the doctor, the workspace), and so do
  C/C++, Go and Node (the doctor). Arduino has none beyond detection and
  its main file: the doctor's `ProjectKind::Arduino` arm is empty
  (`doctor.rs:389`), and the `FeatureCheck` hook its comment points to
  (`doctor.rs:353-357`) has no caller that passes one -- all three pass
  `&[]` (`app/home.rs:157`, `app/pages.rs:1587`, `1715`). The Arduino
  checks are the Arduino pack's.
- **"Cached kinds in `projects.json`."** There are none (§1). The only
  cache is in memory, and it's the thing that goes stale when an
  extension is installed (§7.2).
- **"A Java project IS found as a project today."** Only where no
  marker of Fenix's is above it, or where it is the repository's top
  (§1.1).
- **Unknown manifest sections.** Older Fenix ignores them: `Contributes`
  has no `deny_unknown_fields` in any release from `v1.0.0` to `v1.5.2`
  (`git show v1.5.2:crates/fenix-extensions/src/manifest.rs`, line
  514-515), and the module says so on purpose (`manifest.rs:1-5`). So a
  pack can declare a kind and keep `api = "1.5"` (§6.3).

## 2. The model

### 2.1 What a kind is

A kind is **data**:

| Field | What it is | Who sets it |
|---|---|---|
| `id` | What `kind = "..."` and templates say: `java` | The declaration |
| `name` | What it's called on the settings page, the doctor, the explorer's header: `Java` | The declaration |
| `tag` | The mark in front of a project everywhere: `JAVA`, 1 to 4 characters | The declaration |
| `color` | A theme role, never a colour value (§2.5) | The declaration |
| `markers` | Files at a project's top that say it's of this kind; none for a kind that's only ever declared (Monorepo, Kotlin) | The declaration |
| `main` | The files a project of it is entered through, first that exists | The declaration, optional |
| where it's from | Built in, or the extension that declared it | Fenix |

Fenix's nine are declarations too, made in code (§3). An extension's
are made in its manifest (§6).

### 2.2 In code: an interned id

`ProjectKind` stays `Copy`, so `Role::Kind(ProjectKind)` (`page.rs:56`,
`dashboard.rs:145`), the hub's `kind_filter` and the `HashMap` cache
keep working. It becomes a handle to an **interned id**:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectKind(u16);          // an index into the ids interned so far

impl ProjectKind {
    pub const PYTHON: Self; pub const ARDUINO: Self; pub const RUST: Self;
    pub const TCL: Self; pub const CPP: Self; pub const NODE: Self;
    pub const GO: Self; pub const MONOREPO: Self; pub const OTHER: Self;

    pub fn id(self) -> &'static str;
    /// A built-in id or alias; else an id an extension has declared
    /// (as built: nothing a repository says is interned, §2.6).
    pub fn from_id(id: &str) -> Option<Self>;
    /// Who declares it now: `None` for an id no one does (§2.6).
    pub fn def(self) -> Option<KindDef>;
    pub fn tag(self) -> String;     // the definition's, or §2.6's
    pub fn label(self) -> String;
    pub fn color(self) -> KindColor;
    pub fn is_builtin(self) -> bool;
    /// Every kind declared now: Fenix's in their order, then the
    /// extensions' by name. What the settings page and the hub cycle.
    pub fn known() -> Vec<Self>;
}
```

The built-ins are interned first, at fixed indices, so they're
constants and `match` still works on them (`ProjectKind::RUST =>` is a
pattern, the type deriving `PartialEq` and `Eq`). Interning is
append-only: an id keeps its index for the run, so a handle held by a
page or the cache never points at another kind after extensions change;
what changes is what `def()` returns. The ids interned are bounded by
§6.2's grammar (24 characters) and by what's been read: manifests and
the projects looked at.

The registry beside it is a `RwLock`, set as `EXTENSION_MARKERS` is
(`root.rs:26-31`): `fenix_project::set_extension_kinds(Vec<KindDef>)`.
`fenix-project` doesn't learn about manifests: `fenix-gui` turns
`fenix_extensions::manifest::ProjectKindContribution` into `KindDef`.

`ProjectKind::ALL` goes (`kind.rs:43`); its three users take `known()`
(`project_settings.rs:267`, `project_hub.rs:179`) or the built-in
constants (`kind.rs` tests).

### 2.3 Is a kind a context with a label?

No. They're separate entries in the manifest, and a pack usually has
one of each. A context (`manifest.rs:757-785`) says where a pack's
`SPC m` menu applies, and its markers are also when the pack loads
(`Activation::waits_for`, `manifest.rs:136-150`), which projects its
tasks and checks are for (`WasmHost::is_for`, `wasm_host.rs:564-584`)
and a project's top when nothing else is found (§1.1). A false match
there is cheap: a menu that shows where it isn't needed. A kind says what
a project **is**, everywhere it's named, and one match wins over every
other kind; a false match is a mislabelled project. So their markers
differ in practice: Ruby's context has `Rakefile`, which plenty of
non-Ruby projects have, and its kind shouldn't (§10.2). A context also
has languages and pages, which mean nothing for a kind; a kind has a
tag, a name, a colour and a main file, which mean nothing for a menu.
Folding one into the other would make every context's markers a claim
about identity, and change when packs load whenever a kind is tuned.

What they share is the marker syntax (§6.2) and one rule: a kind's
markers are **also** packs' markers for finding a root, so that a
kind's marker always finds its own project where nothing of Fenix's is
above it. They join `EXTENSION_MARKERS` with the contexts', under the
same fallback rule (§1.1).

### 2.4 Which kind wins

**Revised in review (Decision A, replacing Q4's (a)):** installing a pack
must never change the kind of a project Fenix recognises today. A root
has one kind:

1. **Declared** (`[project] kind`), when someone declares that id now --
   Fenix or a switched-on extension (§2.6).
2. **Fenix's own detection, exactly as on master**: Arduino (a sketch,
   `root.rs:14-21`, or `library.properties`), Rust, Python, Go, Node,
   C/C++, Tcl (`pkgIndex.tcl`, any `*.tcl` at the top) -- the same
   lookups in the same order, stopping at the first.
3. Only for a folder none of those name: **the markers packs add to
   Fenix's kinds**, in Fenix's order (Arduino's, Rust's, ... Tcl's).
4. **The extensions' kinds**, in the extensions' order -- the machine's
   (managed) first, then yours, each by id -- and within one extension
   in the order it lists them.
5. **Other**.

So an extension's kind only ever names a project that would otherwise be
Other. As first planned, packs' kinds sat between Go and Node, so that a
Rails app (`Gemfile` and `package.json`) would be Ruby; the review
showed the cost: a React Native app (`package.json` and a CocoaPods
`Gemfile`), any JavaScript app with a fastlane `Gemfile`, `package.json`
beside `main.tf` or `.luarc.json`, all stopped being JavaScript once a
pack was installed. A Rails or Laravel app now stays `JS` as it is
today, and declares `kind = "ruby"` to be the pack's. It also means a
project of Fenix's costs exactly the lookups it did (§5). The order is
in one function, `detect_kind_from_files`, and a test pins each tier
against the next.

Within the extensions' tier, the order is fixed but arbitrary (by id),
so two packs claiming one marker give the same answer on every machine.
In the store that never happens: `xtask validate` warns (§10.3). A
project that disagrees declares its kind.

**The same id from two extensions**: the first in that order is the
kind; the other's declaration of it is ignored, and the Extensions
page's details for it say so ("its kind `java` is fenix.java's"). Two
packs' kinds may share a tag; Fenix doesn't refuse it, the store does
(Q2). No pack's kind may take one of **Fenix's** tags or names (§6.2).

**The same id as a built-in** (a pack declaring `node`): the built-in
stays -- its tag, name, colour and main files -- and the pack's
**markers are added** to it, counted in step 3 above: only where none of
Fenix's own markers names the folder. That lets the Web pack say
`deno.json` and `tsconfig.json` are JavaScript projects and the C/C++
pack that `compile_commands.json`, `vcpkg.json` and `conanfile.txt` are
C/C++ ones, without either of them changing what a Rust or Node project
looks like when the pack is installed or not. No pack may name one of
Fenix's own markers, for its own kind or another built-in's (M1, §6.2):
`{id = "arduino", markers = ["Cargo.toml"]}` would otherwise have made
every Rust project Arduino. A built-in's presentation fields in a pack's
declaration are refused by `xtask` and ignored by Fenix (§6.2).

**The MIB branch.** It maps `kind = "mib"` to Other (`from_id`, the
branch's `kind.rs:66-68`) and says so in the changelog ("shown as a
plain project (`DIR`)"). That stays a built-in alias, so no extension can
declare `mib`; if the owner later wants MIB folders to have a kind
again, it is the MIB pack's to declare, and the alias goes. Whichever of
the two branches lands second rebases on the other; neither changes
what the other does.

### 2.5 Colour

A kind names a theme role, never a colour, so a tag is readable on every
theme, light ones included, as Fenix's own are (`app/projects.rs:9-10`).
An extension may name one of eight: `keyword`, `string`, `function`,
`type`, `number`, `constant`, `attribute`, `variable` -- the syntax
roles every theme sets and that `Theme::palette` already treats as the
theme's distinct colours (`theme.rs:276-289`). Fenix resolves them with
`Theme::syntax_color` (`theme.rs:211`), which `Role::Syntax` uses already
(`app.rs:19889`). Left out, it's `constant`, which no built-in uses once
the MIB branch lands, so an undecorated pack's kind still reads as
coloured and as not one of Fenix's. Fenix's own keep theirs, including
two no extension can name: Arduino's insert-mode colour and Monorepo's
plain foreground; Other stays the gutter's muted colour.

`KindColor` is `Syntax(&'static str) | Insert | Fg | Muted`, and
`kind_color` (`app/projects.rs:11-24`) becomes one `match` on it.

### 2.6 A declared kind no one declares

**Revised in review (Decision B):** `kind = "java"` in a project where no
installed extension declares `java` -- a teammate without the pack, safe
mode, the pack switched off -- or a misspelt `kind = "pyhton"`:

- **Is ignored for what the project is**, exactly as on master: the
  project's kind, tag, colour, main file and doctor checks are what its
  files make it. First planned was to show it by its id, muted; the
  review showed that a misspelling then turned a Python project into
  `PYHT`, with no main file and none of Python's checks, with no
  extension involved.
- **Is kept**: never rewritten or lost. `declared_id` reads it as
  written (any well-formed id); `declared_kind` answers only for an id
  someone declares now. Nothing writes `project.kind` but the settings
  page's cycler, and while the declared id is one no one knows, the
  cycler's `h` and `l` only move among the choices (`detect`, the known
  kinds, then the kept id, where it starts); Enter on a kind writes that
  kind; Enter on `detect` or on the kept id writes nothing. Reopening
  the page finds it again. Removing it is its own act: `r` on the kind's
  row -- the settings page's reset, as for any project setting --
  removes `kind` from the project's settings, and detection decides; the
  row says so (`pyhton -- no extension declares this kind; r removes
  it`).
- **Isn't interned**: no id a repository's settings, a template or
  anything else a repository controls says is ever interned (§2.2); an
  unknown declared id is carried as text where it's needed (the
  settings page's model). Only Fenix's ids and those switched-on
  extensions declare are, bounded by §6.2's totals.
- **Isn't lost** by the `project.ini` move, which copies the value as
  written (`meta.rs:217-219`).
- **Counts** once a pack declaring it is installed (§7.2).

A template naming such a kind loads (shown as Other), and the project
made from it declares that id in its `.fenix/settings.toml` -- unless
its files already make it that kind, or it declares one itself.

## 3. Fenix's own kinds

Three ways to go, weighed on: Fenix works with no extensions installed;
the size and risk of the change; and the maintainer's direction that
kinds belong in extensions.

| | (a) Keep the enum, add `Extension(id)` | (b) Kinds are data; Fenix registers its nine as built-ins | (c) Even the nine move into packs |
|---|---|---|---|
| No extensions | As today | As today | Rust and Python projects are `DIR`; Cargo, uv and venv checks key on an id no one declares |
| What changes | Every `match` grows an arm; every tag, label and colour asks a registry for the new arm only: two ways to say a kind, one for core and one for everyone else | One type, one registry; built-ins are data that happens to be in code; behaviour keeps its `match` on the constants | Everything (b) does, plus root finding: `MARKERS` (`root.rs:8`) is how a Cargo crate inside a repository is its own project, so moving Rust's markers to a pack changes project boundaries |
| `Copy` | Lost, or `Extension(u16)` -- which is (b) with a seam | Kept (§2.2) | Kept |
| Risk | Low per line, but the asymmetry stays forever | Mechanical rename, ~140 references (Appendix A) | High: depends on packs being installed, and on the store shipping the same day |
| Direction | Kinds stay core's, extensions bolt on | Kinds are declarations; Fenix happens to make nine | Fully there |

**Chosen: (b).** Fenix registers Python, Arduino, Rust, Tcl, C/C++,
JavaScript, Go, Monorepo and Other as built-in declarations, in code,
with the tags, names, colours, markers and main files they have today.
Detection and presentation go through the registry for every kind.
Behaviour that is Fenix's -- the doctor's checks for Python, Rust,
C/C++, Go and Node, the Cargo and uv workspace a new project joins,
Arduino's sketch rule -- stays in core, keyed by the built-in constants
(`ProjectKind::PYTHON` where it says `ProjectKind::Python`), and is
otherwise untouched.

**The end state of this plan**: any pack can declare a kind and have it
named, coloured, filtered, declared, detected and opened as Fenix's
own; Fenix's nine are unchanged with or without packs; the packs for
Fenix's own kinds add markers to them (§2.4).

**Left for later, deliberately**: (c). It becomes small once (b) is in:
the store's packs for Rust, Python, Go, Web, C/C++, Arduino and Tcl
already declare the built-in ids (§10.2), so moving a built-in's
presentation and markers out of core is deleting its registration and
moving its fields into the pack's declaration. What it still needs is a
decision on root finding (`MARKERS`) and on what Fenix with no packs
shows for a Rust project (Q1). Also left: the doctor's core checks for
Go, Node and C/C++, which the packs' doctor checks duplicate
(`wasm_host.rs`'s `doctor_checks`, merged at `app/home.rs:157`) -- a
separate cleanup, not a kinds question.

## 4. Templates and the wizard

- **A template naming an extension's kind**: `kind = "java"`. It's
  shown with that kind's tag and colour in the template list and the
  review (`project_wizard.rs:783-803`, `910`, `946`, `1000`).
- **A template naming a kind no one declares**: loads, and is shown as
  §2.6 shows an unknown declared kind. Today it's refused
  (`template.rs:332`), which hides a template that would work, and pack
  templates are loaded from every switched-on extension's folder
  whether or not the extension that declares its kind is on
  (`available_templates_with`, `template.rs:1233-1260`). Only a value
  that isn't a well-formed id is refused, with today's message. A typo
  in the store is `xtask`'s to catch (§10.3); a typo in your own
  template shows as a muted tag.
- **What a template's kind decides** is only how the wizard shows it,
  and for Rust and Python which workspace the review says the project
  joins (`project_wizard.rs:445-451`, `1036`). The project made is
  detected like any other. A template whose projects can't be detected
  as its kind -- a Kotlin app is a Gradle project, which is `java` by its
  files -- writes the declaration among its files, as the built-in
  Monorepo template already does (`crates/fenix-project/templates/monorepo/files/.fenix/project.ini`).
- **Grouping** stays by the template's `group`, not by kind: the Java
  pack's four templates are under "Java and Kotlin" whatever they're
  tagged. One fix: groups Fenix doesn't list come after "Start from"
  today (`project_wizard.rs:165`, `198-203`: unknown groups rank after
  the last), so Go's, .NET's and Java's groups sit below "Start from".
  "Start from" goes last whatever else is there (step 1).
- **Built-in templates** are unchanged; they name built-in ids.
- **Old Fenix and new templates.** Fenix 1.5 refuses a template naming
  `java` (one message, and that template is hidden; the rest load,
  `template.rs:1236-1244`). So a pack's template can name its own kind
  only in a release for Fenix 1.6 and later (Q5).

## 5. Detection: cost and trust

- **No extension code runs** to detect a kind: kinds come from
  manifests, read when extensions are read. A pack that isn't loaded yet
  -- language packs wait for their files (`waits`, `wasm_host.rs:492-496`)
  -- has its kinds anyway.
- **Nothing is read but names.** Detection is `is_file` on each marker
  at the root, as contexts' `holds` is (`manifest.rs:776-780`). No
  marker is opened, so no kind can depend on what's in `pubspec.yaml`
  (that's why Flutter is a declared kind, §10.2). Fenix's own rules keep
  what they do today: a folder listing for Tcl and sketches; core's
  `.fenix/settings.toml` read for the declaration.
- **Bounded markers** (§6.2): at most 16 per kind and 8 kinds per
  extension, 256 kinds and 256 distinct markers from all extensions; a
  marker is a file name -- no `/`, `\`, `:`, `..`, wildcards, no
  device's name, not ending in `.` or a space -- of at most 64 bytes,
  with `{folder}` at most once. Every marker and `main` part is joined to
  the root only as a plain name (`Component::Normal`), so nothing
  absolute, on another drive (`C:x` on Windows), prefixed or `..` can
  replace or leave the root, whatever a validator misses. A pack's
  `main` file is opened only if it is inside the project once links are
  followed.
- **Fenix's own projects cost what they did** (Decision A): its
  detection runs first, as on master, and the packs' markers are looked
  for only at a folder it calls Other.
- **Never the home folder**: no extension's marker -- a kind's or a
  context's -- makes the home folder, a folder above it or a file
  system's root a project's top (Decision C); a dotfile there
  (`~/.ruby-version`) would make every loose file under it one project.
  Fenix's own markers are as they were.
- **One look per marker.** The unique markers of every kind (about 70
  with every store pack installed, §10.2, and Fenix's own) are checked once per root, and
  the kinds matched from that set in §2.4's order, so the cost doesn't
  grow with how many kinds share a marker. A root's kind is still worked
  out once and cached (§1). The places that ask without the cache --
  the explorer's header on each folder change (`app/explorer_host.rs:441`)
  and the hub's subprojects, off the UI thread (`workspace.rs:119`) --
  pay that once per root.
- **Remote roots** are skipped by name, not by a failed `stat`: a root
  that `fenix_vfs::path::is_remote_text` calls remote is Other unless
  declared -- which, read through the local file system today, it never
  is. That's today's behaviour (a remote root is `DIR`), made explicit so
  no kind check ever goes to the network on the UI thread (§8).

## 6. The manifest: `[[contributes.project_kinds]]`

### 6.1 What it says

```toml
[[contributes.project_kinds]]   # project kinds it knows (Fenix 1.6)
id = "java"                     # what kind = "java" and templates say
name = "Java"                   # the settings page, the doctor, the explorer
tag = "JAVA"                    # in front of a project's name: 1 to 4 of A-Z 0-9 + # .
color = "keyword"               # keyword, string, function, type, number,
                                # constant (the default), attribute, variable
markers = ["pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]
                                # files at its top; {folder} is the folder's
                                # own name; none: only ever declared
main = ["src/Main.java"]        # what it opens on when you've had none of
                                # it open; the first that exists

[[contributes.project_kinds]]   # a built-in id: its markers are added to
id = "node"                     # Fenix's own kind, nothing else
markers = ["deno.json", "tsconfig.json", "jsconfig.json"]
```

### 6.2 Fields, limits and checks

Checked by `Manifest::check` (`manifest.rs:1044`), each refusal saying
which kind and why, as the test providers' are
(`check_test_providers`, `manifest.rs:1179-1215`); `xtask validate`
mirrors every one (§10.3).

| Field | Rule |
|---|---|
| (the list) | At most 8 per extension. Ids once each. Any runtime but `bundle` (which has no contributions, `manifest.rs:1165-1171`): a `runtime = "none"` pack of templates may declare the kind its templates name |
| `id` | Required. 1 to 24 characters, `a-z` first, then `a-z 0-9 - _`. Not `other`, `monorepo`, or an alias of a built-in (`py`, `uv`, `ino`, `sketch`, `rs`, `c++`, `c`, `cmake`, `js`, `ts`, `javascript`, `typescript`, `mib`). A built-in id (`python`, `arduino`, `rust`, `tcl`, `cpp`, `node`, `go`) is allowed: §2.4 |
| `name` | Required unless the id is built-in. 1 to 32 characters, none invisible, at least one visible; not one of Fenix's kinds' names (below) |
| `tag` | Required unless the id is built-in. 1 to 4 characters of `A-Z 0-9 + # .`, at least one letter; not one of Fenix's tags. Upper case, ASCII and one cell each, so every place that puts a tag in a fixed field lines up (§7.3) |
| `color` | Optional, one of the eight in §2.5 |
| `markers` | Optional, at most 16. Each 1 to 64 bytes; no `/`, `\`, `:`, `..`, `*`, `?`, `[`, `]` or control character; `{folder}` at most once and no other `{` or `}`; not a Windows device's name, not ending in `.` or a space. Not a `.fenix` file. Not one of Fenix's own markers (below) |
| `main` | Optional, at most 8. Each 1 to 128 bytes; a path below the project, `/` between parts; no `\`; no part empty, `..`, with a `:`, a device's name, or ending in `.` or a space; not starting with `/`; `{folder}` allowed |

For a built-in id, `name`, `tag`, `color` and `main` are ignored by
Fenix if given (and refused by `xtask`), and its markers join the
built-in's tier. **Not checked against `engine.api`**: the section is
read by Fenix, not by the extension's code, so it isn't part of the
extension API (§6.3).

**The rules where the table above left room** (settled with the store's
`xtask/src/kinds.rs` as step 2 was built; each is the strict reading,
and Fenix and `xtask` hold to the same one):

1. **One pack per id** is a store rule, and Fenix's ids are exempt:
   several packs may add markers to `node`. For one of Fenix's ids a
   declaration carries `id` and `markers` only. `xtask` refuses
   `name`, `tag`, `color` or `main` there; Fenix **ignores them and
   says so** on the Extensions page ("name, tag ignored -- it's one of
   Fenix's own ..."), keeping the markers, since core's presentation
   stays whatever the pack says. Fenix doesn't check an ignored `main`.
2. **`main`**: no part empty -- `a//b` and a trailing `/` are refused --
   as well as no `..` part, no `\`, not starting with `/`.
3. **`{folder}`**: in `main`, any number of times, other braces not
   looked at (it's a path Fenix only ever joins under the project). In
   a marker, at most once, and no other `{` or `}`.
4. **Not a `.fenix` file**: a marker equal to `.fenix` ignoring ASCII
   case (`.FENIX` is the same folder on Windows and macOS). Names that
   start with it are markers like any other: `.fenixignore`.
5. **`name`**: 1 to 32 characters (not bytes), no control character,
   and at least one that isn't whitespace. **Markers**: no control
   character either; a marker given twice is looked for once (kept
   once, not refused).
6. **`tag`**: 1 to 4 characters of `A-Z 0-9 + # .`, at least one letter
   (so ASCII, one byte and one cell each).
7. **Fenix's ids, for templates and for what no pack may declare**: the
   nine (`python`, `arduino`, `rust`, `tcl`, `cpp`, `node`, `go`,
   `monorepo`, `other`) and their aliases (`py`, `uv`, `ino`, `sketch`,
   `rs`, `c++`, `c`, `cmake`, `js`, `ts`, `javascript`, `typescript`),
   in any case; `mib` remains an alias for `other`. Any other id is
   taken only as written: `Java` is not an id, in a template, in
   `[project] kind` or in a manifest.
8. **`color`**: the eight roles of §2.5, by those names exactly.

**Added in review** (M1, M2, S1; Fenix holds every one in the manifest
reader *and* again in the registry, and a test in fenix-gui holds the two
lists together):

9. **Fenix's own markers** are no pack's (M1). A marker equal, ignoring
   ASCII case, to one of these is left out of any pack's kind -- the
   kind stays, with a note -- except for the built-in id that owns it,
   where it's dropped silently as adding nothing:
   - `arduino`: `library.properties`, `sketch.yaml`, `{folder}.ino`,
     `{folder}.pde`
   - `rust`: `Cargo.toml`
   - `python`: `pyproject.toml`, `setup.py`, `requirements.txt`,
     `uv.lock`
   - `go`: `go.mod`
   - `node`: `package.json`
   - `cpp`: `CMakeLists.txt`
   - `tcl`: `pkgIndex.tcl`
   - no kind (a project's top): `.git`, `.projectile`
   (Tcl's "any `*.tcl` at the top" is a rule, not a name; with Decision
   A it ranks above every pack's marker anyway.)
10. **No `:`, devices, trailing dot or space** (M2), in a marker and in
    each part of a `main` path: `:` (on Windows `C:x` names another
    drive's file); a name whose part before its first `.`, trailing
    spaces dropped, is `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9` or
    `LPT1` to `LPT9`, in any case (`nul.txt`, `Aux.json`); a name ending
    in `.` or a space. The kind is left out, as for any bad marker.
11. **Fenix's tags and names** (S1): a tag equal to `PY`, `INO`, `RS`,
    `TCL`, `C++`, `JS`, `GO`, `MONO` or `DIR`; a name equal, ignoring
    case and the spaces around it, to `Python`, `Arduino`, `Rust`, `Tcl`,
    `C/C++`, `JavaScript`, `Go`, `Monorepo` or `Project`. The kind is left
    out.
12. **Invisible characters** (S1): a name may hold none of: a control
    character (Unicode `Cc`: U+0000-U+001F, U+007F-U+009F); a format
    character (Unicode `Cf`, Unicode 16: U+00AD, U+0600-U+0605, U+061C,
    U+06DD, U+070F, U+0890-U+0891, U+08E2, U+180E, U+200B-U+200F,
    U+202A-U+202E, U+2060-U+2064, U+2066-U+206F, U+FEFF, U+FFF9-U+FFFB,
    U+110BD, U+110CD, U+13430-U+1343F, U+1BCA0-U+1BCA3, U+1D173-U+1D17A,
    U+E0001, U+E0020-U+E007F); U+2028, U+2029. And it needs at least one
    character that is neither whitespace nor one of those. The kind is
    left out.

What `xtask` refuses Fenix leaves out, said, and loads the extension
anyway: a manifest is untrusted input, and a bad kind is never why an
extension or Fenix doesn't start. So in Fenix a bad entry, a typo in a
field (`deny_unknown_fields`, per entry), the section in the wrong
shape, kinds past the eighth, and a bundle's kinds are each left out
with a line on the Extensions page saying which and why; the rest of the
extension, and its other kinds, are as they were. Across every
extension Fenix also takes at most 256 kinds and 256 distinct markers
(§5), and an id a second extension declares is the first's (§2.4).

What Fenix keeps: `ProjectKindContribution { id, name, tag, color,
markers, main }` beside `TestProviderContribution`
(`manifest.rs:586-612`), with `#[serde(deny_unknown_fields)]` as every
contribution has, so a typo in a field is an error now rather than in a
later Fenix that reads it.

### 6.3 Older Fenix, and the API number

Fenix 1.0 to 1.5.2 parse `[contributes]` without
`deny_unknown_fields` (§1.2), so `project_kinds` is ignored there: the
pack installs and runs as before, without its kinds. That decides the
rest:

- **No `api = "1.6"`.** A pack keeps `api = "1.5"` and `fenix = ">=1.5,
  <2"` and can ship its declaration today; Fenix 1.6 reads it. The API
  number is for what the extension's code can call
  (`WRITING_EXTENSIONS.md:1415-1430`); this is like `[activation]`'s
  `markers`, which the manifest's docs mark "(Fenix 1.1)" rather than
  with an API (`WRITING_EXTENSIONS.md:1227-1229`).
- **Contrast with `[[keys]]`** in keymaps, which needs API 1.5 because
  Fenix 1.4 refuses the keymap (fenix-extensions `xtask/src/main.rs`,
  `check_chords`); and with test providers, which need 1.6 because they
  are exports (`manifest.rs:1194-1196`). Neither applies here.
- **A template naming the pack's kind is the exception** (§4): Fenix 1.5
  refuses it, so it needs `fenix = ">=1.6"`.

### 6.4 Can an extension ask what kind a project is?

Not today: `here()`'s `project-info` has a name, a root and `meta`
(`crates/fenix-extension-api/wit/world.wit:426-433`), and `meta` holds
only `("jira", key)` (`wasm_host.rs:1678-1681`). Adding a field to the
record isn't additive -- within 1.x nothing in the WIT changes
(`world.wit:37-43`), and a guest built for 1.5 lifts the record as it
was. But `meta` is already an open list of pairs, and the three
extensions that read it look a key up by name (fenix-extensions
`extensions/jira/src/lib.rs:169`, `review/src/lib.rs:262`,
`agenda/src/lib.rs:663`). So Fenix 1.6 adds `("kind", id)` to it for
every project -- the declared or detected id, `other` included. No WIT
change, no API number; `WRITING_EXTENSIONS.md` says it's there from
Fenix 1.6, and an extension treats its absence as "an older Fenix". It
lets the Agent context pack tell an agent "a Java project" without
duplicating detection.

## 7. Lifecycle

### 7.1 At startup

Fenix reads the switched-on extensions' manifests before the first
frame (`code_extensions_installed`, `app.rs:6172-6173`); kinds are read
there with the markers, from every switched-on extension whose manifest
parses and `runs_in` this Fenix -- not only the WebAssembly ones, as
markers are (`installed.rs:209-217`). So the hub and Home never draw a
kind before the extensions' are known, and nothing waits for a pack's
code to load.

But the session is restored, and a file given on the command line
opened, **before** that (`app.rs:6135-6160`), and both can fill the kind
cache and Home's rows with Other. The same function that sets the
registry clears `project_kinds` and asks Home for its data again, after
which `refresh_project_root` (`app.rs:6179`) runs as it does now. The
MIB branch hit the same ordering for markers (its `app.rs` hunk at
6174).

In safe mode only the built-ins exist (`installed.rs:143-148`).

### 7.2 Installed, updated, switched off, removed

`extensions_switched` (`extensions_host.rs:279-292`) is where a change
to what's installed is put in use without a restart, through
`code_extensions_now` (`wasm_host.rs:793-819`), which sets the markers
again. The kinds are set again in the same place, from the same read;
then the kind cache is cleared, Home asked again, and any open hub,
doctor or settings page marked stale. Kinds aren't among what waits for
a restart (`installed::read_at_start`, `installed.rs:259-265`, is themes
and languages).

What a user sees: install the Java pack, and every Java project in the
hub turns from `DIR` to `JAVA`; switch it off, and they're `DIR` again,
except those that declare `java`, which are muted `JAVA` (§2.6). Nothing
is written anywhere either way.

### 7.3 Where kinds are drawn

Every place in Appendix A that draws `kind.tag()` and `Role::Kind(kind)`
keeps doing so. Fixed fields: the hub and the wizard leave five cells
before the name (`project_hub.rs:390`, `410`; `project_wizard.rs:784`,
`787`), which fits four and a space. Home leaves four
(`dashboard.rs:442-445`, tag at `x + 5`, name at `x + 9`), so a
four-character tag touches the name -- core's `MONO` already does; it
moves to five (step 1). The modeline and the doctor put a space after
the tag whatever its width (`app/projects.rs:30`, `project_doctor.rs:204-205`).

## 8. Remote projects, monorepos, the test explorer

- **Remote projects** stay `DIR` (§5): detection and the declared kind
  are read from the local disk (`kind.rs:194-200`,
  `fenix_storage::project_file`), and a remote root isn't on it.
  Reading them through the stand-in is a later change of the remote
  work, one batched question per root ("which of these names exist
  there"), off the UI thread, cached like a local one -- not part of
  these steps.
- **Monorepos and subprojects** (`workspace.rs`): `subprojects` lists
  what `find_project_root` calls a root and gives each its kind
  (`workspace.rs:103-128`); it gets extension kinds for free, but finds
  no more roots than today (§1.1, Q6). `language_root` and
  `workspace_above` (`workspace.rs:44-66`) are about Cargo, uv, npm/pnpm
  and Go workspaces and keep keying on the built-ins. Monorepo stays a
  built-in, declared-only kind.
- **The test explorer's providers** have `markers` too
  (`manifest.rs:602-605`). They stay apart. A provider's markers answer
  "does this runner serve this project", and several do at once -- cargo
  test and nextest, a Rust repository's web front end's Vitest; a kind
  answers "what is this project", once. A provider that keyed on a kind
  would stop serving the Node half of a Rust repository. And the
  providers' section is API 1.6, merged and built against
  (`test-explorer.md` §15, E1b); it doesn't move for this. A pack writes
  the same files in both where they agree.

## 9. What doesn't change

- `find_project_root`'s rule: Fenix's markers nearest-first, packs'
  markers (contexts' and now kinds') only where none is found (Q6).
- Fenix's nine kinds, their order among themselves, their tags, names,
  colours and main files, and every behaviour keyed on them.
- `state/projects.json`: no kind is ever stored by Fenix (§1).
- `[[contributes.contexts]]`: what it means and how packs load.
- The extension API: no WIT change, no new number (§6.3, §6.4).

## 10. The extensions

### 10.1 Where it lands, and when it ships

- **`main`** for every pack whose next release is there: the manifest
  section needs no API, and Fenix 1.5 ignores it (§6.3). `xtask
  validate`'s checks (§10.3) land first, so the declarations are checked
  as they're written.
- **`api-1.6`** for Rust, Python, Go and Web, whose next releases (1.3.0,
  their test providers) wait there for Fenix 1.6.0 (fenix-extensions
  `git log main..api-1.6`). Their declarations go into those releases,
  so the four aren't released twice.
- **Releases**: with the first Fenix that reads the section (1.6, if
  step 2 is merged before 1.6.0 is tagged), all at once, not before -- a
  release only to add a section Fenix 1.5 ignores is an update prompt
  with nothing in it. Packs without templates naming their kind keep
  `fenix = ">=1.5, <2"`; those whose templates switch from `other` to
  their own kind move to `">=1.6, <2"` in the same release (Q5). Each is
  a minor (a new feature, no key moves).
- **`lang-kit`**: nothing. Kinds are manifest data, which the kits
  never read. `xtask new language-pack`'s `extension.toml` template
  (fenix-extensions `xtask/templates/language-pack/extension.toml`) gets
  a commented `[[contributes.project_kinds]]` beside its context.

### 10.2 Which pack declares what

Markers are the pack's context markers less the weak ones, which stay in
the context (§2.3). Colours spread the eight roles over the kinds most
often seen together. `main` uses `{folder}` where a tool names the entry
after the project.

| Pack | Kind id | Tag | Name | Markers | Colour | Main |
|---|---|---|---|---|---|---|
| fenix.java | `java` | `JAVA` | Java | `pom.xml`, `build.gradle`, `build.gradle.kts`, `settings.gradle`, `settings.gradle.kts` | `keyword` | -- (a package path) |
| fenix.java | `kotlin` | `KT` | Kotlin | (declared only: a Kotlin Gradle project's files are a Java one's) | `function` | -- |
| fenix.dotnet | `dotnet` | `C#` | C# and .NET | `{folder}.sln`, `{folder}.slnx`, `{folder}.csproj`, `global.json`, `Directory.Build.props` | `attribute` | `Program.cs` |
| fenix.swift | `swift` | `SW` | Swift | `Package.swift` | `number` | `Sources/{folder}/main.swift` |
| fenix.ruby | `ruby` | `RB` | Ruby | `Gemfile`, `gems.rb`, `.ruby-version`, `config.ru` (not `Rakefile`) | `constant` | `config.ru` |
| fenix.php | `php` | `PHP` | PHP | `composer.json`, `artisan` | `variable` | `public/index.php`, `index.php` |
| fenix.zig | `zig` | `ZIG` | Zig | `build.zig`, `build.zig.zon` | `number` | `src/main.zig` |
| fenix.elixir | `elixir` | `EX` | Elixir | `mix.exs` | `function` | `lib/{folder}.ex` |
| fenix.dart | `dart` | `DART` | Dart | `pubspec.yaml` | `type` | `lib/main.dart`, `bin/{folder}.dart` |
| fenix.dart | `flutter` | `FLUT` | Flutter | (declared only: telling it from Dart means reading `pubspec.yaml`) | `string` | `lib/main.dart` |
| fenix.terraform | `terraform` | `TF` | Terraform | `main.tf`, `versions.tf`, `providers.tf`, `.terraform.lock.hcl` | `attribute` | `main.tf` |
| fenix.haskell | `haskell` | `HS` | Haskell | `cabal.project`, `stack.yaml`, `{folder}.cabal`, `package.yaml` | `keyword` | `app/Main.hs`, `src/Main.hs` |
| fenix.ocaml | `ocaml` | `ML` | OCaml | `dune-project`, `dune-workspace`, `{folder}.opam` | `string` | `bin/main.ml` |
| fenix.lua | `lua` | `LUA` | Lua | `.luarc.json`, `.luarc.jsonc`, `selene.toml`, `stylua.toml`, `.stylua.toml`, `.luacheckrc`, `.busted`, `conf.lua` | `constant` | `main.lua`, `init.lua` |
| fenix.ada | `ada` | `ADA` | Ada | `alire.toml`, `{folder}.gpr` | `type` | `src/{folder}.adb` |
| fenix.rust | `rust` (built-in) | | | adds `rust-toolchain.toml`, `rust-toolchain` | | |
| fenix.python | `python` (built-in) | | | adds `manage.py` | | |
| fenix.go | `go` (built-in) | | | adds `go.work` | | |
| fenix.web | `node` (built-in) | | | adds `tsconfig.json`, `jsconfig.json`, `deno.json` | | |
| fenix.cpp | `cpp` (built-in) | | | adds `CMakePresets.json`, `compile_commands.json`, `vcpkg.json`, `conanfile.txt`, `conanfile.py`, `.clangd` | | |
| fenix.arduino | `arduino` (built-in) | | | (Fenix's sketch rule already covers its context's markers: nothing) | | |
| fenix.tcl | -- | | | (its context has no markers; Fenix's Tcl rule stands) | | |

The packs with templates switch them to their kind in the same release:
Java's four (`java`; a Kotlin one writes `kind = "kotlin"` among its
files, §4), .NET's five, Dart's two console and library (`dart`) and
Flutter's (`flutter`, writing the declaration), Elixir's two, Lua's two,
Haskell's, OCaml's, PHP's, Swift's, Terraform's. Ruby and Zig have no
templates. Protobuf (`buf.yaml`), Containers, MIB and the other tool
packs declare no kind: none of them is what a project is.

### 10.3 `xtask validate`

Fenix-extensions' `xtask` reads manifests with its own types and holds
them to Fenix's rules (`xtask/src/main.rs:20-21`). It adds a
`check_project_kinds` beside `check_chords` (`main.rs:325`, `352`) and, on the
`api-1.6` branch, `check_test_providers`:

- every rule of §6.2, and for a built-in id: `markers` only;
- across the store: one pack per id; no tag twice, nor one of Fenix's
  (`PY`, `INO`, `RS`, `TCL`, `C++`, `JS`, `GO`, `MONO`, `DIR`); a warning
  when two packs' kinds share a marker;
- a template's `kind`: Fenix's ids, its own pack's, or another store
  pack's (a warning unless that pack is a dependency); a template naming
  any but Fenix's needs the pack's `engine.fenix` floor at 1.6 or later.

## 11. The steps

Sizes as Fenix's plans write them: S a few days, M one to two weeks. Each
step is a pull request with its tests and leaves `master` releasable.

| # | Step | Size | Crates | Risk | Verified by |
|---|---|---|---|---|---|
| 0 | This plan and ADR 0006 | S | docs | -- | Review |
| 1 | **Kinds as data**: `ProjectKind` an interned id with built-in constants (§2.2), the registry with Fenix's nine, `known()` for `ALL`, `KindColor` (§2.5); a declared or template kind no one declares kept and shown (§2.6, §4); the cycler keeps it; "Start from" last; Home's tag field five cells; the ~140 references (Appendix A) renamed | M | `fenix-project`, `fenix-gui` | Low to medium: wide but mechanical, one type everywhere | Every existing test, renamed; new: `kind = "java"` with nothing declaring it loads, shows `JAVA` muted, survives a settings-page visit and an `h`/`l` back and forth, and survives the `project.ini` move; a template with `kind = "cobol"` loads (the refusal test at `template.rs:1459` changes to a malformed id); ids and aliases round-trip; a page as text with a four-character tag on Home |
| 2 | **Extensions declare kinds**: `[[contributes.project_kinds]]` and its checks (§6); the registry and `EXTENSION_MARKERS` set from it at startup and on a change, the cache cleared, Home and the project pages refreshed (§7); §2.4's order; built-in ids' markers merged; remote roots skipped (§5); `("kind", id)` in `meta` (§6.4); `WRITING_EXTENSIONS.md` (the manifest's block, a "Project kinds" paragraph under Language packs, `here()`'s row), `FEATURES.md`'s "Project identity" and hub entries, `CONFIGURATION.md`'s `[project] kind`, the changelog | M | `fenix-extensions`, `fenix-project`, `fenix-gui`, docs | Medium: the order of kinds changes some projects' tags (a Rails app from `JS` to `RB`), and startup ordering | Manifest tests for each rule of §6.2; a 1.5-shaped manifest with the section loads (the 1.5 parser's leniency pinned by a test that parses the section through a struct without the field); detection tests, one per tier boundary of §2.4 and the same-marker and same-id cases; an app test that installs a pack with a kind while a project with its marker is open, sees `DIR` become the kind in the modeline, hub and Home, switches it off and sees it go back; the restored session's project shows the pack's kind (§7.1); a remote root is never `stat`ed |

```text
0 ─ 1 ─ 2
```

**What an independent reviewer should attack:**

- *Step 1*: that nothing reads a built-in through a path that now
  allocates every frame (the modeline asks for the tag each frame,
  `app.rs:21562`: `tag()` returning `String` must not cost a registry
  lock and an allocation per frame -- cache the spans or return
  `Cow<'static, str>` for built-ins); that the interner can't grow
  without bound from a hostile settings file (the id grammar caps the
  length; count what's interned per project); that `match` on the new
  constants compiles to what the old `match` did and no arm was lost
  in the rename; that `from_id`'s lowercasing and aliases are unchanged
  for the built-ins; that `template.rs`'s error for a malformed kind is
  still there.
- *Step 2*: the startup order (a restored session's project, a file on
  the command line, Home's first draw); that a kind whose extension is
  switched off doesn't linger in a page already open; that §2.4's tiers
  are what's built, including a built-in id's added markers landing in
  the built-in's tier and not the extensions'; that a pack declaring
  `other`, `ts` or `mib` is refused; that markers with `/`, `..`,
  `{folder}{folder}` or 65 bytes are refused; that the manifest check
  doesn't tie the section to `engine.api`; that the merged markers
  don't change root finding for a project that has a `.git` above
  (§1.1's rule kept); the stat count for a hub of 50 projects with every
  store pack installed.

### In fenix-extensions

| # | Step | Size | Needs | Owns | Done when |
|---|---|---|---|---|---|
| K1 | `xtask validate` checks `[[contributes.project_kinds]]` and templates' kinds (§10.3); `xtask new language-pack`'s stub | S | Nothing of Fenix's: §6.2 as written | `xtask` | Its tests, each rule refused once; the store validates unchanged |
| K2 | **On `main`**: the declarations of §10.2 for Java, .NET, Swift, Ruby, PHP, Zig, Elixir, Dart, Terraform, Haskell, OCaml, Lua, Ada, C/C++ and Arduino (nothing for Arduino if the table stands), each pack's README saying so; templates still `other` | S | K1 | those packs' `extension.toml` | `xtask validate`; each pack in a nightly with step 2: its projects tagged, its declared-only kind declared by hand |
| K3 | **On `api-1.6`**: Rust, Python, Go and Web declare their built-in ids' extra markers | S | K1 merged into the branch | four `extension.toml` | As K2 |
| K4 | **Releases**, after Fenix's step 2 ships: K2's packs a minor each, templates switched to their kinds and `fenix` raised for those that have templates (Q5); K3's with their 1.3.0 | S | Fenix 1.6.0 tagged (or the release that has step 2) | releases | The registry lists them; Fenix 1.5 is offered the previous releases of those whose floor moved |

K1 and K2 can start now, from this plan; nothing in them changes what
Fenix 1.5 does.

## 12. Open questions for the maintainer

The implementers go ahead on the recommendations.

| | Question | Options | Recommended |
|---|---|---|---|
| Q1 | Do Fenix's nine stay built in? | (a) yes, for now -- (b) of §3; (c) move them to packs in this work | **(a)**. Fenix with no packs keeps Rust and Python whole, and root finding doesn't move. The packs already declare the built-in ids (§10.2), so (c) later is deleting registrations, once root finding is decided (Q6). |
| Q2 | Two kinds with one tag | (a) Fenix allows it, the store refuses it; (b) Fenix refuses the second; (c) Fenix shows the second's id instead | **(a)** between packs: a tag is cosmetic, and refusing an extension for one is out of proportion; the store is where it's kept tidy. But no pack may take one of **Fenix's** tags or names (S1, §6.2): that would pass a pack's kind off as Fenix's. |
| Q3 | May an extension give its kind a colour of its own (`#cc7832`)? | (a) no: a theme role from §2.5's eight; (b) yes, as `page-render-ex` may (`page.rs:61-62`) | **(a)**: a fixed colour is unreadable on some theme, and the tag is drawn in Fenix's own chrome (the modeline), not on the extension's page. |
| Q4 | The extensions' kinds come before Node, C/C++ and Tcl (§2.4) | (a) yes; (b) after all of Fenix's; (c) before all of Fenix's but Arduino | ~~(a)~~ **Decided (b) in review** (Decision A, §2.4): installing a pack must never change the kind of a project Fenix recognises today. Under (a) a React Native app (CocoaPods `Gemfile`), any JS app with a fastlane `Gemfile`, `package.json` beside `main.tf` or `.luarc.json` stopped being JavaScript. A Rails app stays `JS` and declares `kind = "ruby"`. |
| Q5 | A pack whose templates name its own kind needs `fenix = ">=1.6"` (§4): Fenix 1.5 users stay on its previous release | (a) switch templates and raise the floor in the kinds release; (b) keep templates `other` until a pack needs 1.6 for something else | **(a)**: it's one release after 1.6.0 and 1.5 users lose nothing they have; under (b) the wizard shows `DIR` on Java's templates for as long as the floors don't move. |
| Q6 | Should a kind's markers make a project inside a repository its own, as `Cargo.toml` does (§1.1)? | (a) not now; (b) yes, for kinds' markers only | **(a)**: it would split a .NET solution into a project per `.csproj`, a Gradle build into one per module, a Terraform repository into one per module folder. It's a decision about project boundaries, to make with (c) of Q1, not one this work needs. |

## 13. As built

Filled in as each step merges: what was built differently from this
plan, and why.

Steps 1 and 2 were reviewed before merging; §13.3 says what the review
changed, and §2.4, §2.6, §5, §6.2 and §12 are revised to match. Where
§13.1 and §13.2 say otherwise, §13.3 wins.

### Step 1: kinds as data

- **`ProjectKind(u16)`** as §2.2 has it, constants `PYTHON` ... `OTHER`
  at indices 0 to 8 and `ProjectKind::BUILT_IN` (their order) for `ALL`.
  `Debug` prints the id. The nine are a table in `kind.rs` (id, tag,
  name, `KindColor`, markers for `def()`, main files); detection and the
  doctor keep their code, on the constants (`_ => {}` for the rest).
- **`tag()` and `label()` return `&'static str`**, not `String`: the
  text of a kind that isn't Fenix's is kept once for the run (a set of
  leaked strings, bounded by the manifests' limits and the ids
  interned), so callers didn't change and the modeline neither
  allocates nor locks for Fenix's kinds; for an extension's it takes the
  registry's read lock. `id()` likewise.
- **The interner is bounded**: at most 4,096 ids a run, Fenix's
  included; past that a new id is read as no kind, as a malformed one
  is. A hostile settings file interns one id.
- **`from_id`**: Fenix's ids and aliases in any case, as before; any
  other id only as written (`Java` isn't one) -- §6.2's grammar, and
  what `xtask` holds templates to. The template's error for what isn't
  an id says the grammar.
- **The cycler** keeps a declared kind no one declares among its
  choices for as long as the page is open (latched the first time it's
  seen), after the known kinds: with it only in the list while it's the
  declared value, `h` then `l` couldn't come back to it.
- **Unknown ids' tags** are the id upper-cased and cut to four,
  whatever its characters (`my-kind` is `MY-K`).
- Home's name and the line under it moved to the sixth cell; "Start
  from" ranks after every group, Fenix's and the packs'.

### Step 2: extensions declare kinds

- **A bad declaration never refuses the manifest** (§6.2's last
  paragraph): `Contributes::project_kinds` is a `ProjectKinds` of the
  kinds kept and the lines saying what was left out, read entry by
  entry. A bundle's kinds are left out and said rather than refusing
  the bundle. The resolutions of §6.2's open points are written into
  §6.2.
- **Totals**: 256 kinds and 256 distinct markers from all extensions
  together (the plan bounded a pack, not the sum). With 30 packs of 8
  kinds and 16 markers each, all distinct, the cap holds and is said.
- **The registry** (`fenix_project::set_extension_kinds`) takes the
  kinds in detection order and returns whether anything changed; the
  same kinds again change nothing, so the cache isn't cleared and
  nothing is asked again. `extension_kind_notes()` has what it left
  out (a second extension's id, past the totals). It also re-checks
  every marker and main path itself (`marker_is_a_name`,
  `main_is_below`), so the bound holds whoever calls it.
- **Which extensions**: every switched-on one whose manifest reads and
  `runs_in` this Fenix, data packs included (`installed::kind_extensions`),
  the machine's first then yours, each by id. Set after
  `code_extensions_installed` at start and in `extensions_switched`
  (whatever `code_extensions_now` found), as §7 says.
- **When they change** (`App::project_kinds_changed`): the kind cache
  and the doctor's health cache (computed by kind) are cleared; Home's
  data is gathered again; an open hub is rebuilt and fetched again (its
  subprojects rescanned off the UI thread); open doctor pages take the
  new kind and check again; the settings page's "detect" is worked out
  again; the explorer's headers are asked again.
- **Detection**: Fenix's own rules look as before (`exists`, the sketch
  rule, the Tcl listing); markers from extensions are `is_file`, as a
  context's are, each looked for once per folder however many kinds
  name it. A marker's case is the file system's: `Path::is_file` on the
  joined name, so `gemfile` is `Gemfile` on Windows' and macOS's
  default file systems and not on Linux; `.fenix` is refused in any
  case for that reason. `main` paths are joined a part at a time
  (`Path::join`). With no extension, a folder costs exactly what it did
  (14 looks at an empty one, pinned by a test).
- **Remote roots**: `detect_kind`, `declared_kind` and `main_file` return
  before touching the disk for a root `is_remote_text` calls remote
  (counted: no look at all).
- **Root finding** takes the kinds' markers (Fenix's ids' added ones
  too) with the contexts', under the fallback rule; `holds_pack_marker`
  (the MIB clean-up) sees them too.
- **`ProjectKind::extension()` and `is_declared()`** were added for the
  pages and tests; `KindDef` has `extension: Option<String>`.
- **The Extensions page** shows a "Project kinds" section in an
  installed extension's details (and its Markdown details): each kind
  (`JAVA Java (java): pom.xml`, `node: adds deno.json`) and each line of
  what was left out.
- **`here()`'s `meta`** has `("kind", id)` from the cached kind, after
  `("jira", key)`. No WIT change; `API` stays `1.6.0`, and nothing here
  needs `engine.api = "1.6"` (a pack with `api = "1.5"`, WebAssembly or
  data, has its kinds read: tested).
- **Measured** (release build, Linux, a temporary folder; the
  `measure_detection_with_thirty_extensions` test, `--ignored`): with no
  extension, a hub of 50 projects listed with their kinds in 2.5 ms
  (11 looks a project), `detect_kind` 7 µs, `find_project_root` for a
  file in no project 8 folders deep 63 µs. With 30 extensions' kinds
  (8 x 16 markers each, capped at 256): the hub 14.9 ms (190 looks a
  project), `detect_kind` 128 µs, `find_project_root` 1.2 ms. The store's
  packs have about 70 markers in all.
- **The store's packs**: every declaration on fenix-extensions'
  `kinds/declare` (14 packs and `fenix.cpp`'s added markers) loads in
  Fenix with nothing left out.

## Appendix A. Every use of a project kind

`master` at `3e77131`. Tests are counted, not listed, except where a
test pins behaviour this plan changes. "Keyed on built-ins" means the
code keeps its `match` or `==`, on `ProjectKind::RUST` and the rest.

### `fenix-project`

| Where | What the kind decides | What becomes of it |
|---|---|---|
| `kind.rs:14-29` | The enum | A `Copy` handle to an interned id, with constants (§2.2) |
| `kind.rs:31-40` | MIB tables | Gone with the MIB branch (moved to `known.rs` there) |
| `kind.rs:43-54` | `ALL`, the order kinds are listed and cycled in | `known()`: built-ins in this order, then extensions' by name |
| `kind.rs:57-70` | `id` | The interned id |
| `kind.rs:72-82` | `from_id`, aliases | Built-in ids and aliases as now; any well-formed id interned |
| `kind.rs:86-99`, `101-114` | `tag`, `label` | From the definition, or §2.6 for an id no one declares |
| `kind.rs:120-122` | `detect_kind` | Unchanged shape |
| `kind.rs:126-150` | `detect_kind_from_files`, the order | §2.4's tiers, the extensions' markers looked at once each (§5) |
| `kind.rs:155-181` | `main_file` per kind | Built-ins' lists in their registration; extensions' `main`; Tcl's first `*.tcl` kept |
| `kind.rs:194-200` | `declared_kind` | Returns an unknown well-formed id instead of `None` (§2.6) |
| `kind.rs:226-293` | Tests (18 uses) | Renamed; new cases for unknown ids and tiers |
| `root.rs:8`, `26-31`, `39-59` | Root finding; `EXTENSION_MARKERS` | Unchanged rule; kinds' markers join `EXTENSION_MARKERS` (§2.3) |
| `workspace.rs:30-37` | Workspace manifests per kind | Keyed on built-ins |
| `workspace.rs:44-56` | `language_root` picks an ecosystem from files, using the enum as a label | Keyed on built-ins (or a private enum: no kind is asked of anyone) |
| `workspace.rs:64-66` | `workspace_above(dir, kind)` | Keyed on built-ins; any other kind has none |
| `workspace.rs:103-128` | `subprojects` gives each its kind | Unchanged; extension kinds come through `detect_kind` |
| `workspace.rs:133-200` | Tests (10 uses) | Renamed |
| `template.rs:13`, `219`, `332` | A template's kind; unknown refused | Any well-formed id; malformed refused (§4) |
| `template.rs:1071` | `Plan::kind` | Unchanged type |
| `template.rs:1439`, `1459` | Tests: MIB plan's kind; "unknown kind" refused | The first as the MIB branch has it; the second becomes a malformed id |
| `doctor.rs:268`, `289` | `Doctor::kind` | Unchanged type |
| `doctor.rs:353-357` | `FeatureCheck`, "Fenix's Arduino, MIB and Tcl features" | No caller passes one (§1.2); left, comment corrected |
| `doctor.rs:366-389` | Core checks for Python, Rust, C/C++, Go, Node; none for the rest | Keyed on built-ins; `_ => {}` for every other kind |
| `doctor.rs:566-793` | Tests (18 uses) | Renamed |
| `meta.rs:128-130` | `set_kind` writes `project.kind` | Writes the id, unknown ones included |
| `meta.rs:145`, `217-219` | The `project.ini` move copies `kind` as written | Unchanged; tested with an unknown id |
| `meta.rs:312` | Test | Renamed |
| `lib.rs:21` | Re-exports | Adds `set_extension_kinds`, `KindDef`, `KindColor` |

### `fenix-gui`

| Where | What the kind decides | What becomes of it |
|---|---|---|
| `app.rs:5158-5163`, `6383` | `project_kinds`, the per-root cache | Cleared also when the extensions' kinds are set (§7) |
| `app.rs:7300-7302` | `project_kind_of` | Unchanged |
| `app.rs:7306-7310`, `21562` | The modeline's project segment | Unchanged; the tag must stay cheap per frame (§11) |
| `app.rs:19888` | A page popup's `Role::Kind` colour | `kind_color` on `KindColor` |
| `app/projects.rs:11-24` | `kind_color`: a theme role per kind | One `match` on `KindColor` (§2.5) |
| `app/projects.rs:28-34` | Modeline spans: tag and colour | Unchanged |
| `app/projects.rs:66` | Test | Renamed |
| `app/home.rs:157`, `171`, `480` | Home's doctor pass and rows; dashboard role colour | Unchanged; Home's data asked again when kinds change |
| `app/pages.rs:39` | Page role colour | As `app.rs:19888` |
| `app/pages.rs:241`, `1581-1600` | Hub fetch: subprojects and the doctor's quick pass per kind | Unchanged |
| `app/pages.rs:1286` | `main_file` for a project you've had nothing of open | Unchanged call; extensions' `main` behind it |
| `app/pages.rs:1475` | Cache cleared after the wizard | Unchanged |
| `app/pages.rs:1513` | Hub rows' kind | Unchanged |
| `app/pages.rs:1695-1697` | Doctor page's kind, and the pack's name | Unchanged |
| `app/pages.rs:1715` | Doctor's deep pass | Unchanged |
| `app/pages.rs:1808-1809` | Settings page: detected and declared kinds | `declared` keeps an unknown id (§2.6) |
| `app/pages.rs:1827-1833` | `SetKind` writes the declaration | Unchanged |
| `app/explorer_host.rs:441` | The explorer header's kind, uncached | Unchanged; cost in §5 |
| `app/wasm_host.rs:780-786`, `810-811` | Packs' markers set | Also the kinds (§7) |
| `app/wasm_host.rs:591-595` | `project_language`: the pack whose context markers a root holds | Kept: the doctor's name for an Other project with no pack kind |
| `app/wasm_host.rs:1678-1681` | `here()`'s `meta` | Adds `("kind", id)` (§6.4) |
| `page.rs:12`, `56` | `Role::Kind(ProjectKind)` | Unchanged (`Copy` kept) |
| `dashboard.rs:56`, `145`, `444` | Home's project item, role, tag field | Field five cells (§7.3) |
| `dashboard.rs:628-629`, `763` | Tests | Renamed |
| `project_hub.rs:21`, `44`, `66` | A hub row's kind | Unchanged |
| `project_hub.rs:97`, `125`, `245-268` | Kind filter (Tab) | Unchanged |
| `project_hub.rs:172-181` | Kinds present, ordered by `ALL` | Ordered by `known()`, unknown ids last |
| `project_hub.rs:357-366`, `390`, `438` | Filter row, row tag, preview tag | Unchanged |
| `project_hub.rs:564-644` | Tests (11 uses) | Renamed; the MIB case as the MIB branch has it |
| `project_settings.rs:43`, `56-57`, `80` | `SetKind`, detected, declared | Unchanged types |
| `project_settings.rs:265-272` | The cycler over `ALL` | Over `known()` and an unknown declared id (§2.6) |
| `project_settings.rs:540-548` | "‹ detect: X ›" | Plus "no installed extension knows this kind" |
| `project_settings.rs:620`, `706` | Tests | Renamed; a new one for the unknown id |
| `project_wizard.rs:165`, `198-203` | Group order | "Start from" last (§4) |
| `project_wizard.rs:445-451` | Workspace joined: Rust and Python only | Keyed on built-ins |
| `project_wizard.rs:783-803`, `910`, `946`, `1000` | Template and plan tags | Unchanged |
| `project_wizard.rs:1036` | "cargo" or "uv" | Keyed on built-ins |
| `project_doctor.rs:38`, `56` | The doctor page's kind | Unchanged |
| `project_doctor.rs:63-68`, `204-209` | Other: the pack's name; else the kind's | Unchanged: an extension kind has its own name |
| `project_doctor.rs:322-388` | Tests | Renamed |
| `explorer_page.rs:39`, `437` | The header's kind fact | Unchanged |
| `explorer_page.rs:725` | Test | Renamed |
| `settings_page.rs:1432` | Test | Renamed |

### Documents

| Where | What it says | What becomes of it |
|---|---|---|
| `docs/FEATURES.md:555-563` | The hub's kind tag and filter | Unchanged |
| `docs/FEATURES.md:624-629` | "Project identity": the kinds (MIB among them), declared or detected | Language packs add kinds; a declared kind no pack knows is kept (step 2) |
| `docs/CONFIGURATION.md:96`, `134` | The kind among a project's own settings; the `project.ini` move | Any kind id, including a pack's |
| `docs/WRITING_EXTENSIONS.md:1210-1330` | The manifest | `[[contributes.project_kinds]]` in the example, and a paragraph under Language packs (§6) |
| `docs/WRITING_EXTENSIONS.md:141` | `here()` | Its project's `kind` in `meta` (§6.4) |

### Review: what changed (§13.3)

The guiding rule: installing a pack never changes the kind of a project
Fenix recognises today, and a user with no extensions sees exactly
master's behaviour.

- **Decision A** (§2.4, Q4): Fenix's own detection runs first, exactly as
  master's, and stops; packs' markers on Fenix's ids, then packs' kinds,
  are looked for only where it says Other. Pinned by
  `kind::tests::kinds_are_detected_in_their_tiers` (a Rails app stays
  `JS`; `deno.json` beside `CMakeLists.txt` stays C/C++) and, in the app,
  `a_packs_kind_comes_and_goes_with_it`.
- **Decision B** (§2.6): a declared kind no one declares is ignored for
  the project's kind and kept on disk. `from_id` interns nothing:
  `set_extension_kinds` is the only place an id is interned (after its
  checks, within the 256 kinds), so a repository can't fill the
  interner (the review's S2), and "too many ids this run" is said as
  that. `declared_id` reads the text; `declared_kind` answers only for a
  declared id. The settings page keeps the text (`Settings::kept`),
  and while it's there `h`/`l` write nothing; Enter on a kind replaces
  it, Enter on `detect` or on it writes nothing; `r` on the row, the
  page's reset, removes it (`r_removes_a_kind_no_one_declares`). A template's kind is
  text (`Template::kind_id`); one no one declares is shown as Other and
  the project made from it declares it (`Plan::declare_kind`), unless its
  files already make it that kind. `from_id("nonsense")` is `None`
  again, as on master; the step-1 tests that showed a muted `JAVA` are
  gone.
- **Decision C** (§5): `find_project_root`'s fallback never makes the
  home folder, a folder above it or a file system's root a project's top
  on an extension's marker, a context's included. Fenix's own markers
  unchanged (`root::tests::a_packs_marker_never_makes_home_a_project`).
  A context marker that isn't a plain file name is no longer looked for
  in root finding either (none in the store is).
- **M1** (§6.2 rule 9): Fenix's markers are no pack's -- in the manifest
  reader (marker dropped, kind kept, noted) and in the registry
  (`a_pack_never_takes_fenixs_markers`, `fenixs_own_markers_are_left_out`).
- **M2** (§6.2 rule 10, §5): `:`, device names and a trailing dot or
  space refused in markers and `main` parts; every marker and `main` part
  is joined only as one `Component::Normal`; a pack's `main` file is
  opened only if its canonical path is under the project's
  (`a_kinds_names_stay_in_the_project`,
  `an_extensions_main_file_doesnt_leave_the_project`). Fenix's own main
  files follow links as on master. What runs only on Windows -- `C:x`
  and `\\?\` being prefixes to `Path::components` -- isn't tested here;
  on Linux `C:x` is refused by the `:` rule before it's joined.
- **S1** (§6.2 rules 11, 12): Fenix's tags and names, and invisible
  characters in names, refused in both places
  (`a_packs_kind_doesnt_look_like_fenixs`,
  `a_kind_needs_a_name_and_a_tag_within_limits`).
- **S3**: the explorer's header takes the cached kind; Fenix's folders
  cost master's lookups exactly; when kinds change, an open hub's kinds
  are looked for again on the hub's fetch thread
  (`PageEvent::HubKinds`), not on the UI thread. Home's five rows and
  open doctor pages are still asked on the UI thread, as any Home
  refresh is.
- **S4**: hub fetches carry `App::kinds_generation`; what one from before
  the last change sends back is dropped
  (`a_hub_fetch_from_before_the_kinds_changed_is_dropped`).
- **Measured again** (`kind::tests::measure_detection`, `--ignored`;
  debug build, Linux, local disk; looks at the file system / time per
  detection):

  | extensions | Rust | Python | Node | C/C++ | Tcl | a pack's (Ruby) | Other |
  |---|---|---|---|---|---|---|---|
  | none | 5 / 2.1 µs | 6 / 2.5 µs | 11 / 4.1 µs | 12 / 4.5 µs | 14 / 6.8 µs | 14 / 6.9 µs | 14 / 6.7 µs |
  | the store's 14 packs | 5 / 1.9 µs | 6 / 2.2 µs | 11 / 4.2 µs | 12 / 4.4 µs | 14 / 7.0 µs | 52 / 37.7 µs | 62 / 46.5 µs |
  | 30 packs, 256 markers | 5 / 1.9 µs | 6 / 2.3 µs | 11 / 4.0 µs | 12 / 4.3 µs | 14 / 6.9 µs | 270 / 339 µs | 270 / 337 µs |

  Only folders Fenix calls Other pay for packs' markers (62 lookups with
  the store's, as the review counted with strace); a project of Fenix's
  costs what it did on master.

