# Test grammars

`tree-sitter-json.wasm` is tree-sitter-json 0.24.8's grammar compiled to
WebAssembly, as its release ships it: the asset of
https://github.com/tree-sitter/tree-sitter-json/releases/tag/v0.24.8,
unchanged (SHA-256 `d2119fb98d5912719b13f9458574f8608d2d29dfbe45f6be1f860ea1fe2a2405`).
tree-sitter-json is MIT-licensed, Copyright (c) 2014 Max Brunsfeld.

It's committed so the tests need no toolchain. To build one yourself, in a
grammar's folder: `tree-sitter build --wasm` (tree-sitter's CLI, 0.25 or
later, fetches the WASI SDK it compiles with, or uses Docker/emscripten).
Fenix's grammar store is tree-sitter 0.26's: it loads grammars of
language ABI 13 to 15.
