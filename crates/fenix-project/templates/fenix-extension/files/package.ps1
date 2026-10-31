# Builds the extension and packs it: dist/<id>/ (the folder Fenix
# installs) and dist/<id>-<version>.fenixext. -Install installs it too.
param([switch] $Install)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
cargo build --release --target wasm32-wasip2
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$manifest = Get-Content -Raw extension.toml
$id = [regex]::Match($manifest, '(?m)^id\s*=\s*"([^"]+)"').Groups[1].Value
$version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
$out = Join-Path 'dist' $id
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item extension.toml, README.md $out
if (Test-Path CHANGELOG.md) { Copy-Item CHANGELOG.md $out }
Copy-Item "target/wasm32-wasip2/release/{{name_snake}}.wasm" (Join-Path $out 'extension.wasm')
$package = Join-Path 'dist' "$id-$version.fenixext"
Compress-Archive -Path (Join-Path $out '*') -DestinationPath "$package.zip" -Force
Move-Item -Force "$package.zip" $package
"Packed $package"
if ($Install) {
    fenix --console --install-extension $out
}
