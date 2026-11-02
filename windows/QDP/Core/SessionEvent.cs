using System.Globalization;

namespace Qdp.Core;

/// <summary>The last second's video, as a <c>stats</c> line tells it.</summary>
/// <param name="Frames">The frames shown.</param>
/// <param name="LatencyMs">Their mean latency, capture to decoded, once known.</param>
/// <param name="DecodeMs">Their mean decode time.</param>
internal sealed record VideoStats(int Frames, double? LatencyMs, double DecodeMs);

/// <summary>Something a session tells the app, from one of its lines.</summary>
internal abstract record SessionEvent
{
    /// <summary>The word that asks again, after an answer that did not work.</summary>
    private const string RetryWord = "retry";

    /// <summary>What a <c>totp</c> line has in place of an enrollment URI when it has none.</summary>
    private const string NoEnrollmentWord = "-";

    /// <summary>Connecting to the host.</summary>
    internal sealed record Connecting : SessionEvent;

    /// <summary>Connected; the session is starting.</summary>
    internal sealed record Connected : SessionEvent;

    /// <summary>The connection was lost and is being made again.</summary>
    internal sealed record Reconnecting(string Error) : SessionEvent;

    /// <summary>The stream window shows the video.</summary>
    internal sealed record WindowShown : SessionEvent;

    /// <summary>The last second's video.</summary>
    internal sealed record Stats(VideoStats Video) : SessionEvent;

    /// <summary>The user turned the microphone on or off in the stream window.</summary>
    internal sealed record Microphone(bool On) : SessionEvent;

    /// <summary>Pairing needs the code the host shows; again after one that did not work.</summary>
    internal sealed record PairingCode(bool Retry) : SessionEvent;

    /// <summary>Pairing succeeded with the host whose key pin this is.</summary>
    internal sealed record Paired(string Pin) : SessionEvent;

    /// <summary>
    /// The gateway named <paramref name="Gateway"/> needs the user's name and
    /// password; again after an attempt that failed.
    /// </summary>
    internal sealed record GatewaySignIn(bool Retry, string Gateway) : SessionEvent;

    /// <summary>
    /// The gateway named <paramref name="Gateway"/> needs a code from the
    /// user's authenticator app; again after one that did not work. The
    /// <paramref name="Enrollment"/> URI, to show as a QR code, adds the
    /// gateway to an app that does not have it yet.
    /// </summary>
    internal sealed record GatewayCode(bool Retry, string? Enrollment, string Gateway) : SessionEvent;

    /// <summary>
    /// The gateway gave a session token, in hexadecimal, valid for
    /// <paramref name="LifetimeSeconds"/>, which later sessions start with
    /// so that they ask nothing.
    /// </summary>
    internal sealed record GatewaySession(string Token, long LifetimeSeconds) : SessionEvent;

    /// <summary>The session ended; with why, if it failed.</summary>
    internal sealed record Ended(string? Failure) : SessionEvent;

    /// <summary>
    /// Returns the event a line tells, or null for one the app does not
    /// use.
    /// </summary>
    internal static SessionEvent? Parse(string line)
    {
        var separator = line.IndexOf(' ');
        var word = separator < 0 ? line : line[..separator];
        var rest = separator < 0 ? "" : line[(separator + 1)..];
        var values = rest.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        return word switch
        {
            "connecting" => new Connecting(),
            "connected" => new Connected(),
            "reconnecting" => new Reconnecting(AfterFirstValue(rest)),
            "window" when rest == "shown" => new WindowShown(),
            "stats" when values.Length >= 3 => new Stats(new VideoStats(
                int.TryParse(values[0], NumberStyles.Integer, CultureInfo.InvariantCulture, out var frames) ? frames : 0,
                double.TryParse(values[1], NumberStyles.Float, CultureInfo.InvariantCulture, out var latency) ? latency : null,
                double.TryParse(values[2], NumberStyles.Float, CultureInfo.InvariantCulture, out var decode) ? decode : 0)),
            "microphone" => new Microphone(rest == "on"),
            "pair" => new PairingCode(rest == RetryWord),
            "paired" => new Paired(rest),
            "signin" when values.Length >= 1 => new GatewaySignIn(values[0] == RetryWord, AfterFirstValue(rest)),
            "totp" when values.Length >= 2 => new GatewayCode(
                values[0] == RetryWord,
                values[1] == NoEnrollmentWord ? null : values[1],
                AfterFirstValue(AfterFirstValue(rest))),
            "gatewaysession" when values.Length >= 2
                && long.TryParse(values[1], NumberStyles.Integer, CultureInfo.InvariantCulture, out var lifetime) =>
                new GatewaySession(values[0], lifetime),
            "ended" => new Ended(null),
            "failed" => new Ended(rest.Length > 0 ? rest : "The session failed."),
            _ => null,
        };
    }

    /// <summary>Returns the values after the first, as <c>reconnecting</c>'s error follows its delay.</summary>
    private static string AfterFirstValue(string values)
    {
        var separator = values.IndexOf(' ');
        return separator < 0 ? "" : values[(separator + 1)..];
    }
}
