$ErrorActionPreference = "Stop"

if (Test-Path (Join-Path $PSScriptRoot "bin\\kelvin-host.exe")) {
    $RootDir = $PSScriptRoot
} else {
    $RootDir = Split-Path -Parent $PSScriptRoot
}

$KelvinHome = if ($env:KELVIN_HOME) { $env:KELVIN_HOME } else { Join-Path $HOME ".kelvinclaw" }
$PluginHome = if ($env:KELVIN_PLUGIN_HOME) { $env:KELVIN_PLUGIN_HOME } else { Join-Path $KelvinHome "plugins" }
$TrustPolicyPath = if ($env:KELVIN_TRUST_POLICY_PATH) { $env:KELVIN_TRUST_POLICY_PATH } else { Join-Path $KelvinHome "trusted_publishers.json" }
$StateDir = if ($env:KELVIN_STATE_DIR) { $env:KELVIN_STATE_DIR } else { Join-Path $KelvinHome "state" }
$DefaultPrompt = if ($env:KELVIN_DEFAULT_PROMPT) { $env:KELVIN_DEFAULT_PROMPT } else { "What is KelvinClaw?" }
$PluginManifestPath = Join-Path $RootDir "share\\official-first-party-plugins.env"
$EnvSearchPaths = @(
    (Join-Path (Get-Location).Path ".env.local"),
    (Join-Path (Get-Location).Path ".env"),
    (Join-Path $KelvinHome ".env.local"),
    (Join-Path $KelvinHome ".env")
)

function Show-Usage {
@"
Usage: .\kelvin.cmd [kelvin-host args]

Release-bundle launcher for KelvinClaw on Windows.

Behavior:
  - with no args, installs required official plugins on first run
  - starts interactive mode in a terminal
  - falls back to a default prompt when not attached to a console

Environment:
  KELVIN_HOME
  KELVIN_PLUGIN_HOME
  KELVIN_TRUST_POLICY_PATH
  KELVIN_STATE_DIR
  KELVIN_DEFAULT_PROMPT
  OPENAI_API_KEY
"@
}

function Require-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing required command: $Name"
    }
}

function Trim-Value([string]$Value) {
    return $Value.Trim()
}

function Strip-WrappingQuotes([string]$Value) {
    if ($Value.Length -ge 2) {
        if (($Value.StartsWith('"') -and $Value.EndsWith('"')) -or ($Value.StartsWith("'") -and $Value.EndsWith("'"))) {
            return $Value.Substring(1, $Value.Length - 2)
        }
    }
    return $Value
}

function Load-EnvVarFromFile([string]$Key, [string]$FilePath) {
    if (-not (Test-Path $FilePath)) {
        return $null
    }

    foreach ($Line in Get-Content $FilePath) {
        $Stripped = $Line.Split("#")[0].Trim()
        if ([string]::IsNullOrWhiteSpace($Stripped)) {
            continue
        }
        if ($Stripped -match '^export\s+') {
            $Stripped = $Stripped -replace '^export\s+', ''
        }
        if ($Stripped -match "^$Key\s*=\s*(.*)$") {
            return (Strip-WrappingQuotes (Trim-Value $Matches[1]))
        }
    }

    return $null
}

function Load-DotenvDefaults {
    if ($env:OPENAI_API_KEY) {
        return
    }

    foreach ($EnvFile in $EnvSearchPaths) {
        $Value = Load-EnvVarFromFile -Key "OPENAI_API_KEY" -FilePath $EnvFile
        if ($Value) {
            $env:OPENAI_API_KEY = $Value
            return
        }
    }
}

function Prompt-ForOpenAIKey([string[]]$CliArgs) {
    if ($env:OPENAI_API_KEY -or $CliArgs.Length -gt 0) {
        return
    }
    if (-not [Environment]::UserInteractive) {
        return
    }

    Write-Host "[kelvin] OPENAI_API_KEY not found in the environment or .env files."
    $Value = Read-Host "[kelvin] Paste your OpenAI API key for this run, or press Enter to continue with echo mode"
    $Value = Trim-Value $Value
    if ($Value) {
        $env:OPENAI_API_KEY = $Value
    }
}

function Plugin-CurrentVersion([string]$PluginId) {
    $CurrentDir = Join-Path (Join-Path $PluginHome $PluginId) "current"
    $ManifestPath = Join-Path $CurrentDir "plugin.json"
    if (-not (Test-Path $ManifestPath)) {
        return $null
    }

    $Manifest = Get-Content $ManifestPath -Raw | ConvertFrom-Json
    return $Manifest.version
}

function Ensure-TrustPolicy([string]$TrustPolicyUrl) {
    if (Test-Path $TrustPolicyPath) {
        return
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $TrustPolicyPath) | Out-Null
    Write-Host "[kelvin] fetching official trust policy"
    Invoke-WebRequest -Uri $TrustPolicyUrl -OutFile $TrustPolicyPath
}

function Extract-PackageCleanly([string]$TarballPath, [string]$ExtractDir) {
    New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null
    & tar -xzf $TarballPath -C $ExtractDir
}

function Install-OfficialPlugin([string]$PluginId, [string]$Version, [string]$PackageUrl, [string]$ExpectedSha, [string]$TrustPolicyUrl) {
    $CurrentVersion = Plugin-CurrentVersion $PluginId
    $VersionDir = Join-Path (Join-Path $PluginHome $PluginId) $Version
    if ($CurrentVersion -eq $Version -and (Test-Path (Join-Path $VersionDir "plugin.json"))) {
        return
    }

    Write-Host "[kelvin] installing official plugin: $PluginId@$Version"
    Ensure-TrustPolicy $TrustPolicyUrl
    New-Item -ItemType Directory -Force -Path (Join-Path $PluginHome $PluginId) | Out-Null

    $WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    $PackagePath = Join-Path $WorkDir "plugin.tar.gz"
    $ExtractDir = Join-Path $WorkDir "extract"
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null

    Invoke-WebRequest -Uri $PackageUrl -OutFile $PackagePath
    $ActualSha = (Get-FileHash -Algorithm SHA256 -Path $PackagePath).Hash.ToLowerInvariant()
    if ($ActualSha -ne $ExpectedSha.ToLowerInvariant()) {
        throw "Checksum mismatch for $PluginId@$Version"
    }

    Extract-PackageCleanly -TarballPath $PackagePath -ExtractDir $ExtractDir

    if (Test-Path $VersionDir) {
        Remove-Item -Recurse -Force $VersionDir
    }
    New-Item -ItemType Directory -Force -Path $VersionDir | Out-Null
    Copy-Item -Recurse -Force (Join-Path $ExtractDir "*") $VersionDir

    $CurrentDir = Join-Path (Join-Path $PluginHome $PluginId) "current"
    if (Test-Path $CurrentDir) {
        Remove-Item -Recurse -Force $CurrentDir
    }
    New-Item -ItemType Directory -Force -Path $CurrentDir | Out-Null
    Copy-Item -Recurse -Force (Join-Path $VersionDir "*") $CurrentDir

    Remove-Item -Recurse -Force $WorkDir
}

function Load-PluginManifest {
    if (-not (Test-Path $PluginManifestPath)) {
        throw "Release bundle is missing $PluginManifestPath"
    }

    $Values = @{}
    foreach ($Line in Get-Content $PluginManifestPath) {
        $Stripped = $Line.Trim()
        if ([string]::IsNullOrWhiteSpace($Stripped) -or $Stripped.StartsWith("#")) {
            continue
        }
        if ($Stripped -match '^([A-Z0-9_]+)="(.*)"$') {
            $Values[$Matches[1]] = $Matches[2]
        }
    }

    return $Values
}

function Bootstrap-OfficialPlugins {
    Require-Command "tar"
    $Manifest = Load-PluginManifest

    Install-OfficialPlugin `
        -PluginId "kelvin.cli" `
        -Version $Manifest["KELVIN_CLI_VERSION"] `
        -PackageUrl $Manifest["KELVIN_CLI_PACKAGE_URL"] `
        -ExpectedSha $Manifest["KELVIN_CLI_SHA256"] `
        -TrustPolicyUrl $Manifest["OFFICIAL_TRUST_POLICY_URL"]

    if ($env:OPENAI_API_KEY) {
        Install-OfficialPlugin `
            -PluginId "kelvin.openai" `
            -Version $Manifest["KELVIN_OPENAI_VERSION"] `
            -PackageUrl $Manifest["KELVIN_OPENAI_PACKAGE_URL"] `
            -ExpectedSha $Manifest["KELVIN_OPENAI_SHA256"] `
            -TrustPolicyUrl $Manifest["OFFICIAL_TRUST_POLICY_URL"]
    }
}

$CliArgs = $args
if ($CliArgs.Length -gt 0 -and ($CliArgs[0] -eq "-h" -or $CliArgs[0] -eq "--help")) {
    Show-Usage
    exit 0
}

Load-DotenvDefaults
Prompt-ForOpenAIKey $CliArgs
Bootstrap-OfficialPlugins

New-Item -ItemType Directory -Force -Path $StateDir | Out-Null
$env:KELVIN_PLUGIN_HOME = $PluginHome
$env:KELVIN_TRUST_POLICY_PATH = $TrustPolicyPath

$DefaultHostArgs = @()
if ($env:OPENAI_API_KEY) {
    $DefaultHostArgs += @("--model-provider", "kelvin.openai")
}

$HostBinary = Join-Path $RootDir "bin\\kelvin-host.exe"
if ($CliArgs.Length -eq 0) {
    if ([Environment]::UserInteractive) {
        & $HostBinary @DefaultHostArgs --interactive --workspace (Get-Location).Path --state-dir $StateDir
        exit $LASTEXITCODE
    }

    & $HostBinary @DefaultHostArgs --prompt $DefaultPrompt --workspace (Get-Location).Path --state-dir $StateDir
    exit $LASTEXITCODE
}

& $HostBinary @CliArgs
exit $LASTEXITCODE
