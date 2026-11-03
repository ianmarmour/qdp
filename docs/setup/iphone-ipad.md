# Installing QDP on iPhone and iPad

The app runs on iPhone and iPad with iOS or iPadOS 26 or later. On the home
screen it's called **Quick Desktop**.

It isn't in the App Store yet. Install it one of two ways:

- **Build it with Xcode** on a Mac, signed with your own Apple account:
  [`apple`](../../apple/README.md#iphone-and-ipad) says how. Apps signed with
  a free Apple account must be signed again every seven days.
- **Sideload the release's unsigned app**,
  [`qdp-ios-unsigned.ipa`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-ios-unsigned.ipa),
  with a tool that signs apps with your Apple account, such as AltStore or
  Sideloadly.

## Hardware

The app decodes video on the GPU on devices with an AV1 decoder: iPhone 15
Pro and later, and iPads with an M4 or A17 Pro. Other devices decode in
software.

## Next steps

[Connect to a computer](apps.md#iphone-and-ipad) from the app.
