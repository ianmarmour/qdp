# 12. Registries

This section collects every code point QDP defines. Unassigned values are reserved
for future versions and extensions.

In the type and identifier spaces of [Sections 12.1](#121-control-message-types) to
[12.5](#125-record-types), values of the form `0x1f × N + 0x21` are reserved for
greasing ([Section 1.6](01-introduction.md#16-extensibility)) and MUST NOT be
assigned. Greasing does not apply to flag masks, to enumerated values inside fields,
or to error codes.

## 12.1 Control message types

| Type | Name | Direction | Section |
|---|---|---|---|
| 0x01 | `CLIENT_HELLO` | Client → host | [4.3.1](04-session.md#431-client_hello-0x01) |
| 0x02 | `HOST_HELLO` | Host → client | [4.3.2](04-session.md#432-host_hello-0x02) |
| 0x03 | `AUTH_START` | Client → host | [4.4.1](04-session.md#441-messages) |
| 0x04 | `AUTH_CONTINUE` | Both | [4.4.1](04-session.md#441-messages) |
| 0x05 | `AUTH_RESULT` | Host → client | [4.4.1](04-session.md#441-messages) |
| 0x06 | `CLIENT_READY` | Client → host | [4.5](04-session.md#45-configuration) |
| 0x07 | `SESSION_START` | Host → client | [4.5](04-session.md#45-configuration) |
| 0x08 | `RESUMPTION_TOKEN` | Host → client | [4.4.7](04-session.md#447-resumption_token-0x08) |
| 0x10 | `DISPLAY_REQUEST` | Client → host | [6.2](06-video.md#62-display_request-0x10) |
| 0x11 | `DISPLAY_CONFIG` | Host → client | [6.3](06-video.md#63-display_config-0x11) |
| 0x12 | `KEYFRAME_REQUEST` | Client → host | [6.10](06-video.md#610-keyframe_request-0x12) |
| 0x18 | `AUDIO_REQUEST` | Client → host | [7.2](07-audio.md#72-audio_request-0x18) |
| 0x19 | `AUDIO_CONFIG` | Host → client | [7.3](07-audio.md#73-audio_config-0x19) |
| 0x30 | `KEYBOARD_INFO` | Client → host | [8.2.3](08-input.md#823-keyboard_info-0x30) |
| 0x31 | `DEVICE_ATTACH` | Client → host | [8.2.1](08-input.md#821-device_attach-0x31) |
| 0x32 | `DEVICE_DETACH` | Client → host | [8.2.2](08-input.md#822-device_detach-0x32) |
| 0x33 | `POINTER_MODE` | Host → client | [8.9](08-input.md#89-pointer_mode-0x33) |
| 0x35 | `HID_FEATURE_REQUEST` | Host → client | [8.8.3](08-input.md#883-hid_feature_request-0x35-and-hid_feature_response-0x36) |
| 0x36 | `HID_FEATURE_RESPONSE` | Client → host | [8.8.3](08-input.md#883-hid_feature_request-0x35-and-hid_feature_response-0x36) |
| 0x37 | `DEVICE_STATUS` | Host → client | [8.2.4](08-input.md#824-device_status-0x37) |
| 0x38 | `CURSOR_SET` | Host → client | [9.3](09-cursor.md#93-cursor_set-0x38) |
| 0x39 | `CURSOR_WARP` | Host → client | [9.4](09-cursor.md#94-cursor_warp-0x39) |
| 0x48 | `CLIPBOARD_OFFER` | Both | [10.2](10-clipboard.md#102-clipboard_offer-0x48) |
| 0x49 | `CLIPBOARD_REQUEST` | Both | [10.3](10-clipboard.md#103-clipboard_request-0x49-and-clipboard_reject-0x4a) |
| 0x4A | `CLIPBOARD_REJECT` | Both | [10.3](10-clipboard.md#103-clipboard_request-0x49-and-clipboard_reject-0x4a) |

## 12.2 Datagram types

| Type | Name | Direction | Section |
|---|---|---|---|
| 0x01 | `VIDEO_FRAGMENT` | Host → client | [6.6](06-video.md#66-video-fragments) |
| 0x02 | `VIDEO_FEEDBACK` | Client → host | [6.7](06-video.md#67-video-feedback) |
| 0x03 | `AUDIO_PACKET` | Both | [7.4](07-audio.md#74-audio_packet-datagram) |
| 0x04 | `INPUT` | Client → host | [8.3](08-input.md#83-input-datagram) |
| 0x05 | `INPUT_ACK` | Host → client | [8.5](08-input.md#85-input_ack-datagram) |
| 0x06 | `DEVICE_OUTPUT` | Host → client | [8.8.1](08-input.md#881-device_output-datagram) |
| 0x07 | `TIME_PING` | Both | [5.2](05-timing.md#52-clock-synchronization) |
| 0x08 | `TIME_PONG` | Both | [5.2](05-timing.md#52-clock-synchronization) |

## 12.3 Unidirectional stream types

| Type | Name | Direction | Section |
|---|---|---|---|
| 0x01 | `CURSOR_IMAGE` | Host → client | [9.2](09-cursor.md#92-cursor-images) |
| 0x02 | `CLIPBOARD_DATA` | Both | [10.4](10-clipboard.md#104-clipboard-data) |
| 0x03 | `HID_OUTPUT` | Host → client | [8.8.2](08-input.md#882-hid-output-streams) |

## 12.4 Capability IDs

| ID | Name | Repeatable | Section |
|---|---|---|---|
| 0x01 | `VIDEO_CODEC` | Yes | [4.6.2](04-session.md#462-video_codec-0x01-repeatable) |
| 0x02 | `REFINEMENT` | No | [4.6.3](04-session.md#463-refinement-0x02) |
| 0x03 | `AUDIO_CODEC` | Yes | [4.6.4](04-session.md#464-audio_codec-0x03-repeatable) |
| 0x04 | `INPUT` | No | [4.6.5](04-session.md#465-input-0x04) |
| 0x05 | `CLIPBOARD` | No | [4.6.6](04-session.md#466-clipboard-0x05) |
| 0x06 | `MONITORS` | No | [4.6.7](04-session.md#467-monitors-0x06) |
| 0x07 | `HDR` | No | [4.6.8](04-session.md#468-hdr-0x07) |

## 12.5 Record types

`VIDEO_FEEDBACK` records ([Section 6.7](06-video.md#67-video-feedback)): 0x01
`DECODED`, 0x02 `NACK`, 0x03 `LOSS`, 0x04 `FRAME_TIMING`, 0x05 `DISPLAY_TIMING`.

## 12.6 Error codes

Error codes are QUIC application error codes. Codes up to 0x0F are used in
CONNECTION_CLOSE frames and MAY also be used for streams; codes from 0x10 are used
only in RESET_STREAM and STOP_SENDING frames. An endpoint MUST treat an unknown
connection error code as `QDP_INTERNAL_ERROR`, and an unknown stream error code as a
plain cancellation of the stream.

| Code | Name | Meaning |
|---|---|---|
| 0x00 | `QDP_NO_ERROR` | Normal end of session |
| 0x01 | `QDP_INTERNAL_ERROR` | An implementation fault |
| 0x02 | `QDP_PROTOCOL_VIOLATION` | The peer broke a rule of this specification |
| 0x03 | `QDP_AUTH_FAILED` | Authentication failed |
| 0x04 | `QDP_NO_COMMON_CAPABILITY` | No video codec in common |
| 0x05 | `QDP_TRANSPORT_REQUIREMENTS` | The peer does not support QUIC datagrams |
| 0x06 | `QDP_CLOSED_CRITICAL_STREAM` | The control stream was closed or reset |
| 0x07 | `QDP_STREAM_CREATION_ERROR` | A stream was opened that is not allowed |
| 0x08 | `QDP_MESSAGE_TOO_LARGE` | A control message exceeded its size limit |
| 0x09 | `QDP_RESOURCE_LIMIT` | The peer exceeded a resource limit |
| 0x0A | `QDP_HANDSHAKE_TIMEOUT` | The session did not become active in time |
| 0x0B | `QDP_SESSION_REPLACED` | A newer session for the same user replaced this one |
| 0x0C | `QDP_HOST_SHUTDOWN` | The host is shutting down or logging the user out |
| 0x0D | `QDP_POLICY` | Administrative policy ended the session |
| 0x10 | `QDP_STREAM_TYPE_UNKNOWN` | Unknown unidirectional stream type |
| 0x11 | `QDP_REQUEST_CANCELLED` | The receiver no longer needs the stream's data |

## 12.7 Enumerations and flags

**Session and media**

| Field | Values |
|---|---|
| Authentication method | 0x01 `CLIENT_CERTIFICATE`, 0x02 `TOKEN`, 0x03 `SCRAM_SHA_256_PLUS`, 0x04 `ANONYMOUS`, 0x05 `PASSWORD` |
| `HDR` capability formats (mask) | 0x01 BT.2020 with PQ, 0x02 BT.2020 with HLG |
| Video stream flags (mask) | 0x01 `FULL_RANGE`, 0x02 `LOSSLESS` |
| Video fragment flags (mask) | 0x01 `KEY_FRAME`, 0x02 `RECOVERY`, 0x04 `RETRANSMISSION`, 0x08 `LAST` |
| Monitor flags (mask) | 0x01 `PRIMARY`, 0x02 `VRR_CAPABLE`, 0x04 `HDR_PQ`, 0x08 `HDR_HLG` (Client Monitor); 0x01 `PRIMARY` (Host Monitor) |
| `DISPLAY_TIMING` flags (mask) | 0x01 `VRR_ACTIVE` |
| Authentication status | 0x00 `SUCCESS`, 0x01 `FAILED_RETRY`, 0x02 `FAILED_FINAL` |
| Video codec | 0x01 AV1 |
| Audio codec | 0x01 Opus, 0x02 PCM L16, 0x03 PCM L24 |
| Video layer | 0x00 `MOTION`, 0x01 `REFINEMENT` |
| Chroma | 0x00 4:2:0, 0x01 4:4:4 |
| Block map encoding | 0x00 `NONE`, 0x01 `ALL`, 0x02 `BITMAP`, 0x03 `RUNS` |
| Audio direction | 0x00 `PLAYBACK`, 0x01 `CAPTURE` |
| Cursor image format | 0x01 RGBA, 8 bits per channel, straight alpha |
| `CURSOR_WARP` flags | 0x01 `FOLLOW` |
| Clipboard reject reason | 0x01 `STALE_OFFER`, 0x02 `UNAVAILABLE`, 0x03 `TOO_LARGE`, 0x04 `DENIED` |

**Input** ([Section 8](08-input.md))

| Field | Values |
|---|---|
| `INPUT` capability classes (mask) | 0x01 `KEYBOARD`, 0x02 `POINTER`, 0x04 `MOUSE`, 0x08 `TOUCHSCREEN`, 0x10 `PEN`, 0x20 `TOUCHPAD`, 0x40 `GAMEPAD`, 0x80 `TEXT`, 0x100 `PASSTHROUGH` |
| Device kind | 0x01 `KEYBOARD`, 0x02 `POINTER`, 0x03 `MOUSE`, 0x04 `TOUCHSCREEN`, 0x05 `PEN`, 0x06 `TOUCHPAD`, 0x07 `GAMEPAD`, 0x08 `TEXT`, 0x10 `PASSTHROUGH` |
| Device status | 0x00 `ACCEPTED`, 0x01 `UNSUPPORTED`, 0x02 `DENIED`, 0x03 `LIMIT_REACHED`, 0x04 `FAILED` |
| Passthrough bus type | 0x00 unknown, 0x01 USB, 0x02 Bluetooth Classic, 0x03 Bluetooth LE, 0x04 I2C, 0x05 SPI |
| Keyboard layout platform | 0x00 unknown, 0x01 Windows, 0x02 XKB, 0x03 macOS, 0x04 Android, 0x05 iOS |
| Feature operation | 0x00 `GET`, 0x01 `SET` |
| Feature status | 0x00 `OK`, 0x01 `FAILED`, 0x02 `DENIED` |
| Pointer mode | 0x00 `ABSOLUTE`, 0x01 `RELATIVE`; flag 0x01 `CONFINED` |
| Pointer and mouse buttons (mask) | 0x01 left, 0x02 right, 0x04 middle, 0x08 back, 0x10 forward |
| Keyboard LEDs (mask) | 0x01 Num Lock, 0x02 Caps Lock, 0x04 Scroll Lock, 0x08 Compose, 0x10 Kana |
| Touch contact flags | 0x01 `CANCELLED` |
| Touchpad contact flags | 0x01 `CONFIDENCE` |
| Touchpad buttons (mask) | 0x01 primary, 0x02 secondary |
| Pen flags (mask) | 0x01 in range, 0x02 tip switch, 0x04 barrel switch, 0x08 eraser, 0x10 invert |
| Device features (mask) | see the table in [Section 8.2.1](08-input.md#821-device_attach-0x31) |
