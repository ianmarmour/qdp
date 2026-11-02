using Qdp.Core;
using Xunit;

namespace Qdp.Tests;

/// <summary>The lines the core tells become the app's events.</summary>
public class SessionEventTests
{
    /// <summary>A second's statistics give its frames, latency and decoding.</summary>
    [Fact]
    public void ParsesStatistics()
    {
        var stats = Assert.IsType<SessionEvent.Stats>(
            SessionEvent.Parse("stats 120 4.2 1.3 18.4 14.1 1.0 0.3 3.0"));
        Assert.Equal(new VideoStats(120, 4.2, 1.3), stats.Video);
    }

    /// <summary>A latency not known yet is none.</summary>
    [Fact]
    public void ParsesUnknownLatency()
    {
        var stats = Assert.IsType<SessionEvent.Stats>(SessionEvent.Parse("stats 0 - 0.0 - - - - -"));
        Assert.Null(stats.Video.LatencyMs);
        Assert.Equal(0, stats.Video.Frames);
    }

    /// <summary>A session ends normally, or fails with its reason.</summary>
    [Fact]
    public void ParsesEndings()
    {
        Assert.Equal(new SessionEvent.Ended(null), SessionEvent.Parse("ended"));
        Assert.Equal(new SessionEvent.Ended("closed by peer"), SessionEvent.Parse("failed closed by peer"));
        Assert.Equal(new SessionEvent.Ended("The session failed."), SessionEvent.Parse("failed"));
    }

    /// <summary>Reconnecting gives the error after its delay.</summary>
    [Fact]
    public void ParsesReconnecting()
    {
        Assert.Equal(
            new SessionEvent.Reconnecting("timed out"),
            SessionEvent.Parse("reconnecting 500 timed out"));
    }

    /// <summary>Pairing, the stream window and the microphone are told as they happen.</summary>
    [Fact]
    public void ParsesTheRest()
    {
        Assert.Equal(new SessionEvent.Connecting(), SessionEvent.Parse("connecting"));
        Assert.Equal(new SessionEvent.Connected(), SessionEvent.Parse("connected"));
        Assert.Equal(new SessionEvent.WindowShown(), SessionEvent.Parse("window shown"));
        Assert.Equal(new SessionEvent.PairingCode(false), SessionEvent.Parse("pair first"));
        Assert.Equal(new SessionEvent.PairingCode(true), SessionEvent.Parse("pair retry"));
        Assert.Equal(new SessionEvent.Paired("ab12"), SessionEvent.Parse("paired ab12"));
        Assert.Equal(new SessionEvent.Microphone(true), SessionEvent.Parse("microphone on"));
        Assert.Equal(new SessionEvent.Microphone(false), SessionEvent.Parse("microphone off"));
    }

    /// <summary>
    /// The gateway asks for the user's name and password, then a code, with
    /// the URI that adds it to an authenticator app the first time, and
    /// gives a session token; its name may have spaces.
    /// </summary>
    [Fact]
    public void ParsesTheGatewaysSignIn()
    {
        Assert.Equal(
            new SessionEvent.GatewaySignIn(false, "Example Corp"),
            SessionEvent.Parse("signin first Example Corp"));
        Assert.Equal(
            new SessionEvent.GatewaySignIn(true, "gw.corp.example.com"),
            SessionEvent.Parse("signin retry gw.corp.example.com"));
        Assert.Equal(
            new SessionEvent.GatewayCode(false, "otpauth://totp/Example%20Corp:alice?secret=AB", "Example Corp"),
            SessionEvent.Parse("totp first otpauth://totp/Example%20Corp:alice?secret=AB Example Corp"));
        Assert.Equal(
            new SessionEvent.GatewayCode(true, null, "Example Corp"),
            SessionEvent.Parse("totp retry - Example Corp"));
        Assert.Equal(
            new SessionEvent.GatewaySession("01ab", 43200),
            SessionEvent.Parse("gatewaysession 01ab 43200"));
        Assert.Null(SessionEvent.Parse("gatewaysession 01ab soon"));
    }

    /// <summary>Lines the app does not use are none of its events.</summary>
    [Fact]
    public void IgnoresOtherLines()
    {
        Assert.Null(SessionEvent.Parse("video 1920 1080"));
        Assert.Null(SessionEvent.Parse("pointer relative"));
        Assert.Null(SessionEvent.Parse("something new 1 2 3"));
        Assert.Null(SessionEvent.Parse("window hidden"));
    }
}
