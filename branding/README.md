# Branding

QDP's logo is the Orbit Q: an ivory Q with a short, rounded tail.

| File | What it is |
| --- | --- |
| [`qdp-orbit-q.svg`](qdp-orbit-q.svg) | The Q on its deep teal square: the logo, and the source of every app icon. |
| [`qdp-orbit-q-transparent.svg`](qdp-orbit-q-transparent.svg) | The Q alone, for placing on other backgrounds. |

| Color | Value |
| --- | --- |
| Deep teal | `#025559` |
| Ivory | `#FCF5E3` |

## App icons

[`generate-icons.py`](generate-icons.py) draws every app's icon from
`qdp-orbit-q.svg`, so after changing the logo, run it again (it needs
Pillow) and commit what it writes:

```bash
python3 branding/generate-icons.py
```

| App | Icon | What it is |
| --- | --- | --- |
| iPhone and iPad | `apple/iOS/Assets.xcassets/AppIcon.appiconset` | The square, which the system rounds, and the Q alone for the dark and tinted home screens. |
| Mac | `apple/macOS/Assets.xcassets/AppIcon.appiconset` | The square as the rounded tile of Apple's icon template, at each size. |
| Android | `android/app/src/main/res/drawable/ic_launcher_foreground.xml` | The Q of the adaptive icon, within the safe zone every launcher's mask keeps; it is also the themed icon. Its background is the teal of `values/colors.xml`. |
| Windows | `windows/QDP/Assets/qdp.ico`, `qdp.png` | The square as a rounded tile, drawn for each size Windows shows. |
