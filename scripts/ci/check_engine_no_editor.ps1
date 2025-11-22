$ErrorActionPreference = "Stop"

Write-Host "Checking kestrel_engine without editor features..."

cargo check -p kestrel_engine --no-default-features @args
