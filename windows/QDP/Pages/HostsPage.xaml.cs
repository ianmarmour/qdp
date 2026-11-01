using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Qdp.Core;

namespace Qdp.Pages;

/// <summary>
/// The computers to stream: those QDP's host advertises on the network and
/// those paired with, found or not; choosing one streams it, or pairs with
/// it first.
/// </summary>
public sealed partial class HostsPage : Page
{
    /// <summary>The main window, once the page is shown.</summary>
    private MainWindow? window;

    /// <summary>Makes the page.</summary>
    public HostsPage() => InitializeComponent();

    /// <inheritdoc />
    protected override void OnNavigatedTo(NavigationEventArgs args)
    {
        base.OnNavigatedTo(args);
        if (window is null && args.Parameter is MainWindow main)
        {
            window = main;
            window.HostsPage = this;
            window.Hosts.Changed += Refresh;
            if (window.Problem is { } problem)
            {
                ShowProblem("QDP can't stream", problem);
                AddButton.IsEnabled = false;
            }
        }
        window?.Hosts.SetListShown(true);
        Refresh();
    }

    /// <inheritdoc />
    protected override void OnNavigatedFrom(NavigationEventArgs args)
    {
        base.OnNavigatedFrom(args);
        window?.Hosts.SetListShown(false);
    }

    /// <summary>Shows the hosts as they now stand.</summary>
    private void Refresh()
    {
        if (window is null)
        {
            return;
        }
        var rows = window.Hosts.Rows();
        HostList.ItemsSource = rows;
        EmptyState.Visibility = rows.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    /// <summary>Says what went wrong.</summary>
    internal void ShowProblem(string title, string message)
    {
        ProblemBar.Title = title;
        ProblemBar.Message = message;
        ProblemBar.IsOpen = true;
    }

    /// <summary>Stops saying what went wrong.</summary>
    internal void ClearProblem() => ProblemBar.IsOpen = false;

    /// <summary>Streams the host chosen, or pairs with it first.</summary>
    private void OnHostClicked(object sender, ItemClickEventArgs args)
    {
        if (args.ClickedItem is HostRow { Target: { } target })
        {
            window?.Connect(target);
        }
    }

    /// <summary>Forgets a paired host.</summary>
    private void OnForgetClicked(object sender, RoutedEventArgs args)
    {
        // A flyout's items do not inherit their host's data context, so the
        // row rides in the item's tag.
        if ((sender as FrameworkElement)?.Tag is HostRow { Paired: { } paired })
        {
            window?.Hosts.Forget(paired.Pin);
        }
    }

    /// <summary>Adds a host by its address: pairing with it, or with its key pin and a token.</summary>
    private async void OnAddClicked(object sender, RoutedEventArgs args)
    {
        if (window is null)
        {
            return;
        }
        var target = await AddHostDialog.Ask(XamlRoot);
        if (target is not null)
        {
            window.Connect(target);
        }
    }
}
