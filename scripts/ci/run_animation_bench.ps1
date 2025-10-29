Param(
    [string]$OutputDirectory
)

Set-StrictMode -Version 3
$ErrorActionPreference = "Stop"

$workspaceRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))

Push-Location $workspaceRoot
try {
    Write-Host "Running animation benchmark harness (release build, ignored test)..." -ForegroundColor Cyan
    cargo test --release animation_bench_run -- --ignored --exact --nocapture
} finally {
    Pop-Location
}

$targetDir = if ($env:CARGO_TARGET_DIR) {
    (Resolve-Path $env:CARGO_TARGET_DIR).Path
} else {
    Join-Path $workspaceRoot "target"
}

if (-not (Test-Path $targetDir)) {
    throw "Cargo target directory '$targetDir' not found after benchmark run."
}

$csvSource = Join-Path $targetDir "benchmarks/animation_sprite_timelines.csv"
if (-not (Test-Path $csvSource)) {
    throw "Expected benchmark CSV at '$csvSource' but the file was not generated."
}

Write-Host "Animation benchmark CSV available at '$csvSource'."

if ($PSBoundParameters.ContainsKey('OutputDirectory') -and $OutputDirectory) {
    $destinationRoot = Resolve-Path $OutputDirectory -ErrorAction SilentlyContinue
    if (-not $destinationRoot) {
        $destinationRoot = New-Item -ItemType Directory -Path $OutputDirectory -Force |
            Select-Object -ExpandProperty FullName
    } else {
        $destinationRoot = $destinationRoot.Path
    }

    $destinationCsv = Join-Path $destinationRoot "animation_sprite_timelines.csv"
    Copy-Item -Path $csvSource -Destination $destinationCsv -Force
    Write-Host "Copied benchmark CSV to '$destinationCsv'."
}

$csv = Import-Csv -Path $csvSource
$failedRows = @()
foreach ($row in $csv) {
    if ($row.meets_budget -and $row.meets_budget.Trim().ToLowerInvariant() -eq "fail") {
        $failedRows += $row
    }
}

if ($failedRows.Count -gt 0) {
    Write-Host "Budget violations detected in animation benchmark:" -ForegroundColor Red
    foreach ($row in $failedRows) {
        Write-Host ("  animators={0} mean_step_ms={1} budget_ms={2}" -f $row.animators, $row.mean_step_ms, $row.budget_ms)
    }
    throw "Animation benchmark exceeded configured budgets."
}

Write-Host "Animation benchmark completed within configured budgets." -ForegroundColor Green
