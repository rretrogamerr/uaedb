param(
    [string]$OutputDir = "dist\\uaedb-portable",
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

function Sanitize-Name {
    param([string]$Name)
    return [regex]::Replace($Name, "[^A-Za-z0-9._-]+", "_")
}

function Write-ThirdPartyNotices {
    param(
        [string]$LicenseDir,
        [array]$Includes
    )

    $lines = @(
        "# Third-Party Notices",
        ""
    )

    foreach ($item in $Includes) {
        $path = $item.Path
        $name = $item.Name
        if (-not $path -or -not (Test-Path $path)) {
            continue
        }
        $base = Split-Path -Leaf $path
        $targetName = Sanitize-Name "$name-$base"
        $targetPath = Join-Path $LicenseDir $targetName
        Copy-Item $path $targetPath -Force
        $lines += "- $name - $targetName"
    }

    $summaryPath = Join-Path $LicenseDir "THIRD_PARTY_NOTICES.md"
    Set-Content -Path $summaryPath -Value $lines -Encoding utf8
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
Copy-Item "$root\\patch.bat" "$OutputDir\\patch.bat"
Copy-Item "$root\\docs\\USAGE.md" "$OutputDir\\docs\\USAGE.md"
Copy-Item "$root\\docs\\USAGE_KO.md" "$OutputDir\\docs\\USAGE_KO.md"

Write-Host "Copying xdelta3.exe..."
Copy-Item "$XdeltaPath" "$OutputDir\\runtime\\xdelta\\xdelta3.exe"

$xdeltaDir = Split-Path -Parent $XdeltaPath
$xdeltaLicense = Find-LicensePath $xdeltaDir
if (-not $xdeltaLicense) {
    throw "LICENSE not found for xdelta starting from: $xdeltaDir"
}

Write-Host "Collecting licenses..."
$includes = @(
    @{ Name = "xdelta"; Path = $xdeltaLicense }
)
Write-ThirdPartyNotices -LicenseDir "$OutputDir\\licenses" -Includes $includes

Write-Host "Creating zip archive..."
$zipPath = "$OutputDir.zip"
if (Test-Path $zipPath) {
    Remove-Item $zipPath -Force
}
Compress-Archive -Path "$OutputDir\\*" -DestinationPath $zipPath

Write-Host "Done: $zipPath"
