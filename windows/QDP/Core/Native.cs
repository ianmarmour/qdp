using System.Runtime.InteropServices;

namespace Qdp.Core;

/// <summary>
/// The Rust core's C interface, crates/qdp-windows/include/qdp_windows.h,
/// which qdp_windows.dll exports.
/// </summary>
internal static partial class Native
{
    /// <summary>The core's library, next to the app.</summary>
    private const string Library = "qdp_windows";

    /// <summary><c>QDP_AUTH_TOKEN</c>: a pinned key and a token.</summary>
    internal const int AuthToken = 0;

    /// <summary><c>QDP_AUTH_PAIRED</c>: a host the device paired with.</summary>
    internal const int AuthPaired = 1;

    /// <summary><c>QDP_AUTH_PAIR</c>: pairing with the code the host shows.</summary>
    internal const int AuthPair = 2;

    /// <summary>What a session connects with, as <c>QdpConfig</c> lays it out.</summary>
    [StructLayout(LayoutKind.Sequential)]
    internal struct QdpConfig
    {
        public IntPtr Host;
        public IntPtr Pin;
        public IntPtr Token;
        public IntPtr DeviceName;
        public IntPtr Title;
        public IntPtr IconPath;
        public nint OwnerWindow;
        public int Authentication;
        public uint Width;
        public uint Height;
        public int Decoder;
        public int Microphone;
        public int Screen;
        public int Sync;
        public int Shortcuts;
        public int Clipboard;
    }

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial void qdp_init(string dataDirectory);

    [LibraryImport(Library)]
    internal static partial IntPtr qdp_version();

    [LibraryImport(Library)]
    internal static partial IntPtr qdp_client_pin();

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static partial bool qdp_is_pin(string text);

    [LibraryImport(Library)]
    internal static partial IntPtr qdp_session_start(in QdpConfig config, out IntPtr error);

    [LibraryImport(Library)]
    internal static partial IntPtr qdp_session_next_event(IntPtr session, uint timeoutMillis);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial void qdp_session_pairing_code(IntPtr session, string? code);

    [LibraryImport(Library)]
    internal static partial void qdp_session_stop(IntPtr session);

    [LibraryImport(Library)]
    internal static partial void qdp_session_free(IntPtr session);

    [LibraryImport(Library)]
    internal static partial void qdp_string_free(IntPtr text);

    /// <summary>
    /// Returns a string the core returned, freeing it, or null for none.
    /// </summary>
    internal static string? TakeString(IntPtr text)
    {
        if (text == IntPtr.Zero)
        {
            return null;
        }
        try
        {
            return Marshal.PtrToStringUTF8(text);
        }
        finally
        {
            qdp_string_free(text);
        }
    }

    /// <summary>Returns the core's version.</summary>
    internal static string Version => Marshal.PtrToStringUTF8(qdp_version()) ?? "unknown";

    /// <summary>
    /// Returns the device's key pin, which a host's owner may add instead of
    /// pairing, or null if the device's key cannot be kept.
    /// </summary>
    internal static string? ClientPin => TakeString(qdp_client_pin());
}
