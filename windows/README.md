# QDP for Windows

The QDP app for Windows 10 or later on x86-64. To install and use it, see
[Installing on Windows](../docs/setup/windows.md) and
[Using the apps](../docs/setup/apps.md#windows).

## Requirements

- Windows 10 or later
- Rust, NASM and the .NET 8 SDK, which [Building](../docs/building.md#windows) sets up

## Build

From the repository's root:

```powershell
.\windows\build.ps1
```

This builds the app, with its Rust core, into `windows\publish`. Its installer
builds from there: see [Packaging](../packaging/README.md).

## Test

```powershell
dotnet test windows/QDP.Tests/QDP.Tests.csproj -c Release
```

## License

Apache-2.0
