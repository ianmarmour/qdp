# QDP for Windows

The QDP app for Windows 10 or later on x86-64. To install and use it, see
[Installing on Windows](../docs/setup/windows.md) and
[Using the apps](../docs/setup/apps.md#windows).

## Dependencies

| Dependency | Version |
|---|---|
| Rust | 1.97.0 |
| NASM | Latest |
| .NET SDK | 8 |
| Windows App SDK | 1.7.250310001 |
| Windows SDK build tools | 10.0.26100.1742 |
| WiX Toolset, for the installer | 5.0.2 |
| Minimum Windows | 10, version 1809 |

## Build

On Windows, install Rust, NASM and the .NET SDK as
[Building](../docs/building.md#windows) describes. Then, from the repository's
root:

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
