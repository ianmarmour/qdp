# Using the apps

Every QDP app lists **your computers**, the ones this device paired with, and
**nearby computers** on the local network. Choose a nearby computer, or add
one by its address, and type the four-digit code it shows; from then on, one
tap connects. A computer's menu in the list forgets it.

## Android

- Touch works as a trackpad: one finger moves the pointer, a tap clicks, two
  fingers right-click or scroll, and a press held still drags. The stream's
  menu switches to direct touch.
- Controllers, a handheld's built-in controls, a mouse and a keyboard all work
  as on a computer. A controller also drives the app itself.
- Back, or Select, Start and both bumpers together, opens the stream's menu,
  with the on-screen keyboard, the on-screen controller, statistics and
  Disconnect.

## iPhone and iPad

- Touch works as a trackpad or as the computer's touchscreen, as Settings or
  the stream's menu choose.
- Controllers, and an iPad's keyboard, mouse or trackpad, work as on a
  computer.
- The button in the top corner, or Select, Start and both shoulder buttons
  together, opens the stream's menu, with the on-screen keyboard, the
  on-screen controller, statistics and Disconnect.

## Mac

Each stream opens in its own window, which can go full screen. Click into it
to send your keyboard and mouse. When a game captures the mouse, press
Control, Option, Shift and Z together to release it.

## Windows

The app streams full screen or in a window, as its settings choose. In full
screen, the Windows key, Alt+Tab and Alt+F4 go to the computer. Hold Ctrl, Alt
and Shift, then press:

| Key | Action |
|---|---|
| Z | Release the mouse |
| X | Switch between full screen and a window |
| S | Show or hide statistics |
| M | Turn the microphone on or off |
| Q | Disconnect |

## Linux

Each stream opens in its own window; closing it disconnects. Its preferences
(Ctrl+,) choose the resolution, sound, microphone and clipboard. While
streaming, Ctrl+Alt+Shift+Z releases a captured mouse, and Ctrl+Alt+Shift+M
turns the microphone on or off.

## Through your organization's gateway

1. In the app's settings, enter the gateway's address and your user name, as
   your administrator gives them.
2. Add a computer by its name or ID.
3. The first time, sign in with your password, then scan the QR code the app
   shows with an authenticator app and type its code.

Later connections sign in without asking until your session expires.

## The command line

On Linux, Windows and macOS, `qdp connect` streams in a window, with the same
shortcuts as the Linux app. Pair the first time, then connect by address:

```bash
qdp connect 192.0.2.7 --pair
qdp connect 192.0.2.7
```

`qdp hosts` lists the computers this device paired with, and `qdp help` lists
every option.
