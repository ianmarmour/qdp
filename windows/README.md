# QDP for Windows

The Windows app streams from a QDP host to PCs running Windows 10 or later on
x86-64. It's a WinUI 3 app in C#, around a Rust core,
[`qdp-windows`](../crates/qdp-windows/README.md), which it loads as
`qdp_windows.dll`. Each stream shows in a window of the core's own, drawn by
the core's render thread with a flip-model swap chain, so that in full screen
the system hands it to the display without composing it.

To install and use it, see [Installing on Windows](../docs/install/windows.md)
and [Connecting](../docs/connecting.md#windows).

## Building

The app needs Rust, NASM and the .NET 8 SDK:
[Building](../docs/building.md#windows) sets them up. The Windows App SDK
comes from NuGet.

From the repository's root:

```powershell
.\windows\build.ps1
```

This builds the Rust core, `qdp_windows.dll`, then the app around it,
published self-contained into `windows\publish\QDP.exe`, and zips it into
`qdp-windows-app-x64.zip`. The installer, which adds the `qdp` command, builds
from there: see [`packaging`](../packaging/README.md#windows-installers).

## Testing

The unit tests:

```powershell
dotnet test windows/QDP.Tests/QDP.Tests.csproj -c Release
```

[`smoke.ps1`](smoke.ps1) tries the built app as a user would, and streams
from a host on the same machine through its core, failing if either doesn't
work. It needs the `qdp` command built (`cargo build --release -p qdp-cli`)
and a desktop to draw on, and runs in Windows PowerShell, for UI Automation.
Screenshots of each step, and the logs, go to `windows\screenshots`.

## Code

| Path | What it holds |
|---|---|
| `QDP/MainWindow.xaml` | The main window: the computers list and settings, and a card that says how a stream is going while it starts or runs |
| `QDP/Pages` | The computers list, settings, adding a computer, and the gateway's sign-in dialogs |
| `QDP/Core/Native.cs` | The core's C interface, from `crates/qdp-windows/include/qdp_windows.h` |
| `QDP/Core/StreamSession.cs`, `SessionEvent.cs` | Running a stream, and the events the core reports |
| `QDP/Core/Discovery.cs` | Finds the hosts on the local network through Windows' DNS-SD browser |
| `QDP/Core/HostStore.cs`, `HostRows.cs`, `Tokens.cs` | The hosts kept between launches, the list's rows, and tokens in the Credential Locker |
| `QDP/Core/Storage.cs` | Where the app keeps the device's key and its log |
| `QDP.Tests` | Unit tests |

[`.github/workflows/windows-app.yml`](../.github/workflows/windows-app.yml)
builds and tests the app and its installer for each release.

The app's icon is drawn from QDP's logo by
[`branding/generate-icons.py`](../branding/README.md).
