using Qdp.Core;
using Xunit;

namespace Qdp.Tests;

/// <summary>Hosts found on the network and those paired with make the app's list.</summary>
public class HostRowsTests
{
    /// <summary>A key pin.</summary>
    private static readonly string PinA = new('a', 64);

    /// <summary>Another key pin.</summary>
    private static readonly string PinB = new('b', 64);

    /// <summary>A host paired with as Desk, last reached at 10.0.0.2.</summary>
    private static readonly PairedHost Desk = new() { Name = "Desk", Address = "10.0.0.2:4433", Pin = PinA };

    /// <summary>A paired host found again streams at the address it has now.</summary>
    [Fact]
    public void ListsFoundPairedHostsReady()
    {
        var row = Assert.Single(HostRows.Build([new DiscoveredHost("Desk", "10.0.0.5:4433", PinA, false)], [Desk]));
        Assert.Equal(HostStatus.Ready, row.Status);
        Assert.Equal(new StreamTarget("Desk", "10.0.0.5:4433", Authentication.Paired, PinA), row.Target);
        Assert.True(row.CanForget);
    }

    /// <summary>A paired host not found is listed after those found, to try anyway.</summary>
    [Fact]
    public void ListsUnseenPairedHostsLast()
    {
        var rows = HostRows.Build([new DiscoveredHost("Laptop", "10.0.0.9:4433", PinB, true)], [Desk]);
        Assert.Equal(["Laptop", "Desk"], rows.Select(row => row.Name));
        Assert.Equal(HostStatus.NotSeen, rows[1].Status);
        Assert.Equal("10.0.0.2:4433", rows[1].Target!.Address);
    }

    /// <summary>A new host that offers pairing pairs; one that does not cannot be chosen.</summary>
    [Fact]
    public void PairsWithNewHostsThatOfferIt()
    {
        var rows = HostRows.Build(
            [
                new DiscoveredHost("Offering", "10.0.0.7:4433", PinB, true),
                new DiscoveredHost("Shut", "10.0.0.8:4433", PinB, false),
            ],
            []);
        Assert.Equal(HostStatus.New, rows[0].Status);
        Assert.Equal(Authentication.Pair, rows[0].Target!.Authentication);
        Assert.Equal(HostStatus.PairingOff, rows[1].Status);
        Assert.False(rows[1].CanChoose);
    }

    /// <summary>
    /// A host under a paired host's name with another key may pose as it: it
    /// is told apart, and the paired host is listed as not found.
    /// </summary>
    [Fact]
    public void WarnsOfAnotherKeyUnderAPairedName()
    {
        var rows = HostRows.Build([new DiscoveredHost("Desk", "10.0.0.6:4433", PinB, true)], [Desk]);
        Assert.Equal(HostStatus.DifferentKey, rows[0].Status);
        Assert.Equal(Authentication.Pair, rows[0].Target!.Authentication);
        Assert.Equal(HostStatus.NotSeen, rows[1].Status);
    }

    /// <summary>A host whose address is not known yet cannot be chosen.</summary>
    [Fact]
    public void WaitsForAddresses()
    {
        var row = Assert.Single(HostRows.Build([new DiscoveredHost("Desk", null, PinA, false)], []));
        Assert.Equal(HostStatus.Resolving, row.Status);
        Assert.False(row.CanChoose);
    }

    /// <summary>Hosts found are listed by name.</summary>
    [Fact]
    public void SortsFoundHostsByName()
    {
        var rows = HostRows.Build(
            [
                new DiscoveredHost("zeta", "10.0.0.3:4433", null, true),
                new DiscoveredHost("Alpha", "10.0.0.4:4433", null, true),
            ],
            []);
        Assert.Equal(["Alpha", "zeta"], rows.Select(row => row.Name));
    }
}
