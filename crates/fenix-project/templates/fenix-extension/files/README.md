# {{name}}

{{description}}.

An extension for [Fenix](https://github.com/tpedneault/fenix) that runs
code: written in Rust with `fenix-extension-api`, built for WebAssembly,
and run by Fenix in a sandbox.

## Building

It needs Rust's WebAssembly target, once:

```text
rustup target add wasm32-wasip2
```

Then, from Fenix (`SPC p t`) or a terminal:

```text
cargo build --release --target wasm32-wasip2   # build
pwsh -File package.ps1                         # dist/<id>/ and a .fenixext
pwsh -File package.ps1 -Install                # and install it into Fenix
```

Fenix uses it from its next start: `SPC :` then "{{name}}" finds its
commands, and `SPC h l` shows what it logs.

## What it may do

`extension.toml`'s `[permissions]` says what it may do beyond its own
folder: programs by name, HTTP hosts, secrets, the project's files.
Fenix holds it to that, and shows it before anyone installs it.
