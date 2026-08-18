<#
.SYNOPSIS
    Uninstall Nexum from a user prefix (InstalledLayoutV1).

.DESCRIPTION
    Removes <Prefix>/lib/nexum, <Prefix>/bin shims and (optionally) the
    <Prefix>/bin entry from the user PATH.
#>
[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'nexum'),
    [switch]$KeepPath
)

$ErrorActionPreference = 'Stop'

$libRoot = Join-Path $Prefix 'lib\nexum'
$binDir = Join-Path $Prefix 'bin'

foreach ($target in @($libRoot)) {
    if (Test-Path $target) {
        $item = Get-Item $target
        if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { Remove-Item -Force $target }
        else { Remove-Item -Recurse -Force $target }
        Write-Host "removed: $target"
    }
}

foreach ($command in @('nexum', 'nexum-acp-host', 'nexum-autologin-reconcile')) {
    $shim = Join-Path $binDir "$command.cmd"
    if (Test-Path $shim) {
        Remove-Item -Force $shim
        Write-Host "removed: $shim"
    }
}

if ($KeepPath) {
    Write-Host "PATH entry kept (per -KeepPath)."
} else {
    $envKey = 'HKCU:\Environment'
    $userPath = (Get-ItemProperty -Path $envKey -Name Path -ErrorAction SilentlyContinue).Path
    if ($userPath) {
        $entries = @($userPath -split ';' | Where-Object { $_ -and $_ -ne $binDir })
        $newPath = $entries -join ';'
        Set-ItemProperty -Path $envKey -Name Path -Value $newPath
        Write-Host "removed from PATH: $binDir"
    }
}

if ((Test-Path $libRoot) -eq $false -and (Test-Path $binDir) -eq $false) {
    if ((Test-Path $Prefix) -and -not (Get-ChildItem -Force $Prefix | Select-Object -First 1)) {
        Remove-Item -Force $Prefix
    }
}

Write-Host "Nexum uninstalled." -ForegroundColor Green
