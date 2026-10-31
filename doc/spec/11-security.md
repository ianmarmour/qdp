# 11. Security considerations

A QDP session gives the client control of the host's keyboard and pointer and a
view of everything on its screen. Security failures are therefore severe, and
this section is normative where it uses requirement keywords.

## 11.1 Host authentication

TLS authenticates the host in every session. A client MUST verify the host's
certificate in one of these ways:

- **Pinned key**: the SHA-256 hash of the host certificate's SubjectPublicKeyInfo
  matches a value the client obtained in advance, for example from a connection
  broker or an administrator. Clients MUST support this method.
- **Public key infrastructure**: the certificate chains to a trusted certificate
  authority and is valid for the host's name. Clients MAY support this method.

A client MAY offer trust on first use: showing the key's fingerprint to the user
on the first connection and pinning it if the user accepts. It MUST then warn
loudly, and refuse to connect by default, if the key later changes.

## 11.2 Client authentication

- No QDP data other than the hello and authentication messages may be processed
  before authentication succeeds ([Section 4](04-session.md)). In particular a
  host MUST NOT send video, audio, cursor or clipboard data, and MUST NOT inject
  input, before then.
- SCRAM-SHA-256-PLUS binds the password exchange to the TLS connection with
  `tls-exporter` [RFC9266]. A host that relays the exchange to another host
  cannot use it there, and the password itself never crosses the network.
- Tokens are bearer credentials: anyone holding one can use it. Brokers SHOULD
  issue tokens that expire within minutes and are valid for one session.
- Hosts MUST limit failed attempts per connection
  ([Section 4.4.1](04-session.md#441-messages)) and SHOULD limit them per source
  address and per user across connections.
- Hosts SHOULD send the same `Reason` for every kind of failure, so that
  responses do not reveal which user names exist.

## 11.3 Replay

0-RTT data is forbidden ([Section 3.4](03-transport.md#34-early-data)) because an
attacker who replays it could repeat keystrokes or clicks. All other QDP data is
protected against replay by QUIC.

## 11.4 Local control and privacy

- A host SHOULD tell its local user, where one may be present, that a remote
  session is active, for example with a visible indicator.
- The client MUST always offer a local way to release keyboard and pointer
  capture ([Section 8.7](08-input.md#87-pointer_mode-0x33)), so that a host cannot
  trap the user's input.
- Clipboard sharing can move sensitive data between machines. Hosts MUST let
  administrators disable it in either direction, and MUST enforce that policy
  even if a client ignores the advertised capabilities.

## 11.5 Untrusted media

Video, audio and cursor images come from the host and are untrusted input to the
client's decoders. Clients SHOULD use well-maintained decoders and SHOULD
isolate decoding from the rest of the client where the platform allows, for
example in a separate process. Hosts treat microphone audio and clipboard data
from the client the same way.

## 11.6 Resource exhaustion

Endpoints MUST enforce the limits this specification defines and MUST close the
connection with `QDP_RESOURCE_LIMIT` or `QDP_MESSAGE_TOO_LARGE` when a peer
exceeds them. The limits include:

- control messages of at most 1 MiB, and 64 KiB of pipelined messages before
  authentication;
- at most 64 capabilities of at most 4096 bytes each;
- frame payloads of at most 64 MiB and at most 65535 fragments per frame, with a
  bounded number of incomplete frames per stream;
- cursor images of at most 512 × 512 pixels;
- clipboard data no larger than the receiver's advertised Max Size.

Implementations SHOULD also bound the number of open unidirectional streams and
the memory buffered on them.

## 11.7 Traffic analysis

QUIC encrypts all QDP data, but packet sizes and timing remain visible. An
observer can tell when the user is typing from the pattern of small input
datagrams, and can estimate screen activity from video volume. Endpoints MAY
reduce this leakage by sending input datagrams at a constant rate while a session
is focused, at some cost in bandwidth.
