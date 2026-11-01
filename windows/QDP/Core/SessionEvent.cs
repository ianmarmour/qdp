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
            "pair" => new PairingCode(rest == "retry"),
            "paired" => new Paired(rest),
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
