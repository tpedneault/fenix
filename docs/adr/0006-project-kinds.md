# ADR 0006: Project kinds -- data, declared by Fenix and by extensions

- **Status:** proposed 2026-11-14, for Fenix 1.6
- **Date:** 2026-11-14
- **Plan:** [`docs/plans/project-kinds.md`](../plans/project-kinds.md)

## Context

A project's kind is what Fenix names it by: the tag in front of it in
the modeline, the hub, Home and the wizard (`RS`, `PY`, `DIR`), its
colour, its name on the settings page and the doctor, and the file it
opens on. It is a closed enum of Fenix's own, Python, Arduino, Rust,
Tcl, C/C++, JavaScript, Go, Monorepo and Other
(`crates/fenix-project/src/kind.rs:14-29`, MIB going on another branch).

The store has language packs for Java, .NET, Swift, Ruby, PHP, Zig,
Elixir, Dart, Terraform, Haskell, OCaml, Lua, Ada and more. Their
projects are found, by the packs' context markers where nothing of
Fenix's is above them (`root.rs:39-59`), but they're all `DIR`: Fenix
has no way to name them. `kind = "java"` in a project's settings is
ignored (`kind.rs:194-200`), and the settings page's kind cycler writes
over it at the first key (`project_settings.rs:265-272`). A template
naming a kind Fenix doesn't have is refused (`template.rs:332`), so 21
of the store's 33 pack templates say `kind = "other"`.

What Fenix does with a kind is mostly presentation. Its behaviour by
kind -- the doctor's checks for Python (uv, the venv), Rust (cargo,
rust-analyzer, the standard library's source), C/C++, Go and Node
(`doctor.rs:366-389`), the Cargo and uv workspace a new project joins
(`workspace.rs:30-66`, `project_wizard.rs:445-451`), and a main file per
kind (`kind.rs:155-181`) -- is in `fenix-project`; `fenix-gui` has 15
uses of a specific kind outside its tests.

An older Fenix ignores a `[contributes]` section it doesn't know:
`Contributes` has had no `deny_unknown_fields` from 1.0 to 1.5.2
(`crates/fenix-extensions/src/manifest.rs:521-522`).

## Decision

1. **A kind is data**: an id, a name, a tag of one to four characters, a
   colour as a theme role, the files at a project's top that mark it,
   and the files it's entered through. In code, `ProjectKind` is a
   `Copy` handle to an interned id, with constants for Fenix's own.
2. **Fenix declares its nine**, in code, as they are today, and keeps
   what it does for them -- the doctor's checks, the workspaces,
   Arduino's sketch rule -- keyed by those constants. Fenix with no
   extensions is unchanged.
3. **Extensions declare theirs** in `[[contributes.project_kinds]]`
   (`id`, `name`, `tag`, `color`, `markers`, `main`), held to fixed
   limits: at most 8 kinds and 16 markers each, markers file names only,
   colours one of eight syntax roles. A pack may name one of Fenix's
   ids; then only its markers count, added to Fenix's kind.
4. **Detection runs no extension code and reads no file's contents**:
   one existence check per marker at a root. The order: declared, then
   Arduino, then Rust, Python, Go, then the extensions' kinds, then Node,
   C/C++, Tcl, Other. A kind's markers also find a project's top as the
   packs' context markers do, under the same rule.
5. **A declared kind no one declares is kept**: loaded, shown by its id
   in a muted tag, never rewritten, and named again when its pack is
   installed. A template may name any well-formed id.
6. **No extension API change.** The section is read by Fenix, not the
   extension's code; it needs no `api = "1.6"`, and a pack can ship it
   to Fenix 1.5, which ignores it. The WIT doesn't change: a project's
   kind reaches extensions as `("kind", id)` in `project-info`'s
   existing `meta` list.

## Rejected

- **An `Extension(id)` case beside the enum.** Two ways to say a kind,
  one for Fenix and one for everyone else, every `match` with an arm
  for the second and every tag and colour asking a registry for it
  only. Kept `Copy`, it is the chosen design with a seam in it.
- **Moving Fenix's own kinds into the store's packs now.** Fenix with
  no packs would show Rust and Python projects as `DIR` while still
  checking Cargo and uv for them, and Fenix's markers are what make a
  Cargo crate inside a repository its own project
  (`root.rs:8`); moving them changes project boundaries. The packs
  declaring Fenix's ids now makes this a small step later, once that is
  decided.
- **A kind as a context with a label.** A context's markers decide when
  a pack loads and where its menu applies, where a false match is cheap;
  a kind's decide what a project is called everywhere. Packs' lists
  differ for that reason (Ruby's `Rakefile`).
- **Kinds that read a marker's contents** (Flutter from `pubspec.yaml`):
  file contents read for every project in the hub, on the UI thread,
  by rules from extensions. Such a kind is declared instead.
- **A raw colour** for a kind: unreadable on some theme, in Fenix's own
  chrome.
- **Gating the section on API 1.6.** It would keep packs from shipping
  a harmless section to Fenix 1.5 for no gain.

## Consequences

- A Java, .NET, Ruby ... project is named as Fenix's own are once its
  pack is installed, and a teammate without the pack sees its declared
  kind by name.
- Some projects change tag when packs are installed: a Rails or Laravel
  app with a `package.json` becomes Ruby or PHP rather than `JS`.
- Packs whose templates name their own kind need Fenix 1.6, since 1.5
  refuses such a template; the registry keeps 1.5 users on their
  previous release.
- The kind cache is cleared whenever extensions change, and after the
  extensions are read at startup.
- `xtask validate` in fenix-extensions holds the store to one pack per
  id and one tag per kind; Fenix allows duplicates and takes the first.
