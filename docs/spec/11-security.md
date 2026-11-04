# 11. Security considerations

A QDP session gives the client control of the host's keyboard and pointer and a
view of everything on its screen, and gives the host access to the client's
devices, microphone and clipboard. Security failures are therefore severe, and this
section is normative where it uses requirement keywords.

## 11.1 Host authentication

TLS authenticates the host in every session. A client MUST verify the host's
certificate in one of these ways:

- **Pinned key**: the SHA-256 hash of the host certificate's SubjectPublicKeyInfo
  matches a value the client obtained in advance, for example from a connection
  broker or an administrator. Clients MUST support this method.
- **Public key infrastructure**: the certificate chains to a trusted certificate
  authority and is valid for the host's name. Clients MAY support this method.
- **Pairing**: the client pins the key of a host with which it completed a
  `PAIRING` exchange ([Section 4.4.9](04-session.md#449-pairing-0x06)), using a code
  its user read on the host. Clients SHOULD support this method.

A client MAY offer trust on first use: showing the key's fingerprint to the user on
the first connection and pinning it if the user accepts. It MUST then warn loudly,
and refuse to connect by default, if the key later changes.

TLS session resumption is not used
([Section 3.4](03-transport.md#34-early-data-and-session-resumption)), so every
connection presents the host's certificate and a changed pin always takes effect.

## 11.2 Client authentication

- No QDP data other than the hello and authentication messages may be processed
  before authentication succeeds ([Section 4](04-session.md)). In particular a host
  MUST NOT send video, audio, cursor or clipboard data, and MUST NOT inject input,
  before then.
- SCRAM-SHA-256-PLUS binds the password exchange to the TLS connection with
  `tls-exporter` [RFC9266], so a host that relays the exchange to another host
  cannot use it there, and the password itself is never sent. An impostor host that
  the user has wrongly trusted can, however, attempt offline guessing of the
  password from the exchange, which is why clients reject low iteration counts and
  should not use passwords with hosts verified only by first-use trust.
- The `PASSWORD` method ([Section 4.4.8](04-session.md#448-password-0x05)) reveals
  the password to the host, so clients send it only to hosts whose key they have
  verified by pinning or by public key infrastructure.
- Tokens are bearer credentials: anyone holding one can use it. Hosts MUST reject
  tokens that were not issued for them, for example by checking that the token
  names the host's key. Brokers SHOULD issue tokens that expire within minutes and
  are valid for one session, and SHOULD give the client the host's key to pin along
  with the token. Clients MUST NOT send a token to a host whose key they have not
  verified by pinning or by public key infrastructure.
- Resumption tokens ([Section 4](04-session.md)) are also bearer credentials.
  Clients MUST store them only in the platform's protected credential storage, such
  as the Keychain on Apple platforms, the Keystore on Android or the user's
  protected store on Windows and Linux. Hosts accept each at most once and only for
  the identity that earned it.
- A pairing code is short so that users can type it, and is safe only because it is
  used once, in an exchange that gives an attacker one guess
  ([Section 4.4.9](04-session.md#449-pairing-0x06)): a passive observer learns
  nothing about it, and an active one, on either side, learns only whether one
  guess was right. Hosts therefore never accept a code twice, limit how often
  pairing may be tried, backing off exponentially after failures, and show every
  pairing request to their user, who can notice requests they did not make. A host that shows the code where people other
  than its user can see it, such as on a shared screen, lets them pair.
- A one-time password ([Section 4.4.10](04-session.md#4410-totp-0x09)) adds a
  factor an attacker who learned a password or copied a client's key does not
  have, and a code seen by someone else is useless once used. Its secret is shared
  with the verifier, which therefore protects it as it would a password database.
- Hosts MUST limit failed attempts per connection
  ([Section 4.4.1](04-session.md#441-messages)) and SHOULD limit them per source
  address and per user across connections. Hosts SHOULD send the same `Reason` for
  every kind of failure, so that responses do not reveal which user names exist.

## 11.3 Replay

0-RTT data is forbidden ([Section 3.4](03-transport.md#34-early-data-and-session-resumption)) because an
attacker who replays it could repeat keystrokes or clicks. All other QDP data is
protected against replay by QUIC.

## 11.4 Local control and privacy

- A host SHOULD tell its local user, where one may be present, that a remote session
  is active, for example with a visible indicator, and SHOULD offer to blank its
  local displays and suppress local input during a session.
- When a session ends, the host SHOULD leave the desktop as locked as the session
  found it: locked again if it was locked when the session began, and as it is if
  not, or as an administrator's policy says. A host that cannot tell whether the
  desktop was locked SHOULD lock it when the session ends other than by the user
  disconnecting, for example because the network dropped or the session was
  replaced, so that it is not left unlocked.
- The client MUST always offer a local way to release keyboard and pointer capture
  ([Section 8.9](08-input.md#89-pointer_mode-0x33)), so that a host cannot trap the
  user's input, and applies the host's pointer warps and confinement only within the
  limits of [Section 9.4](09-cursor.md#94-cursor_warp-0x39).
- **Microphone**: a client MUST NOT capture or send microphone audio unless the user
  has enabled the microphone for this session, MUST ignore a capture stream it did
  not request, MUST stop sending as soon as the user turns the microphone off, and
  SHOULD show an indicator while capturing ([Section 7.7](07-audio.md#77-microphone)).
- **Clipboard**: clipboard sharing can move sensitive data between machines. Hosts
  MUST let administrators disable it in either direction and MUST enforce that
  policy even if a client ignores the advertised capabilities. Clients MUST let the
  user control each direction. Neither endpoint offers content its platform marks as
  concealed or transient ([Section 10.2](10-clipboard.md#102-clipboard_offer-0x48)).

## 11.5 Untrusted media

Video, audio and cursor images come from the host and are untrusted input to the
client's decoders. Clients SHOULD use well-maintained decoders and SHOULD isolate
decoding from the rest of the client where the platform allows, for example in a
separate process. Clipboard data is untrusted in both directions: receivers SHOULD
sanitize HTML and isolate image decoding. Hosts treat microphone audio, HID
descriptors and HID reports from the client in the same way.

## 11.6 HID passthrough

Passthrough ([Section 8.2.5](08-input.md#825-passthrough-devices)) exposes the host's
HID stack, and through matching vendor and product IDs its device drivers, to data
chosen by the client, and exposes the client's physical devices to output and feature
reports chosen by host software. Besides the rules of Section 8.2.5:

- On a host that serves several users, a virtual device that is not confined to the
  session's user or seat would deliver one user's input to another user's session.
  This is why hosts refuse passthrough where they cannot confine devices.
- A passthrough device whose descriptor declares keyboard, mouse or system controls
  would bypass the input classes and policy the host applies to standard devices.
  This is why such collections are refused by default.
- Output and feature reports can reconfigure, and on some devices reflash, the
  physical device. Clients SHOULD refuse to pass through devices that are in a
  firmware update mode, and MAY deny feature operations or output reports that their
  policy does not allow.

## 11.7 Resource exhaustion

Endpoints MUST enforce the limits this specification defines, with the behaviour
each section specifies. Dropping the oldest incomplete frame
([Section 6.6.3](06-video.md#663-reassembly)) is ordinary loss handling, not an
error. The limits include:

- control messages of at most 1 MiB;
- at most 64 capabilities of at most 4096 bytes each
  ([Section 4.6.1](04-session.md#461-format));
- frame payloads of at most 64 MiB, at most 65535 fragments per frame, and a bounded
  number of incomplete frames per stream;
- report descriptors of at most 16384 bytes, passthrough input reports of at most
  1024 bytes, and as many attached devices as the host accepts, at least 16;
- at most 128 cursor images of at most 512 × 512 pixels;
- clipboard data no larger than the receiver's advertised Max Size.

Before authentication succeeds, every control message the host receives, including
`CLIENT_HELLO`, counts against one budget of 64 KiB. A host that reads a Message
Length larger than the remaining budget MUST close the connection with
`QDP_MESSAGE_TOO_LARGE` without reading the payload. Hosts MUST limit the number of
unauthenticated connections, per source address and in total, and SHOULD use QUIC
Retry to validate client addresses when under load. The capabilities in
`HOST_HELLO` are visible to unauthenticated clients, so hosts should not put
anything sensitive in them.

Implementations SHOULD also bound the number of open unidirectional streams and the
memory buffered on them.

## 11.8 Traffic analysis

QUIC encrypts all QDP data, but packet sizes and timing remain visible. An observer
can tell when the user is typing from the pattern of small input datagrams, and can
estimate screen activity from video volume. Endpoints MAY reduce this leakage by
sending input datagrams at a constant rate while a session is focused, at some cost
in bandwidth.
