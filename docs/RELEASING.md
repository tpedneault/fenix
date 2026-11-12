# How releases work

Changes land on `master` continuously, each with its tests and a line
under **Unreleased** in the [changelog](../CHANGELOG.md). Two kinds of
build come out of that:

- **The nightly**, built from `master` each night it changed, for
  trying what's next. One rolling pre-release on GitHub, tagged
  `nightly`, replaced each time.
- **Releases**, tagged by hand every few weeks, from a commit that has
  already been in use as a nightly.

This page says what each kind of release may carry, how often they
come, and how to run the nightly.

## What goes in a release

Fenix follows [Semantic Versioning](https://semver.org). For you, that
means:

| Release | Example | May carry | Doesn't |
|---|---|---|---|
| **Patch** | 1.5.3 | Bug fixes, crash and data-loss fixes, security fixes, a fix to a doc that's wrong. | New settings, commands, keys or pages; a change in what an existing key does. |
| **Minor** | 1.6.0 | New features, settings and keys; changes that keep existing settings, files and keys working. | Anything that breaks your settings, stored data or muscle memory. |
| **Major** | 2.0.0 | A change that would break those, with a migration and a changelog entry that says what to do. | Surprises. |

A changelog section with an **Added** heading belongs to a minor
release, not a patch. (1.5.1 and 1.5.2 carried new features as patches;
under these rules they would have been 1.6.0.)

## How often

- **A nightly every night `master` changed**, and none when it didn't.
  It's the next minor's pre-release, dated: with `1.5.2` in
  `Cargo.toml`, the nightly of 12 November 2026 is
  `1.6.0-nightly.20261112`. A second one the same day (built by hand)
  is `1.6.0-nightly.20261112.2`.
- **A minor release every few weeks**, when there's something worth
  having, tagged from a commit that has been in use as a nightly for a
  few days without trouble. If **Unreleased** holds only fixes, a patch
  goes out instead.
- **A patch release when a fix matters**, batched, except for a crash,
  lost work or a security fix, which goes out as soon as it's ready.
- **A major release only with a migration**, and with the change in the
  nightly for a few weeks ahead.

There are no release candidates: the nightly is where what's next is
tried.

## For users: running the nightly

Download it from the
[`nightly` release](https://github.com/tpedneault/fenix/releases/tag/nightly).
Its files have the same names every night, so a link to one keeps
working:

| | File |
|---|---|
| Windows installer | `fenix-nightly-x86_64-pc-windows-msvc-setup.exe` |
| Windows, portable | `fenix-nightly-x86_64-pc-windows-msvc.zip` |
| Linux archive | `fenix-nightly-x86_64-unknown-linux-gnu.tar.gz` |
| Debian, Ubuntu | `fenix-nightly_amd64.deb` |
| Fedora, openSUSE | `fenix-nightly.x86_64.rpm` |

Each has a `.sha256` beside it. `fenix --version` says which nightly
you're running and the commit it was built from:
`fenix 1.6.0-nightly.20261112 (0123456789ab)`. Home says the same,
under the date.

- **It installs over a release**, and the next release installs over it,
  in place. The Windows installer is the release's, so it upgrades the
  Fenix that's there, whichever it is. The `.deb` and `.rpm` are the
  same `fenix` package, versioned `1.6.0~nightly.20261112`, which `apt`,
  `dnf` and `rpm` sort after 1.5.x and before 1.6.0.
- **Going back** from a nightly to the release before it is a downgrade:
  on Linux, `apt install --allow-downgrades ./fenix_1.5.2_amd64.deb` or
  `dnf downgrade`; on Windows, install the release over it.
- **Being told when there's a newer one**: set
  `updates.channel = "nightly"` with `updates.check` on, and Home says
  *Fenix 1.6.0-nightly.20261113 (...) (nightly) is out*. A release
  newer than your nightly is still offered. Back on `stable` (the
  default), nightlies are never mentioned. Nothing is downloaded or
  installed for you either way.
- **With Scoop**, the nightly is its own package in the
  [bucket](https://github.com/tpedneault/scoop-fenix):
  `scoop install fenix-nightly`, and `scoop update fenix-nightly` for
  the newest. It can sit beside `fenix`, which stays on releases; both
  provide the `fenix` command, and `scoop reset fenix` or
  `scoop reset fenix-nightly` picks which. winget takes releases only.

A nightly keeps the same promise about your settings and files as the
release it leads to, but it's had a day's testing, not a few weeks'.
Please report what goes wrong in an
[issue](https://github.com/tpedneault/fenix/issues), with what
`fenix --version` prints.

## For maintainers

### The nightly

[`nightly.yml`](../.github/workflows/nightly.yml) runs every night and
can be run by hand (Actions, Nightly, Run workflow). Its first job,
on Linux and in seconds, compares `master`'s head with the commit the
`nightly` tag points at (and the record in the release's notes, which
must agree): when `master` hasn't moved, nothing else runs. Run by
hand, `force` builds anyway.

Otherwise it builds `master`'s head with
[`build.yml`](../.github/workflows/build.yml), the same build as a
release's, with the version and commit stamped into the binary
(`FENIX_VERSION`, `FENIX_COMMIT`) -- nothing is committed for it.
Then it moves the `nightly` tag to that commit, replaces the release's
files with the new ones, and rewrites its notes: the version, the
commit, the date, a link to what changed since the last nightly, and the
changelog's **Unreleased** section. The release is a pre-release and
never "latest", so the download links and `releases/latest` (what the
update check reads on `stable`) keep pointing at the release.

The decisions it makes (whether to build, the version, the notes) are
[`scripts/nightly.sh`](../scripts/nightly.sh), tested by
`scripts/nightly-test.sh` in CI.

How the packaging handles a pre-release version:

- **The Windows installer** gets `AppVersion=1.6.0-nightly.20261112`
  (shown in Programs and Features) and `NumericVersion=1.6.0.0` for
  Windows' version resource, which takes numbers alone. It's the same
  `AppId` as a release's, and every file is copied whatever its version,
  so either installs over the other.
- **The `.deb` and `.rpm`** are versioned `1.6.0~nightly.20261112` by
  nfpm (a `-` would make `dpkg` read `nightly.20261112` as a revision of
  1.6.0, after it). `build-packages.sh` names the files
  `fenix-nightly_amd64.deb` and `fenix-nightly.x86_64.rpm`, as a `~`
  in a file name isn't kept by GitHub's downloads.
- **Scoop and winget** stay on releases: the bucket's `fenix` manifest
  and the winget manifests (`packaging/winget`) should only ever follow
  `releases/latest` or a `vX.Y.Z` tag, which a nightly never is. The
  bucket's separate `fenix-nightly` manifest follows the `nightly`
  release: its files keep their names, so the bucket's workflow reads
  the version from the record at the end of the release's notes
  (`<!-- fenix-nightly version=... commit=... -->`, which
  `scripts/nightly.sh notes` writes) and the hash from the zip's
  `.sha256`. Changing that record, the zip's name or the folder inside
  it means changing that manifest too.

### Cutting a release

As [CONTRIBUTING.md](../CONTRIBUTING.md) says: move the **Unreleased**
entries in the changelog under the new version, bump `version` in
`Cargo.toml`, merge to `master`, then tag the merge commit `vX.Y.Z` and
push the tag. The [release workflow](../.github/workflows/release.yml)
builds the Windows zip and installer and the Linux archive, `.deb` and
`.rpm` (with `build.yml`, as the nightly does), and publishes the
GitHub release with that version's changelog entry.
[`linux-packages.yml`](../.github/workflows/linux-packages.yml)
rebuilds and checks the Linux packages of a release that's already out.

Before tagging:

- **Check the changelog's headings**: **Added** means a minor.
- **Tag a commit that's been a nightly**: the release notes of the
  `nightly` release say which commit it was built from. A minor should
  have been the nightly for a few days; a patch for a crash or a
  security fix can go out sooner.

Once the release is out, the next nightly is the minor after it
(`1.7.0-nightly...` after 1.6.0), so a nightly is always newer than the
last release and older than the next.
