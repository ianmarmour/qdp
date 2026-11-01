namespace Qdp.Core;

/// <summary>
/// A host on the local network: its service name, its address and port once
/// resolved, the key pin it claims and whether it offers pairing. Nothing
/// here is authenticated: the pin only recognizes a host already paired
/// with, which the connection then verifies (Section 3.13).
/// </summary>
/// <param name="Name">Its DNS-SD instance name.</param>
/// <param name="Address">Its address and port, once resolved.</param>
/// <param name="Pin">The key pin it claims, if any.</param>
/// <param name="Pairing">Whether it offers pairing.</param>
internal sealed record DiscoveredHost(string Name, string? Address, string? Pin, bool Pairing);

/// <summary>How a host the list shows stands, and what choosing it does.</summary>
public enum HostStatus
{
    /// <summary>Kept and on this network: choosing it streams.</summary>
    Ready,

    /// <summary>Kept, but not seen on this network: choosing it tries anyway.</summary>
    NotSeen,

    /// <summary>New and offering pairing: choosing it pairs.</summary>
    New,

    /// <summary>New, with pairing off.</summary>
    PairingOff,

    /// <summary>
    /// Found under a kept host's name with another key, which may pose as
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

    /// <summary>The kept host it is, if kept.</summary>
    internal KeptHost? Kept { get; init; }

    /// <summary>The row itself, for a binding that carries it.</summary>
    public HostRow Self => this;

    /// <summary>Whether choosing it does anything.</summary>
    public bool CanChoose => Target is not null;

    /// <summary>Whether it can be forgotten.</summary>
    public bool CanForget => Kept is not null;

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

    /// <summary>
    /// What screen readers say of the row, as the list's items are named by
    /// their data: its name and how it stands.
    /// </summary>
    public override string ToString() => $"{Name}, {Detail}";
}

/// <summary>
/// The list the app shows: hosts found on the network, recognized as kept by
/// their pin, then kept hosts not found, by the same rules as the Mac and
/// Android apps.
/// </summary>
internal static class HostRows
{
    /// <summary>
    /// The rows to list: hosts found on the network, recognized as kept by
    /// their pin, then kept hosts not found, most recently kept first.
    /// </summary>
    internal static List<HostRow> Build(IEnumerable<DiscoveredHost> discovered, IReadOnlyList<KeptHost> kept)
    {
        var found = new HashSet<string>();
        var rows = new List<HostRow>();
        foreach (var host in discovered.OrderBy(host => host.Name, StringComparer.CurrentCultureIgnoreCase))
        {
            var match = kept.FirstOrDefault(other => other.Pin == host.Pin);
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
                    Target = match.Target(host.Name, host.Address),
                    Kept = match,
                });
                continue;
            }
            if (kept.FirstOrDefault(other => other.Name == host.Name) is { } namesake)
            {
                var known = namesake.Access == HostAccess.Token ? "added" : "paired with";
                rows.Add(new HostRow
                {
                    Name = host.Name,
                    Detail = $"This isn't the {host.Name} you {known}",
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
        rows.AddRange(kept.Where(host => !found.Contains(host.Pin)).Select(host => new HostRow
        {
            Name = host.Name,
            Detail = $"Not on this network · {host.Address}",
            Status = HostStatus.NotSeen,
            Target = host.Target(host.Name, host.Address),
            Kept = host,
        }));
        return rows;
    }
}
