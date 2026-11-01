using Microsoft.UI.Dispatching;

namespace Qdp.Core;

/// <summary>
/// The hosts paired with, kept between launches, and those on the local
/// network, combined into the list the app shows.
/// </summary>
internal sealed class HostStore
{
    /// <summary>Finds hosts on the local network.</summary>
    private readonly Discovery discovery;

    /// <summary>The hosts paired with, most recently kept first.</summary>
    private List<PairedHost> paired = Storage.LoadHosts();

    /// <summary>The hosts on the local network.</summary>
    private IReadOnlyList<DiscoveredHost> discovered = [];

    /// <summary>Whether the host list shows.</summary>
    private bool listShown;

    /// <summary>Whether a stream runs, during which the network is not searched.</summary>
    private bool streaming;

    /// <summary>Returns the hosts, telling of them on <paramref name="dispatcher"/>.</summary>
    internal HostStore(DispatcherQueue dispatcher)
    {
        discovery = new Discovery(dispatcher);
        discovery.Changed += hosts =>
        {
            discovered = hosts;
            Changed?.Invoke();
        };
    }

    /// <summary>Raised on the UI thread when the rows change.</summary>
    internal event Action? Changed;

    /// <summary>Keeps <paramref name="host"/>, replacing the host with its pin.</summary>
    internal void Keep(PairedHost host)
    {
        paired = [host, .. paired.Where(other => other.Pin != host.Pin)];
        Storage.SaveHosts(paired);
        Changed?.Invoke();
    }

    /// <summary>Forgets the host whose pin is <paramref name="pin"/>.</summary>
    internal void Forget(string pin)
    {
        paired.RemoveAll(host => host.Pin == pin);
        Storage.SaveHosts(paired);
        Changed?.Invoke();
    }

    /// <summary>Notes whether the host list shows, which searches the network unless a stream runs.</summary>
    internal void SetListShown(bool shown)
    {
        listShown = shown;
        UpdateBrowsing();
    }

    /// <summary>Notes whether a stream runs.</summary>
    internal void SetStreaming(bool running)
    {
        streaming = running;
        UpdateBrowsing();
    }

    /// <summary>Searches the network while the list shows and no stream runs.</summary>
    private void UpdateBrowsing()
    {
        if (listShown && !streaming)
        {
            discovery.Start();
        }
        else
        {
            discovery.Stop();
        }
    }

    /// <summary>The rows to list, as <see cref="HostRows.Build"/> makes them.</summary>
    internal List<HostRow> Rows() => HostRows.Build(discovered, paired);
}
