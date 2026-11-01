using System.Globalization;
using System.Runtime.InteropServices;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Qdp.Core;
using Qdp.Pages;
using Windows.Graphics;
using WinRT.Interop;

namespace Qdp;

/// <summary>
/// The main window: the computers to stream and the settings; and, while a
/// stream starts or runs, a card that says how it goes. The stream itself
/// shows in the core's own window, and this one steps back once it does.
/// </summary>
public sealed partial class MainWindow : Window
{
    /// <summary>The window's size as it opens, in effective pixels.</summary>
    private static readonly SizeInt32 InitialSize = new(1040, 700);

    /// <summary>The display scale of 100 %, in dots per inch.</summary>
    private const double DefaultDpi = 96.0;

    /// <summary>
    /// How long a running stream may be reconnecting before the window comes
    /// back to say so: shorter losses pass unnoticed.
    /// </summary>
    private static readonly TimeSpan ReconnectingNotice = TimeSpan.FromSeconds(2);

    /// <summary>Brings the window back while a running stream reconnects.</summary>
    private readonly DispatcherTimer reconnectingTimer = new() { Interval = ReconnectingNotice };

    /// <summary>Whether the stream window shows the video.</summary>
    private bool streaming;

    /// <summary>Whether the window came back because the stream is reconnecting.</summary>
    private bool cameBackToReconnect;

    /// <summary>The stream, while one starts or runs.</summary>
    private StreamSession? session;

    /// <summary>The pairing code dialog, while it shows.</summary>
    private ContentDialog? pairingDialog;

    /// <summary>Why the app cannot stream at all, if it cannot.</summary>
    private readonly string? problem;

    /// <summary>The hosts paired with and those on the network.</summary>
    internal HostStore Hosts { get; }

    /// <summary>What the user chose for streams.</summary>
    internal AppSettings Settings { get; }

    /// <summary>The hosts page, once shown, which shows what went wrong.</summary>
    internal HostsPage? HostsPage { get; set; }

    /// <summary>Opens the window; <paramref name="problem"/> says why it cannot stream, if it cannot.</summary>
    public MainWindow(string? problem)
    {
        this.problem = problem;
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "qdp.ico"));
        PlaceWindow();
        Settings = Storage.LoadSettings();
        Hosts = new HostStore(DispatcherQueue);
        Navigation.SelectedItem = HostsItem;
        reconnectingTimer.Tick += (_, _) =>
        {
            reconnectingTimer.Stop();
            if (session is not null && streaming)
            {
                cameBackToReconnect = true;
                ComeBack();
            }
        };
        Closed += (_, _) => session?.Stop();
    }

    /// <summary>Opens the window at its initial size, centred on its monitor's work area.</summary>
    private void PlaceWindow()
    {
        var scale = GetDpiForWindow(WindowNative.GetWindowHandle(this)) / DefaultDpi;
        var size = new SizeInt32((int)(InitialSize.Width * scale), (int)(InitialSize.Height * scale));
        var area = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Nearest).WorkArea;
        AppWindow.MoveAndResize(new RectInt32(
            area.X + Math.Max(0, (area.Width - size.Width) / 2),
            area.Y + Math.Max(0, (area.Height - size.Height) / 2),
            Math.Min(size.Width, area.Width),
            Math.Min(size.Height, area.Height)));
    }

    /// <summary>Shows the page the user chose.</summary>
    private void OnNavigationChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        var page = args.IsSettingsSelected ? typeof(SettingsPage) : typeof(HostsPage);
        if (ContentFrame.CurrentSourcePageType != page)
        {
            ContentFrame.Navigate(page, this);
        }
    }

    /// <summary>Returns why the app cannot stream at all, if it cannot.</summary>
    internal string? Problem => problem;

    /// <summary>Starts a stream of <paramref name="target"/>, unless one runs.</summary>
    internal void Connect(StreamTarget target)
    {
        if (session is not null || problem is not null)
        {
            return;
        }
        HostsPage?.ClearProblem();
        try
        {
            session = StreamSession.Start(
                target, Settings, WindowNative.GetWindowHandle(this), DispatcherQueue, OnSessionEvent);
        }
        catch (Exception exception) when (exception is InvalidOperationException or DllNotFoundException or EntryPointNotFoundException)
        {
            HostsPage?.ShowProblem($"Couldn't connect to {target.Name}", exception.Message);
            return;
        }
        Hosts.SetStreaming(true);
        ShowStarting(target.Name, "Connecting…");
    }

    /// <summary>Shows the card for a stream starting.</summary>
    private void ShowStarting(string name, string status)
    {
        StreamOverlay.Visibility = Visibility.Visible;
        StreamProgress.IsActive = true;
        StreamProgress.Visibility = Visibility.Visible;
        StreamIcon.Visibility = Visibility.Collapsed;
        StatsPanel.Visibility = Visibility.Collapsed;
        ShortcutsPanel.Visibility = Visibility.Collapsed;
        ShowStreamButton.Visibility = Visibility.Collapsed;
        StreamTitle.Text = $"Connecting to {name}";
        StreamStatus.Text = status;
        StopButton.Content = "Cancel";
    }

    /// <summary>Shows the card for a stream that runs.</summary>
    private void ShowStreaming(string name)
    {
        StreamProgress.IsActive = false;
        StreamProgress.Visibility = Visibility.Collapsed;
        StreamIcon.Visibility = Visibility.Visible;
        StatsPanel.Visibility = Visibility.Visible;
        ShortcutsPanel.Visibility = Visibility.Visible;
        ShowStreamButton.Visibility = Visibility.Visible;
        StreamTitle.Text = $"Streaming {name}";
        StreamStatus.Text = Settings.Microphone ? "The microphone is on." : "";
        StopButton.Content = "Disconnect";
    }

    /// <summary>Follows the stream.</summary>
    private async void OnSessionEvent(SessionEvent received)
    {
        if (session is not { } running)
        {
            return;
        }
        var name = running.Target.Name;
        switch (received)
        {
            case SessionEvent.Connecting:
                StreamStatus.Text = "Connecting…";
                break;
            case SessionEvent.Connected:
                StreamStatus.Text = streaming ? "" : "Starting the stream…";
                reconnectingTimer.Stop();
                if (cameBackToReconnect)
                {
                    cameBackToReconnect = false;
                    StepBack();
                }
                break;
            case SessionEvent.Reconnecting:
                StreamStatus.Text = "The connection was lost. Reconnecting…";
                if (streaming)
                {
                    reconnectingTimer.Start();
                }
                break;
            case SessionEvent.WindowShown:
                streaming = true;
                ShowStreaming(name);
                StepBack();
                break;
            case SessionEvent.Stats(var video):
                ShowStats(video);
                break;
            case SessionEvent.Microphone(var on):
                StreamStatus.Text = on ? "The microphone is on." : "The microphone is off.";
                break;
            case SessionEvent.PairingCode(var retry):
                running.AnswerPairing(await AskPairingCode(name, retry));
                break;
            case SessionEvent.Paired(var pin):
                Hosts.Keep(new PairedHost { Name = name, Address = running.Target.Address, Pin = pin });
                break;
            case SessionEvent.Ended(var failure):
                Ended(name, failure);
                break;
        }
    }

    /// <summary>Shows the last second's video.</summary>
    private void ShowStats(VideoStats video)
    {
        static string Milliseconds(double value) => value.ToString("0.0", CultureInfo.CurrentCulture) + " ms";
        FrameRateText.Text = video.Frames.ToString(CultureInfo.CurrentCulture);
        LatencyText.Text = video.LatencyMs is { } latency ? Milliseconds(latency) : "–";
        DecodeText.Text = Milliseconds(video.DecodeMs);
    }

    /// <summary>Puts the window away once the stream shows, so that the stream has the screen.</summary>
    private void StepBack()
    {
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.Minimize();
        }
    }

    /// <summary>Brings the window back once the stream ended.</summary>
    private void ComeBack()
    {
        if (AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter)
        {
            presenter.Restore();
        }
        Activate();
    }

    /// <summary>Follows the end of the stream, saying why it failed, if it did.</summary>
    private void Ended(string name, string? failure)
    {
        session = null;
        streaming = false;
        cameBackToReconnect = false;
        reconnectingTimer.Stop();
        pairingDialog?.Hide();
        StreamOverlay.Visibility = Visibility.Collapsed;
        Hosts.SetStreaming(false);
        ComeBack();
        if (failure is not null)
        {
            HostsPage?.ShowProblem($"The stream of {name} stopped", failure);
        }
    }

    /// <summary>Asks for the code the host shows, returning it, or null if the user declined.</summary>
    private async Task<string?> AskPairingCode(string name, bool retry)
    {
        var code = new TextBox
        {
            Header = "Pairing code",
            PlaceholderText = "The code on the computer's screen",
            MaxLength = 32,
            InputScope = new InputScope { Names = { new InputScopeName(InputScopeNameValue.Number) } },
        };
        var explanation = new TextBlock
        {
            Text = retry
                ? $"That code didn't work. {name} shows a new one: enter it to pair."
                : $"{name} shows a code. Enter it to pair, so that this PC can connect from now on.",
            TextWrapping = TextWrapping.Wrap,
        };
        var content = new StackPanel { Spacing = 16 };
        content.Children.Add(explanation);
        content.Children.Add(code);
        var dialog = new ContentDialog
        {
            XamlRoot = Content.XamlRoot,
            Title = $"Pair with {name}",
            Content = content,
            PrimaryButtonText = "Pair",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = false,
        };
        code.TextChanged += (_, _) => dialog.IsPrimaryButtonEnabled = code.Text.Trim().Length > 0;
        pairingDialog = dialog;
        var result = await dialog.ShowAsync();
        pairingDialog = null;
        return result == ContentDialogResult.Primary ? code.Text.Trim() : null;
    }

    /// <summary>Ends the stream, or stops it starting.</summary>
    private void OnStopClicked(object sender, RoutedEventArgs args)
    {
        StreamStatus.Text = "Disconnecting…";
        session?.Stop();
    }

    /// <summary>Steps back for the stream's window.</summary>
    private void OnShowStreamClicked(object sender, RoutedEventArgs args) => StepBack();

    [LibraryImport("user32.dll")]
    private static partial uint GetDpiForWindow(IntPtr window);
}
