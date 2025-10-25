param(
    [switch]$Release
)

$manifestPath = Join-Path $PSScriptRoot '..\config\plugins.json'
if (!(Test-Path $manifestPath)) {
    Write-Error "Manifest not found at $manifestPath"
    exit 1
}

try {
    $manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
} catch {
    Write-Error "Failed to parse manifest: $_"
    exit 1
}

if (-not $manifest.plugins) {
    Write-Host "No plugins listed"
    exit 0
}

foreach ($plugin in $manifest.plugins) {
    if (-not $plugin.enabled) {
        Write-Host "[skip] $($plugin.name) disabled"
        continue
    }
    if (-not $plugin.path) {
        Write-Warning "[skip] $($plugin.name) missing path"
        continue
    }

    $artifact = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\$($plugin.path)"))
    $normalized = $artifact.Replace('\', '/').ToLower()
    $marker = '/target/'
    $idx = $normalized.IndexOf($marker)
    if ($idx -lt 0) {
        Write-Warning "[skip] $($plugin.name) failed to infer crate root from $artifact"
        continue
    }
    $crateDir = $artifact.Substring(0, $idx)
    $cargoToml = Join-Path $crateDir 'Cargo.toml'
    if (!(Test-Path $cargoToml)) {
        Write-Warning "[skip] $($plugin.name) Cargo.toml not found at $cargoToml"
        continue
    }
    $args = @('build', '--manifest-path', $cargoToml)
    if ($Release) { $args += '--release' }
    Write-Host "[build] $($plugin.name) -> $crateDir $($Release ? 'release' : 'debug')"
    $proc = Start-Process cargo -ArgumentList $args -WorkingDirectory $crateDir -NoNewWindow -PassThru -Wait
    if ($proc.ExitCode -ne 0) {
        Write-Error "Cargo build failed for $($plugin.name)"
        exit $proc.ExitCode
    }
}

Write-Host "Plugin builds complete."
