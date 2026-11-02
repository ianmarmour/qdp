using Microsoft.UI.Dispatching;

namespace Qdp.Core;

/// <summary>
/// The hosts kept between launches, paired with, streamed with a token or
/// reached through the gateway, and those on the local network, combined
/// into the list the app shows.
/// </summary>
internal sealed class HostStore
{
    /// <summary>Finds hosts on the local network.</summary>
    private readonly Discovery discovery;

    /// <summary>The hosts kept, most recently kept first.</summary>
    private List<KeptHost> kept = Storage.LoadHosts();

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

    /// <summary>
    /// Keeps <paramref name="host"/> first, replacing the host with its key,
    /// its pin or what the gateway knows it as, with <paramref name="token"/>
    /// if it accepts a token. A host whose token the Credential Locker cannot
    /// keep is not kept, as the list could not stream it.
    /// </summary>
    internal void Keep(KeptHost host, string token = "")
    {
        if (host.Access == HostAccess.Token && !Tokens.Keep(host.Pin, token))
        {
            return;
        }
        if (host.Access != HostAccess.Token && Find(host.Key) is { Access: HostAccess.Token })
        {
            Tokens.Forget(host.Pin);
        }
        kept = [host, .. kept.Where(other => other.Key != host.Key)];
        Storage.SaveHosts(kept);
        Changed?.Invoke();
    }

    /// <summary>Forgets the host whose key is <paramref name="key"/>, and its token if it has one.</summary>
    internal void Forget(string key)
    {
        if (Find(key) is not { } host)
        {
            return;
        }
        if (host.Access == HostAccess.Token)
        {
            Tokens.Forget(host.Pin);
        }
        kept.Remove(host);
        Storage.SaveHosts(kept);
        Changed?.Invoke();
    }

    /// <summary>Returns the kept host whose key is <paramref name="key"/>, if any.</summary>
    private KeptHost? Find(string key) => kept.Find(host => host.Key == key);

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
    internal List<HostRow> Rows() => HostRows.Build(discovered, kept);
}
