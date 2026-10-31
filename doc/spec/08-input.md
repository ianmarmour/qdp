# 8. Input

## 8.1 Model

All input in QDP is described in terms of USB HID [HID]. The client announces
each input device it shares with `DEVICE_ATTACH`, and each device then produces
**input reports**. There are two kinds of device:

- **Standard devices**: keyboard, pointer, mouse, touchscreen, pen, touchpad,
  gamepad and text. Their report formats are defined in
  [Section 8.4](#84-standard-device-reports). The client builds their reports from
  its operating system's input events, so they work with any physical device,
  including built-in keyboards and trackpads that are not HID devices and
  devices the operating system does not let applications read directly. The host
  needs no HID descriptor parser to use them.
- **Passthrough devices**: the client forwards a physical HID device's report
  descriptor and reports unchanged, so that applications on the host see the real
  device. This is intended for game controllers, steering wheels, flight sticks
  and similar devices.

Every input report carries a sequence number shared by all devices. The host
applies reports strictly in sequence order, so the order of actions across
devices, such as holding Ctrl on the keyboard and clicking the mouse, is
preserved. Standard reports state the device's complete current state. The
client repeats every report until the host acknowledges it, so a lost datagram
never loses input and never waits for a retransmission.

The keyboard, pointer and mouse are always attached when their input classes
have been negotiated ([Section 4.6.5](04-session.md#465-input-0x04)).

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
  Vendor ID (16),
  Product ID (16),
  Version (16),
  Report Descriptor (Octets),
}
```

- **Device ID**: chosen by the client, unique among attached devices. A Device
  ID may be reused after `DEVICE_DETACH`.
- **Device Kind**: from the table below.
- **Features**: kind-specific flags, from the table below.
- **Name**: the product name, at most 255 bytes.
- **Touchpad Info**: present only for `TOUCHPAD` devices. The size of the touch
  surface in hundredths of a millimetre.
- **Passthrough Info**: present only for `PASSTHROUGH` devices. The vendor ID,
  product ID and version (`bcdDevice`) the physical device reports, and its HID
  report descriptor, at most 16384 bytes.

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

A `GAMEPAD` without a layout flag has an Xbox-style layout.

During configuration, the client MUST attach one `KEYBOARD`, one `POINTER` and
one `MOUSE`, and one `TEXT` device if the `TEXT` class was negotiated, before
sending `CLIENT_READY`. It may attach and detach other devices at any time.

The host SHOULD create a matching virtual device for gamepads, touchscreens, pens
and touchpads. For a passthrough device it SHOULD create a virtual HID device
with the same descriptor, vendor ID, product ID and version, so that applications
recognize it. A host that cannot support a device ignores its reports.

### 8.2.2 DEVICE_DETACH (0x32)

```
DEVICE_DETACH Payload {
  Device ID (i),
}
```

The host returns the device to rest, releasing anything it holds, and removes its
virtual device.

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
  layout identifier, 0x02 XKB layout name, 0x03 macOS input source identifier.
- **Layout**: the client's keyboard layout, for example `00000809` on Windows or
  `gb` with XKB.

Keys are sent as physical key positions, so the host SHOULD select a matching
layout. Then each key produces the character printed on the user's keyboard.

## 8.3 INPUT datagram

```
INPUT Datagram {
  Datagram Type (i) = 0x04,
  Timestamp (i),
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
- **Sequence**: starts at 1 and increases by 1 for each report, across all
  devices. The reports in one datagram MUST have consecutive, ascending sequence
  numbers.
- **Time Offset**: how many microseconds before Timestamp the report was
  produced.
- **Report Data**: for a standard device, a report in the format of
  [Section 8.4](#84-standard-device-reports). For a passthrough device, an input
  report exactly as the device produced it, including its report ID byte if the
  descriptor defines report IDs.

## 8.4 Standard device reports

Standard reports are fixed layouts. Each states the device's complete current
state, except that relative motion, wheel and pan fields give the movement since
the device's previous report. A receiver MUST ignore bytes after the fields
defined here.

Positions are desktop positions in 1/256 pixel
([Section 1.5.6](01-introduction.md#156-positions)), sent as signed 32-bit
integers. The client maps the pointer's location in its display area to the host
desktop using `DISPLAY_CONFIG`.

### 8.4.1 Keyboard

```
Keyboard Report {
  Keys (256),
  Consumer Key (16) ...,
}
```

- **Keys**: a bitmap of held keys on the Keyboard/Keypad page (0x07). Usage u is
  held when byte ⌊u / 8⌋ has the bit with mask `1 << (u mod 8)` set. Usages 0x00
  to 0x03 are error codes in [HID] and MUST be clear. A usage names a key's
  physical position, not its character: usage 0x04 is the key labelled A on a US
  layout, whatever layout the user has. Modifier keys are usages 0xE0 to 0xE7.
- **Consumer Key**: exactly four slots holding up to four held keys from the
  Consumer page (0x0C), such as 0x00E9 Volume Increment. Unused slots are 0;
  order does not matter.

The report is 40 bytes. Clients MUST NOT generate automatic key repeats; the host
generates repeats itself.

The keyboard's output report ([Section 8.8.1](#881-device_output-datagram)) is:

```
Keyboard Output {
  LEDs (8),
}
```

LEDs are 0x01 Num Lock, 0x02 Caps Lock, 0x04 Scroll Lock, 0x08 Compose and
0x10 Kana, as on the HID LED page. When the session gains keyboard focus and the
host's Num, Caps or Scroll Lock state differs from the client's, the client
SHOULD send press and release reports for those lock keys so that the host
matches the client.

### 8.4.2 Pointer

The pointer is an absolute pointing device, used in absolute pointer mode
([Section 8.9](#89-pointer_mode-0x33)).

```
Pointer Report {
  Buttons (8),
  X (32),
  Y (32),
  Wheel (16),
  Pan (16),
}
```

- **Buttons**: 0x01 left, 0x02 right, 0x04 middle, 0x08 back, 0x10 forward
  (buttons 1 to 5 of the HID Button page).
- **X**, **Y**: the pointer's desktop position.
- **Wheel**: signed vertical scrolling since the device's previous report, in
  1/120ths of a wheel detent. Positive means the wheel turned away from the user.
- **Pan**: signed horizontal scrolling in the same units. Positive means right.

Each report carries buttons and position together, so a click is applied exactly
where the pointer was. Smooth touchpad scrolling is expressed with Wheel and Pan
values smaller than one detent.

### 8.4.3 Mouse

The mouse is a relative pointing device, used in relative pointer mode.

```
Mouse Report {
  Buttons (8),
  X (32),
  Y (32),
  Wheel (16),
  Pan (16),
}
```

Buttons, Wheel and Pan are as for the pointer. X and Y are signed movement since
the device's previous report, in 1/256ths of a device count. Clients SHOULD
report raw, unaccelerated motion where the platform provides it.

### 8.4.4 Touchscreen

```
Touchscreen Report {
  Contact Count (8),
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

The report lists every contact currently touching, at most 16. A contact present
in the device's previous report but absent from this one has lifted. Contact IDs
identify a finger while it touches and may be reused afterwards. Flag 0x01
`CANCELLED` means the contact ends without effect, for example after palm
rejection; such a contact appears in exactly one report with this flag and is
then absent. Pressure runs from 0 to 65535 and is meaningful only if the device
has the pressure feature. Hosts that support touchscreens MUST accept at least
10 simultaneous contacts.

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
  0x01 in range (hovering or touching), 0x02 tip switch (touching),
  0x04 barrel switch, 0x08 eraser (eraser end touching), 0x10 invert (eraser end
  towards the surface).
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
edges of the surface; its physical size is in `DEVICE_ATTACH`. Flag 0x01
`CONFIDENCE` means the contact is a finger rather than a palm. Buttons are 0x01
primary and 0x02 secondary. As for touchscreens, at most 16 contacts are listed
and an absent contact has lifted.

By default the client interprets touchpad gestures itself, applying the user's
own settings for speed, scrolling direction and inertia, and sends pointer
movement and scrolling as Pointer reports. A client SHOULD attach a touchpad
device only when the user asks for the host's native gestures.

### 8.4.7 Gamepad

The standard gamepad is a fallback for controllers the client cannot pass
through, for example because the platform does not expose them as HID devices.

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
  for n from 0 to 16; bits 17 to 31 are reserved.
- **Triggers**: unsigned, 0 released to 65535 fully pressed.
- **Sticks**: signed, −32768 to 32767; negative values mean left or up, as in
  [GAMEPAD].
- **Gyro**, **Accel**: signed; gyro in 1/16 degree per second, acceleration in
  1/4096 of standard gravity. Holding the controller level in front of the
  player, X points right, Y points up and Z points towards the player. Rotation
  is positive counter-clockwise about each axis. These fields are 0 when the
  device lacks the motion sensors feature.

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

A text report is committed Unicode text, for example from an input method
editor: 1 to 256 bytes of UTF-8 with no length prefix. Longer text is split into
consecutive reports at character boundaries. The host injects it as text,
without layout translation. Unlike other standard reports, each text report is
an action, not a state.

## 8.5 INPUT_ACK datagram

```
INPUT_ACK Datagram {
  Datagram Type (i) = 0x05,
  Largest Sequence (i),
}
```

`Largest Sequence` is the highest sequence number up to which the host has
applied or skipped every report. The host SHOULD send `INPUT_ACK` promptly after
processing new reports, at most once per millisecond.

## 8.6 Client rules

1. The client produces a report for a standard device whenever its state
   changes. For pointer, mouse, touchscreen, pen and touchpad devices it SHOULD
   merge changes that only move positions or add motion or scrolling into at
   most one report per device per millisecond. A change of buttons, keys,
   contacts or pen flags always starts a new report.
2. Every `INPUT` datagram MUST carry the newest report and, immediately before it,
   as many of the preceding unacknowledged reports as fit, in ascending order
   with no gaps. The client SHOULD send a datagram as soon as a new report
   exists, combining reports from at most 1 ms.
3. The client keeps each unacknowledged report for at least 250 ms. It may drop
   reports older than that, and reports that no longer fit in a datagram. After
   dropping an unacknowledged report, the client MUST produce a fresh report
   with the current state of every attached standard device, and SHOULD resend
   the latest input report of each passthrough device as a new report.
4. If the newest report is still unacknowledged one smoothed round-trip time plus
   5 ms after it was last sent, the client SHOULD send another datagram carrying
   it.
5. While any key, button, contact or gamepad control is away from rest, the
   client MUST send an `INPUT` datagram at least every 100 ms, repeating its
   newest report if nothing has changed.
6. In absolute pointer mode, pointer movement and buttons are reported by the
   `POINTER` device; in relative mode, by the `MOUSE` device. Movement in
   absolute mode MUST NOT be added to `MOUSE` reports. When the mode changes, the
   client sends a report with no buttons held on the old mode's device, and a
   report with the held buttons on the new one.

## 8.7 Host rules

1. The host applies each report exactly once, in ascending sequence order, and
   ignores reports at or below the last sequence it applied or skipped.
2. If the first report in a datagram has a sequence more than one above the last
   one applied, the missing reports will never arrive: by rule 8.6.2 the client
   would otherwise have included them. The host skips them immediately. Because
   standard reports carry complete state, the next report of each device puts it
   right.
3. The host applies reports as soon as they arrive. It MAY use Time Offset to
   keep the spacing of reports that arrive together, such as pen samples, but
   SHOULD NOT otherwise delay input.
4. The host treats the buttons of all pointing devices as one set, combined with
   a logical OR, as operating systems do for several mice. In relative mode it
   ignores the X and Y of `POINTER` reports but still applies their buttons.
5. The host MAY inject standard devices through the operating system's input
   interfaces or through virtual HID devices built from the report layouts.
6. The host MUST return every device to rest, releasing all keys, buttons and
   contacts, when the session ends. It SHOULD do the same if no `INPUT` datagram
   has arrived for one second while anything is held.

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

Report Data is a Keyboard Output or Gamepad Output report. The client applies a
datagram only if its Output Sequence is higher than the last one applied for that
device. The host sends `DEVICE_OUTPUT` when an output changes, repeats the current
output of each such device at least once per second, and repeats it every 100 ms
while any rumble value is non-zero. The client MUST stop all rumble on a device if
no `DEVICE_OUTPUT` for it has arrived for 250 ms while it was rumbling.

### 8.8.2 HID_OUTPUT (0x34)

```
HID_OUTPUT Payload {
  Device ID (i),
  Report Data (Octets),
}
```

The host forwards, in order, each output report that an application sent to a
passthrough device's virtual device, including any report ID byte. The client
writes it to the physical device. Passthrough output reports travel on the
control stream because their meaning is unknown: they may be commands rather
than state, so none may be lost or reordered.

### 8.8.3 HID_FEATURE_REQUEST (0x35) and HID_FEATURE_RESPONSE (0x36)

```
HID_FEATURE_REQUEST Payload {
  Request ID (i),
  Device ID (i),
  Operation (8),
  Report ID (8),
  Report Data (Octets),
}

HID_FEATURE_RESPONSE Payload {
  Request ID (i),
  Status (8),
  Report Data (Octets),
}
```

When an application on the host reads (Operation 0x00, `GET`) or writes
(Operation 0x01, `SET`) a feature report of a passthrough device, the host sends
`HID_FEATURE_REQUEST`. Report Data is empty for `GET`. The client performs the
operation on the physical device and answers with `HID_FEATURE_RESPONSE`, with
Status 0x00 `OK`, 0x01 `FAILED` or 0x02 `DENIED`, and the report's data for a
successful `GET`. Request ID increases with each request. A host that has no
response after one second SHOULD report failure to the application.

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
- **Mode 0x01, `RELATIVE`**: an application on the host, typically a game, has
  hidden and captured the pointer. While the session has focus, the client hides
  its cursor, captures the pointer, for example with pointer lock, and reports
  through the `MOUSE` device.
- **Flags 0x01, `CONFINED`**: in absolute mode, the host has confined the pointer
  to the rectangle that follows, in desktop positions. The client SHOULD confine
  its pointer to the same area while the session has focus.

The host sends `POINTER_MODE` during configuration and whenever the mode
changes. The client MUST always give the user a local way to release pointer and
keyboard capture.
