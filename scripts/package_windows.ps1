param(
    [string]$OutputDir = "dist\\uaedb-portable",
    [string]$Python = "python",
    [string]$XdeltaPath = ""
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path "$PSScriptRoot\\.."
Set-Location $root

if (-not $XdeltaPath) {
    $XdeltaPath = Resolve-Path "$root\\..\\xdelta\\xdelta3\\xdelta3.exe"
}
if (-not (Test-Path $XdeltaPath)) {
    throw "xdelta3.exe not found: $XdeltaPath"
}

function Find-LicensePath {
    param([string]$StartDir)

    $dir = Resolve-Path $StartDir
    while ($true) {
        $candidate = Join-Path $dir "LICENSE"
        if (Test-Path $candidate) {
            return $candidate
        }
        $parent = Split-Path -Parent $dir
        if (-not $parent -or $parent -eq $dir) {
            break
        }
        $dir = $parent
    }
    return $null
}

Write-Host "Building UAEDB..."
cargo build --release

if (Test-Path $OutputDir) {
    Remove-Item $OutputDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
New-Item -ItemType Directory -Force -Path "$OutputDir\\runtime\\xdelta" | Out-Null
New-Item -ItemType Directory -Force -Path "$OutputDir\\docs" | Out-Null
New-Item -ItemType Directory -Force -Path "$OutputDir\\licenses" | Out-Null

Copy-Item "$root\\target\\release\\uaedb.exe" "$OutputDir\\uaedb.exe"
Copy-Item "$root\\docs\\USAGE.md" "$OutputDir\\docs\\USAGE.md"
Copy-Item "$root\\docs\\USAGE_KO.md" "$OutputDir\\docs\\USAGE_KO.md"

Write-Host "Copying xdelta3.exe..."
Copy-Item "$XdeltaPath" "$OutputDir\\runtime\\xdelta\\xdelta3.exe"

$xdeltaDir = Split-Path -Parent $XdeltaPath
$xdeltaLicense = Find-LicensePath $xdeltaDir
if (-not $xdeltaLicense) {
    throw "LICENSE not found for xdelta starting from: $xdeltaDir"
}

$pydepsDir = Join-Path $env:TEMP "uaedb-pydeps"
if (Test-Path $pydepsDir) {
    Remove-Item $pydepsDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $pydepsDir | Out-Null

Write-Host "Collecting licenses..."
& $Python "$root\\scripts\\collect_licenses.py" --pydeps "$pydepsDir" --out "$OutputDir\\licenses" --include "$xdeltaLicense"

Remove-Item $pydepsDir -Recurse -Force

Write-Host "Creating zip archive..."
$zipPath = "$OutputDir.zip"
if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}
Compress-Archive -Path "$OutputDir\\*" -DestinationPath $zipPath

Write-Host "Done: $zipPath"
