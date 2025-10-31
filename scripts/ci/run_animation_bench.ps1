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

$csvArtifacts = @(
    @{
        Path = Join-Path $targetDir "benchmarks/animation_sprite_timelines.csv"
        Name = "animation_sprite_timelines.csv"
        Label = "Sprite timeline"
    },
    @{
        Path = Join-Path $targetDir "benchmarks/animation_transform_clips.csv"
        Name = "animation_transform_clips.csv"
        Label = "Transform clip"
    }
)

foreach ($artifact in $csvArtifacts) {
    if (-not (Test-Path $artifact.Path)) {
        throw "Expected benchmark CSV at '$($artifact.Path)' for $($artifact.Label) results but the file was not generated."
    }
}

foreach ($artifact in $csvArtifacts) {
    Write-Host ("{0} benchmark CSV available at '{1}'." -f $artifact.Label, $artifact.Path)
}

if ($PSBoundParameters.ContainsKey('OutputDirectory') -and $OutputDirectory) {
    $destinationRoot = Resolve-Path $OutputDirectory -ErrorAction SilentlyContinue
    if (-not $destinationRoot) {
        $destinationRoot = New-Item -ItemType Directory -Path $OutputDirectory -Force |
            Select-Object -ExpandProperty FullName
    } else {
        $destinationRoot = $destinationRoot.Path
    }

    foreach ($artifact in $csvArtifacts) {
        $destinationCsv = Join-Path $destinationRoot $artifact.Name
        Copy-Item -Path $artifact.Path -Destination $destinationCsv -Force
        Write-Host ("Copied {0} CSV to '{1}'." -f $artifact.Label, $destinationCsv)
    }
}

function Test-CsvBudget {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    $csv = Import-Csv -Path $Path
    $failedRows = @()
    foreach ($row in $csv) {
        if ($row.meets_budget -and $row.meets_budget.Trim().ToLowerInvariant() -eq "fail") {
            $failedRows += $row
        }
    }

    if ($failedRows.Count -gt 0) {
        Write-Host ("Budget violations detected in {0} benchmark:" -f $Label) -ForegroundColor Red
        foreach ($row in $failedRows) {
            Write-Host ("  animators={0} mean_step_ms={1} budget_ms={2}" -f $row.animators, $row.mean_step_ms, $row.budget_ms)
        }
        throw ("{0} benchmark exceeded configured budgets." -f $Label)
    }

    Write-Host ("{0} benchmark completed within configured budgets." -f $Label) -ForegroundColor Green
}

foreach ($artifact in $csvArtifacts) {
    Test-CsvBudget -Path $artifact.Path -Label $artifact.Label
}
