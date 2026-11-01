# Builds one of QDP's Windows installers with WiX Toolset 5, from what the
# release builds left in the repository:
#
#   build-msi.ps1 app OUTPUT [BUILD]       QDP.wxs: windows\publish, from
#                                          windows\build.ps1, and
#                                          target\release\qdp.exe
#   build-msi.ps1 gateway OUTPUT [BUILD]   QDPGateway.wxs:
#                                          target\release\qdp-gateway.exe
#
# The installer's version is Cargo.toml's major and minor version and BUILD,
# by default the CI run's number, which only grows, so that each build
# upgrades the one before it. It needs the wix tool (dotnet tool install
# --global wix --version 5.0.2) and, for the gateway, its firewall extension
# (wix extension add --global WixToolset.Firewall.wixext/5.0.2).
param(
    [Parameter(Mandatory)] [ValidateSet('app', 'gateway')] [string] $Installer,
    [Parameter(Mandatory)] [string] $Output,
    [int] $Build = $env:GITHUB_RUN_NUMBER
)
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
# Returns the absolute path of PATH, relative to the repository, which must exist.
function Find([string] $path) { (Resolve-Path (Join-Path $root $path)).Path }

$release = (Select-String -Path (Find 'Cargo.toml') -Pattern '^version = "(\d+)\.(\d+)\.' |
    Select-Object -First 1).Matches[0]
$version = "$($release.Groups[1].Value).$($release.Groups[2].Value).$Build"

$common = @('-arch', 'x64', '-d', "Version=$version", '-d', "Icon=$(Find 'windows\QDP\Assets\qdp.ico')")
$arguments = switch ($Installer) {
    'app' {
        @((Find 'packaging\windows\QDP.wxs')) + $common + @(
            '-d', "AppDir=$(Find 'windows\publish')",
            '-d', "QdpExe=$(Find 'target\release\qdp.exe')")
    }
    'gateway' {
        @((Find 'packaging\windows\QDPGateway.wxs'), '-ext', 'WixToolset.Firewall.wixext') + $common + @(
            '-d', "GatewayExe=$(Find 'target\release\qdp-gateway.exe')",
            '-d', "ExampleConfig=$(Find 'packaging\gateway.toml')")
    }
}
wix build @arguments -o $Output
if ($LASTEXITCODE -ne 0) { throw "the $Installer installer did not build" }
Write-Host "built $Output, version $version"
