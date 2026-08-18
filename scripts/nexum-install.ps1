<#
.SYNOPSIS
    Install a Nexum InstalledLayoutV1 artifact on Windows.

.DESCRIPTION
    Installs Nexum into a user-writable prefix without requiring a checkout or
    administrative rights. The artifact can be a directory produced by
    `nexum-package` (Linux tooling) or a ZIP archive containing the same layout.

    Layout installed (InstalledLayoutV1):
        <Prefix>/lib/nexum/<version>/...   versioned runtime (MANIFEST.json)
        <Prefix>/lib/nexum/current         symlink to the active version
        <Prefix>/bin/nexum.cmd             command shim (no admin needed)
        <Prefix>/bin/nexum-acp-host.cmd
        <Prefix>/bin/nexum-autologin-reconcile.cmd

    PATH: <Prefix>/bin is appended to the user PATH (HKCU\Environment) when
    -AddToPath is set.

.EXAMPLE
    .\scripts\nexum-install.ps1 -Artifact C:\dist\nexum-0.1.4-rc.4-linux-x86_64.zip -AddToPath
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Artifact,

    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'nexum'),

    [switch]$AddToPath,

    [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Get-NexumTempDir {
    $temp = Join-Path ([System.IO.Path]::GetTempPath()) ('nexum-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temp | Out-Null
    return $temp
}

function Get-NexumVersionFromManifest {
    param([string]$ManifestPath)
    $manifest = Get-Content -Raw -Path $ManifestPath | ConvertFrom-Json
    if (-not $manifest.layout) { throw "invalid manifest: missing layout field" }
    if ($manifest.layout -ne 'InstalledLayoutV1') { throw "unsupported layout: $($manifest.layout)" }
    return $manifest.slot
}

function Assert-NexumManifest {
    param([string]$VersionRoot)
    $manifestPath = Join-Path $VersionRoot 'MANIFEST.json'
    if (-not (Test-Path $manifestPath)) { throw "manifest not found: $manifestPath" }
    $version = Get-NexumVersionFromManifest $manifestPath
    if ((Split-Path -Leaf $VersionRoot) -ne $version) {
        throw "manifest slot '$version' does not match directory name '$(Split-Path -Leaf $VersionRoot)'"
    }
    foreach ($payload in @('nexum', 'nexum-acp-host', 'provider-catalog-output.json', 'provider-route-registry.json', 'provider-catalog-base.json', 'catalog-contract.json', 'reserved-models.json')) {
        if (-not (Test-Path (Join-Path $VersionRoot $payload))) {
            throw "required payload missing: $payload"
        }
    }
    return $version
}

function Expand-NexumArtifact {
    param([string]$ArtifactPath)
    $temp = Get-NexumTempDir
    if ((Get-Item $ArtifactPath).PSIsContainer) {
        $source = $ArtifactPath
    } elseif ($ArtifactPath -match '\.zip$') {
        $source = Join-Path $temp 'artifact'
        Expand-Archive -Path $ArtifactPath -DestinationPath $source
    } else {
        throw "unsupported artifact (directory or .zip required): $ArtifactPath"
    }
    $libRoot = Join-Path $source 'lib\nexum'
    if (-not (Test-Path $libRoot)) { throw "artifact does not contain lib/nexum layout" }
    $versions = @(Get-ChildItem -Directory $libRoot | Where-Object { $_.Name -ne 'current' })
    if ($versions.Count -ne 1) { throw "artifact must contain exactly one versioned runtime (found $($versions.Count))" }
    return @{ Source = $source; VersionRoot = $versions[0].FullName; Temp = $temp }
}

function New-NexumShim {
    param([string]$BinDir, [string]$Command, [string]$ExePath)
    $shim = Join-Path $BinDir "$Command.cmd"
    $content = "@echo off`r`n`"$ExePath`" %*"
    Set-Content -Path $shim -Value $content -Encoding ASCII
}

# --- resolve artifact (local only; release download is out of scope for v0.1) ---
if (-not (Test-Path $Artifact)) { throw "artifact not found: $Artifact" }
$resolved = Expand-NexumArtifact $Artifact
$version = Assert-NexumManifest $resolved.VersionRoot

$destRoot = Join-Path $Prefix "lib\nexum\$version"
if (Test-Path $destRoot) {
    if (-not $Force) { throw "version already installed: $version (use -Force to replace)" }
    Remove-Item -Recurse -Force $destRoot
}

# --- copy versioned runtime ---
New-Item -ItemType Directory -Path (Split-Path $destRoot) -Force | Out-Null
Copy-Item -Recurse -Path $resolved.VersionRoot -Destination $destRoot

# --- current link: prefer directory junction (no admin required for junctions) ---
$current = Join-Path $Prefix 'lib\nexum\current'
if (Test-Path $current) {
    $item = Get-Item $current
    if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { Remove-Item -Force $current }
    else { Remove-Item -Recurse -Force $current }
}
New-Item -ItemType Junction -Path $current -Target $destRoot | Out-Null

# --- command shims in <Prefix>/bin ---
$binDir = Join-Path $Prefix 'bin'
New-Item -ItemType Directory -Path $binDir -Force | Out-Null
foreach ($command in @('nexum', 'nexum-acp-host', 'nexum-autologin-reconcile')) {
    $exe = Join-Path $current "$command.exe"
    if (Test-Path $exe) {
        New-NexumShim $binDir $command $exe
    }
}

# --- user PATH (HKCU\Environment, broadcast WM_SETTINGCHANGE) ---
if ($AddToPath) {
    $envKey = 'HKCU:\Environment'
    $userPath = (Get-ItemProperty -Path $envKey -Name Path -ErrorAction SilentlyContinue).Path
    if ($userPath) {
        $entries = @($userPath -split ';' | Where-Object { $_ -and $_ -ne $binDir })
        $newPath = ($entries + $binDir) -join ';'
    } else {
        $newPath = $binDir
    }
    Set-ItemProperty -Path $envKey -Name Path -Value $newPath
    # notify Explorer
    Add-Type -Namespace NexumInstall -Name Native -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
    [NexumInstall.Native]::SendMessageTimeout([IntPtr]0xFFFF, 0x1A, [UIntPtr]::Zero, $newPath, 0x0002, 1000, [ref]([UIntPtr]::Zero)) | Out-Null
}

if (Test-Path $resolved.Temp) { Remove-Item -Recurse -Force $resolved.Temp }

Write-Host "Nexum $version installed:" -ForegroundColor Green
Write-Host "  runtime : $destRoot"
Write-Host "  current : $current"
Write-Host "  bin     : $binDir (nexum.cmd)"
Write-Host "Run 'nexum --version' and 'nexum doctor' to verify."
