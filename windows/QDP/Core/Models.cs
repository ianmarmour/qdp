namespace Qdp.Core;

/// <summary>How a stream authenticates with its host.</summary>
internal enum Authentication
{
    /// <summary>A host the device paired with, which accepts its key.</summary>
    Paired,

    /// <summary>Pairing with the code the host shows.</summary>
    Pair,

    /// <summary>The host's key pin and a token it accepts.</summary>
    Token,
}

/// <summary>The host a stream connects to and how it authenticates.</summary>
/// <param name="Name">The host's name for people.</param>
/// <param name="Address">Its address or name, with or without a port.</param>
/// <param name="Authentication">How the stream authenticates.</param>
/// <param name="Pin">The host's key pin, empty while pairing.</param>
/// <param name="Token">The token, for token authentication.</param>
internal sealed record StreamTarget(
    string Name,
    string Address,
    Authentication Authentication,
    string Pin = "",
    string Token = "");

/// <summary>
/// A host the device paired with: its name, the address it was last reached
/// at, and its key pin.
/// </summary>
public sealed class PairedHost
{
    /// <summary>The host's name for people.</summary>
    public string Name { get; set; } = "";

    /// <summary>The address it was last reached at.</summary>
    public string Address { get; set; } = "";

    /// <summary>Its key pin, in hexadecimal.</summary>
    public string Pin { get; set; } = "";
}

/// <summary>The video size a stream asks the host for.</summary>
public enum Resolution
{
    /// <summary>The stream window's own size, pixel for pixel.</summary>
    Window,

    /// <summary>720 rows, in the window's shape.</summary>
    P720,

    /// <summary>1080 rows.</summary>
    P1080,

    /// <summary>1440 rows.</summary>
    P1440,

    /// <summary>2160 rows.</summary>
    P2160,
}

/// <summary>Where the video is decoded.</summary>
public enum DecoderChoice
{
    /// <summary>On the GPU where it decodes AV1, on the processor otherwise.</summary>
    Automatic,

    /// <summary>On the GPU only.</summary>
    Hardware,

    /// <summary>On the processor only.</summary>
    Software,
}

/// <summary>
/// When Windows' own shortcuts, such as the Windows key, Alt+Tab and
/// Alt+F4, go to the computer streamed instead of acting on this PC.
/// </summary>
public enum ShortcutsChoice
{
    /// <summary>While the stream is full screen.</summary>
    FullScreen,

    /// <summary>Whenever the stream's window is in front.</summary>
    Always,

    /// <summary>Never.</summary>
    Never,
}

/// <summary>What the user chose for streams, kept between launches.</summary>
public sealed class AppSettings
{
    /// <summary>The video size to ask for.</summary>
    public Resolution Resolution { get; set; } = Resolution.Window;

    /// <summary>Where the video is decoded.</summary>
    public DecoderChoice Decoder { get; set; } = DecoderChoice.Automatic;

    /// <summary>Whether streams open full screen.</summary>
    public bool FullScreen { get; set; } = true;

    /// <summary>
    /// Whether pictures are shown as soon as they are drawn, tearing, for
    /// the least delay, rather than at the next refresh.
    /// </summary>
    public bool AllowTearing { get; set; }

    /// <summary>Whether the microphone is on as streams start.</summary>
    public bool Microphone { get; set; }

    /// <summary>Whether the clipboard is shared with the computer streamed, both ways.</summary>
    public bool ShareClipboard { get; set; } = true;

    /// <summary>When Windows' shortcuts go to the computer streamed.</summary>
    public ShortcutsChoice Shortcuts { get; set; } = ShortcutsChoice.FullScreen;

    /// <summary>The rows a resolution asks for, or 0 for the window's own.</summary>
    internal static uint Rows(Resolution resolution) => resolution switch
    {
        Resolution.P720 => 720,
        Resolution.P1080 => 1080,
        Resolution.P1440 => 1440,
        Resolution.P2160 => 2160,
        _ => 0,
    };
}
