using Qdp.Core;
using Xunit;

namespace Qdp.Tests;

/// <summary>Hosts found on the network and those kept make the app's list.</summary>
public class HostRowsTests
{
    /// <summary>A key pin.</summary>
    private static readonly string PinA = new('a', 64);

    /// <summary>Another key pin.</summary>
    private static readonly string PinB = new('b', 64);

    /// <summary>A host paired with as Desk, last reached at 10.0.0.2.</summary>
    private static readonly KeptHost Desk = new() { Name = "Desk", Address = "10.0.0.2:4433", Pin = PinA };

    /// <summary>A host streamed with a token as Lab, last reached at 10.0.0.3.</summary>
    private static readonly KeptHost Lab = new()
    {
        Name = "Lab",
        Address = "10.0.0.3:4433",
        Pin = PinB,
        Access = HostAccess.Token,
    };

    /// <summary>A host reached through the gateway as build-01, named Build.</summary>
    private static readonly KeptHost BuildMachine = new()
    {
        Name = "Build",
        Address = "build-01",
        Access = HostAccess.Gateway,
    };

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
        Assert.Equal(new[] { "Laptop", "Desk" }, rows.Select(row => row.Name));
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

    /// <summary>
    /// A host kept with a token streams with it, found or not; the token
    /// itself is taken from the Credential Locker as the stream starts.
    /// </summary>
    [Fact]
    public void StreamsTokenHostsWithTheirToken()
    {
        var found = Assert.Single(HostRows.Build([new DiscoveredHost("Lab PC", "10.0.0.4:4433", PinB, false)], [Lab]));
        Assert.Equal(HostStatus.Ready, found.Status);
        Assert.Equal(new StreamTarget("Lab PC", "10.0.0.4:4433", Authentication.Token, PinB), found.Target);
        Assert.True(found.CanForget);

        var unseen = Assert.Single(HostRows.Build([], [Lab]));
        Assert.Equal(HostStatus.NotSeen, unseen.Status);
        Assert.Equal(new StreamTarget("Lab", "10.0.0.3:4433", Authentication.Token, PinB), unseen.Target);
    }

    /// <summary>Another key under a token host's name is told apart as under a paired one's.</summary>
    [Fact]
    public void WarnsOfAnotherKeyUnderATokenHostsName()
    {
        var rows = HostRows.Build([new DiscoveredHost("Lab", "10.0.0.6:4433", PinA, true)], [Lab]);
        Assert.Equal(HostStatus.DifferentKey, rows[0].Status);
        Assert.Equal("This isn't the Lab you added", rows[0].Detail);
        Assert.Equal(HostStatus.NotSeen, rows[1].Status);
    }

    /// <summary>Screen readers say a row's name and how it stands.</summary>
    [Fact]
    public void ReadsAsNameAndDetail()
    {
        var row = Assert.Single(HostRows.Build([], [Desk]));
        Assert.Equal("Desk, Not on this network · 10.0.0.2:4433", row.ToString());
    }

    /// <summary>A host whose address is not known yet cannot be chosen.</summary>
    [Fact]
    public void WaitsForAddresses()
    {
        var row = Assert.Single(HostRows.Build([new DiscoveredHost("Desk", null, PinA, false)], []));
        Assert.Equal(HostStatus.Resolving, row.Status);
        Assert.False(row.CanChoose);
    }

    /// <summary>
    /// A host reached through the gateway streams through it, by what the
    /// gateway knows it as; it is listed after those found and before those
    /// not found, and never as a host on the network, whatever its name.
    /// </summary>
    [Fact]
    public void ListsHostsReachedThroughTheGateway()
    {
        var rows = HostRows.Build(
            [new DiscoveredHost("Build", "10.0.0.7:4433", PinB, true)],
            [BuildMachine, Desk]);
        Assert.Equal(
            new[] { HostStatus.New, HostStatus.Gateway, HostStatus.NotSeen },
            rows.Select(row => row.Status));
        var row = rows[1];
        Assert.Equal(new StreamTarget("Build", "build-01", Authentication.Gateway), row.Target);
        Assert.Equal("Build, Through the gateway · build-01", row.ToString());
        Assert.Equal("Connect", row.Action);
        Assert.True(row.CanForget);
    }

    /// <summary>Hosts reached through the gateway are kept by what it knows them as, the others by their pin.</summary>
    [Fact]
    public void KeysHostsByPinOrGatewayTarget()
    {
        Assert.Equal(PinA, Desk.Key);
        Assert.Equal("gateway:build-01", BuildMachine.Key);
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
        Assert.Equal(new[] { "Alpha", "zeta" }, rows.Select(row => row.Name));
    }
}
