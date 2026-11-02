# Installs the gateway's installer INSTALLER as an administrator would,
# configures and starts the service, upgrades it to UPGRADE, a later build,
# and removes it, failing if any of it does not work. msiexec's logs go to
# the current folder.
param(
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [string] $Upgrade
)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'installer-test.psm1')

$folder = Join-Path $env:ProgramFiles 'QDP Gateway'
$program = Join-Path $folder 'qdp-gateway.exe'
$data = Join-Path $env:ProgramData 'qdp-gateway'
$config = Join-Path $data 'gateway.toml'
$log = Join-Path $data 'gateway.log'
# The well-known SIDs of LocalService and of all users.
$localService = 'S-1-5-19'
$users = 'S-1-5-32-545'

# Returns the service as WMI describes it, or nothing if there is none.
function Get-Gateway { Get-CimInstance Win32_Service -Filter "Name='qdp-gateway'" }

# Waits up to thirty seconds for the service to run, and fails if it does not.
function Wait-Running([string] $when) {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and (Get-Gateway).State -ne 'Running') {
        Start-Sleep -Milliseconds 500
    }
    $why = if (Test-Path $log) { Get-Content $log -Raw } else { 'no log' }
    Assert-That ((Get-Gateway).State -eq 'Running') "the service does not run $when`: $why"
}

Invoke-Installer @('/i', $Installer) 'gateway-install.log'
Assert-That (Test-Path $program) 'the gateway is not installed'
Assert-That (Test-OnPath $folder) 'qdp-gateway is not on the PATH'
$service = Get-Gateway
Assert-That ($null -ne $service) 'the service is not installed'
Assert-That ($service.StartMode -eq 'Auto') "the service starts $($service.StartMode)"
Assert-That ($service.StartName -eq 'NT AUTHORITY\LocalService') "the service runs as $($service.StartName)"
Assert-That ($service.PathName -like "*service run --config `"$config`"") "the service runs $($service.PathName)"
Assert-That ($service.State -eq 'Stopped') 'the service started before it was configured'
Assert-That (Test-Path $config) 'no example configuration'

# Only the system, administrators and the service may use the folder.
$acl = Get-Acl $data
$sids = $acl.Access | ForEach-Object {
    $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
}
Assert-That $acl.AreAccessRulesProtected "the folder inherits ProgramData's permissions"
Assert-That ($sids -contains $localService) 'the service may not use its folder'
Assert-That (-not ($sids -contains $users)) "users may read the gateway's folder"

$rule = Get-NetFirewallRule -DisplayName 'QDP Gateway' -ErrorAction SilentlyContinue
Assert-That ($null -ne $rule) 'no firewall rule'
$ports = $rule | Get-NetFirewallPortFilter
Assert-That ($ports.Protocol -eq 'UDP' -and $ports.LocalPort -eq '443') `
    "the firewall rule lets in $($ports.Protocol) $($ports.LocalPort)"

# Configured, as far as starting goes: the example lacks only the directory
# account's password, which the directory checks at sign-in.
Set-Content -Path (Join-Path $data 'directory-password') -Value 'not-a-real-password' -NoNewline
Start-Service qdp-gateway
Wait-Running 'once configured'
& $program keys
Assert-That ($LASTEXITCODE -eq 0) 'the gateway has no keys'

Add-Content -Path $config -Value '# edited'
Invoke-Installer @('/i', $Upgrade) 'gateway-upgrade.log'
Assert-That ((Get-Content $config -Raw) -like '*# edited*') 'the upgrade replaced the configuration'
Wait-Running 'after the upgrade'

Invoke-Installer @('/x', $Upgrade) 'gateway-uninstall.log'
Assert-That ($null -eq (Get-Gateway)) 'removing the gateway left its service'
Assert-That (-not (Test-Path $program)) 'removing the gateway left its program'
Assert-That (-not (Test-OnPath $folder)) 'removing the gateway left it on the PATH'
Assert-That ($null -eq (Get-NetFirewallRule -DisplayName 'QDP Gateway' -ErrorAction SilentlyContinue)) `
    'removing the gateway left its firewall rule'
Assert-That (Test-Path $config) 'removing the gateway deleted its configuration'
Write-Host 'the installer installs, starts, upgrades and removes the gateway'
