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

    /// <summary>Through the organization's gateway, which vouches for the user.</summary>
    Gateway,
}

/// <summary>The host a stream connects to and how it authenticates.</summary>
/// <param name="Name">The host's name for people.</param>
/// <param name="Address">
/// Its address or name, with or without a port; through the gateway, what
/// the gateway knows it as.
/// </param>
/// <param name="Authentication">How the stream authenticates.</param>
/// <param name="Pin">The host's key pin, empty while pairing.</param>
/// <param name="Token">The token, for token authentication.</param>
internal sealed record StreamTarget(
    string Name,
    string Address,
    Authentication Authentication,
    string Pin = "",
    string Token = "");

/// <summary>How the device authenticates with a host it keeps.</summary>
public enum HostAccess
{
    /// <summary>It paired with the host, which accepts its key.</summary>
    Paired,

    /// <summary>The host accepts a token, which Windows' Credential Locker keeps.</summary>
    Token,

    /// <summary>It is reached through the organization's gateway, which vouches for the user.</summary>
    Gateway,
}

/// <summary>
/// A host the device keeps in its list, having paired with it, streamed it
/// with a token or reached it through the gateway: its name, the address it
/// was last reached at, its key pin, and how the device authenticates with
/// it.
/// </summary>
public sealed class KeptHost
{
    /// <summary>What <see cref="Key"/> starts with for a host reached through the gateway.</summary>
    private const string GatewayKeyPrefix = "gateway:";

    /// <summary>The host's name for people.</summary>
    public string Name { get; set; } = "";

    /// <summary>
    /// The address it was last reached at; through the gateway, what the
    /// gateway knows it as: its ID there, or its name inside the network.
    /// </summary>
    public string Address { get; set; } = "";

    /// <summary>Its key pin, in hexadecimal; empty for one reached through the gateway, which checks it.</summary>
    public string Pin { get; set; } = "";

    /// <summary>How the device authenticates with it.</summary>
    public HostAccess Access { get; set; } = HostAccess.Paired;

    /// <summary>
    /// What tells it from the other hosts kept: its key pin, or what the
    /// gateway knows it as.
    /// </summary>
    internal string Key => Access == HostAccess.Gateway ? GatewayKeyPrefix + Address : Pin;

    /// <summary>
    /// The stream of the host under <paramref name="name"/> at
    /// <paramref name="address"/>. A token host's token is not in it: it is
    /// taken from the Credential Locker as the stream starts.
    /// </summary>
    internal StreamTarget Target(string name, string address) => new(
        name,
        address,
        Access switch
        {
            HostAccess.Token => Authentication.Token,
            HostAccess.Gateway => Authentication.Gateway,
            _ => Authentication.Paired,
        },
        Pin);
}

/// <summary>
/// The organization's gateway that hosts are reached through (Section 13
/// of the specification), as the user set it; empty fields are unset.
/// </summary>
public sealed class GatewaySettings
{
    /// <summary>Its address: a name or IP address, with an optional port.</summary>
    public string Address { get; set; } = "";

    /// <summary>
    /// Its key pin, for a gateway whose certificate the system's certificate
    /// authorities do not vouch for.
    /// </summary>
    public string Pin { get; set; } = "";

    /// <summary>The user's name in the organization's directory.</summary>
    public string User { get; set; } = "";

    /// <summary>Whether a gateway is set.</summary>
    internal bool IsSet => Address.Trim().Length > 0;
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

    /// <summary>Whether the stream's statistics show over it as it starts.</summary>
    public bool ShowStatistics { get; set; }

    /// <summary>When Windows' shortcuts go to the computer streamed.</summary>
    public ShortcutsChoice Shortcuts { get; set; } = ShortcutsChoice.FullScreen;

    /// <summary>The organization's gateway, if the user set one.</summary>
    public GatewaySettings Gateway { get; set; } = new();

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
