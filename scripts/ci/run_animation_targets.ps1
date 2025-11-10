Param(
    [string]$OutputDirectory
)

Set-StrictMode -Version 3
$ErrorActionPreference = "Stop"

$workspaceRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

Push-Location $workspaceRoot
try {
    Write-Host "Running animation target measurement (release-fat profile, ignored test)..." -ForegroundColor Cyan
    cargo test --profile release-fat animation_targets_measure -- --ignored --exact --nocapture
} finally {
    Pop-Location
}

$targetDir = if ($env:CARGO_TARGET_DIR) {
    (Resolve-Path $env:CARGO_TARGET_DIR).Path
} else {
    Join-Path $workspaceRoot "target"
}

$reportPath = Join-Path $targetDir "animation_targets_report.json"
if (-not (Test-Path $reportPath)) {
    throw "Expected animation target report at '$reportPath' but the file was not generated."
}

Write-Host ("Animation target report available at '{0}'." -f $reportPath)

if ($PSBoundParameters.ContainsKey('OutputDirectory') -and $OutputDirectory) {
    $destinationRoot = Resolve-Path $OutputDirectory -ErrorAction SilentlyContinue
    if (-not $destinationRoot) {
        $destinationRoot = New-Item -ItemType Directory -Path $OutputDirectory -Force |
            Select-Object -ExpandProperty FullName
    } else {
        $destinationRoot = $destinationRoot.Path
    }

    $destination = Join-Path $destinationRoot "animation_targets_report.json"
    Copy-Item -Path $reportPath -Destination $destination -Force
    Write-Host ("Copied animation target report to '{0}'." -f $destination)
}
