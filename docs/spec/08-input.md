# 8. Input

## 8.1 Model

All input in QDP is described in terms of USB HID [HID] [HID-CLASS]. The client
announces each input device it shares with `DEVICE_ATTACH`; once the host
accepts it, the device produces **input reports**. There are two kinds of
device:

- **Standard devices**: keyboard, pointer, mouse, touchscreen, pen, touchpad,
  gamepad and text. Their report formats are defined in
  [Section 8.4](#84-standard-device-reports). The client builds their reports
  from its operating system's input events, so they work with any physical
  device, including built-in keyboards and trackpads that are not HID devices,
  devices the operating system does not let applications read directly, and the
  touchscreens and on-screen keyboards of phones and tablets. The host needs no
  HID descriptor parser to use them.
- **Passthrough devices**: the client forwards a physical HID device's report
  descriptor and reports unchanged, so that applications on the host see the
  real device. This is intended for game controllers, steering wheels, flight
  sticks and similar devices.

Every input report carries a sequence number shared by all devices, and the host
applies reports strictly in sequence order, so the order of actions across
devices, such as holding Ctrl on the keyboard and clicking the mouse, is
preserved. Standard reports state the device's complete current state; relative
motion and scrolling are running totals. The client keeps sending every report
until the host acknowledges it, so a lost datagram neither loses input nor
waits for a round trip.

A device whose input classes are negotiated ([Section 4.6.5](04-session.md#465-input-0x04))
works on every client platform. A client without a physical keyboard or mouse,
such as a phone, still attaches `KEYBOARD`, `POINTER` and `MOUSE` devices and
produces their reports from touch gestures and its on-screen keyboard.

## 8.2 Devices

### 8.2.1 DEVICE_ATTACH (0x31)

```
DEVICE_ATTACH Payload {
  Device ID (i),
  Device Kind (i),
  Features (i),
  Name (String),
  [Touchpad Info (..)],
  [Passthrough Info (..)],
}

Touchpad Info {
  Width (i),
  Height (i),
}

Passthrough Info {
  Bus Type (8),
  Vendor ID (16),
  Product ID (16),
  Version (16),
  Report Descriptor (Octets),
  Rest Report Count (i),
  Rest Report (Octets) ...,
}
```

- **Device ID**: chosen by the client. Device IDs start at 1, increase with each
  device attached, and are never reused within a session.
- **Device Kind**: from the table below. A host that receives an unknown kind
  answers `UNSUPPORTED` ([Section 8.2.4](#824-device_status-0x37)).
- **Features**: kind-specific flags, from the table below.
- **Name**: the product name, at most 255 bytes.
- **Touchpad Info**: present only for `TOUCHPAD` devices. The size of the touch
  surface in hundredths of a millimetre.
- **Passthrough Info**: present only for `PASSTHROUGH` devices:
  - **Bus Type**: the transport the physical device uses: 0x00 unknown, 0x01
    USB, 0x02 Bluetooth Classic, 0x03 Bluetooth LE, 0x04 I2C, 0x05 SPI. Report
    formats can differ by transport, so the host SHOULD create its virtual device
    with the same bus type where the platform allows.
  - **Vendor ID**, **Product ID**, **Version**: as the device reports them
    (`idVendor`, `idProduct` and `bcdDevice` for USB; the equivalent Device ID
    values on other buses, in that bus's vendor namespace).
  - **Report Descriptor**: the device's HID report descriptor [HID-CLASS], at most
    16384 bytes. The client MUST NOT pass through a device whose descriptor
    declares an input report longer than 1024 bytes.
  - **Rest Report**: one input report per input report ID, describing the
    device with every control at rest, in the format of
    [Section 8.3](#83-input-datagram). The host injects them when the device must
    be returned to rest ([Section 8.7](#87-host-rules)).

| Kind | Name | Features |
|---|---|---|
| 0x01 | `KEYBOARD` | none |
| 0x02 | `POINTER` | none |
| 0x03 | `MOUSE` | none |
| 0x04 | `TOUCHSCREEN` | 0x01 pressure |
| 0x05 | `PEN` | 0x01 pressure, 0x02 tilt, 0x04 twist, 0x08 eraser, 0x10 barrel button |
| 0x06 | `TOUCHPAD` | 0x01 clickpad (the surface is a button) |
| 0x07 | `GAMEPAD` | 0x01 rumble, 0x02 trigger rumble, 0x04 motion sensors, 0x08 LED, 0x10 PlayStation button layout, 0x20 Nintendo button layout |
| 0x08 | `TEXT` | none |
| 0x10 | `PASSTHROUGH` | none |

A `GAMEPAD` with neither layout flag has an Xbox-style layout; setting both
layout flags is a connection error of type `QDP_PROTOCOL_VIOLATION`.

During configuration the client MUST attach one `KEYBOARD`, one `POINTER` and one
`MOUSE`, and one `TEXT` device if the `TEXT` class was negotiated, before sending
`CLIENT_READY`. It may attach and detach other devices at any time. Passthrough
devices MUST NOT be attached in messages pipelined before authentication
([Section 4.4.6](04-session.md#446-pipelining)).

### 8.2.2 DEVICE_DETACH (0x32)

```
DEVICE_DETACH Payload {
  Device ID (i),
}
```

When it sends `DEVICE_DETACH`, the client discards any reports of that device it
still retains ([Section 8.6](#86-client-rules)) and stops reading the device's HID
output stream ([Section 8.8.2](#882-hid-output-streams)). The host returns the
device to rest and removes its virtual device.

### 8.2.3 KEYBOARD_INFO (0x30)

```
KEYBOARD_INFO Payload {
  Language (String),
  Layout Platform (8),
  Layout (String),
}
```

- **Language**: the BCP 47 tag [RFC5646] of the user's input language, for
  example `en-GB`.
- **Layout Platform**: how to read `Layout`: 0x00 unknown, 0x01 Windows keyboard
  layout identifier, 0x02 XKB layout name, 0x03 macOS input source identifier,
  0x04 Android keyboard layout, 0x05 iOS keyboard identifier.
- **Layout**: the client's keyboard layout, for example `00000809` on Windows or
  `gb` with XKB.

Keys are sent as physical key positions, so the host SHOULD select a matching
layout. Then each key produces the character printed on the user's keyboard.

### 8.2.4 DEVICE_STATUS (0x37)

```
DEVICE_STATUS Payload {
  Device ID (i),
  Status (8),
  Reason (String),
}
```

The host answers every `DEVICE_ATTACH` with one `DEVICE_STATUS`. For devices
attached during configuration it sends them before `SESSION_START`.

| Status | Name | Meaning |
|---|---|---|
| 0x00 | `ACCEPTED` | The host will apply the device's reports. |
| 0x01 | `UNSUPPORTED` | The host cannot support this kind of device. |
| 0x02 | `DENIED` | Policy forbids this device. |
| 0x03 | `LIMIT_REACHED` | The host has as many devices as it allows. |
| 0x04 | `FAILED` | The host tried and failed to create the device. |

Reason is a human-readable explanation of at most 255 bytes. The client MUST NOT
send reports for a device before it has received `ACCEPTED` for it. When a device
is refused, the client MAY attach a standard device in its place, for example a
standard `GAMEPAD` for a refused passthrough controller. A host MUST be able to
accept at least 16 attached devices.

### 8.2.5 Passthrough devices

Passthrough lets a client present arbitrary HID devices to the host's operating
system, so both endpoints restrict it:

- A host MUST NOT create virtual HID devices, for passthrough or standard devices,
  unless the platform confines them to the session's user or seat. Where it
  cannot, it injects standard devices through session-scoped operating system
  interfaces and refuses passthrough with `DENIED`.
- A host MUST apply its input policy to passthrough devices by the usage of each
  top-level collection in the descriptor. By default it SHOULD accept only game
  controllers (Generic Desktop Joystick 0x04, Gamepad 0x05 and Multi-axis
  Controller 0x08, and the Simulation Controls page 0x02), and it MUST refuse
  keyboard, mouse, pointer, digitizer, system control, consumer control and FIDO
  (page 0xF1D0) collections unless an administrator has enabled them.
- A host MUST validate the descriptor against the limits of [HID-CLASS] before
  creating a device, SHOULD parse it outside the operating system kernel, and
  SHOULD reproduce the vendor and product IDs only for devices on an allowlist,
  so that untrusted IDs cannot select vendor-specific kernel drivers.
- A client SHOULD pass a device through only when the user has chosen it, and
  MUST NOT offer keyboards, mice or FIDO authenticators for passthrough by
  default.

## 8.3 INPUT datagram

```
INPUT Datagram {
  Datagram Type (i) = 0x04,
  Timestamp (i),
  Oldest Retained (i),
  Display Config ID (i),
  Warp ID (i),
  Input Report (..) ...,
}

Input Report {
  Sequence (i),
  Device ID (i),
  Time Offset (i),
  Report Length (i),
  Report Data (..),
}
```

- **Timestamp**: the client-clock time at which the datagram was sent.
- **Oldest Retained**: the lowest sequence number the client still retains. The
  client will keep sending every report at or above it until acknowledged
  ([Section 8.6](#86-client-rules)).
- **Display Config ID**: the Config ID of the `DISPLAY_CONFIG`
  ([Section 6.3](06-video.md#63-display_config-0x11)) whose layout the client
  used to compute the positions in this datagram.
- **Warp ID**: the Warp ID of the latest `CURSOR_WARP`
  ([Section 9.4](09-cursor.md#94-cursor_warp-0x39)) the client had processed, or
  0 if none.
- **Sequence**: starts at 1 and increases by 1 for each report, across all
  devices. The reports in one datagram MUST be in ascending sequence order; gaps
  are allowed, because reports may be spread over several datagrams.
- **Time Offset**: how many microseconds before Timestamp the report was
  produced.
- **Report Data**: for a standard device, a report in the format of
  [Section 8.4](#84-standard-device-reports). For a passthrough device, one input
  report exactly as the device produced it, preceded by its report ID byte, which
  is 0x00 when the descriptor defines no report IDs.

## 8.4 Standard device reports

Standard reports are fixed layouts. Each states the device's complete current
state. Relative motion, wheel and pan fields are running totals since the device
was attached: signed 32-bit integers that wrap around modulo 2³². The host applies
the difference, modulo 2³², from the last report of that device it applied, so a
skipped report loses no movement. A receiver MUST ignore bytes after the fields
defined here.

Positions are desktop positions in 1/256 pixel
([Section 1.5.6](01-introduction.md#156-positions)), sent as signed 32-bit
integers, in the desktop space of the `DISPLAY_CONFIG` named by the datagram's
Display Config ID.

### 8.4.1 Keyboard

```
Keyboard Report {
  Keys (256),
  Consumer Key 1 (16),
  Consumer Key 2 (16),
  Consumer Key 3 (16),
  Consumer Key 4 (16),
}
```

- **Keys**: a bitmap of held keys on the Keyboard/Keypad page (0x07). Usage u is
  held when byte ⌊u / 8⌋ has the bit with mask `1 << (u mod 8)` set. Usages 0x00
  to 0x03 are error codes in [HID] and MUST be clear. A usage names a key's
  physical position, not its character: usage 0x04 is the key labelled A on a US
  layout, whatever layout the user has. Modifier keys are usages 0xE0 to 0xE7.
- **Consumer Key 1** to **4**: up to four held keys from the Consumer page (0x0C),
  such as 0x00E9 Volume Increment. Unused slots are 0; order does not matter.

The report is 40 bytes. Clients MUST NOT generate automatic key repeats; the host
generates repeats itself.

By default the client reports physical keys and performs no character
composition of its own; the host composes characters with the layout from
`KEYBOARD_INFO`. Text that has no key presses on the client, such as input method
commits, on-screen keyboard input and dictation, is sent as text reports
([Section 8.4.8](#848-text)). Keys consumed by a client-side input method MUST NOT
be reported as held.

The keyboard's output report ([Section 8.8.1](#881-device_output-datagram)) is:

```
Keyboard Output {
  LEDs (8),
}
```

LEDs are 0x01 Num Lock, 0x02 Caps Lock, 0x04 Scroll Lock, 0x08 Compose and
0x10 Kana, as on the HID LED page. The host MUST send a keyboard's output report
as soon as it can after accepting it: immediately in the `ACTIVE` state, or right
after `SESSION_START` for a keyboard accepted during configuration, since datagrams
are not used earlier. When the session gains keyboard focus and the
host's Num, Caps or Scroll Lock state differs from the client's, the client SHOULD
send press and release reports for those lock keys so that the host matches the
client. It MUST NOT do so before it has received the keyboard's output report, nor
while any of its lock-key reports is unacknowledged.

### 8.4.2 Pointer

The pointer is an absolute pointing device, used in absolute pointer mode
([Section 8.9](#89-pointer_mode-0x33)).

```
Pointer Report {
  Buttons (8),
  X (32),
  Y (32),
  Wheel Total (32),
  Pan Total (32),
}
```

- **Buttons**: 0x01 left, 0x02 right, 0x04 middle, 0x08 back, 0x10 forward
  (buttons 1 to 5 of the HID Button page).
- **X**, **Y**: the pointer's desktop position.
- **Wheel Total**: the running total of vertical scrolling, in 1/120ths of a wheel
  detent. An increase means the wheel turned away from the user.
- **Pan Total**: the running total of horizontal scrolling in the same units. An
  increase means scrolling to the right.

Each report carries buttons and position together, so a click is applied exactly
where the pointer was. Pixel-precise scrolling, as from a touchpad, SHOULD be
converted at 3 units per logical pixel (one detent for 40 logical pixels). The sign
is final: the client applies the user's scrolling direction preference, and the
host MUST NOT apply its own. This version defines no scroll phases.

### 8.4.3 Mouse

The mouse is a relative pointing device, used in relative pointer mode.

```
Mouse Report {
  Buttons (8),
  Motion X Total (32),
  Motion Y Total (32),
  Wheel Total (32),
  Pan Total (32),
}
```

Buttons, Wheel Total and Pan Total are as for the pointer. Motion X Total and
Motion Y Total are running totals of relative motion, in 1/256ths of a device
count. Clients SHOULD report raw, unaccelerated motion where the platform
provides it.

### 8.4.4 Touchscreen

```
Touchscreen Report {
  Contact Count (8),
  Contact Size (8),
  Touch Contact (..) ...,
}

Touch Contact {
  Contact ID (8),
  Flags (8),
  X (32),
  Y (32),
  Pressure (16),
}
```

The report lists every contact currently touching, at most 16. Contact Size is the
number of bytes in each Touch Contact (12 in this version); a receiver reads the
fields it knows from each contact and skips to the next by Contact Size. A contact
present in the device's previous report but absent from this one has lifted.
Contact IDs identify a finger while it touches and may be reused afterwards. Flag
0x01 `CANCELLED` means the contact ends without effect, for example after palm
rejection; such a contact appears in exactly one report with this flag and is then
absent. Pressure runs from 0 to 65535 and is meaningful only if the device has the
pressure feature. Hosts that support touchscreens MUST accept at least 10
simultaneous contacts.

### 8.4.5 Pen

```
Pen Report {
  Flags (8),
  X (32),
  Y (32),
  Pressure (16),
  Tilt X (16),
  Tilt Y (16),
  Twist (16),
}
```

- **Flags**, with the meanings of the corresponding HID Digitizer usages:
  0x01 in range (hovering or touching), 0x02 tip switch (the writing tip is
  touching), 0x04 barrel switch, 0x08 eraser (the eraser end is touching),
  0x10 invert (the eraser end is towards the surface). When the eraser end
  touches, eraser and invert are set and tip switch is clear.
- **Pressure**: 0 to 65535.
- **Tilt X**, **Tilt Y**: signed, in 1/100 degree from −9000 to 9000.
- **Twist**: unsigned, in 1/100 degree from 0 to 35999.

Tilt and twist follow the conventions of [POINTER].

### 8.4.6 Touchpad

The touchpad device carries raw finger contacts, for hosts whose operating
system recognizes touchpad gestures itself.

```
Touchpad Report {
  Buttons (8),
  Contact Count (8),
  Contact Size (8),
  Touchpad Contact (..) ...,
}

Touchpad Contact {
  Contact ID (8),
  Flags (8),
  X (16),
  Y (16),
}
```

X and Y run from 0 at the left and top edges to 65535 at the right and bottom
edges of the surface; its physical size is in `DEVICE_ATTACH`. Contact Size is the
number of bytes in each Touchpad Contact (6 in this version). Flag 0x01
`CONFIDENCE` means the contact is a finger rather than a palm. Buttons are 0x01
primary and 0x02 secondary. As for touchscreens, at most 16 contacts are listed and
an absent contact has lifted.

By default the client interprets touchpad gestures itself, applying the user's own
settings for speed, scrolling direction and inertia, and sends pointer movement and
scrolling as Pointer reports. A client SHOULD attach a touchpad device only when the
user asks for the host's native gestures. While a touchpad device is attached, the
client MUST NOT also produce Pointer reports from the same contacts, and the host
reports the resulting pointer movement with `CURSOR_WARP` so that the client can
draw the cursor ([Section 9.4](09-cursor.md#94-cursor_warp-0x39)).

### 8.4.7 Gamepad

The standard gamepad is a fallback for controllers the client cannot pass through,
for example because the platform does not expose them as HID devices.

```
Gamepad Report {
  Buttons (32),
  Left Trigger (16),
  Right Trigger (16),
  Left Stick X (16),
  Left Stick Y (16),
  Right Stick X (16),
  Right Stick Y (16),
  Gyro X (16),
  Gyro Y (16),
  Gyro Z (16),
  Accel X (16),
  Accel Y (16),
  Accel Z (16),
}
```

- **Buttons**: the Standard Gamepad layout of [GAMEPAD]. Bit `1 << n` is button n,
  for n from 0 to 16; bits 17 to 31 are reserved. Bits 6 and 7 (the triggers) MUST
  be set exactly when the corresponding trigger value is at least 32768; hosts use
  the analog trigger fields.
- **Triggers**: unsigned, 0 released to 65535 fully pressed.
- **Sticks**: signed, −32768 to 32767; negative values mean left or up, as in
  [GAMEPAD].
- **Gyro**, **Accel**: signed; gyro in 1/16 degree per second, acceleration in
  1/4096 of standard gravity. Holding the controller level in front of the player,
  X points right, Y points up and Z points towards the player. At rest and level,
  Accel Y reads +4096. Rotation follows the right-hand rule: positive rotation is
  counter-clockwise when viewed from the positive end of the axis. These fields are
  0 when the device lacks the motion sensors feature.

The gamepad's output report is:

```
Gamepad Output {
  Low Frequency Rumble (16),
  High Frequency Rumble (16),
  Left Trigger Rumble (16),
  Right Trigger Rumble (16),
  LED Red (8),
  LED Green (8),
  LED Blue (8),
}
```

Rumble strengths run from 0 (off) to 65535 (full).

### 8.4.8 Text

A text report is committed Unicode text: 1 to 256 bytes of UTF-8 with no length
prefix. Longer text is split into consecutive reports at character boundaries. The
host injects it as text, without layout translation. Unlike other standard reports,
each text report is an action, not a state.

## 8.5 INPUT_ACK datagram

```
INPUT_ACK Datagram {
  Datagram Type (i) = 0x05,
  Largest Sequence (i),
}
```

`Largest Sequence` is the highest sequence number up to which the host has applied
or skipped every report. The host MUST answer every `INPUT` datagram with an
`INPUT_ACK`, including datagrams that carry only reports it has already applied,
but SHOULD send at most one `INPUT_ACK` per millisecond.

## 8.6 Client rules

**Producing reports**

1. The client produces a report for a standard device whenever its state changes.
   For pointer, mouse, touchscreen, pen, touchpad and gamepad devices it SHOULD
   merge changes that only move positions, axes or totals into at most one report
   per device per millisecond, and MAY merge them over longer intervals, up to 8 ms,
   when its retained reports do not fit in one datagram. Because motion is sent as
   running totals, merging loses no movement.
2. A report is a **transition** if it is a text report, or if it changes a key, a
   consumer key, a button, the set of contacts or a contact's flags, a pen flag or
   a gamepad button relative to the device's previous report. Transitions are never
   merged away.

**Retaining and sending reports**

3. The client retains every report it sends. **Oldest Retained** only increases; the
   client advances it past a report only when that report has been acknowledged,
   has been retained for one second, or is **superseded**: a later report of the
   same device exists, and the report is not a transition. A passthrough report is
   superseded by a later report of the same device with the same report ID byte.
   Every report at or above Oldest Retained stays retained.
4. The client sends each new report as soon as it exists, combining reports from at
   most 1 ms into one datagram. It MUST send every retained, unacknowledged report
   again at least once per repeat interval, max(2 ms, smoothed RTT / 4), until it is
   acknowledged, using as many datagrams as needed. Each datagram is filled with new
   reports first, then with retained reports, oldest first.
5. When a report leaves the retained set because it is a second old, rather than
   because it was acknowledged or superseded, the client MUST produce one fresh
   report with the current state of that device if it is a standard device other
   than text.
6. While any key, button, contact or gamepad control is away from rest, or a
   passthrough device's latest report differs from its rest report, the client MUST
   send an `INPUT` datagram at least every 100 ms, repeating its newest report if
   nothing has changed.

**Focus**

7. When the session loses input focus, or the user releases capture with the local
   release action, the client MUST immediately send reports that release every key,
   consumer key, button, contact and pen tip of its `KEYBOARD`, `POINTER`, `MOUSE`,
   `TOUCHSCREEN`, `PEN` and `TOUCHPAD` devices. It SHOULD do the same for gamepads
   and passthrough devices unless the user has allowed background input. When focus
   returns, keys and buttons that are already held are not reported until they are
   pressed again. Keys consumed by local shortcuts, including the release action,
   MUST NOT be reported.

**Pointer**

8. In absolute pointer mode, pointer position, buttons and scrolling are reported by
   the `POINTER` device; in relative mode, by the `MOUSE` device. When the mode
   changes, the client first sends a report with the held buttons on the new mode's
   device, then a report with no buttons on the old mode's device.
9. The client MUST NOT send pointer, touchscreen or pen positions computed with a
   layout it has not yet received in a `DISPLAY_CONFIG`.

## 8.7 Host rules

1. The host applies each report exactly once, in ascending sequence order, and
   ignores reports at or below the last sequence it applied or skipped.
2. When a sequence number is missing, the host compares it with the Oldest Retained
   of the latest `INPUT` datagram it has received. Below Oldest Retained, the report
   will never arrive and the host skips it immediately. At or above Oldest Retained,
   the report will be sent again: the host holds later reports until it arrives, but
   for at most max(20 ms, smoothed RTT), after which it skips it.
3. Reports for a Device ID that the host has not accepted, or that has been
   detached, are skipped without being parsed.
4. The host applies reports as soon as they are in order. It MAY use Time Offset to
   keep the spacing of reports that arrive together, such as pen samples, but SHOULD
   NOT otherwise delay input.
5. The host treats the buttons of all pointing devices as one set, combined with a
   logical OR, as operating systems do for several mice, and injects a button change
   only when the combined set changes. In absolute mode it ignores the motion totals
   of `MOUSE` reports, apart from updating its baseline, and in relative mode it
   ignores the X and Y of `POINTER` reports; it applies buttons and scrolling from
   both devices in both modes.
6. Positions computed with an older layout than the host's current one, according to
   the datagram's Display Config ID, are mapped through that older layout if the host
   still has it, and are otherwise ignored while buttons and scrolling are applied.
   Positions in datagrams whose Warp ID is older than the host's latest `CURSOR_WARP`
   are ignored, while buttons and scrolling are applied.
7. The host MAY inject standard devices through the operating system's input
   interfaces or through virtual HID devices built from the report layouts, subject
   to [Section 8.2.5](#825-passthrough-devices).
8. The host MUST return every device to rest, releasing all keys, buttons and
   contacts and injecting the rest reports of passthrough devices, when the session
   ends. It SHOULD do the same if no `INPUT` datagram has arrived for one second while
   anything is held.

## 8.8 Output and feature reports

### 8.8.1 DEVICE_OUTPUT datagram

Output reports of standard devices (keyboard LEDs, gamepad rumble and LEDs) are
state, so they travel in datagrams:

```
DEVICE_OUTPUT Datagram {
  Datagram Type (i) = 0x06,
  Device ID (i),
  Output Sequence (i),
  Report Data (..),
}
```

Report Data is a Keyboard Output or Gamepad Output report. Output Sequence starts at
1 for each device and increases by 1 for each new output state; repeats of an
unchanged state keep their number. The client applies a datagram only if its Output
Sequence is higher than the last one applied for that device, starting from 0. The
host sends `DEVICE_OUTPUT` when an output changes, repeats the current output of each
such device at least once per second, and repeats it every 100 ms while any rumble
value is non-zero. The client MUST stop all rumble on a device if no `DEVICE_OUTPUT`
for it has arrived for 250 ms while it was rumbling.

### 8.8.2 HID output streams

Output reports that applications send to a passthrough device travel on a
unidirectional stream of type 0x03 that the host opens for that device:

```
HID_OUTPUT Stream {
  Stream Type (i) = 0x03,
  Device ID (i),
  HID Output Report (..) ...,
}

HID Output Report {
  Report Length (i),
  Report Data (..),
}
```

Report Data is one output report, preceded by its report ID byte (0x00 when the
descriptor defines no report IDs). The host opens at most one such stream per device
and sends the device's output reports on it in order. Their meaning is unknown to
QDP, so none may be lost or reordered; a stream per device lets QUIC's flow control
slow down only that device when it cannot keep up. The client writes the reports to
the physical device in order, and MUST NOT let a slow device delay other processing.

### 8.8.3 HID_FEATURE_REQUEST (0x35) and HID_FEATURE_RESPONSE (0x36)

```
HID_FEATURE_REQUEST Payload {
  Request ID (i),
  Device ID (i),
  Operation (8),
  Report Data (Octets),
}

HID_FEATURE_RESPONSE Payload {
  Request ID (i),
  Status (8),
  Report Data (Octets),
}
```

When an application on the host reads (Operation 0x00, `GET`) or writes (Operation
0x01, `SET`) a feature report of a passthrough device, the host sends
`HID_FEATURE_REQUEST`. Report Data always begins with the report ID byte (0x00 when
the descriptor defines no report IDs): for `GET` it is just that byte; for `SET` it
is the whole feature report. Request ID starts at 1 and increases with each request.

The client performs the operation on the physical device and answers with
`HID_FEATURE_RESPONSE`: Status 0x00 `OK`, 0x01 `FAILED` or 0x02 `DENIED`, with the
whole feature report, including its report ID byte, for a successful `GET`. Responses
may be sent in any order. The client answers `FAILED` for an unknown or detached
Device ID, and MUST NOT delay later control messages while an operation is pending. A
host that has no response after one second SHOULD report failure to the application,
and ignores responses with unknown or timed-out Request IDs.

These messages are allowed once the host has accepted the device, including during
configuration, because operating system drivers read feature reports while setting
up a new device.

## 8.9 POINTER_MODE (0x33)

```
POINTER_MODE Payload {
  Mode (8),
  Flags (8),
  [Confine X (si), Confine Y (si), Confine Width (i), Confine Height (i)],
}
```

- **Mode 0x00, `ABSOLUTE`**: the client draws the cursor itself
  ([Section 9](09-cursor.md)) and reports through the `POINTER` device.
- **Mode 0x01, `RELATIVE`**: an application on the host, typically a game, has hidden
  and captured the pointer. While the session has focus, the client hides its cursor,
  captures the pointer, for example with pointer lock, and reports through the
  `MOUSE` device.
- **Flags 0x01, `CONFINED`**: in absolute mode, the host has confined the pointer to
  the rectangle that follows, in desktop positions. The client SHOULD confine its
  pointer to the same area, but only while the session has focus and only within the
  area showing the session, and MUST drop the confinement when the user invokes the
  local release action.

The host sends `POINTER_MODE` during configuration and whenever the mode changes.
Immediately after a `POINTER_MODE` that switches to `ABSOLUTE`, the host MUST send a
`CURSOR_WARP` with the pointer's current position, and the client moves its pointer
there before it sends positions. The client MUST always give the user a local way to
release pointer and keyboard capture.
