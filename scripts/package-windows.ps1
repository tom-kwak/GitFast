# Builds gf.exe and a .zip for it into dist/.
#
#   scripts/package-windows.ps1 [-Version 0.1.0]
#
# The output names are fixed (dist/gf-windows-x64.zip and its .sha256), so
# .../releases/latest/download/gf-windows-x64.zip always gets the newest
# build (scripts/install.ps1 relies on it). The exe is not code-signed.
[CmdletBinding()]
param(
    # Must match Cargo.toml's workspace version (default: that version).
    [string]$Version
)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$crate = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $Version) { $Version = $crate }
if ($Version -ne $crate) {
    throw "version $Version does not match Cargo.toml ($crate): bump it, or use $crate"
}

$target = 'x86_64-pc-windows-msvc'
$name = 'gf-windows-x64'
cargo build --release --locked -p gf-app --target $target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

$dist = 'dist'
$stage = Join-Path $dist $name
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item "target/$target/release/gf.exe" $stage
Copy-Item README.md $stage
Get-ChildItem -Path . -Filter 'LICENSE*' | Copy-Item -Destination $stage
Set-Content -Path (Join-Path $stage 'VERSION') -Value $Version -NoNewline

$zip = Join-Path $dist "$name.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
# `sha256sum` format: "<hash>  <file name>".
Set-Content -Path "$zip.sha256" -Value "$hash  $name.zip" -NoNewline -Encoding ascii
Remove-Item -Recurse -Force $stage
Write-Output (Resolve-Path $zip).Path
