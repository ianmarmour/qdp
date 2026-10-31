# 10. Clipboard

## 10.1 Model

Clipboard sharing is optional and can be limited to one direction by
administrative policy ([Section 4.6.6](04-session.md#466-clipboard-0x05)). It
follows the delayed-rendering model of RDP [MS-RDPECLIP]: when its clipboard
changes, an endpoint announces which formats it can provide, and data is
transferred only when the user pastes on the other side. Copying a large image on
one side therefore costs nothing until it is pasted on the other.

Formats are named by media type [RFC6838]. An endpoint that supports the
clipboard MUST support `text/plain;charset=utf-8`. It MAY support `text/html`,
`image/png` and other types.

## 10.2 CLIPBOARD_OFFER (0x48)

```
CLIPBOARD_OFFER Payload {
  Offer ID (i),
  Format Count (i),
  Format (String) ...,
}
```

An endpoint allowed to send its clipboard sends `CLIPBOARD_OFFER` when its local
clipboard changes, and once when the session becomes active if the clipboard is
not empty. Offer ID increases with each offer. A new offer replaces the sender's
previous one; an offer with no formats means the clipboard was cleared. At most 32
formats, each at most 255 bytes.

## 10.3 CLIPBOARD_REQUEST (0x49) and CLIPBOARD_REJECT (0x4A)

```
CLIPBOARD_REQUEST Payload {
  Request ID (i),
  Offer ID (i),
  Format (String),
}

CLIPBOARD_REJECT Payload {
  Request ID (i),
  Reason (i),
}
```

When the user pastes on the receiving side, the receiver sends
`CLIPBOARD_REQUEST` for one format of the most recent offer. Request ID increases
with each request. The offering endpoint answers either with a `CLIPBOARD_DATA`
stream ([Section 10.4](#104-clipboard-data)) or with `CLIPBOARD_REJECT`:

| Reason | Name |
|---|---|
| 0x01 | `STALE_OFFER` (the offer has been replaced) |
| 0x02 | `UNAVAILABLE` (the data can no longer be produced) |
| 0x03 | `TOO_LARGE` (the data exceeds the requester's Max Size) |
| 0x04 | `DENIED` (policy forbids the transfer) |

## 10.4 Clipboard data

```
CLIPBOARD_DATA Stream {
  Stream Type (i) = 0x02,
  Request ID (i),
  Data (..),
}
```

The data runs to the end of the stream. It MUST NOT exceed the requester's Max
Size; a requester that receives more MUST abort the stream with
`QDP_RESOURCE_LIMIT`. A requester that no longer needs the data, for example
because the user cancelled the paste, aborts reading with
`QDP_REQUEST_CANCELLED`.

Text in `text/plain;charset=utf-8` MUST be valid UTF-8 and SHOULD use LF line
endings; each endpoint converts line endings to its platform's convention.
