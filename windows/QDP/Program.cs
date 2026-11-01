using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace Qdp;

/// <summary>
/// Where the app starts. One instance runs at a time: launching QDP again,
/// as from the Start menu while it is open, brings the open window forward
/// rather than opening a second app with streams of its own.
/// </summary>
public static partial class Program
{
    /// <summary>The key the running instance registers under.</summary>
    private const string InstanceKey = "QDP";

    /// <summary>The app, once made, which instances launched later hand over to.</summary>
    private static App? app;

    /// <summary>Starts the app, or hands the launch over to the instance already running.</summary>
    [STAThread]
    private static void Main()
    {
        WinRT.ComWrappersSupport.InitializeComWrappers();
        var instance = AppInstance.FindOrRegisterForKey(InstanceKey);
        if (!instance.IsCurrent)
        {
            HandOver(instance);
            return;
        }
        // Raised on a thread of its own when a later launch hands over.
        instance.Activated += (_, _) => app?.BringForward();
        Application.Start(_ =>
        {
            SynchronizationContext.SetSynchronizationContext(
                new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread()));
            app = new App();
        });
    }

    /// <summary>
    /// Hands this launch over to <paramref name="running"/>, letting it take
    /// the foreground, which this process holds as the one just launched.
    /// </summary>
    private static void HandOver(AppInstance running)
    {
        AllowSetForegroundWindow(running.ProcessId);
        var arguments = AppInstance.GetCurrent().GetActivatedEventArgs();
        // Handed over from a thread of the pool, as this one is single
        // threaded; waiting on it here keeps COM's messages flowing.
        Task.Run(() => running.RedirectActivationToAsync(arguments).AsTask()).Wait();
    }

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool AllowSetForegroundWindow(uint processId);
}
