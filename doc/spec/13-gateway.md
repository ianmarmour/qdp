# 13. Gateways

## 13.1 Overview

A gateway lets clients outside a private network, such as an enterprise's, reach
QDP hosts inside it through one public endpoint. The gateway authenticates users
against the organization's directory, with a second factor, before any of their
packets reaches a host, and only forwards them to the hosts its policy lets each
user reach.

The client opens a QUIC connection to the gateway (the outer connection),
authenticates, and chooses a host. It then runs an ordinary QDP connection to the
host (the inner connection) inside the outer one: each UDP payload of the inner
connection travels in one QUIC DATAGRAM frame [RFC9221] of the outer connection,
and the gateway forwards it to the host as one UDP datagram, and the host's
replies back the same way. This is the approach of proxying UDP in HTTP [RFC9298],
without HTTP.

```
 client                        gateway                         host
   |== outer QUIC (qdp-gateway) ==|                              |
   |  authenticate, OPEN host     |                              |
   |  inner QUIC packets in       |   the same packets as UDP    |
   |  DATAGRAM frames  ---------> | ---------------------------> |
   |  <-------------------------  | <--------------------------- |
   |<=========== inner QUIC (qdp-draft-00), end to end ==========>|
```

The inner connection is end to end. The host authenticates itself to the client
by TLS as in every session ([Section 11.1](11-security.md#111-host-authentication)),
and the gateway, which holds none of the inner connection's keys, can neither read
nor change the session; it sees only the sizes and times of its packets. The
gateway gives the client the host's key to pin, and can vouch to the host for the
user with a signed identity assertion ([Section 13.7](#137-identity-assertions)),
which the host accepts in place of its own credentials.

Hosts need nothing to be reached through a gateway: to them, the gateway is a
client address. A host that should admit users on the gateway's word trusts the
gateway's assertion key.

## 13.2 Transport

The outer connection is QUIC version 1 with TLS 1.3, as in
[Section 3.1](03-transport.md#31-quic-and-tls). Gateways listen on UDP port 443
by default, which networks that allow HTTP/3 let through. Deployments MAY use any
port.

**ALPN.** Endpoints identify the gateway protocol with the ALPN token
`qdp-gateway-draft-00` for this draft, and `qdp-gateway/1` for its final version.

**Gateway authentication.** The client MUST verify the gateway's certificate by a
pinned key or by public key infrastructure, as for a host
([Section 11.1](11-security.md#111-host-authentication)). The client sends the
user's directory password to the gateway, so it MUST NOT connect to a gateway
whose key it has not verified this way, and MUST NOT offer trust on first use.

**Client certificate.** The gateway requests a client certificate. The client
SHOULD present its QDP identity's certificate, the same one it presents to hosts:
identity assertions name its key, and a host admits the assertion only on an inner
connection that presents the same key
([Section 13.7](#137-identity-assertions)). A gateway MAY require one.

**Transport parameters and size.** Both endpoints MUST send the
`max_datagram_frame_size` transport parameter with the value 65535, and follow
[Section 3.3](03-transport.md#33-transport-parameters) and
[Section 3.4](03-transport.md#34-early-data-and-session-resumption) otherwise:
no 0-RTT and no TLS session resumption. QUIC needs UDP payloads of at least 1200
bytes [RFC9000], so the outer connection must carry DATAGRAM frames with
1200-byte payloads, which a QUIC packet carries with about 32 bytes of overhead.
Endpoints SHOULD therefore use a maximum UDP payload size of at least 1232 bytes
from the start of the connection, which every IPv6 path carries, and SHOULD run
path MTU discovery [RFC9000] to use larger packets where the path allows. A client
MUST NOT start the inner connection until the outer connection can carry DATAGRAM
frames with 1200-byte payloads, and closes the connection with
`GATEWAY_PATH_TOO_SMALL` if it cannot within 3 seconds of the handshake.

## 13.3 Control stream

The control stream is the first client-initiated bidirectional stream of the outer
connection. The client opens it after the handshake and sends `GATEWAY_HELLO`
first. Messages are framed as in
[Section 3.6.1](03-transport.md#361-framing), with the message types of this
section, and are processed as [Section 3.6.2](03-transport.md#362-processing-rules)
describes; a message of a known type sent in the wrong direction or state is a
connection error of type `GATEWAY_PROTOCOL_VIOLATION`. Before authentication
succeeds, the gateway accepts at most 16 KiB of control messages, and closes the
connection with `GATEWAY_PROTOCOL_VIOLATION` beyond that.

| Type | Name | Direction |
|---|---|---|
| 0x01 | `GATEWAY_HELLO` | Client |
| 0x02 | `GATEWAY_WELCOME` | Gateway |
| 0x03 | `AUTH_START` | Client |
| 0x04 | `AUTH_CONTINUE` | Both |
| 0x05 | `AUTH_RESULT` | Gateway |
| 0x06 | `SESSION_TOKEN` | Gateway |
| 0x07 | `HOSTS_REQUEST` | Client |
| 0x08 | `HOSTS` | Gateway |
| 0x09 | `OPEN` | Client |
| 0x0A | `OPENED` | Gateway |
| 0x0B | `OPEN_FAILED` | Gateway |

```
GATEWAY_HELLO Payload {
  Version (i),
  Client Name (String),
}

GATEWAY_WELCOME Payload {
  Version (i),
  Gateway Name (String),
  Method Count (i),
  Auth Method (i) ...,
}
```

- **Version**: 0 for this draft. The gateway answers with the version it uses, at
  most the client's, or closes the connection with `GATEWAY_VERSION` if it
  supports none.
- **Client Name**: the client device's name for users, at most 64 bytes, which the
  gateway logs.
- **Gateway Name**: the gateway's name for users, such as the organization's, at
  most 64 bytes, which the client shows when asking for credentials.
- **Auth Method**: the methods the client may start an attempt with
  ([Section 13.4](#134-authentication)).

The client MAY send `AUTH_START` right after `GATEWAY_HELLO` with a method it
knows the gateway accepts, such as `TOKEN` with a session token.

## 13.4 Authentication

The authentication messages, their limits and the `NEXT_FACTOR` status for
further factors are those of [Section 4.4.1](04-session.md#441-messages). A
gateway stands at the edge of the network, so it MUST require two factors of
every user: one that identifies the user, such as the directory password, then
`TOTP`, which it MUST NOT let its configuration turn off. Only a session token,
which the user earned with both, stands for them together. As for hosts, the
`Reason` of a failure SHOULD be the same whichever factor failed and whether the
user exists.

The gateway accepts these methods:

- **`PASSWORD` (0x05)**: a directory user name and password, with the Auth Data of
  [Section 4.4.8](04-session.md#448-password-0x05). The gateway verifies them with
  the organization's directory, such as LDAP [RFC4511] or Active Directory, over
  a connection protected by TLS, and SHOULD send the same failure for a wrong
  password, a locked or disabled account and an unknown user. A gateway SHOULD
  stop trying a user name before the directory's own lockout threshold, so that
  attempts through the gateway cannot lock users out of the organization's other
  systems.
- **`TOTP` (0x09)**: a code from the user's authenticator app, as
  [Section 4.4.10](04-session.md#4410-totp-0x09) describes, including its
  enrollment.
- **`TOKEN` (0x02)**: a session token from `SESSION_TOKEN` (below), which stands for
  both factors.
- **`CLIENT_CERTIFICATE` (0x01)**: the key of the client's certificate, which the
  gateway's policy may accept as the first factor, for example for devices the
  organization enrolled; the user still passes `TOTP`.

**Session tokens.** After `AUTH_RESULT` with `SUCCESS`, the gateway MAY send:

```
SESSION_TOKEN Payload {
  Token (Octets),
  Lifetime (i),
}
```

Token is opaque and at most 256 bytes; Lifetime is the number of seconds it
remains valid. The client authenticates later outer connections with `TOKEN` and
this token, so that it can open another host, or reconnect after the network
changed, without asking its user again. The gateway MUST bind the token to the
client certificate's key, if the client presented one, and accept it only on
connections that present the same key; it MAY accept it on several connections
at once, and SHOULD let administrators revoke the tokens of a user. Clients store
session tokens as resumption tokens
([Section 11.2](11-security.md#112-client-authentication)), and discard them when
the user signs out of the gateway.

## 13.5 Choosing a host

Once authenticated, the client MAY send `HOSTS_REQUEST`, with an empty payload, and
the gateway answers with the hosts the user may reach:

```
HOSTS Payload {
  Host Count (i),
  Host Entry (..) ...,
}

Host Entry {
  Host ID (String),
  Name (String),
  Description (String),
}
```

- **Host ID**: the gateway's identifier of the host, at most 255 bytes, which
  `OPEN` can name.
- **Name**: the host's name for users, at most 64 bytes.
- **Description**: more about it, such as its location, at most 255 bytes; empty
  if there is none.

The client opens a host with:

```
OPEN Payload {
  Target (String),
}

OPENED Payload {
  Host Pin (Octets),
  Server Name (String),
  Assertion (Octets),
}

OPEN_FAILED Payload {
  Code (i),
  Reason (String),
}
```

**Target** names the host, in at most 255 bytes, in one of two ways:

- a Host ID from `HOSTS`, for a client that lets its user choose from the
  gateway's list; or
- the host's name or address as a client would reach it inside the private
  network, such as `build-01.corp.example.com` or `10.0.4.20`, with an optional
  `:PORT`, 4433 if absent, and an IPv6 address in brackets. A client configured to
  reach hosts through a gateway sends the name its user gave, so that the same
  names work inside the network and through the gateway.

The gateway looks Target up among its Host IDs first. Otherwise it resolves the
name with the private network's DNS and checks that its policy lets the user
reach both the name and every address it resolves to, on that port, so that a
name resolving to an address the user may not reach is refused. A gateway MUST
NOT open a target its policy does not allow. It then opens the tunnel to the
host's address ([Section 13.6](#136-tunnel)) and answers `OPENED` with:

- **Host Pin**: the 32-byte key pin of the host's certificate, if the gateway knows
  it from its configuration, or empty. When present, the client MUST verify the
  inner connection's host certificate against it, as a pinned key
  ([Section 11.1](11-security.md#111-host-authentication)), and a client that
  already pins another key for the same host, for example from pairing, MUST
  refuse to connect and tell its user. When empty, the client verifies the host
  as it would without a gateway: by a key it pinned for that host, by public key
  infrastructure, or by pairing through the tunnel
  ([Section 4.4.9](04-session.md#449-pairing-0x06)).
- **Server Name**: the name the client sends in the inner TLS handshake, at most
  255 bytes.
- **Assertion**: an identity assertion for this host
  ([Section 13.7](#137-identity-assertions)), or empty. An assertion names the
  host's key, so a gateway issues one only with a Host Pin.

or `OPEN_FAILED`, after which the client may open another host:

| Code | Name | Meaning |
|---|---|---|
| 0x01 | `UNKNOWN_HOST` | The gateway knows no such host, or the user may not reach it. |
| 0x02 | `UNREACHABLE` | The gateway cannot reach the host. |
| 0x03 | `BUSY` | The gateway has reached a limit, such as of tunnels. |

The gateway answers `UNKNOWN_HOST` for a host it does not know, a name that does
not resolve, and a host the user may not reach alike, so that users cannot
discover hosts they may not reach. An outer
connection carries one tunnel: after `OPENED`, a client that sends `OPEN` again
commits a `GATEWAY_PROTOCOL_VIOLATION`. A client opens other hosts on other outer
connections, authenticated with its session token.

## 13.6 Tunnel

After sending `OPENED`, the gateway forwards:

- each DATAGRAM frame's payload from the client, unchanged, as one UDP datagram to
  the host's address, from a UDP socket it uses for this tunnel only; and
- each UDP datagram that socket receives from the host's address, unchanged, as one
  DATAGRAM frame's payload to the client.

The gateway drops DATAGRAM frames that arrive before it has sent `OPENED`, UDP
datagrams from any other address, and datagrams too large for the outer
connection's current path, which the inner connection's own path MTU discovery
then avoids. It MUST NOT forward to any address but the host's, and MUST NOT
otherwise read, change or reorder the datagrams.

The client sends each UDP payload of the inner connection as one DATAGRAM frame,
and treats each DATAGRAM frame it receives as a UDP payload from the host. The
inner connection sees one fixed peer address and does not migrate; the outer
connection migrates instead.

The tunnel lasts as long as the outer connection. When the outer connection closes,
the gateway closes the tunnel's socket. The gateway SHOULD close an outer
connection whose tunnel carried nothing in either direction for 60 seconds with
`GATEWAY_IDLE`; QDP sessions send packets at least every 10 seconds
([Section 3.3](03-transport.md#33-transport-parameters)). When a host session
ends, the client closes the outer connection with `GATEWAY_NO_ERROR`.

## 13.7 Identity assertions

An identity assertion is a statement, signed by the gateway, that a user it
authenticated is connecting to a host. The client presents it to the host as the
token of the `TOKEN` method ([Section 4.4.3](04-session.md#443-token-0x02)), and
a host that trusts the gateway's assertion key admits the user without
credentials of its own.

```
Identity Assertion {
  Magic (32) = 0x51444741,
  Version (i) = 0,
  Key ID (64),
  Host Pin (256),
  Client Pin (256),
  Issued At (i),
  Expires At (i),
  Nonce (128),
  User Name (String),
  Display Name (String),
  Group Count (i),
  Group (String) ...,
  Signature (512),
}
```

- **Magic**: "QDGA" in ASCII, which tells an assertion from other tokens.
- **Key ID**: the first 8 bytes of the SHA-256 hash of the gateway's 32-byte
  Ed25519 public key, which tells a host trusting several keys, as while a gateway
  rotates its key, which one to check.
- **Host Pin**: the key pin of the host the assertion is for.
- **Client Pin**: the key pin of the client certificate the client presented on the
  outer connection.
- **Issued At**, **Expires At**: seconds since the Unix epoch. Gateways SHOULD issue
  assertions that expire within 2 minutes: the client uses it once, when its inner
  connection authenticates, and later reconnections use the host's resumption
  token ([Section 4.4.7](04-session.md#447-resumption_token-0x08)).
- **Nonce**: 16 random bytes.
- **User Name**: the user's name in the directory, such as `alice` or
  `alice@corp.example.com`, at most 255 bytes.
- **Display Name**: the user's full name for people, at most 255 bytes; empty if the
  directory has none.
- **Group**: the names of the directory groups the user belongs to that the gateway
  is configured to report, each at most 255 bytes, at most 64 of them.
- **Signature**: the Ed25519 signature [RFC8032] of the ASCII string
  `QDP gateway assertion`, a zero byte, and every byte of the assertion before
  Signature, with the gateway's assertion key.

The assertion is at most 4096 bytes, the limit of a token. A gateway issues one
only to a client that presented a certificate, so that the assertion names its
key. A host that trusts a gateway's assertion key:

- MUST verify the signature with the key whose Key ID the assertion names, and
  reject an assertion whose Magic, Version or Key ID it does not know;
- MUST reject an assertion whose Host Pin is not its own key's pin, or whose Client
  Pin is not the pin of the certificate the client presented on this connection, so
  that a stolen assertion is useless to anyone without the client's private key;
- MUST reject an assertion outside the times it names, allowing at most 60 seconds
  of clock skew, and SHOULD reject a Nonce it has accepted before, remembering
  nonces until their assertions expire;
- decides from the User Name and Groups whether to admit the user, and as which of
  its accounts, as for `PASSWORD`
  ([Section 4.4.8](04-session.md#448-password-0x05)).

A gateway's assertion key is an Ed25519 key pair of its own, not its TLS key, so
that rotating its certificate does not change what hosts trust, and so that no
signature made for TLS can be mistaken for an assertion.

## 13.8 Errors

These QUIC application error codes close the outer connection. Codes of
[Section 12.6](12-registries.md#126-error-codes) are not used on it.

| Code | Name | Meaning |
|---|---|---|
| 0x00 | `GATEWAY_NO_ERROR` | The client is done, or the host session ended |
| 0x01 | `GATEWAY_INTERNAL_ERROR` | An implementation fault |
| 0x02 | `GATEWAY_PROTOCOL_VIOLATION` | The peer broke a rule of this section |
| 0x03 | `GATEWAY_AUTH_FAILED` | Authentication failed |
| 0x04 | `GATEWAY_VERSION` | No version in common |
| 0x05 | `GATEWAY_IDLE` | The tunnel carried nothing for too long |
| 0x06 | `GATEWAY_PATH_TOO_SMALL` | The path cannot carry 1200-byte datagrams |
| 0x07 | `GATEWAY_POLICY` | The gateway's administrator ended the connection, or revoked the user |
| 0x08 | `GATEWAY_SHUTDOWN` | The gateway is shutting down |

## 13.9 Security considerations

- A gateway holds the keys to an organization's desktops: it sees its users'
  directory passwords and decides who reaches which host. It SHOULD run with the
  least privilege it needs, bind to the directory with an account that can only
  read the users and groups it needs, and keep its TLS key, assertion key and TOTP
  secrets readable only by itself.
- The gateway SHOULD log every authentication, successful or not, and every tunnel
  opened and closed, with the user, the client's address and key pin, the host and
  the amount of data, so that an organization can audit access.
- A tunnel forwards only to the host the gateway chose for it, so a client cannot
  use a gateway to reach other addresses, and the gateway sends a host nothing
  before a user has authenticated. Packets from a host return only through the
  tunnel whose socket received them.
- A gateway cannot read or change inner connections, but can see their traffic
  patterns ([Section 11.8](11-security.md#118-traffic-analysis)), and can refuse
  to forward them. It can also open its own connections to the hosts it reaches,
  as any client on the private network can; hosts still authenticate every
  client.
- An identity assertion is a bearer credential bound to two keys: the host's, so
  that it is useless at other hosts, and the client's, so that it is useless to
  anyone who copies it without the client's private key. A host that trusts a
  gateway's assertion key trusts the gateway to have authenticated the user, so
  administrators trust only their own gateways, and remove a key they no longer
  trust from every host.
- Session tokens, like resumption tokens, let a client reconnect without its user.
  Gateways bind them to the client's key, limit their lifetime, typically to a
  working day, and revoke them when a user's directory account is disabled.
