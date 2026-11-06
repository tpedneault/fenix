# Continuous integration

The `CI` workflow runs on pushes to branches, pull requests from forks
(a branch of this repository's was already checked by its push), and
manual dispatch. No secrets
or live services are required. Actions are pinned to commit hashes and the token
has read-only repository access. Dependency resolution uses `Cargo.lock` through
`--locked`. The compiler follows Rust stable; each run records the exact compiler
and Cargo versions so toolchain changes can be diagnosed.

## Gates

| Check | Coverage |
| --- | --- |
| Windows workspace | Full workspace tests, strict Clippy for the whole workspace, and a debug GUI executable build on Windows Server 2022/MSVC. No package or test-name exclusions. |
| Linux reliability | Storage, core, configuration, project tool settings, recovery, task process groups, RPC framing, the control channel (Unix socket, private directory, protocol), LSP edits, DAP, and Vim tests; strict Clippy for the crates it tests that build without a display. No GUI/system-display dependency. |

The Windows checkout path contains a space. Storage tests also exercise Unicode
paths with spaces, CRLF byte preservation, failed serialization, no-clobber
creation, read-only files, and Windows sharing violations. Process tests cover
cancel/drop, descendant cleanup, bounded output, and stale run events. Protocol
tests cover framing and malformed messages, UTF-16 edit ranges, versions, and
transaction rejection. GUI tests cover recovery and multi-file refactor apply/undo.

Tests marked ignored still need their documented external services or manual
environment. Rust subprocess fixtures are also marked ignored, but their parent
tests explicitly execute them. Do not pass `--include-ignored` to this workflow:
some fixtures intentionally sleep or produce unbounded output.

## Speed

The Windows gates run as two jobs at once: `Tests`, and `Lint` (Clippy,
then the debug build). Both jobs and the Linux one cache the compiled
dependencies (`Swatinem/rust-cache`); pushes write the cache, pull
requests only read it, and a branch's first run reads master's. CI
builds with line tables rather than full debug info
(`CARGO_PROFILE_DEV_DEBUG`) and without incremental artifacts. It
links with `link.exe`, not `rust-lld`: lld links tree-sitter's C wasm
store to wasmtime's C API as if from a DLL, and loading a wasm grammar
then crashes.

## Run locally

Install Rust with Clippy, Git, PowerShell 7, and [Universal Ctags](https://ctags.io/)
(the Tcl completion tests use it). On Windows, install Visual Studio C++ Build
Tools and a Windows SDK. From the repository root:

```powershell
pwsh -NoProfile -File ./scripts/ci.ps1 -Suite Workspace
pwsh -NoProfile -File ./scripts/ci.ps1 -Suite Reliability
```

`-Gates Tests` or `-Gates Lint` runs one half, as CI's jobs do.

The script uses the active local Rust toolchain and returns failure if any gate
fails. Each invocation gets a unique directory under `target/ci/`, with command
logs and `summary.json` containing exit codes and timings. CI retains diagnostic
logs even on failure. Successful Windows runs also retain `fenix.exe` for seven
days; this is a debug build, not a release.

## Limits

The gates don't cover interactive GUI behavior, or the live integration
tests that need a service (GitLab, Jira, a language server); those are
checked by hand. Whole-workspace `rustfmt` isn't a gate yet; new code
is formatted with it, and the rest is reformatted as it's touched.

Releases are built by a separate workflow, `release.yml`, when a `v*`
tag is pushed, or when it's run by hand on `master` with the tag (Actions,
Release, Run workflow); publishing the release then makes the tag. It
builds the Windows zip and installer (`packaging/windows/fenix.iss`,
Inno Setup 6), and the Linux archive with its `.deb` and `.rpm`
(`packaging/linux`), each with a `.sha256`. Either way, the tag must match
the workspace version in `Cargo.toml`, and `CHANGELOG.md` must have an
entry for it.

Action references: [checkout](https://github.com/actions/checkout) and
[upload-artifact](https://github.com/actions/upload-artifact).
