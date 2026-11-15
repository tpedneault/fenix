# Builds the test extensions provider.wasm, plain16.wasm and callers.wasm,
# and the model hello-model.glb, which are committed so the
# tests need no wasm toolchain. Needs rustup's $toolchain (below) with
# wasm32-wasip2: the one the committed guests were built with.
#
# hello.wasm, counter.wasm and keys.wasm are kept as they were built, on
# earlier APIs (hello on 1.2's WIT, counter on the 1.0 crate -- its
# Cargo.lock is 1.0's -- keys on the 1.5 crate): they show that an extension
# built then still runs. Rebuilt here they'd be this API's, and hello no
# longer builds against it. Build one again only from the commit that last
# changed it (`git log -- crates/fenix-wasm/tests/guests/keys.wasm`).
$toolchain = "1.100.0"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
$rustup = Join-Path $env:USERPROFILE ".cargo\bin\rustup.exe"
# That toolchain's own rustc, not one an installer put first on PATH.
$env:RUSTC = & $rustup which rustc --toolchain $toolchain
foreach ($guest in "provider", "plain16", "callers") {
    Push-Location (Join-Path $here $guest)
    try {
        & $cargo +$toolchain build --release --target wasm32-wasip2
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Copy-Item "target\wasm32-wasip2\release\$guest.wasm" (Join-Path $here "$guest.wasm") -Force
    } finally {
        Pop-Location
    }
}
Remove-Item Env:RUSTC
# The model hello.model loads.
& $cargo +$toolchain run --quiet -p fenix-model --example stick -- (Join-Path $here "hello-model.glb")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
