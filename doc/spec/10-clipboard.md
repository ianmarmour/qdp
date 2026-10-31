# 10. Clipboard

## 10.1 Model

Clipboard sharing is optional and can be limited to one direction by
administrative policy ([Section 4.6.6](04-session.md#466-clipboard-0x05)) and by
the user. It is lazy: when its clipboard changes, an endpoint announces which
formats it can provide, and data is transferred only when the user pastes on the
other side. Copying a large image on one side therefore costs nothing until it is
pasted on the other. This is also how the clipboards of the operating systems QDP
runs on work locally, so an endpoint can place the peer's announcement on its own
clipboard as a promise to provide the data later.

Formats are named by media type [RFC6838]. An endpoint that supports the clipboard
MUST support `text/plain;charset=utf-8`. It MAY support:

- `text/html`: UTF-8 HTML, a fragment or a document, without platform-specific
  headers such as the Windows CF_HTML header;
- `image/png`: a complete PNG file;
- other media types that do not refer to files or other local resources.

Formats that name files or other resources on the sender's system, such as
`text/uri-list`, `x-special/gnome-copied-files` and `public.file-url`, MUST NOT be
offered or accepted in this version.

Format names are sent in canonical form: lowercase, with no whitespace, and with
parameters as `;name=value`. A requester copies the format string exactly from the
offer. A receiver compares format names case-insensitively for the type, subtype and
parameter names and the `charset` value, ignoring whitespace.

## 10.2 CLIPBOARD_OFFER (0x48)

```
CLIPBOARD_OFFER Payload {
  Offer ID (i),
  Peer Offer ID (i),
  Format Count (i),
  Format (String) ...,
}
```

- **Offer ID**: starts at 1 and increases with each offer the sender makes.
- **Peer Offer ID**: the highest Offer ID the sender had received from its peer when
  it made this offer, or 0.
- **Format**: the formats the sender can provide. At most 32, each at most 255 bytes.
  An offer with no formats means the sender's clipboard was cleared.

An endpoint allowed to send its clipboard sends `CLIPBOARD_OFFER` when its local
clipboard changes, but MUST NOT send one for a change it made itself to publish the
peer's offer on its local clipboard. A new offer replaces the sender's previous one.

When the session becomes active, the client sends its initial offer first, possibly
with no formats. The host sends its initial offer only after receiving the client's,
so the two never cross at the start.

Two offers cross when each endpoint sends one before receiving the other's. An
endpoint detects this when a received offer's Peer Offer ID is lower than its own
latest Offer ID. The client's offer wins: the host installs the client's offer, and
the client ignores the host's.

An endpoint SHOULD send offers only while the session has focus on the client.
Endpoints MUST NOT offer content that their platform marks as concealed or
transient, such as passwords copied from a password manager (on macOS the
`org.nspasteboard.ConcealedType` and `TransientType` markers, on Windows content
excluded from clipboard monitoring or history, on KDE the
`x-kde-passwordManagerHint` marker).

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

When the user pastes on the receiving side, the receiver sends `CLIPBOARD_REQUEST`
for one format of the most recent offer. Request ID starts at 1 and increases with
each request. Because an endpoint never offers a clipboard change it made to publish
the peer's offer ([Section 10.2](#102-clipboard_offer-0x48)), it is never asked for
data that it holds only as a promise from the peer.

The offering endpoint answers within 5 seconds, either with a `CLIPBOARD_DATA`
stream ([Section 10.4](#104-clipboard-data)) or with `CLIPBOARD_REJECT`:

| Reason | Name |
|---|---|
| 0x01 | `STALE_OFFER` (the offer has been replaced) |
| 0x02 | `UNAVAILABLE` (the data can no longer be produced, or the format was not offered) |
| 0x03 | `TOO_LARGE` (the data exceeds the requester's Max Size) |
| 0x04 | `DENIED` (policy or the user forbids the transfer) |

The offering endpoint SHOULD limit how often it serves requests for one offer, and a
client MAY ask its user to confirm requests from the host. A requester that has no
answer after 5 seconds MAY give up.

## 10.4 Clipboard data

```
CLIPBOARD_DATA Stream {
  Stream Type (i) = 0x02,
  Request ID (i),
  Data (..),
}
```

The data runs to the end of the stream. It MUST NOT exceed the requester's Max Size;
a requester that receives more MUST abort the stream with STOP_SENDING carrying
`QDP_RESOURCE_LIMIT`. A requester that no longer needs the data, for example because
the user cancelled the paste or it gave up waiting, aborts reading with
`QDP_REQUEST_CANCELLED`.

Text in `text/plain;charset=utf-8` MUST be valid UTF-8 and SHOULD use LF line
endings; each endpoint converts line endings to its platform's convention. Both
endpoints treat received clipboard data as untrusted
([Section 11.5](11-security.md#115-untrusted-media)).
