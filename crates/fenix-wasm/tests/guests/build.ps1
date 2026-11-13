# Builds the test extensions counter.wasm, provider.wasm and plain16.wasm,
# and the model hello-model.glb, which are committed so the tests need no
# wasm toolchain. Needs rustup's 1.98.0 with wasm32-wasip2.
#
# hello.wasm and keys.wasm are kept as they were built, on earlier APIs
# (hello on 1.2's WIT, keys on the 1.5 crate): they show that an extension
# built then still runs. Rebuilt here they'd be this API's, and hello no
# longer builds against it. Build one again only from the commit that last
# changed it (`git log -- crates/fenix-wasm/tests/guests/keys.wasm`).
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
$rustup = Join-Path $env:USERPROFILE ".cargo\bin\rustup.exe"
# That toolchain's own rustc, not one an installer put first on PATH.
$env:RUSTC = & $rustup which rustc --toolchain 1.98.0
foreach ($guest in "counter", "provider", "plain16") {
    Push-Location (Join-Path $here $guest)
    try {
        & $cargo +1.98.0 build --release --target wasm32-wasip2
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Copy-Item "target\wasm32-wasip2\release\$guest.wasm" (Join-Path $here "$guest.wasm") -Force
    } finally {
        Pop-Location
    }
}
Remove-Item Env:RUSTC
# The model hello.model loads.
& $cargo +1.98.0 run --quiet -p fenix-model --example stick -- (Join-Path $here "hello-model.glb")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
