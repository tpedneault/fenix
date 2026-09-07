#requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Workspace', 'Reliability')]
    [string] $Suite = 'Workspace'
)

$ErrorActionPreference = 'Stop'
# Native exit codes are checked explicitly, including after Tee-Object.
$PSNativeCommandUseErrorActionPreference = $false
$repoRoot = Split-Path $PSScriptRoot -Parent
$runName = '{0}-{1}-{2}' -f $Suite.ToLowerInvariant(), (Get-Date -Format 'yyyyMMdd-HHmmss'), ([guid]::NewGuid().ToString('N').Substring(0, 8))
$logRoot = Join-Path $repoRoot "target/ci/$runName"
New-Item -ItemType Directory -Path $logRoot -Force | Out-Null
$results = [System.Collections.Generic.List[object]]::new()

function Invoke-Gate {
    param([string] $Name, [string] $Program, [string[]] $Arguments)
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $log = Join-Path $logRoot "$Name.log"
    Write-Host "Running $Name`: $Program $($Arguments -join ' ')"
    $code = 1
    try {
        & $Program @Arguments 2>&1 | Tee-Object -FilePath $log
        $code = $LASTEXITCODE
        if ($code -ne 0) { throw "$Name failed (exit $code). See $log" }
    } finally {
        $results.Add([ordered]@{ name = $Name; exitCode = $code; seconds = [math]::Round($timer.Elapsed.TotalSeconds, 2); log = "$Name.log" })
    }
}

Push-Location $repoRoot
try {
    Invoke-Gate 'rustc' 'rustc' @('--version', '--verbose')
    Invoke-Gate 'cargo' 'cargo' @('--version')
    # --locked prevents CI from silently choosing a different dependency graph.
    $testArgs = @('test', '--locked', '--no-fail-fast')
    if ($Suite -eq 'Workspace') {
        $testArgs += '--workspace'
    } else {
        # No display, container engine, network service, or language server needed.
        foreach ($package in @('fenix-storage', 'fenix-core', 'fenix-snippets', 'fenix-project', 'fenix-config', 'fenix-recovery', 'fenix-tasks', 'fenix-rpc', 'fenix-lsp', 'fenix-dap', 'fenix-vim')) {
            $testArgs += @('-p', $package)
        }
    }
    Invoke-Gate 'tests' 'cargo' $testArgs
    # Start strict linting with the newly hardened libraries. The rest of the
    # repository has existing warnings; it remains covered by tests and build.
    Invoke-Gate 'clippy' 'cargo' @('clippy', '--locked', '-p', 'fenix-storage', '-p', 'fenix-tasks', '-p', 'fenix-lsp', '-p', 'fenix-project', '--all-targets', '--', '-D', 'warnings')
    if ($Suite -eq 'Workspace') {
        Invoke-Gate 'build' 'cargo' @('build', '--locked', '-p', 'fenix-gui')
    }
    Write-Host "All $Suite gates passed. Logs: $logRoot"
} finally {
    $results | ConvertTo-Json -Depth 4 -AsArray | Set-Content -LiteralPath (Join-Path $logRoot 'summary.json') -Encoding utf8
    Pop-Location
}
