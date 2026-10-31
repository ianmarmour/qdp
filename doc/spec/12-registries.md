# 12. Registries

This section collects every code point QDP defines. In each registry, values of
the form `0x1f × N + 0x21` are reserved for greasing
([Section 1.6](01-introduction.md#16-extensibility)) and MUST NOT be assigned.
Unassigned values are reserved for future versions and extensions.

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
| 0x10 | `DISPLAY_REQUEST` | Client → host | [6.2](06-video.md#62-display_request-0x10) |
| 0x11 | `DISPLAY_CONFIG` | Host → client | [6.3](06-video.md#63-display_config-0x11) |
| 0x12 | `KEYFRAME_REQUEST` | Client → host | [6.10](06-video.md#610-keyframe_request-0x12) |
| 0x18 | `AUDIO_REQUEST` | Client → host | [7.2](07-audio.md#72-audio_request-0x18) |
| 0x19 | `AUDIO_CONFIG` | Host → client | [7.3](07-audio.md#73-audio_config-0x19) |
| 0x30 | `KEYBOARD_INFO` | Client → host | [8.2.1](08-input.md#821-keyboard_info-0x30) |
| 0x31 | `DEVICE_ATTACH` | Client → host | [8.2.2](08-input.md#822-device_attach-0x31) |
| 0x32 | `DEVICE_DETACH` | Client → host | [8.2.3](08-input.md#823-device_detach-0x32) |
| 0x33 | `POINTER_MODE` | Host → client | [8.7](08-input.md#87-pointer_mode-0x33) |
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
| 0x05 | `INPUT_ACK` | Host → client | [8.4](08-input.md#84-input_ack-datagram) |
| 0x06 | `DEVICE_OUTPUT` | Host → client | [8.8](08-input.md#88-device_output-datagram) |
| 0x07 | `TIME_PING` | Both | [5.2](05-timing.md#52-clock-synchronization) |
| 0x08 | `TIME_PONG` | Both | [5.2](05-timing.md#52-clock-synchronization) |

## 12.3 Unidirectional stream types

| Type | Name | Direction | Section |
|---|---|---|---|
| 0x01 | `CURSOR_IMAGE` | Host → client | [9.2](09-cursor.md#92-cursor-images) |
| 0x02 | `CLIPBOARD_DATA` | Both | [10.4](10-clipboard.md#104-clipboard-data) |

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

## 12.5 Record and event types

Record types are scoped to the datagram that carries them.

**`VIDEO_FEEDBACK` records**: 0x01 `DECODED`, 0x02 `NACK`, 0x03 `LOSS`,
0x04 `FRAME_TIMING`, 0x05 `DISPLAY_TIMING`.

**`INPUT` records**: 0x01 `POINTER_POSITION`, 0x02 `POINTER_MOTION`, 0x03 `KEYS`,
0x04 `BUTTONS`, 0x05 `LOCKS`, 0x06 `GAMEPAD`, 0x07 `GAMEPAD_MOTION`,
0x10 `EVENTS`.

**Input events**: 0x01 `KEY`, 0x02 `BUTTON`, 0x03 `WHEEL`, 0x04 `TEXT`,
0x05 `TOUCH`, 0x06 `PEN`.

## 12.6 Error codes

Error codes are QUIC application error codes. Codes up to 0x0F are used in
CONNECTION_CLOSE frames and MAY also be used for streams; codes from 0x10 are
used only in RESET_STREAM and STOP_SENDING frames.

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
| 0x08 | `QDP_MESSAGE_TOO_LARGE` | A control message exceeded 1 MiB |
| 0x09 | `QDP_RESOURCE_LIMIT` | The peer exceeded a resource limit |
| 0x0A | `QDP_HANDSHAKE_TIMEOUT` | The session did not become active in time |
| 0x0B | `QDP_SESSION_REPLACED` | A newer session for the same user replaced this one |
| 0x0C | `QDP_HOST_SHUTDOWN` | The host is shutting down or logging the user out |
| 0x0D | `QDP_POLICY` | Administrative policy ended the session |
| 0x10 | `QDP_STREAM_TYPE_UNKNOWN` | Unknown unidirectional stream type |
| 0x11 | `QDP_REQUEST_CANCELLED` | The receiver no longer needs the stream's data |

## 12.7 Other code points

| Field | Values |
|---|---|
| Authentication method | 0x01 `CLIENT_CERTIFICATE`, 0x02 `TOKEN`, 0x03 `SCRAM_SHA_256_PLUS`, 0x04 `ANONYMOUS` |
| Authentication status | 0x00 `SUCCESS`, 0x01 `FAILED_RETRY`, 0x02 `FAILED_FINAL` |
| Video codec | 0x01 AV1 |
| Audio codec | 0x01 Opus, 0x02 PCM L16, 0x03 PCM L24 |
| Video layer | 0x00 `MOTION`, 0x01 `REFINEMENT` |
| Chroma | 0x00 4:2:0, 0x01 4:4:4 |
| Block map encoding | 0x00 `NONE`, 0x01 `ALL`, 0x02 `BITMAP`, 0x03 `RUNS` |
| Audio direction | 0x00 `PLAYBACK`, 0x01 `CAPTURE` |
| Device class | 0x01 `GAMEPAD`, 0x02 `TOUCHSCREEN`, 0x03 `PEN` |
| Pointer mode | 0x00 `ABSOLUTE`, 0x01 `RELATIVE` |
| Keyboard layout platform | 0x00 unknown, 0x01 Windows, 0x02 XKB, 0x03 macOS |
| Cursor image format | 0x01 RGBA, 8 bits per channel, straight alpha |
| Clipboard reject reason | 0x01 `STALE_OFFER`, 0x02 `UNAVAILABLE`, 0x03 `TOO_LARGE`, 0x04 `DENIED` |
