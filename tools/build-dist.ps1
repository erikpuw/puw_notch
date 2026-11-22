$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $toml = Get-Content (Join-Path $root 'Cargo.toml') -Raw
    if ($toml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
        throw 'Cannot read version from Cargo.toml'
    }
    $ver = $Matches[1]

    Write-Host "Building puw_notch v$ver (release)..."
    cargo build --release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "Building puw_notch_setup v$ver (release)..."
    cargo build --release -p puw_notch_setup
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null

    $setup = Join-Path $dist "puw-notch-setup-v$ver-x64.exe"
    Copy-Item -Force (Join-Path $root 'target\release\puw_notch_setup.exe') $setup

    Get-Item $setup | Select-Object Name, Length, @{Name='KB';Expression={[math]::Round($_.Length/1024,1)}} | Format-Table -AutoSize
} finally {
    Pop-Location
}
