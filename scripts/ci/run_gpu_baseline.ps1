Param(
    [string]$Output = "perf/gpu_baseline_ci.json",
    [string]$Baseline = ""
)

Set-StrictMode -Version 3
$ErrorActionPreference = "Stop"

$workspaceRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Push-Location $workspaceRoot
try {
    Write-Host "Running gpu_baseline..." -ForegroundColor Cyan
    $args = @("--output", $Output, "--frames", "240")
    if ($Baseline) {
        $args += @("--baseline", $Baseline)
    }
    cargo run --bin gpu_baseline -- @args
} finally {
    Pop-Location
}
