# How releases work

Changes land on `master` continuously, each with its tests and a line
under **Unreleased** in the [changelog](../CHANGELOG.md). A release is
cut when enough has gathered there, never on a timer alone. This page
says what each kind of release may carry, how often they come, and how
to try what's next before it's released.

> **Draft.** The version rules below are the ones
> [CONTRIBUTING.md](../CONTRIBUTING.md) already states. The cadence and
> the pre-release channel are a proposal for the maintainer to decide;
> until then, releases go out as they always have. The update check
> already understands pre-releases (see
> [For users: trying pre-releases](#for-users-trying-pre-releases)); the
> release workflow doesn't publish them yet
> ([what it would take](#what-the-release-workflow-would-need)).

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

Proposed, for the maintainer to decide:

- **Minor releases about every four weeks**, when there's something worth
  having. If **Unreleased** holds only fixes at that point, a patch goes
  out instead.
- **Patch releases when a fix matters**, batched: at most one a week,
  except for a crash, lost work or a security fix, which goes out as
  soon as it's ready.
- **A pre-release a week before each minor**: `v1.6.0-rc.1`, then
  `-rc.2` if something needed fixing. The minor is the last
  pre-release's commit with only the version changed.
- **A major release only with a migration** and a pre-release at least
  two weeks ahead.

A steady rhythm tells people what to expect: an upgrade a month that
keeps their settings, fixes in between when they need them.

## For users: trying pre-releases

Turn on `updates.check` (Home tells you when a newer release is out)
and `updates.prereleases` (it tells you about pre-releases too). Then:

- Home says *Fenix 1.6.0-rc.1 (a pre-release) is out* when there's one
  newer than yours, and the release once it's out.
- Nothing is downloaded or installed for you: get it from the
  [releases page](https://github.com/tpedneault/fenix/releases), where
  GitHub marks it as a pre-release.
- Turn `updates.prereleases` off again and Home goes back to releases
  alone, at once. A pre-release is upgraded by its release like any
  other version.

A pre-release keeps the same promise about your settings and files as
the release it leads to. Please report what goes wrong in an
[issue](https://github.com/tpedneault/fenix/issues), saying which
pre-release you're running.

## For maintainers: cutting a release

As [CONTRIBUTING.md](../CONTRIBUTING.md) says: move the **Unreleased**
entries in the changelog under the new version, bump `version` in
`Cargo.toml`, merge to `master`, then tag the merge commit `vX.Y.Z` and
push the tag. The [release workflow](../.github/workflows/release.yml)
builds the Windows zip and installer and the Linux archive, `.deb` and
`.rpm`, and publishes the GitHub release with that version's changelog
entry. [`linux-packages.yml`](../.github/workflows/linux-packages.yml)
rebuilds and checks the Linux packages of a release that's already out.

Before tagging, check the changelog's headings: **Added** means a
minor.

### What the release workflow would need

For pre-releases, these are the changes the workflow and packaging
would need. None has been made: each one changes how a release is
built, so they're left for the maintainer to make and try.

1. **The tag.** A pushed `v1.6.0-rc.1` already matches the workflow's
   `v*.*.*`. Run by hand, the `A new tag` step rejects it: its pattern
   would become `^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$`.
2. **The version check** compares the tag with `Cargo.toml`, so the
   release branch carries `version = "1.6.0-rc.1"` for the
   pre-release. Fenix parses that as a version already.
3. **The notes.** The workflow reads `## [1.6.0-rc.1]` from the
   changelog. Rather than add headings for pre-releases, a pre-release
   tag could take the **Unreleased** section's text.
4. **Publishing.** The last step runs `gh release edit --latest`. For a
   pre-release it should be `--prerelease` and not `--latest`, so the
   download links and the update check's `releases/latest` keep pointing
   at the release.
5. **The Windows installer.** `VersionInfoVersion` in
   `packaging/windows/fenix.iss` has to be numbers only (`1.6.0.0`), so
   it would need a value of its own; `AppVersion` can say `1.6.0-rc.1`.
6. **The Linux packages.** A `.deb` or `.rpm` must sort a pre-release
   before its release: `1.6.0~rc.1`, not `1.6.0-rc.1` (which `dpkg`
   reads as revision `rc.1` of 1.6.0). Check what nfpm makes of the
   version `build-packages.sh` hands it.
7. **Other channels** (the Scoop bucket, winget) should take releases
   only.
