using Microsoft.UI.Dispatching;

namespace Qdp.Core;

/// <summary>
/// The hosts kept between launches, paired with or streamed with a token,
/// and those on the local network, combined into the list the app shows.
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
    /// Keeps <paramref name="host"/> first, replacing the host with its pin,
    /// with <paramref name="token"/> if it accepts a token. A host whose
    /// token the Credential Locker cannot keep is not kept, as the list could
    /// not stream it.
    /// </summary>
    internal void Keep(KeptHost host, string token = "")
    {
        if (host.Access == HostAccess.Token && !Tokens.Keep(host.Pin, token))
        {
            return;
        }
        if (host.Access != HostAccess.Token && Find(host.Pin) is { Access: HostAccess.Token })
        {
            Tokens.Forget(host.Pin);
        }
        kept = [host, .. kept.Where(other => other.Pin != host.Pin)];
        Storage.SaveHosts(kept);
        Changed?.Invoke();
    }

    /// <summary>Forgets the host whose pin is <paramref name="pin"/>, and its token if it has one.</summary>
    internal void Forget(string pin)
    {
        if (Find(pin) is not { } host)
        {
            return;
        }
        if (host.Access == HostAccess.Token)
        {
            Tokens.Forget(pin);
        }
        kept.Remove(host);
        Storage.SaveHosts(kept);
        Changed?.Invoke();
    }

    /// <summary>Returns the kept host whose pin is <paramref name="pin"/>, if any.</summary>
    private KeptHost? Find(string pin) => kept.Find(host => host.Pin == pin);

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
