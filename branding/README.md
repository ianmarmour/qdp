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
