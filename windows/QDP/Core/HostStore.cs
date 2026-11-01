using Microsoft.UI.Dispatching;

namespace Qdp.Core;

/// <summary>How a host the list shows stands, and what choosing it does.</summary>
public enum HostStatus
{
    /// <summary>Paired and on this network: choosing it streams.</summary>
    Ready,

    /// <summary>Paired, but not seen on this network: choosing it tries anyway.</summary>
    NotSeen,

    /// <summary>New and offering pairing: choosing it pairs.</summary>
    New,

    /// <summary>New, with pairing off.</summary>
    PairingOff,

    /// <summary>
    /// Found under a paired host's name with another key, which may pose as
    /// it.
    /// </summary>
    DifferentKey,

    /// <summary>Found, but its address is not known yet.</summary>
    Resolving,
}

/// <summary>A host the list shows.</summary>
public sealed class HostRow
{
    /// <summary>The host's name.</summary>
    public string Name { get; init; } = "";

    /// <summary>What the row says of it.</summary>
    public string Detail { get; init; } = "";

    /// <summary>How it stands.</summary>
    public HostStatus Status { get; init; }

    /// <summary>The stream choosing it starts, if it starts one.</summary>
    internal StreamTarget? Target { get; init; }

    /// <summary>The paired host it is, if paired.</summary>
    internal PairedHost? Paired { get; init; }

    /// <summary>The row itself, for a binding that carries it.</summary>
    public HostRow Self => this;

    /// <summary>Whether choosing it does anything.</summary>
    public bool CanChoose => Target is not null;

    /// <summary>Whether it can be forgotten.</summary>
    public bool CanForget => Paired is not null;

    /// <summary>The action choosing it takes, for its button.</summary>
    public string Action => Status switch
    {
        HostStatus.Ready => "Connect",
        HostStatus.NotSeen => "Try to connect",
        HostStatus.New or HostStatus.DifferentKey => "Pair",
        _ => "",
    };

    /// <summary>The Segoe Fluent Icons glyph of its status.</summary>
    public string StatusGlyph => Status switch
    {
        HostStatus.Ready => "\uE73E",
        HostStatus.NotSeen => "\uE774",
        HostStatus.New => "\uE710",
        HostStatus.DifferentKey => "\uE7BA",
        HostStatus.PairingOff => "\uE72E",
        _ => "\uE895",
    };
}

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

    /// <summary>
    /// The rows to list: hosts found on the network, recognized as paired by
    /// their pin, then paired hosts not found.
    /// </summary>
    internal List<HostRow> Rows()
    {
        var found = new HashSet<string>();
        var rows = new List<HostRow>();
        foreach (var host in discovered.OrderBy(host => host.Name, StringComparer.CurrentCultureIgnoreCase))
        {
            var match = paired.FirstOrDefault(paired => paired.Pin == host.Pin);
            if (host.Address is null)
            {
                rows.Add(new HostRow { Name = host.Name, Detail = "Finding its address…", Status = HostStatus.Resolving });
                continue;
            }
            if (match is not null)
            {
                found.Add(match.Pin);
                rows.Add(new HostRow
                {
                    Name = host.Name,
                    Detail = $"Ready · {host.Address}",
                    Status = HostStatus.Ready,
                    Target = new StreamTarget(host.Name, host.Address, Authentication.Paired, match.Pin),
                    Paired = match,
                });
                continue;
            }
            if (paired.Any(other => other.Name == host.Name))
            {
                rows.Add(new HostRow
                {
                    Name = host.Name,
                    Detail = $"This isn't the {host.Name} you paired with",
                    Status = HostStatus.DifferentKey,
                    Target = host.Pairing ? new StreamTarget(host.Name, host.Address, Authentication.Pair) : null,
                });
                continue;
            }
            rows.Add(host.Pairing
                ? new HostRow
                {
                    Name = host.Name,
                    Detail = $"New · {host.Address}",
                    Status = HostStatus.New,
                    Target = new StreamTarget(host.Name, host.Address, Authentication.Pair),
                }
                : new HostRow
                {
                    Name = host.Name,
                    Detail = "Pairing is off on this computer",
                    Status = HostStatus.PairingOff,
                });
        }
        rows.AddRange(paired.Where(host => !found.Contains(host.Pin)).Select(host => new HostRow
        {
            Name = host.Name,
            Detail = $"Not on this network · {host.Address}",
            Status = HostStatus.NotSeen,
            Target = new StreamTarget(host.Name, host.Address, Authentication.Paired, host.Pin),
            Paired = host,
        }));
        return rows;
    }
}
