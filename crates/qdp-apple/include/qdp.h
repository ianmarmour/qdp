/*
 * QDP's core for the Apple apps: sessions with a host, which show their video
 * in a Core Animation layer with Metal, and take the user's input.
 *
 * A session runs on threads of its own. Call qdp_session_start, the resize
 * function and qdp_session_stop on the main thread; the input functions and
 * qdp_session_next_event may be called from any thread. Positions are in
 * video pixels. Strings the core returns are freed with qdp_string_free.
 */

#ifndef QDP_H
#define QDP_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* How a session authenticates with the host and recognizes its key. */
typedef enum {
    /* The host's key is pinned, and the device presents a token. */
    QDP_AUTH_TOKEN = 0,
    /* A host the device paired with: its key is pinned, and the device
       presents its own key. */
    QDP_AUTH_PAIRED = 1,
    /* Pairing with the code the host shows, asked for with a "pair" event. */
    QDP_AUTH_PAIR = 2,
    /* Through the gateway, which vouches for the user, asking for their
       password and a code with "signin" and "totp" events. */
    QDP_AUTH_GATEWAY = 3,
} QdpAuthentication;

/* Where video is decoded. */
typedef enum {
    QDP_DECODER_AUTO = 0,
    QDP_DECODER_HARDWARE = 1,
    QDP_DECODER_SOFTWARE = 2,
} QdpDecoder;

/* A finger's touch phase. */
typedef enum {
    QDP_TOUCH_DOWN = 0,
    QDP_TOUCH_MOVE = 1,
    QDP_TOUCH_UP = 2,
    QDP_TOUCH_CANCEL = 3,
} QdpTouchPhase;

/* A pointer button. */
typedef enum {
    QDP_BUTTON_LEFT = 0,
    QDP_BUTTON_RIGHT = 1,
    QDP_BUTTON_MIDDLE = 2,
    QDP_BUTTON_BACK = 3,
    QDP_BUTTON_FORWARD = 4,
} QdpButton;

/* What a session connects with. Strings are UTF-8 and may be NULL where
   unused. */
typedef struct {
    /* The host's address or name, with or without a port. */
    const char *host;
    QdpAuthentication authentication;
    /* The host's key pin, 64 hexadecimal digits, for QDP_AUTH_TOKEN and
       QDP_AUTH_PAIRED. */
    const char *pin;
    /* The token, for QDP_AUTH_TOKEN. */
    const char *token;
    /* The device's name, which the host shows when pairing. */
    const char *device_name;
    /* The display the session asks the host for. */
    uint32_t width;
    uint32_t height;
    uint32_t refresh_millihertz;
    /* The interface scale the session asks for, in percent. */
    uint32_t scale_percent;
    QdpDecoder decoder;
    /* Whether the microphone starts on. */
    bool microphone;
    /* The organization's gateway the host is reached through, or NULL or
       empty to reach it directly: its address; its key pin, or NULL or empty
       to verify it by the system's certificate authorities; and the token of
       the last "gatewaysession" event, or NULL or empty to sign in. Through
       a gateway, host is the computer's ID there or its name inside the
       network. */
    const char *gateway_address;
    const char *gateway_pin;
    const char *gateway_token;
} QdpConfig;

/* The host's cursor image. */
typedef struct {
    uint32_t width;
    uint32_t height;
    uint32_t hotspot_x;
    uint32_t hotspot_y;
    /* The image's scale relative to the video, in percent: at 100 it is drawn
       at its pixel size scaled as the video is, at 200 at half that. */
    uint32_t scale_percent;
    /* width * height RGBA pixels, straight alpha, row by row; freed with
       qdp_cursor_free. */
    uint8_t *rgba;
} QdpCursor;

/* A running session. */
typedef struct QdpSession QdpSession;

/* Sets up logging and keeps the app's private directory for the device's
   key. Call once, before anything else. */
void qdp_init(const char *files_directory);

/* Returns the device's key pin, generating its key the first time, or NULL
   if it cannot be kept. */
char *qdp_client_pin(void);

/* Returns whether text is a key pin. */
bool qdp_is_pin(const char *text);

/* Starts a session with config, showing its video in a Metal layer added to
   layer (a CALayer), which is width x height pixels at scale pixels a point.
   Returns NULL, and sets *error if error is not NULL, if it cannot start. */
QdpSession *qdp_session_start(const QdpConfig *config, void *layer, double width,
                              double height, double scale, char **error);

/* Ends the session and frees it, returning once its layer is no longer
   drawn into. */
void qdp_session_stop(QdpSession *session);

/* Tells the session that its layer is now width x height pixels at scale
   pixels a point. */
void qdp_session_resize(QdpSession *session, double width, double height, double scale);

/* Returns the session's next event as a line of text, waiting up to
   timeout_millis for one, or NULL if none came. The lines are those of the
   Android app: "connecting", "connected", "reconnecting <ms> <error>",
   "video <w> <h>", "cursor", "warp <x> <y> move|follow",
   "pointer absolute|relative", "rumble <slot> <low> <high>",
   "stats <fps> <latency|-> <decode> <worst...>", "pair first|retry",
   "paired <pin>", "ended", "failed <error>". */
char *qdp_session_next_event(QdpSession *session, uint32_t timeout_millis);

/* Copies the host's cursor into *cursor, returning false while it is
   hidden. */
bool qdp_session_cursor(QdpSession *session, QdpCursor *cursor);

/* Frees a cursor's pixels. */
void qdp_cursor_free(QdpCursor *cursor);

/* Draws the host's cursor over the video at the pointer, for touch input
   where the system draws none, or stops drawing it. */
void qdp_session_draw_cursor(QdpSession *session, bool draw);

/* Moves the cursor drawn over the video to x, y without moving the host's
   pointer, as after a "warp" event. */
void qdp_session_cursor_at(QdpSession *session, float x, float y);

/* Answers a "pair" event with the code typed, or declines with NULL, which
   ends the session. */
void qdp_session_pairing_code(QdpSession *session, const char *code);

/* Answers a "signin" event with the user's name and password, or declines
   with a NULL user, which ends the session. */
void qdp_session_sign_in(QdpSession *session, const char *user, const char *password);

/* Answers a "totp" event with the code from the user's authenticator app, or
   declines with NULL, which ends the session. */
void qdp_session_sign_in_code(QdpSession *session, const char *code);

/* Tells the host the display changed. */
void qdp_session_display(QdpSession *session, uint32_t width, uint32_t height,
                         uint32_t refresh_millihertz, uint32_t scale_percent);

/* Sends a key by its USB HID usage on the keyboard page. */
void qdp_session_key(QdpSession *session, uint16_t usage, bool pressed);

/* Returns the HID usage of a macOS virtual key code, or 0 for none. */
uint16_t qdp_mac_key_usage(uint16_t key_code);

/* Sends text typed without key presses. */
void qdp_session_text(QdpSession *session, const char *text);

/* Moves the pointer to x, y. */
void qdp_session_pointer(QdpSession *session, float x, float y);

/* Moves the captured mouse by dx, dy. */
void qdp_session_mouse_move(QdpSession *session, float dx, float dy);

/* Presses or releases a pointer button. */
void qdp_session_button(QdpSession *session, QdpButton button, bool pressed);

/* Scrolls by 1/120ths of a wheel detent, positive up and right. */
void qdp_session_scroll(QdpSession *session, int32_t vertical, int32_t horizontal);

/* Sends a finger's touch, pressing with pressure from 0 to 1, or a negative
   pressure if unknown. */
void qdp_session_touch(QdpSession *session, uint32_t finger, QdpTouchPhase phase, float x,
                       float y, float pressure);

/* Connects gamepad slot. */
void qdp_session_gamepad_connected(QdpSession *session, uint32_t slot, const char *name,
                                   bool rumble);

/* Disconnects gamepad slot. */
void qdp_session_gamepad_disconnected(QdpSession *session, uint32_t slot);

/* Sends gamepad slot's state: buttons a mask of Standard Gamepad button
   indices, sticks from -1 to 1 (negative left and up), triggers from 0 to 1. */
void qdp_session_gamepad(QdpSession *session, uint32_t slot, uint32_t buttons, float left_x,
                         float left_y, float right_x, float right_y, float left_trigger,
                         float right_trigger);

/* The on-screen gamepad, which the iPhone and iPad app shows over the
   stream from its menu: the app draws its controls and passes every touch on
   them, and the core lays them out, works out the gamepad's state and sends
   it as a gamepad of its own. */
typedef struct QdpVirtualGamepad QdpVirtualGamepad;

/* A control of the on-screen gamepad, as the app draws it. */
typedef struct {
    /* Which control it is: a Standard Gamepad button index (0 A, 1 B, 2 X,
       3 Y, 4 and 5 the left and right shoulder buttons, 6 and 7 the left and
       right triggers, 8 Select, 9 Start, 10 and 11 the left and right stick
       buttons), 12 the directional pad, or 17 and 18 the left and right
       sticks. */
    uint32_t control;
    /* Its frame in the view, in points: a circle's bounding square for the
       sticks and round buttons, the rectangle of a shoulder button or
       trigger, the directional pad's bounding square. */
    float x;
    float y;
    float width;
    float height;
    /* Whether a touch holds it. */
    bool held;
    /* A stick's tilt, each from -1 to 1, negative left and up. */
    float tilt_x;
    float tilt_y;
    /* The directional pad's directions held: 1 up, 2 down, 4 left, 8 right. */
    uint32_t directions;
} QdpVirtualControl;

/* qdp_virtual_gamepad_touch's bit saying the controls look different. */
#define QDP_VIRTUAL_GAMEPAD_REDRAW 1u
/* qdp_virtual_gamepad_touch's bit saying a button or direction was pressed,
   for a haptic tap. */
#define QDP_VIRTUAL_GAMEPAD_PRESSED 2u

/* Returns a new on-screen gamepad, freed with qdp_virtual_gamepad_free. */
QdpVirtualGamepad *qdp_virtual_gamepad_new(void);

/* Frees an on-screen gamepad. */
void qdp_virtual_gamepad_free(QdpVirtualGamepad *gamepad);

/* Connects the on-screen gamepad to the session as a gamepad of its own, or
   disconnects it with every control released. */
void qdp_virtual_gamepad_connect(QdpVirtualGamepad *gamepad, QdpSession *session, bool connected);

/* Lays the on-screen gamepad out on a view width x height points, inside the
   safe area the insets leave, releasing every control held. */
void qdp_virtual_gamepad_lay_out(QdpVirtualGamepad *gamepad, QdpSession *session, float width,
                                 float height, float left, float top, float right, float bottom);

/* Takes a finger's touch at x, y in the view's points, sending the gamepad's
   state to the session if it changed. Returns QDP_VIRTUAL_GAMEPAD_REDRAW and
   QDP_VIRTUAL_GAMEPAD_PRESSED bits. */
uint32_t qdp_virtual_gamepad_touch(QdpVirtualGamepad *gamepad, QdpSession *session,
                                   uint32_t finger, QdpTouchPhase phase, float x, float y);

/* Releases every control held, as when the stream's menu opens. */
void qdp_virtual_gamepad_release(QdpVirtualGamepad *gamepad, QdpSession *session);

/* Writes up to capacity controls to controls, returning how many there are. */
size_t qdp_virtual_gamepad_controls(QdpVirtualGamepad *gamepad, QdpVirtualControl *controls,
                                    size_t capacity);

/* Tells the host whether the app has focus; losing it releases everything. */
void qdp_session_focus(QdpSession *session, bool focused);

/* Turns the microphone on or off. */
void qdp_session_microphone(QdpSession *session, bool on);

/* Releases every key and button held. */
void qdp_session_release_all(QdpSession *session);

/* Frees a string the core returned. */
void qdp_string_free(char *string);

#ifdef __cplusplus
}
#endif

#endif
