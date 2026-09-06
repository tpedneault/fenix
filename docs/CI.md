# Continuous integration

The `CI` workflow runs on pushes, pull requests, and manual dispatch. No secrets
or live services are required. Actions are pinned to commit hashes and the token
has read-only repository access. Dependency resolution uses `Cargo.lock` through
`--locked`. The compiler follows Rust stable; each run records the exact compiler
and Cargo versions so toolchain changes can be diagnosed.

## Gates

| Check | Coverage |
| --- | --- |
| Windows workspace | Full workspace tests, strict Clippy for storage/tasks/LSP/project, and a debug GUI executable build on Windows Server 2022/MSVC. No package or test-name exclusions. |
| Linux reliability | Storage, core, configuration, project tool settings, recovery, task process groups, RPC framing, LSP edits, DAP, and Vim tests; the same scoped Clippy gate. No GUI/system-display dependency. |

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

## Run locally

Install Rust with Clippy, Git, and PowerShell 7. On Windows, install Visual Studio
C++ Build Tools and a Windows SDK. From the repository root:

```powershell
pwsh -NoProfile -File ./scripts/ci.ps1 -Suite Workspace
pwsh -NoProfile -File ./scripts/ci.ps1 -Suite Reliability
```

The script uses the active local Rust toolchain and returns failure if any gate
fails. Each invocation gets a unique directory under `target/ci/`, with command
logs and `summary.json` containing exit codes and timings. CI retains diagnostic
logs even on failure. Successful Windows runs also retain `fenix.exe` for seven
days; this is a debug build, not a signed release or an installer.

## Repository setup and limits

After the workflow is pushed and has run, configure the repository ruleset to
require **Windows workspace** and **Linux reliability** before merging. Workflow
files alone cannot enforce branch protection. These settings are not changed by
the local implementation.

Hosted jobs have not been run merely by creating these files. Linux execution,
interactive GUI behavior, packaging, and live integration smoke tests remain
separate validation. Whole-workspace formatting and strict Clippy are not gates
yet because the existing repository has unrelated formatting/lint debt.

Action references: [checkout](https://github.com/actions/checkout) and
[upload-artifact](https://github.com/actions/upload-artifact).
