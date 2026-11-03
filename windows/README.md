# QDP for Windows

The QDP app for Windows 10 or later on x86-64. To install and use it, see
[Installing on Windows](../docs/setup/windows.md) and
[Using the apps](../docs/setup/apps.md#windows).

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Rust | 1.97.0 | Windows | Building the app's Rust core |
| Visual Studio C++ build tools | 2022+ | Windows | Building the app's Rust core |
| NASM | 2.15+ | Windows | Building the codecs |
| .NET SDK | 8 | Windows | Building the app |
| Windows SDK build tools | 10.0.26100.1742 | Windows | Building the app |
| Windows App SDK | 1.7.250310001 | Windows | The app's interface |
| WiX Toolset | 5.0.2 | Windows | Building the installer |

The app runs on Windows 10, version 1809, or later.

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

QDP for Windows is licensed under the [Apache License 2.0](../LICENSE).
