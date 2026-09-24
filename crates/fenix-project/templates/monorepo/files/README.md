# {{name}}

A monorepo.

{{#if workspaces == cargo}}
- `crates/` -- Rust crates, one Cargo workspace. `cargo new crates/<name>`
  adds one to it.
{{/if}}
{{#if workspaces == uv}}
- `python/` -- Python packages, one uv workspace sharing a `.venv`.
  `uv init python/<name>` adds one.
{{/if}}
{{#if workspaces == npm}}
- `packages/` -- JavaScript packages, one npm workspace.
{{/if}}

In Fenix, `SPC p c` with the folder set inside this repository creates a
project that joins the right workspace (and doesn't start a second git);
`SPC p p` lists everything in here under it.
