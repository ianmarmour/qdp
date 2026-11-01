using System.Diagnostics;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Qdp.Core;
using Windows.ApplicationModel.DataTransfer;

namespace Qdp.Pages;

/// <summary>
/// What streams ask for and how they show, kept as soon as the user
/// changes it; and the app's version and this PC's key pin.
/// </summary>
public sealed partial class SettingsPage : Page
{
    /// <summary>The settings, once the page is shown.</summary>
    private AppSettings? settings;

    /// <summary>Whether the controls are being set from the settings, which changes nothing.</summary>
    private bool loading;

    /// <summary>Makes the page.</summary>
    public SettingsPage() => InitializeComponent();

    /// <inheritdoc />
    protected override void OnNavigatedTo(NavigationEventArgs args)
    {
        base.OnNavigatedTo(args);
        if (args.Parameter is not MainWindow window)
        {
            return;
        }
        settings = window.Settings;
        loading = true;
        ResolutionBox.SelectedIndex = (int)settings.Resolution;
        DecoderBox.SelectedIndex = (int)settings.Decoder;
        FullScreenSwitch.IsOn = settings.FullScreen;
        TearingSwitch.IsOn = settings.AllowTearing;
        MicrophoneSwitch.IsOn = settings.Microphone;
        ShortcutsBox.SelectedIndex = (int)settings.Shortcuts;
        ClipboardSwitch.IsOn = settings.ShareClipboard;
        StatisticsSwitch.IsOn = settings.ShowStatistics;
        loading = false;
        if (window.Problem is null)
        {
            VersionText.Text = $"Version {Native.Version}";
            var pin = Native.ClientPin;
            PinText.Text = pin ?? "This PC's key cannot be kept.";
            CopyPinButton.IsEnabled = pin is not null;
        }
        else
        {
            VersionText.Text = window.Problem;
            CopyPinButton.IsEnabled = false;
        }
    }

    /// <summary>Keeps a choice the user changed.</summary>
    private void OnSelectionChanged(object sender, SelectionChangedEventArgs args) => Save();

    /// <summary>Keeps a switch the user flipped.</summary>
    private void OnToggled(object sender, RoutedEventArgs args) => Save();

    /// <summary>Keeps what the controls say.</summary>
    private void Save()
    {
        if (loading || settings is null)
        {
            return;
        }
        settings.Resolution = (Resolution)Math.Max(ResolutionBox.SelectedIndex, 0);
        settings.Decoder = (DecoderChoice)Math.Max(DecoderBox.SelectedIndex, 0);
        settings.FullScreen = FullScreenSwitch.IsOn;
        settings.AllowTearing = TearingSwitch.IsOn;
        settings.Microphone = MicrophoneSwitch.IsOn;
        settings.Shortcuts = (ShortcutsChoice)Math.Max(ShortcutsBox.SelectedIndex, 0);
        settings.ShareClipboard = ClipboardSwitch.IsOn;
        settings.ShowStatistics = StatisticsSwitch.IsOn;
        Storage.SaveSettings(settings);
    }

    /// <summary>Copies this PC's key pin.</summary>
    private void OnCopyPinClicked(object sender, RoutedEventArgs args)
    {
        var package = new DataPackage();
        package.SetText(PinText.Text);
        Clipboard.SetContent(package);
    }

    /// <summary>Opens the folder the log is kept in.</summary>
    private void OnOpenLogsClicked(object sender, RoutedEventArgs args) =>
        Process.Start(new ProcessStartInfo("explorer.exe", $"\"{Storage.DataDirectory}\"") { UseShellExecute = true });
}
