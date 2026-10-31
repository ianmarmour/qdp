# 9. Cursor

## 9.1 Model

In absolute pointer mode ([Section 8.7](08-input.md#87-pointer_mode-0x33)) the
client draws the cursor itself, at its own pointer position, so pointer movement
has no network latency. The host tells the client which cursor image to draw,
sends each image once, and reports when an application moves the pointer.

The host MUST NOT draw the cursor into the video it sends.

## 9.2 Cursor images

A cursor image is sent on its own unidirectional stream of type 0x01:

```
CURSOR_IMAGE Stream {
  Stream Type (i) = 0x01,
  Cursor ID (i),
  Width (i),
  Height (i),
  Hotspot X (i),
  Hotspot Y (i),
  Scale (i),
  Format (i),
  Pixel Data (..),
}
```

- **Cursor ID**: chosen by the host, at least 1. Sending an image with an ID
  that is already defined replaces the earlier image.
- **Width**, **Height**: in pixels, from 1 to 512 each.
- **Hotspot X**, **Hotspot Y**: the pixel within the image that marks the
  pointer position. Hotspot X MUST be less than Width and Hotspot Y less than
  Height.
- **Scale**: the interface scale, in percent, the image was drawn for. The client
  SHOULD resize the image when it shows it on a display with a different scale.
- **Format**: 0x01, the only format defined: 8-bit red, green, blue and alpha
  per pixel, in that order, with sRGB color and straight (not premultiplied)
  alpha.
- **Pixel Data**: Width × Height pixels, row by row from the top. The stream
  ends after the last pixel; a stream that ends early or carries extra bytes is
  reset by the client with `QDP_PROTOCOL_VIOLATION` and the image is ignored.

The client caches images by Cursor ID for the whole session. A client MAY limit
its cache to 128 images; the host SHOULD reuse IDs of images it no longer needs.

## 9.3 CURSOR_SET (0x38)

```
CURSOR_SET Payload {
  Cursor ID (i),
}
```

The host sends `CURSOR_SET` whenever the cursor shape changes. Cursor ID 0 hides
the cursor. If the client has not yet received the named image, it keeps showing
the previous cursor until the image arrives.

## 9.4 CURSOR_WARP (0x39)

```
CURSOR_WARP Payload {
  Monitor ID (i),
  X (i),
  Y (i),
}
```

The host sends `CURSOR_WARP` when the pointer moves without the client's input,
for example when an application centres the pointer in a dialog. In absolute mode
the client MUST move its drawn cursor to the new position, which is given in
Positions on the named monitor, and SHOULD move its local pointer to match when
it has permission to.
