# Installs the QDP installer INSTALLER as a user would, opens the installed
# app, upgrades it to UPGRADE, a later build, and removes it, failing if any
# of it does not work. msiexec's logs go to the current folder.
param(
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [string] $Upgrade
)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'installer-test.psm1')

$folder = Join-Path $env:ProgramFiles 'QDP'
$bin = Join-Path $folder 'bin'
$app = Join-Path $folder 'QDP.exe'
$shortcut = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Quick Desktop.lnk'

Invoke-Installer @('/i', $Installer) 'app-install.log'
foreach ($file in @($app, (Join-Path $folder 'qdp_windows.dll'), (Join-Path $bin 'qdp.exe'))) {
    Assert-That (Test-Path $file) "$file is not installed"
}
Assert-That (Test-Path $shortcut) 'no Start menu shortcut'
Assert-That (Test-OnPath $bin) 'qdp is not on the PATH'
& (Join-Path $bin 'qdp.exe') help | Out-Null
Assert-That ($LASTEXITCODE -eq 0) 'the installed qdp does not run'

# The installed app opens its window, and keeps it open.
$process = Start-Process $app -PassThru
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline -and -not $process.HasExited -and $process.MainWindowHandle -eq 0) {
    Start-Sleep -Milliseconds 500
    $process.Refresh()
}
Assert-That (-not $process.HasExited) "the installed app quit, with $($process.ExitCode)"
Assert-That ($process.MainWindowHandle -ne 0) 'the installed app shows no window'
Stop-Process -Id $process.Id -Force
$process.WaitForExit()

Invoke-Installer @('/i', $Upgrade) 'app-upgrade.log'
Assert-That (Test-Path $app) 'the upgrade removed the app'
Assert-That (Test-OnPath $bin) 'the upgrade took qdp off the PATH'

Invoke-Installer @('/x', $Upgrade) 'app-uninstall.log'
Assert-That (-not (Test-Path $folder)) "removing QDP left $folder"
Assert-That (-not (Test-Path $shortcut)) 'removing QDP left its shortcut'
Assert-That (-not (Test-OnPath $bin)) 'removing QDP left it on the PATH'
Write-Host 'the installer installs, upgrades and removes QDP'
