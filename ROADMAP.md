# Roadmap

> **Draft for the maintainer to edit.** This was put together from the
> open [issues](https://github.com/tpedneault/fenix/issues), the plans in
> [docs/plans](docs/plans/) and the [design records](docs/adr/) as of
> November 2026. The order inside each list is a suggestion, not a
> commitment, and nothing here has a date.

What Fenix is working towards: an editor you can use all day, on the
machine you have, for any language, without giving up the keyboard.
"Now" is being worked on, "next" comes after it, "later" is wanted but
not started. When something ships it moves to the
[changelog](CHANGELOG.md).

## Now

- **Close the Vim gaps.** The missing motions
  ([#22](https://github.com/tpedneault/fenix/issues/22): `g_`, `ge`,
  `|`, `+`, `-`, `(`, `)`, `H`, `M`, `L`, ...), operators and commands
  ([#23](https://github.com/tpedneault/fenix/issues/23): `R`, `gU`,
  `gu`, `g~`, `gJ`, `gi`, `gq`, a count on `.`, ...) and Insert-mode
  keys ([#25](https://github.com/tpedneault/fenix/issues/25): `Ctrl-W`,
  `Ctrl-U`, `Ctrl-R`, ...), and a page saying where Fenix still differs
  from Vim on purpose.
- **Performance.** No redrawing while nothing changes
  ([#74](https://github.com/tpedneault/fenix/issues/74)); a fast first
  frame however many extensions are installed, without restarting them
  all on a project switch
  ([#33](https://github.com/tpedneault/fenix/issues/33)); extensions and
  their settings that take effect without a restart, or say clearly
  when they need one ([#40](https://github.com/tpedneault/fenix/issues/40)).
- **Remote projects over SSH, phase 3**: terminals, Git and search on a
  remote project, after files (phases 1 and 2, in preview now). See
  [the plan](docs/plans/remote-ssh.md).
- **Accessibility.** Text sent as Unicode input reaching Fenix
  ([#37](https://github.com/tpedneault/fenix/issues/37)), right-to-left
  text laid out properly
  ([#17](https://github.com/tpedneault/fenix/issues/17)), and a first
  step towards screen readers.

## Next

- **macOS builds.** The code already has macOS paths in places (the
  keychain, notifications, sound), but nothing builds, tests or packages
  for it. CI on a macOS runner first, then a signed, notarised `.dmg`.
- **One key to install a language server.** Today Fenix finds the
  servers on your `PATH` and the doctor says what's missing; a missing
  server should be installable from where it's reported.
- **`.editorconfig`**: indent, tab width, line endings and the final
  newline from a project's `.editorconfig`, under its
  `.fenix/settings.toml`.
- **Inlay hints** from language servers (types and parameter names in
  the line, dimmed), beside the code lens and semantic highlighting that
  shipped in 1.5.
- **Remote projects, phases 4 and 5**: language servers, tasks, the
  debugger and agents on a remote project, then out of preview.
- **Bring your settings with you**: import keys, themes and settings
  from Neovim, Helix, VS Code and Zed, with guides for people coming
  from each.

## Process

- **Nightly builds** from `master` each night it changed, and minor
  releases every few weeks tagged from a nightly that has been in use
  ([how releases work](docs/RELEASING.md)). Release candidates are
  dropped. Next: a `fenix-nightly` manifest in the Scoop bucket.

## Later

- **More install channels**: winget (a manifest is in
  [packaging/winget](packaging/winget/) and waiting for review),
  Homebrew once macOS builds, the AUR, Flatpak, and ARM64 Linux.
- **Screen-reader support** across the editor and its pages, beyond the
  first step above.
- **The smaller open issues**: the tab strip scrolling to the active tab
  ([#50](https://github.com/tpedneault/fenix/issues/50)), the doctor and
  Tool Status agreeing ([#43](https://github.com/tpedneault/fenix/issues/43),
  [#52](https://github.com/tpedneault/fenix/issues/52),
  [#35](https://github.com/tpedneault/fenix/issues/35)), extension pages
  that can take `Esc` and `:` ([#72](https://github.com/tpedneault/fenix/issues/72)),
  a minimap and scrollbar marks ([#104](https://github.com/tpedneault/fenix/issues/104)),
  and the rest of the [issue list](https://github.com/tpedneault/fenix/issues).

## How this list changes

It's revised when a release goes out (see
[how releases work](docs/RELEASING.md)). To argue for something, open
or comment on an issue: what you'd use it for helps more than a +1.
