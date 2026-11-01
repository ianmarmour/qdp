# Builds the Windows app: the Rust core, qdp_windows.dll, then the WinUI 3
# app around it, published self-contained into windows\publish, and zipped
# into qdp-windows-app-x64.zip at the repository's root.
#
# It needs Rust (with NASM on the PATH, for the codecs' assembly) and the
# .NET 8 SDK; the Windows App SDK comes from NuGet.
$ErrorActionPreference = 'Stop'

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$publish = Join-Path $PSScriptRoot 'publish'
$archive = Join-Path $root 'qdp-windows-app-x64.zip'

# The core, as a shared library for the app to load.
cargo rustc --manifest-path (Join-Path $root 'Cargo.toml') -p qdp-windows --lib --release --locked --crate-type cdylib
if ($LASTEXITCODE -ne 0) { throw 'the core did not build' }

# The app, with the core beside it.
if (Test-Path $publish) { Remove-Item -Recurse -Force $publish }
dotnet publish (Join-Path $PSScriptRoot 'QDP\QDP.csproj') -c Release -r win-x64 -p:Platform=x64 -o $publish
if ($LASTEXITCODE -ne 0) { throw 'the app did not build' }
Copy-Item (Join-Path $root 'target\release\qdp_windows.dll') $publish

if (Test-Path $archive) { Remove-Item -Force $archive }
Compress-Archive -Path (Join-Path $publish '*') -DestinationPath $archive
Write-Host "built $archive"
