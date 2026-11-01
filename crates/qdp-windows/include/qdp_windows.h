/*
 * The C interface of the Windows app's core of Quick Desktop Protocol
 * (QDP), which qdp_windows.dll exports and the app calls from C#.
 *
 * Strings are UTF-8. Strings the core returns are the app's to free with
 * qdp_string_free, except qdp_version's.
 *
 * A session runs in a stream window of the core's own: qdp_session_start
 * opens it hidden and returns at once; the window shows once the video
 * arrives, reads the user's keyboard, mouse, touch, pen and gamepads, and
 * closes when the session ends. A thread of the app's reads the session's
 * lines with qdp_session_next_event until the last, "ended" or
 * "failed <error>", and then frees the session with qdp_session_free.
 *
 * The lines, each a word followed by its values, separated by spaces:
 *
 *   connecting                     Connecting to the host.
 *   connected                      Connected; the session is starting.
 *   reconnecting <ms> <error>      The connection was lost; reconnecting
 *                                  in <ms> milliseconds.
 *   video <width> <height>         The video's size, in pixels, changed.
 *   window shown                   The stream window shows the video; the
 *                                  app may step back.
 *   pointer absolute|relative      The host's pointer mode changed; in
 *                                  relative mode the window captures the
 *                                  pointer.
 *   microphone on|off              The user turned the microphone on or
 *                                  off in the stream window.
 *   stats <fps> <latency_ms|-> <decode_ms> <worst_ms> <to_first_fragment_ms>
 *         <transfer_ms> <queued_ms> <worst_decode_ms>
 *                                  The last second's video; "-" for what
 *                                  is not known.
 *   pair first|retry               Pairing needs the code the host shows;
 *                                  answer with qdp_session_pairing_code.
 *   paired <pin>                   Pairing succeeded with the host whose
 *                                  key pin is <pin>, in hexadecimal, which
 *                                  the app keeps to connect with from then
 *                                  on.
 *   ended                          The session ended: the user or the host
 *                                  closed it.
 *   failed <error>                 The session failed.
 *
 * Lines the app does not know are to be ignored.
 *
 * In the stream window, with Ctrl, Alt and Shift held, which the host is
 * not sent:
 *
 *   Z  releases the pointer and the keys held.
 *   M  turns the microphone on or off.
 *   X  goes full screen, or back to a window.
 *   Q  ends the session.
 */

#ifndef QDP_WINDOWS_H
#define QDP_WINDOWS_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* How a session authenticates. */
enum QdpAuthentication {
    /* The host's key is pinned and the device presents a token. */
    QDP_AUTH_TOKEN = 0,
    /* A host the device paired with, whose key is pinned. */
    QDP_AUTH_PAIRED = 1,
    /* Pairing with the code the host shows. */
    QDP_AUTH_PAIR = 2,
};

/* Where the video is decoded. */
enum QdpDecoder {
    /* On the GPU where it decodes AV1, on the processor otherwise. */
    QDP_DECODER_AUTO = 0,
    /* On the GPU only. */
    QDP_DECODER_HARDWARE = 1,
    /* On the processor only. */
    QDP_DECODER_SOFTWARE = 2,
};

/* How the stream window opens. */
enum QdpScreen {
    /* In a window the user may move and resize. */
    QDP_SCREEN_WINDOWED = 0,
    /* Over the whole monitor, without borders. */
    QDP_SCREEN_FULL = 1,
};

/* How the video meets the display's refresh. */
enum QdpSync {
    /* Each picture waits for the next refresh: no tearing, at most a
     * refresh of delay. */
    QDP_SYNC_VSYNC = 0,
    /* Each picture is shown as soon as it is drawn: the least delay,
     * tearing where the display does not refresh variably. */
    QDP_SYNC_TEARING = 1,
};

/* When Windows' own shortcuts, such as the Windows key, Alt+Tab and
 * Alt+F4, go to the host instead of acting on this PC. */
enum QdpShortcuts {
    /* While the stream window is full screen. */
    QDP_SHORTCUTS_FULL_SCREEN = 0,
    /* Whenever the stream window is in front. */
    QDP_SHORTCUTS_ALWAYS = 1,
    /* Never. */
    QDP_SHORTCUTS_NEVER = 2,
};

/* What a session connects with. */
typedef struct QdpConfig {
    /* The host's address or name, with or without a port. */
    const char *host;
    /* The host's key pin, in hexadecimal, for token and paired
     * authentication. */
    const char *pin;
    /* The token, for token authentication. */
    const char *token;
    /* The device's name, which the host shows when pairing. */
    const char *device_name;
    /* The stream window's title; the host's address if null or empty. */
    const char *title;
    /* The .ico file the stream window shows, or null. */
    const char *icon_path;
    /* The app's window, whose monitor the stream window opens on, or 0 for
     * the primary monitor. */
    intptr_t owner_window;
    /* A QdpAuthentication value. */
    int32_t authentication;
    /* The size of the video to ask for, in pixels, scaled to fit the
     * window: 0 and 0 for the window's own size; 0 and a height for that
     * many rows in the window's shape, or the window's own size where it
     * has no more; or exactly this width and height. */
    uint32_t width;
    uint32_t height;
    /* A QdpDecoder value. */
    int32_t decoder;
    /* 1 if the microphone starts on, 0 if not. */
    int32_t microphone;
    /* A QdpScreen value. */
    int32_t screen;
    /* A QdpSync value. */
    int32_t sync;
    /* A QdpShortcuts value. */
    int32_t shortcuts;
    /* 1 if the clipboard is shared with the host, both ways; 0 if not. */
    int32_t clipboard;
} QdpConfig;

/* A running session. */
typedef struct QdpSession QdpSession;

/* Sets up logging, to qdp.log there, and keeps the app's data directory,
 * where the device's key is kept. Call it once, before anything else. */
void qdp_init(const char *data_directory);

/* Returns the core's version; the app must not free it. */
const char *qdp_version(void);

/* Returns the device's key pin, in hexadecimal, generating its key the
 * first time, or null if the key cannot be kept. */
char *qdp_client_pin(void);

/* Returns whether text is a key pin, as a host's DNS-SD TXT record's
 * "pin" gives it. */
bool qdp_is_pin(const char *text);

/* Starts a session in a stream window of its own, returning at once.
 * Returns null, with the reason in *error for the app to free, if the
 * configuration is wrong; a session that fails later says so in its last
 * line. error may be null. */
QdpSession *qdp_session_start(const QdpConfig *config, char **error);

/* Returns the session's next line, waiting up to timeout_millis for one,
 * or null if none came. After the last line it waits out the timeout and
 * returns null. Call it from one thread at a time. */
char *qdp_session_next_event(QdpSession *session, uint32_t timeout_millis);

/* Answers a "pair" line with the code the host shows, or declines with
 * null. */
void qdp_session_pairing_code(QdpSession *session, const char *code);

/* Ends the session and closes its window, returning at once; its last line
 * follows once it has ended. Any thread may call it. */
void qdp_session_stop(QdpSession *session);

/* Ends the session if it has not ended, and frees it. No thread may use
 * the session then or after, so the thread reading its lines must have
 * stopped. */
void qdp_session_free(QdpSession *session);

/* Frees a string the core returned. */
void qdp_string_free(char *string);

#ifdef __cplusplus
}
#endif

#endif
