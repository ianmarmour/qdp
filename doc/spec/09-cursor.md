# 9. Cursor

## 9.1 Model

In absolute pointer mode ([Section 8.9](08-input.md#89-pointer_mode-0x33)) the
client draws the cursor itself, at its own pointer position, so pointer movement
has no network latency. The host tells the client which cursor image to draw,
sends each image once, and reports when the pointer moves for reasons other than
the client's own pointer reports.

The host MUST NOT draw the cursor into the video it sends.

## 9.2 Cursor images

A cursor image is sent on its own unidirectional stream of type 0x01:

```
CURSOR_IMAGE Stream {
  Stream Type (i) = 0x01,
  Cursor ID (i),
  Image Serial (i),
  Width (i),
  Height (i),
  Hotspot X (i),
  Hotspot Y (i),
  Scale (i),
  Format (i),
  Pixel Data (..),
}
```

- **Cursor ID**: chosen by the host, at least 1. Sending an image for an ID that is
  already defined replaces the earlier image. The host MUST NOT have more than 128
  Cursor IDs defined at once; to define another, it replaces an existing one. Clients
  MUST be able to cache 128 images.
- **Image Serial**: starts at 1 and increases with every cursor image the host sends,
  across all Cursor IDs. Streams complete in any order, so a client that has already
  cached an image for this Cursor ID with a higher serial MUST discard this one.
- **Width**, **Height**: in pixels, from 1 to 512 each.
- **Hotspot X**, **Hotspot Y**: the pixel within the image that marks the pointer
  position. Hotspot X MUST be less than Width and Hotspot Y less than Height.
- **Scale**: the interface scale, in percent, the image was drawn for.
- **Format**: 0x01, the only format defined: 8-bit red, green, blue and alpha per
  pixel, in that order, with sRGB color and straight (not premultiplied) alpha.
- **Pixel Data**: Width × Height pixels, row by row from the top. The stream ends
  after the last pixel. A client that receives a stream that ends early or carries
  extra bytes aborts reading it with STOP_SENDING carrying `QDP_PROTOCOL_VIOLATION`
  and ignores the image.

The image is drawn so that it keeps its size relative to the session content: its
displayed size is its pixel size × (the client display's scale ÷ the image's Scale)
× (the width of the client's display area ÷ the host monitor's Width).

A client can receive a cursor image before it has processed `SESSION_START`,
because QUIC does not order streams with the control stream. It buffers such
streams as described in [Section 4.5](04-session.md#45-configuration).

## 9.3 CURSOR_SET (0x38)

```
CURSOR_SET Payload {
  Cursor ID (i),
  Image Serial (i),
}
```

The host sends `CURSOR_SET` whenever the cursor shape changes. Cursor ID 0 hides
the cursor; its Image Serial is then 0. Otherwise Image Serial is the serial of the
image the host means. If the client has not yet received that image, it keeps
showing the previous cursor until the image arrives. When the image of the current
Cursor ID is replaced by one with the current serial, the client redraws the cursor.

## 9.4 CURSOR_WARP (0x39)

```
CURSOR_WARP Payload {
  Warp ID (i),
  Flags (8),
  X (si),
  Y (si),
}
```

- **Warp ID**: starts at 1 and increases with each `CURSOR_WARP`. The client echoes
  the latest Warp ID it has processed in its `INPUT` datagrams, so the host can
  ignore pointer positions computed before the warp
  ([Section 8.7](08-input.md#87-host-rules)).
- **Flags**: 0x01 `FOLLOW`.
- **X**, **Y**: the pointer's new desktop position
  ([Section 1.5.6](01-introduction.md#156-positions)).

The host sends `CURSOR_WARP` when the pointer moves without the client's pointer
reports:

- **Without `FOLLOW`**: an application moved the pointer, for example to centre it
  in a dialog. In absolute mode the client MUST move its drawn cursor to the new
  position, and SHOULD move its own pointer to match when the platform allows.
- **With `FOLLOW`**: the pointer is being driven by a device whose movement the
  client does not know, such as a raw touchpad or a passthrough device. The client
  draws the cursor at the reported position but does not move its own pointer. The
  host sends these at most once per display refresh while such movement continues.

The client applies warps to its own pointer only while the session has input
focus, only within the area showing the session, and at most 60 times per second.
