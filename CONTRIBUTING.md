# Contributing to Fenix

Thanks for your interest. Bug reports, ideas and pull requests are all
welcome.

## Reporting a bug

Open an [issue](https://github.com/tpedneault/fenix/issues) with:

- what you did, what you expected, and what happened instead;
- the Fenix version (the release you downloaded, or the commit you
  built) and your operating system;
- anything Fenix printed to the terminal it was started from, and a
  screenshot if the problem is visual.

## Building and testing

You need a recent stable [Rust](https://rustup.rs) toolchain with Clippy.
On Windows, you also need the Visual Studio C++ Build Tools.

```bash
cargo build                         # debug build
cargo run -p fenix-gui              # run it
cargo test --workspace              # every crate's tests
```

CI runs the same gates through one script, which you can run locally
with PowerShell 7:

```powershell
pwsh -NoProfile -File ./scripts/ci.ps1 -Suite Workspace     # what the Windows job runs
pwsh -NoProfile -File ./scripts/ci.ps1 -Suite Reliability   # what the Linux job runs
```

See [docs/CI.md](docs/CI.md) for what each gate covers. Tests marked
`#[ignore]` need a live service or a manual setup, such as the GitLab
instance in [`dev/gitlab`](dev/gitlab/README.md).

## Making a change

1. Branch from `master`.
2. Keep each pull request to one change. Add or update tests alongside
   it: most of Fenix lives in host-agnostic crates precisely so it can
   be unit-tested (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).
3. Update the docs a user would read: [docs/FEATURES.md](docs/FEATURES.md),
   [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md) or
   [docs/CONFIGURATION.md](docs/CONFIGURATION.md). The settings table in
   the last one is generated from the schema, and a test fails when it
   falls behind.
4. Add a line under **Unreleased** in [CHANGELOG.md](CHANGELOG.md).
5. Open the pull request. CI must pass before it's merged, and it's
   squash-merged, so its title becomes the commit subject.

Commit subjects name the area first, then say what changed for the
user: `Git: new requests are assigned to you`, `Tabs: gt and gT move
between tabs`.

## Versions and releases

Fenix follows [Semantic Versioning](https://semver.org). The version is
set once, in the workspace `Cargo.toml`, and every crate inherits it.
From the user's point of view:

- **patch** (1.0.x): bug fixes only;
- **minor** (1.x.0): new features, and changes that keep existing
  settings, files and keys working;
- **major** (x.0.0): anything that breaks existing settings, stored data
  or muscle memory without a migration.

To release, move the **Unreleased** entries in the changelog under the
new version, bump `version` in `Cargo.toml`, merge that to `master`, then
tag the merge commit `vX.Y.Z` and push the tag. The
[release workflow](.github/workflows/release.yml) builds Windows and
Linux binaries and publishes the GitHub release.

## License

Unless you explicitly state otherwise, any contribution you
intentionally submit for inclusion in Fenix, as defined in the
Apache-2.0 license, shall be dual licensed under MIT OR Apache-2.0, as
described in the [README](README.md#license), without any additional
terms or conditions.
