# Tries the Windows app built into windows\publish as a user would, and
# streams from a host on the same machine through its core, failing if
# either does not work. Screenshots of each, and the logs, go to
# windows\screenshots.
#
# It needs the qdp command built (cargo build --release -p qdp-cli), for the
# host, and a desktop to draw on. It runs in Windows PowerShell, for UI
# Automation.
$ErrorActionPreference = 'Stop'

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$publish = Join-Path $PSScriptRoot 'publish'
$shots = Join-Path $PSScriptRoot 'screenshots'
$scratch = Join-Path ([System.IO.Path]::GetTempPath()) 'qdp-smoke'
New-Item -ItemType Directory -Force $shots, $scratch | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing, UIAutomationClient, UIAutomationTypes

# Saves the screen as windows\screenshots\NAME.png.
function Save-Screen([string] $name) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $bitmap.Save((Join-Path $shots "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()
}

# Returns the element named NAME in WINDOW, waiting up to ten seconds for it.
function Find-Named($window, [string] $name) {
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, $name)
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) {
        $found = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
        if ($null -ne $found) { return $found }
        Start-Sleep -Milliseconds 500
    }
    throw "the app shows no '$name'"
}

# The app: its window opens on the computers, and shows its settings.
$app = Start-Process -FilePath (Join-Path $publish 'QDP.exe') -PassThru
$deadline = (Get-Date).AddSeconds(60)
while ($app.MainWindowHandle -eq [IntPtr]::Zero) {
    if ($app.HasExited) { throw "the app exited with code $($app.ExitCode)" }
    if ((Get-Date) -gt $deadline) { throw 'the app opened no window' }
    Start-Sleep -Milliseconds 500
    $app.Refresh()
}
$window = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
Find-Named $window 'Computers' | Out-Null
Start-Sleep -Seconds 3
Save-Screen 'computers'
$settings = Find-Named $window 'Settings'
try {
    $settings.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
} catch {
    $settings.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
}
Find-Named $window 'Resolution' | Out-Null
Start-Sleep -Seconds 2
Save-Screen 'settings'
if ($app.HasExited) { throw "the app exited with code $($app.ExitCode)" }
$app.CloseMainWindow() | Out-Null
if (-not $app.WaitForExit(15000)) {
    $app.Kill()
    throw 'the app did not close'
}
Write-Host 'the app opened, showed its pages and closed'

# A host on this machine, streaming its test pattern.
$qdp = Join-Path $root 'target\release\qdp.exe'
$identity = Join-Path $scratch 'host'
# Run apart, as Windows PowerShell stops at a native command's logging to
# standard error.
$pinFile = Join-Path $scratch 'pin.txt'
Start-Process -FilePath $qdp -ArgumentList @('pin', '--identity', $identity) -Wait -NoNewWindow `
    -RedirectStandardOutput $pinFile -RedirectStandardError (Join-Path $shots 'pin.log')
$pin = (Get-Content $pinFile | Select-Object -Last 1).Trim()
$token = 'smoke-test-token'
$server = Start-Process -FilePath $qdp -PassThru -NoNewWindow `
    -RedirectStandardError (Join-Path $shots 'host.log') `
    -ArgumentList @('serve', '--listen', '127.0.0.1:4433', '--identity', $identity,
        '--token', $token, '--source', 'test-pattern', '--pairing', 'off',
        '--advertise', 'off', '--lock', 'off', '--audio', 'off')
Start-Sleep -Seconds 3

# The core, as the app calls it.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class QdpCore {
    [StructLayout(LayoutKind.Sequential)]
    public struct Config {
        public IntPtr Host, Pin, Token, DeviceName, Title, IconPath;
        public IntPtr OwnerWindow;
        public int Authentication;
        public uint Width, Height;
        public int Decoder, Microphone, Screen, Sync, Shortcuts, Clipboard, Statistics;
    }

    [DllImport("qdp_windows", CallingConvention = CallingConvention.Cdecl)]
    public static extern void qdp_init(IntPtr dataDirectory);
    [DllImport("qdp_windows", CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr qdp_session_start(ref Config config, out IntPtr error);
    [DllImport("qdp_windows", CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr qdp_session_next_event(IntPtr session, uint timeoutMillis);
    [DllImport("qdp_windows", CallingConvention = CallingConvention.Cdecl)]
    public static extern void qdp_session_stop(IntPtr session);
    [DllImport("qdp_windows", CallingConvention = CallingConvention.Cdecl)]
    public static extern void qdp_session_free(IntPtr session);
    [DllImport("qdp_windows", CallingConvention = CallingConvention.Cdecl)]
    public static extern void qdp_string_free(IntPtr text);

    public static IntPtr Utf8(string text) {
        byte[] bytes = Encoding.UTF8.GetBytes(text + "\0");
        IntPtr copy = Marshal.AllocHGlobal(bytes.Length);
        Marshal.Copy(bytes, 0, copy, bytes.Length);
        return copy;
    }

    public static string Take(IntPtr text) {
        if (text == IntPtr.Zero) return null;
        try {
            int length = 0;
            while (Marshal.ReadByte(text, length) != 0) length++;
            byte[] bytes = new byte[length];
            Marshal.Copy(text, bytes, 0, length);
            return Encoding.UTF8.GetString(bytes);
        } finally {
            qdp_string_free(text);
        }
    }

    public static string NextLine(IntPtr session, uint timeoutMillis) {
        return Take(qdp_session_next_event(session, timeoutMillis));
    }
}
'@

$env:PATH = "$publish;$env:PATH"
$data = Join-Path $scratch 'app'
New-Item -ItemType Directory -Force $data | Out-Null
[QdpCore]::qdp_init([QdpCore]::Utf8($data))
$config = New-Object QdpCore+Config
$config.Host = [QdpCore]::Utf8('127.0.0.1:4433')
$config.Pin = [QdpCore]::Utf8($pin)
$config.Token = [QdpCore]::Utf8($token)
$config.DeviceName = [QdpCore]::Utf8('smoke test')
$config.Title = [QdpCore]::Utf8('QDP smoke test')
$config.IconPath = [QdpCore]::Utf8((Join-Path $publish 'Assets\qdp.ico'))
$config.Authentication = 0
$config.Statistics = 1
$errorText = [IntPtr]::Zero
$session = [QdpCore]::qdp_session_start([ref] $config, [ref] $errorText)
if ($session -eq [IntPtr]::Zero) { throw "the session cannot start: $([QdpCore]::Take($errorText))" }

# The stream window shows, with frames, and closes when the session stops.
$shown = $false
$frames = 0
$ended = $null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline -and $null -eq $ended -and -not ($shown -and $frames -gt 0)) {
    $line = [QdpCore]::NextLine($session, 500)
    if ($null -eq $line) { continue }
    Write-Host "core: $line"
    if ($line -eq 'window shown') { $shown = $true }
    if ($line -match '^stats (\d+)') { $frames = [int] $Matches[1] }
    if ($line -match '^(ended|failed)') { $ended = $line }
}
Start-Sleep -Seconds 3
Save-Screen 'stream'
[QdpCore]::qdp_session_stop($session)
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline -and $null -eq $ended) {
    $line = [QdpCore]::NextLine($session, 500)
    if ($null -eq $line) { continue }
    Write-Host "core: $line"
    if ($line -match '^(ended|failed)') { $ended = $line }
}
[QdpCore]::qdp_session_free($session)
Stop-Process -Id $server.Id -ErrorAction SilentlyContinue
Copy-Item (Join-Path $data 'qdp.log') (Join-Path $shots 'core.log') -ErrorAction SilentlyContinue

if (-not $shown) { throw "the stream window never showed: $ended" }
if ($frames -le 0) { throw 'the stream window showed no frames' }
if ($ended -ne 'ended') { throw "the session did not end cleanly: $ended" }
Write-Host "streamed $frames frames a second, and ended cleanly"
