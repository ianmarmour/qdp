using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;

namespace Qdp.Core;

/// <summary>The gateway a stream reaches its host through.</summary>
/// <param name="Address">The gateway's address.</param>
/// <param name="Pin">Its key pin, or empty to verify it by the system's certificate authorities.</param>
/// <param name="Token">The session token of the last sign-in, or empty to sign in.</param>
internal sealed record GatewayRoute(string Address, string Pin, string Token);

/// <summary>
/// A session the core runs in a stream window of its own, telling the app
/// of it on the UI thread until it ends.
/// </summary>
internal sealed class StreamSession
{
    /// <summary>How long the reader waits for a line before waiting again.</summary>
    private const uint LineWaitMillis = 500;

    /// <summary>Guards the handle against the reader freeing it while the UI uses it.</summary>
    private readonly object gate = new();

    /// <summary>The core's session, until the reader frees it.</summary>
    private IntPtr handle;

    /// <summary>Where the app hears of the session.</summary>
    private readonly DispatcherQueue dispatcher;

    /// <summary>The stream's target.</summary>
    internal StreamTarget Target { get; }

    /// <summary>The gateway the stream reaches its host through, if it does.</summary>
    internal GatewayRoute? Gateway { get; }

    /// <summary>Raised on the UI thread for each event of the session.</summary>
    internal event Action<SessionEvent>? EventReceived;

    private StreamSession(IntPtr handle, StreamTarget target, GatewayRoute? gateway, DispatcherQueue dispatcher)
    {
        this.handle = handle;
        this.dispatcher = dispatcher;
        Target = target;
        Gateway = gateway;
    }

    /// <summary>
    /// Starts a stream of <paramref name="target"/>, through
    /// <paramref name="gateway"/> if given, as <paramref name="settings"/>
    /// say, opening on <paramref name="owner"/>'s monitor, or returns why it
    /// cannot.
    /// </summary>
    internal static StreamSession Start(
        StreamTarget target,
        GatewayRoute? gateway,
        AppSettings settings,
        IntPtr owner,
        DispatcherQueue dispatcher,
        Action<SessionEvent> received)
    {
        var strings = new List<IntPtr>();
        IntPtr Utf8(string? text)
        {
            var pointer = Marshal.StringToCoTaskMemUTF8(text ?? "");
            strings.Add(pointer);
            return pointer;
        }
        try
        {
            var config = new Native.QdpConfig
            {
                Host = Utf8(target.Address),
                Pin = Utf8(target.Pin),
                Token = Utf8(target.Token),
                DeviceName = Utf8(Environment.MachineName),
                Title = Utf8($"{target.Name} – QDP"),
                IconPath = Utf8(Path.Combine(AppContext.BaseDirectory, "Assets", "qdp.ico")),
                OwnerWindow = owner,
                Authentication = target.Authentication switch
                {
                    Authentication.Paired => Native.AuthPaired,
                    Authentication.Pair => Native.AuthPair,
                    Authentication.Gateway => Native.AuthGateway,
                    _ => Native.AuthToken,
                },
                Width = 0,
                Height = AppSettings.Rows(settings.Resolution),
                Decoder = (int)settings.Decoder,
                Microphone = settings.Microphone ? 1 : 0,
                Screen = settings.FullScreen ? 1 : 0,
                Sync = settings.AllowTearing ? 1 : 0,
                Shortcuts = (int)settings.Shortcuts,
                Clipboard = settings.ShareClipboard ? 1 : 0,
                Statistics = settings.ShowStatistics ? 1 : 0,
                GatewayAddress = Utf8(gateway?.Address),
                GatewayPin = Utf8(gateway?.Pin),
                GatewayToken = Utf8(gateway?.Token),
            };
            var handle = Native.qdp_session_start(in config, out var error);
            if (handle == IntPtr.Zero)
            {
                throw new InvalidOperationException(Native.TakeString(error) ?? "The stream could not start.");
            }
            var session = new StreamSession(handle, target, gateway, dispatcher);
            session.EventReceived += received;
            var reader = new Thread(session.Read) { IsBackground = true, Name = "QDP session lines" };
            reader.Start();
            return session;
        }
        finally
        {
            strings.ForEach(Marshal.FreeCoTaskMem);
        }
    }

    /// <summary>Answers a request for the pairing code, or declines with null.</summary>
    internal void AnswerPairing(string? code)
    {
        lock (gate)
        {
            if (handle != IntPtr.Zero)
            {
                Native.qdp_session_pairing_code(handle, code);
            }
        }
    }

    /// <summary>
    /// Answers the gateway's request for the user's name and password, or
    /// declines with a null <paramref name="user"/>.
    /// </summary>
    internal void AnswerSignIn(string? user, string? password)
    {
        lock (gate)
        {
            if (handle != IntPtr.Zero)
            {
                Native.qdp_session_sign_in(handle, user, password);
            }
        }
    }

    /// <summary>Answers the gateway's request for a code from the user's authenticator app, or declines with null.</summary>
    internal void AnswerSignInCode(string? code)
    {
        lock (gate)
        {
            if (handle != IntPtr.Zero)
            {
                Native.qdp_session_sign_in_code(handle, code);
            }
        }
    }

    /// <summary>Ends the session; its last event follows once it has ended.</summary>
    internal void Stop()
    {
        lock (gate)
        {
            if (handle != IntPtr.Zero)
            {
                Native.qdp_session_stop(handle);
            }
        }
    }

    /// <summary>
    /// Reads the session's lines until the last, telling the app of each,
    /// then frees the session.
    /// </summary>
    private void Read()
    {
        while (true)
        {
            var line = Native.TakeString(Native.qdp_session_next_event(handle, LineWaitMillis));
            if (line is null)
            {
                continue;
            }
            var received = SessionEvent.Parse(line);
            if (received is not null)
            {
                dispatcher.TryEnqueue(() => EventReceived?.Invoke(received));
            }
            if (received is SessionEvent.Ended)
            {
                break;
            }
        }
        lock (gate)
        {
            Native.qdp_session_free(handle);
            handle = IntPtr.Zero;
        }
    }
}
