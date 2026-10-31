# Builds the test extensions to hello.wasm and counter.wasm, and the
# model hello-model.glb, which are
# committed so the tests need no wasm toolchain. Needs rustup's 1.98.0
# with wasm32-wasip2.
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
$rustup = Join-Path $env:USERPROFILE ".cargo\bin\rustup.exe"
# That toolchain's own rustc, not one an installer put first on PATH.
$env:RUSTC = & $rustup which rustc --toolchain 1.98.0
foreach ($guest in "hello", "counter") {
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
