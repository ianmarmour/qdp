using Microsoft.UI.Xaml;
using Qdp.Core;

namespace Qdp;

/// <summary>
/// The app: starts the Rust core, which keeps the device's key and the log
/// in the app's data directory, and opens the main window.
/// </summary>
public partial class App : Application
{
    /// <summary>The main window, once open.</summary>
    private MainWindow? window;

    /// <summary>Makes the app.</summary>
    public App() => InitializeComponent();

    /// <inheritdoc />
    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        Storage.Prepare();
        string? problem = null;
        try
        {
            Native.qdp_init(Storage.DataDirectory);
        }
        catch (Exception exception) when (exception is DllNotFoundException or EntryPointNotFoundException)
        {
            problem = "QDP's core, qdp_windows.dll, is missing from the app's folder. Download the app again.";
        }
        window = new MainWindow(problem);
        window.Activate();
    }
}
