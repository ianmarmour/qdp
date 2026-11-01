# QDP documentation

This directory holds the design and the specification of the Quick Desktop
Protocol (QDP): a low-latency protocol for remote desktops and game streaming,
intended to replace RDP and VNC on modern networks.

**Status: draft 00.** Anything here may change before version 1 is final.
Implementations of this draft identify themselves with the ALPN token
`qdp-draft-00` ([Section 3.2](spec/03-transport.md#32-alpn)).

## Documents

| Document | Contents |
|---|---|
| [Design rationale](design-rationale.md) | Goals, target conditions, the reasoning and measurements behind each decision, alternatives rejected, open questions. Non-normative. |
| [1. Introduction](spec/01-introduction.md) | Scope, terminology, wire notation, common data types, extensibility rules, references. |
| [2. Architecture](spec/02-architecture.md) | Roles, channels, session lifecycle, latency principles, conformance requirements. |
| [3. Transport](spec/03-transport.md) | How QDP uses QUIC: ALPN, transport parameters, the control stream, unidirectional streams, datagrams, priorities, errors. |
| [4. Session](spec/04-session.md) | Session states, hello messages, authentication, configuration, capabilities. |
| [5. Timing](spec/05-timing.md) | Session clocks, clock synchronization, display timing, presentation, rate control. |
| [6. Display and video](spec/06-video.md) | Monitors, video streams and layers, AV1 requirements, fragmentation, feedback, loss recovery, composition. |
| [7. Audio](spec/07-audio.md) | Audio streams, Opus and PCM, jitter buffering, synchronization, microphone. |
| [8. Input](spec/08-input.md) | Input modelled on USB HID: standard keyboard, pointer, mouse, touchscreen, pen, touchpad, gamepad and text devices, HID passthrough for controllers, pointer modes, output and feature reports. |
| [9. Cursor](spec/09-cursor.md) | Client-drawn cursor, cursor images, warps. |
| [10. Clipboard](spec/10-clipboard.md) | Lazy, announce-then-fetch clipboard sharing. |
| [11. Security](spec/11-security.md) | Authentication, replay, privacy, microphone and clipboard consent, untrusted media, HID passthrough, resource limits. |
| [12. Registries](spec/12-registries.md) | Every code point in one place. |
| [13. Gateways](spec/13-gateway.md) | Reaching hosts in a private network through a gateway: directory and TOTP authentication, a tunnel for the end-to-end QDP connection, and identity assertions hosts can trust. |

## Reading order

- **To understand the design**, read the [design rationale](design-rationale.md),
  then [Architecture](spec/02-architecture.md).
- **To implement QDP**, read chapters 1 to 4 in order, then the chapters for the
  features you need. Every wire format is defined in its chapter, and the
  [registries](spec/12-registries.md) list all code points. Section 2.6 lists
  which features are required.

## Conventions

The specification uses the requirement keywords of RFC 2119 and the wire
notation of QUIC ([RFC 9000](https://www.rfc-editor.org/rfc/rfc9000.html),
Section 1.3), both explained in [Chapter 1](spec/01-introduction.md).
Sections marked *non-normative* give guidance only.
