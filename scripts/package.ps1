# Builds the release binaries and zips them as a portable package: dist\SonarLite-<version>-portable.zip
# Usage (from the repository root):  powershell -File scripts\package.ps1
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'build failed' }

$name = "SonarLite-$version-portable"
$stage = "dist\$name"
Remove-Item dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\sonar-lite.exe, target\release\sonar-lite-cli.exe, README.md, LICENSE $stage
Compress-Archive -Path $stage -DestinationPath "dist\$name.zip"
$zip = Get-Item "dist\$name.zip"
"{0}  ({1:N1} MB)" -f $zip.FullName, ($zip.Length / 1MB)
