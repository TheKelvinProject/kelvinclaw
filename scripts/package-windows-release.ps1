$ErrorActionPreference = "Stop"

param(
    [Parameter(Mandatory = $true)]
    [string]$Target,
    [string]$OutputDir = "",
    [string]$TargetDir = "",
    [string]$Version = ""
)

$RootDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$OutputDir = if ($OutputDir) { $OutputDir } else { Join-Path $PSScriptRoot "..\\dist\\releases" }
$TargetDir = if ($TargetDir) { $TargetDir } else { Join-Path $PSScriptRoot "..\\target\\releases" }
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)
$TargetDir = [System.IO.Path]::GetFullPath($TargetDir)

function Require-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing required command: $Name"
    }
}

function Workspace-Version {
    $Metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    foreach ($Package in $Metadata.packages) {
        if ($Package.name -eq "kelvin-host") {
            return $Package.version
        }
    }
    throw "Failed to resolve workspace version"
}

function Platform-Label([string]$Triple) {
    switch ($Triple) {
        "x86_64-pc-windows-msvc" { return "windows-x86_64" }
        default { throw "Unsupported target triple: $Triple" }
    }
}

function Sha256-File([string]$Path) {
    return (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLowerInvariant()
}

function Smoke-TestZip([string]$ZipPath, [string]$RootName) {
    $WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    Expand-Archive -Path $ZipPath -DestinationPath $WorkDir

    & (Join-Path $WorkDir "$RootName\\kelvin.cmd") --help | Out-Null
    & (Join-Path $WorkDir "$RootName\\bin\\kelvin-host.exe") --help | Out-Null
    & (Join-Path $WorkDir "$RootName\\bin\\kelvin-gateway.exe") --help | Out-Null
    & (Join-Path $WorkDir "$RootName\\bin\\kelvin-registry.exe") --help | Out-Null
    & (Join-Path $WorkDir "$RootName\\bin\\kelvin-memory-controller.exe") --help | Out-Null

    Remove-Item -Recurse -Force $WorkDir
}

Require-Command "cargo"
Require-Command "rustup"

if (-not $Version) {
    $Version = Workspace-Version
}

$PlatformLabel = Platform-Label $Target
$ArchiveRoot = "kelvinclaw-$Version-$PlatformLabel"
$ArchivePath = Join-Path $OutputDir "$ArchiveRoot.zip"
$ChecksumPath = "$ArchivePath.sha256"
$StageParent = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
$StageRoot = Join-Path $StageParent $ArchiveRoot

try {
    New-Item -ItemType Directory -Force -Path (Join-Path $StageRoot "bin") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $StageRoot "share") | Out-Null
    New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

    rustup target add $Target | Out-Null

    $env:CARGO_TARGET_DIR = $TargetDir
    cargo build --locked --release --target $Target -p kelvin-host
    cargo build --locked --release --target $Target -p kelvin-gateway --features memory_rpc
    cargo build --locked --release --target $Target -p kelvin-registry
    cargo build --locked --release --target $Target -p kelvin-memory-controller

    Copy-Item (Join-Path $TargetDir "$Target\\release\\kelvin-host.exe") (Join-Path $StageRoot "bin\\")
    Copy-Item (Join-Path $TargetDir "$Target\\release\\kelvin-gateway.exe") (Join-Path $StageRoot "bin\\")
    Copy-Item (Join-Path $TargetDir "$Target\\release\\kelvin-registry.exe") (Join-Path $StageRoot "bin\\")
    Copy-Item (Join-Path $TargetDir "$Target\\release\\kelvin-memory-controller.exe") (Join-Path $StageRoot "bin\\")
    Copy-Item (Join-Path $RootDir "LICENSE") $StageRoot
    Copy-Item (Join-Path $RootDir "README.md") $StageRoot
    Copy-Item (Join-Path $RootDir "scripts\\kelvin-release-launcher.ps1") (Join-Path $StageRoot "kelvin.ps1")
    Copy-Item (Join-Path $RootDir "scripts\\kelvin-release-launcher.cmd") (Join-Path $StageRoot "kelvin.cmd")
    Copy-Item (Join-Path $RootDir "release\\official-first-party-plugins.env") (Join-Path $StageRoot "share\\official-first-party-plugins.env")

    $ManifestPath = Join-Path $RootDir "release\\official-first-party-plugins.env"
    $CliVersion = Select-String -Path $ManifestPath -Pattern '^KELVIN_CLI_VERSION="(.+)"$' | ForEach-Object { $_.Matches[0].Groups[1].Value }
    $OpenAIVersion = Select-String -Path $ManifestPath -Pattern '^KELVIN_OPENAI_VERSION="(.+)"$' | ForEach-Object { $_.Matches[0].Groups[1].Value }

    @"
version=$Version
target=$Target
platform=$PlatformLabel
required_plugin=kelvin.cli@$CliVersion
optional_plugin=kelvin.openai@$OpenAIVersion
"@ | Set-Content -NoNewline (Join-Path $StageRoot "BUILD_INFO.txt")

    if (Test-Path $ArchivePath) {
        Remove-Item -Force $ArchivePath
    }
    if (Test-Path $ChecksumPath) {
        Remove-Item -Force $ChecksumPath
    }

    Compress-Archive -Path (Join-Path $StageParent $ArchiveRoot) -DestinationPath $ArchivePath
    "$((Sha256-File $ArchivePath))  $([System.IO.Path]::GetFileName($ArchivePath))" | Set-Content -NoNewline $ChecksumPath
    Smoke-TestZip -ZipPath $ArchivePath -RootName $ArchiveRoot

    Write-Output "archive=$ArchivePath"
    Write-Output "checksum=$ChecksumPath"
}
finally {
    if (Test-Path $StageParent) {
        Remove-Item -Recurse -Force $StageParent
    }
}
