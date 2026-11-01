#!/usr/bin/env python3
"""Renders every app's icon from the Orbit Q in qdp-orbit-q.svg.

    python3 branding/generate-icons.py

The mark, its colors and its place on the square all come from the SVG, so
changing it and running this again changes every icon. Each platform gets
the square fitted to its own shape:

- iPhone and iPad: the full square, which the system rounds, and the mark
  alone for the dark and tinted home screens.
- Mac: the square drawn as the rounded tile macOS icons sit on.
- Android: an adaptive icon, its mark inside the safe zone every launcher's
  mask keeps, which also serves as the themed icon.
- Windows: a rounded tile, in an icon file with each size Windows shows.

Needs Pillow.
"""

import json
import math
import re
import struct
import xml.etree.ElementTree as ElementTree
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

BRANDING = Path(__file__).resolve().parent
ROOT = BRANDING.parent
SOURCE = BRANDING / "qdp-orbit-q.svg"
SVG = "{http://www.w3.org/2000/svg}"

# Each output pixel's coverage is measured on a grid of samples this many
# across, enough for smooth edges at every size, within the widest canvas.
MIN_SAMPLES = 4
MAX_SAMPLES = 16
MAX_CANVAS = 8192
# The length, in samples, of the straight pieces curves are drawn with.
CURVE_STEP = 4.0

# Apple's continuous corner, which its icons' tiles have: the curve from the
# top edge to the right edge as offsets from the corner, in radii, each
# point measured (left of the corner, below it).
CORNER_START = (1.52866483, 0.0)
CORNER_CURVES = (
    ((1.08849296, 0.0), (0.86840694, 0.0), (0.63149399, 0.07491100)),
    ((0.37282392, 0.16905899), (0.16905899, 0.37282392), (0.07491100, 0.63149399)),
    ((0.0, 0.86840694), (0.0, 1.08849296), (0.0, 1.52866483)),
)

# iPhone and iPad: one square image the system rounds and scales.
IOS_SIZE = 1024
IOS_ICONS = BRANDING.parent / "apple/iOS/Assets.xcassets/AppIcon.appiconset"
# The tinted icon's mark, which the system tints by its brightness.
TINT_COLOR = "#FFFFFF"

# The Mac: Apple's icon template, an 824-point tile with a 185.4-point
# continuous corner, 100 points in from each side of a 1024-point icon.
MAC_TEMPLATE = 1024.0
MAC_TILE_INSET = 100.0
MAC_TILE_RADIUS = 185.4
MAC_ICONS = BRANDING.parent / "apple/macOS/Assets.xcassets/AppIcon.appiconset"
# The sizes, in points, and scales of a Mac app icon.
MAC_SIZES = (16, 32, 128, 256, 512)
MAC_SCALES = (1, 2)

# Android: an adaptive icon's 108dp layers, of which every launcher's mask
# keeps the circle 66dp across in the middle.
ANDROID_CANVAS_DP = 108.0
ANDROID_SAFE_RADIUS_DP = 33.0
ANDROID_FOREGROUND = ROOT / "android/app/src/main/res/drawable/ic_launcher_foreground.xml"

# Linux: freedesktop's hicolor theme, which GNOME, KDE Plasma and the other
# desktops read app icons from, the icon named by the app's ID: a scalable
# one, and pictures at the sizes menus and docks ask for. The tile fills
# 112 of 128 units with a 16-unit corner, as GNOME's icon guidelines draw.
LINUX_ICONS = ROOT / "crates/qdp-gtk/data/icons/hicolor"
LINUX_ICON_NAME = "io.github.ianmarmour.qdp"
LINUX_CANVAS = 128.0
LINUX_INSET = 8.0
LINUX_RADIUS = 16.0
LINUX_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)

# Windows: the sizes the shell and the window frames show an icon at, and
# the tile's margin and corner as parts of the icon and of the tile.
WINDOWS_SIZES = (16, 20, 24, 32, 40, 48, 64, 256)
WINDOWS_INSET = 1 / 32
WINDOWS_RADIUS = 0.1875
WINDOWS_ASSETS = ROOT / "windows/QDP/Assets"
# The image the app shows in its title bar and settings.
WINDOWS_PICTURE_SIZE = 256
# An icon file's header: reserved, the icon type, and the image count.
ICO_HEADER = "<HHH"
ICO_TYPE = 1
# An image's directory entry: width, height, palette size, reserved,
# planes, bits per pixel, data size and data offset.
ICO_ENTRY = "<BBBBHHII"
ICO_BITS = 32
# An icon file writes 256 as 0, its width and height being a byte each.
ICO_LARGEST = 256

Point = tuple[float, float]


@dataclass(frozen=True)
class Segment:
    """A piece of an outline: a straight line, or a cubic Bézier curve
    through two control points, to `end`."""

    end: Point
    controls: tuple[Point, Point] | None = None


@dataclass(frozen=True)
class Outline:
    """A closed outline: its start and the pieces that go round it."""

    start: Point
    segments: tuple[Segment, ...]

    def mapped(self, scale: float, left: float, top: float) -> "Outline":
        """Returns the outline scaled by `scale`, then moved by `left`,
        `top`."""

        def move(point: Point) -> Point:
            return (left + point[0] * scale, top + point[1] * scale)

        return Outline(
            move(self.start),
            tuple(
                Segment(move(segment.end), segment.controls and (move(segment.controls[0]), move(segment.controls[1])))
                for segment in self.segments
            ),
        )


@dataclass(frozen=True)
class Mark:
    """The Orbit Q as the source draws it: on a square of `side` units of
    `background`, outlines filled with `color`, by the even-odd rule."""

    side: float
    background: str
    color: str
    path: str
    outlines: tuple[Outline, ...]

    def placed(self, left: float, top: float, side: float) -> list[Outline]:
        """Returns the mark's outlines with its square at `left`, `top`,
        `side` across, in pixels."""
        return [outline.mapped(side / self.side, left, top) for outline in self.outlines]


def read_mark() -> Mark:
    """Reads the mark, its colors and its square from the source."""
    root = ElementTree.parse(SOURCE).getroot()
    _, _, width, height = (float(value) for value in root.attrib["viewBox"].split())
    if width != height:
        raise ValueError("the source's view box must be square")
    background = root.find(f"{SVG}rect[@id='background']")
    mark = root.find(f"{SVG}path[@id='mark']")
    if background is None or mark is None:
        raise ValueError("the source needs a rect 'background' and a path 'mark'")
    if mark.attrib.get("fill-rule") != "evenodd":
        raise ValueError("the mark must be filled by the even-odd rule")
    path = mark.attrib["d"]
    return Mark(width, background.attrib["fill"], mark.attrib["fill"], path, parse_path(path))


def parse_path(path: str) -> tuple[Outline, ...]:
    """Parses the absolute moves, lines and cubic curves of SVG path data,
    the commands the mark is drawn with."""
    tokens = re.findall(r"[A-Za-z]|-?\d*\.?\d+(?:[eE][-+]?\d+)?", path)
    outlines: list[Outline] = []
    start: Point | None = None
    segments: list[Segment] = []
    current: Point = (0.0, 0.0)
    position = 0

    def numbers(count: int) -> list[float]:
        nonlocal position
        values = [float(token) for token in tokens[position : position + count]]
        position += count
        return values

    def finish() -> None:
        nonlocal start, segments
        if start is not None:
            outlines.append(Outline(start, tuple(segments)))
        start, segments = None, []

    while position < len(tokens):
        command = tokens[position]
        position += 1
        if command == "M":
            finish()
            x, y = numbers(2)
            start = current = (x, y)
        elif command == "L":
            x, y = numbers(2)
            current = (x, y)
            segments.append(Segment(current))
        elif command == "H":
            (x,) = numbers(1)
            current = (x, current[1])
            segments.append(Segment(current))
        elif command == "V":
            (y,) = numbers(1)
            current = (current[0], y)
            segments.append(Segment(current))
        elif command == "C":
            x1, y1, x2, y2, x, y = numbers(6)
            current = (x, y)
            segments.append(Segment(current, ((x1, y1), (x2, y2))))
        elif command == "Z":
            finish()
        else:
            raise ValueError(f"the mark's path uses {command!r}, which this script does not draw")
    finish()
    return tuple(outlines)


def square(left: float, top: float, side: float) -> Outline:
    """Returns a square's outline."""
    right, bottom = left + side, top + side
    return Outline((left, top), (Segment((right, top)), Segment((right, bottom)), Segment((left, bottom))))


def tile(left: float, top: float, side: float, radius: float) -> Outline:
    """Returns a square's outline with Apple's continuous corners of
    `radius`, going clockwise from the top edge."""
    right, bottom = left + side, top + side
    # How each corner places an offset (left of the corner, below it) of
    # the top right corner's curve, turned to fit it.
    corners = (
        lambda a, b: (right - a * radius, top + b * radius),
        lambda a, b: (right - b * radius, bottom - a * radius),
        lambda a, b: (left + a * radius, bottom - b * radius),
        lambda a, b: (left + b * radius, top + a * radius),
    )
    segments: list[Segment] = []
    for corner in corners:
        segments.append(Segment(corner(*CORNER_START)))
        for first, second, end in CORNER_CURVES:
            segments.append(Segment(corner(*end), (corner(*first), corner(*second))))
    return Outline(segments[-1].end, tuple(segments))


def flatten(outline: Outline, samples: int) -> list[Point]:
    """Returns the outline as a polygon on the sample grid, whose sample
    centers are half a sample in from its cells' corners."""

    def grid(point: Point) -> Point:
        return (point[0] * samples - 0.5, point[1] * samples - 0.5)

    points = [grid(outline.start)]
    for segment in outline.segments:
        end = grid(segment.end)
        if segment.controls is None:
            points.append(end)
            continue
        begin = points[-1]
        first, second = grid(segment.controls[0]), grid(segment.controls[1])
        hull = math.dist(begin, first) + math.dist(first, second) + math.dist(second, end)
        steps = max(2, math.ceil(hull / CURVE_STEP))
        for step in range(1, steps + 1):
            t = step / steps
            u = 1 - t
            points.append(
                tuple(
                    u * u * u * begin[axis] + 3 * u * u * t * first[axis] + 3 * u * t * t * second[axis] + t * t * t * end[axis]
                    for axis in range(2)
                )
            )
    return points


def coverage(outlines: list[Outline], size: int) -> Image.Image:
    """Returns how much of each pixel of a `size`-pixel square the outlines
    cover, by the even-odd rule, from 0 to 255."""
    samples = max(MIN_SAMPLES, min(MAX_SAMPLES, MAX_CANVAS // size))
    canvas = (size * samples, size * samples)
    filled = Image.new("1", canvas, 0)
    for outline in outlines:
        layer = Image.new("1", canvas, 0)
        ImageDraw.Draw(layer).polygon(flatten(outline, samples), fill=1)
        filled = ImageChops.logical_xor(filled, layer)
    return filled.convert("L").resize((size, size), Image.Resampling.BOX)


def picture(size: int, layers: list[tuple[list[Outline], str]]) -> Image.Image:
    """Draws each layer's outlines in its color, in order, on a transparent
    `size`-pixel square."""
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    for outlines, color in layers:
        paint = Image.new("RGBA", (size, size), color)
        paint.putalpha(coverage(outlines, size))
        image = Image.alpha_composite(image, paint)
    return image


def save_png(image: Image.Image, path: Path) -> None:
    """Saves `image` at `path`, making its folder."""
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, optimize=True)


def write_json(value: object, path: Path) -> None:
    """Writes `value` at `path` as Xcode formats asset catalogs."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def catalog_info() -> dict[str, object]:
    """Returns an asset catalog entry's info."""
    return {"author": "xcode", "version": 1}


def write_catalog(icons: Path, images: list[dict[str, object]]) -> None:
    """Writes the contents of the icon set at `icons` and of the asset
    catalog that holds it."""
    write_json({"images": images, "info": catalog_info()}, icons / "Contents.json")
    write_json({"info": catalog_info()}, icons.parent / "Contents.json")


def ios_icons(mark: Mark) -> None:
    """Writes the iPhone and iPad icons: the opaque square, and the mark
    alone for the dark and tinted home screens, which give it their own
    background."""
    whole = mark.placed(0, 0, IOS_SIZE)
    light = picture(IOS_SIZE, [([square(0, 0, IOS_SIZE)], mark.background), (whole, mark.color)])
    # The App Store refuses an iPhone icon with an alpha channel.
    save_png(light.convert("RGB"), IOS_ICONS / "AppIcon.png")
    save_png(picture(IOS_SIZE, [(whole, mark.color)]), IOS_ICONS / "AppIcon-Dark.png")
    save_png(picture(IOS_SIZE, [(whole, TINT_COLOR)]), IOS_ICONS / "AppIcon-Tinted.png")
    image = {"idiom": "universal", "platform": "ios", "size": f"{IOS_SIZE}x{IOS_SIZE}"}
    write_catalog(
        IOS_ICONS,
        [
            {"filename": "AppIcon.png", **image},
            {
                "appearances": [{"appearance": "luminosity", "value": "dark"}],
                "filename": "AppIcon-Dark.png",
                **image,
            },
            {
                "appearances": [{"appearance": "luminosity", "value": "tinted"}],
                "filename": "AppIcon-Tinted.png",
                **image,
            },
        ],
    )


def mac_icons(mark: Mark) -> None:
    """Writes the Mac icon at each size and scale, each drawn for its
    pixels: the square as macOS's rounded tile."""
    images = []
    for points in MAC_SIZES:
        for scale in MAC_SCALES:
            pixels = points * scale
            unit = pixels / MAC_TEMPLATE
            inset, side = MAC_TILE_INSET * unit, (MAC_TEMPLATE - 2 * MAC_TILE_INSET) * unit
            image = picture(
                pixels,
                [
                    ([tile(inset, inset, side, MAC_TILE_RADIUS * unit)], mark.background),
                    (mark.placed(inset, inset, side), mark.color),
                ],
            )
            name = f"AppIcon-{points}@{scale}x.png"
            save_png(image, MAC_ICONS / name)
            images.append({"filename": name, "idiom": "mac", "scale": f"{scale}x", "size": f"{points}x{points}"})
    write_catalog(MAC_ICONS, images)


def android_foreground(mark: Mark) -> None:
    """Writes the adaptive icon's foreground: the mark as a vector, its
    square scaled so that its farthest point from the middle lies on the
    safe zone's edge."""
    middle = mark.side / 2
    farthest = max(
        math.dist((middle, middle), point) for outline in mark.outlines for point in flatten(outline, 1)
    )
    # The viewport is the 108dp layer in the source's units, the source's
    # square in its middle.
    viewport = ANDROID_CANVAS_DP * farthest / ANDROID_SAFE_RADIUS_DP
    offset = (viewport - mark.side) / 2
    color = "#FF" + mark.color.lstrip("#").upper()
    ANDROID_FOREGROUND.write_text(
        f"""<?xml version="1.0" encoding="utf-8"?>
<!--
The launcher icon's mark, the Orbit Q of branding/qdp-orbit-q.svg, inside
the adaptive icon's 66dp safe zone. Being one color, it also serves as the
monochrome layer of themed icons. Written by branding/generate-icons.py.
-->
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="{ANDROID_CANVAS_DP:g}dp"
    android:height="{ANDROID_CANVAS_DP:g}dp"
    android:viewportWidth="{viewport:.1f}"
    android:viewportHeight="{viewport:.1f}">
    <group
        android:translateX="{offset:.1f}"
        android:translateY="{offset:.1f}">
        <path
            android:fillColor="{color}"
            android:fillType="evenOdd"
            android:pathData="{mark.path}" />
    </group>
</vector>
"""
    )


def windows_icon(mark: Mark, size: int) -> Image.Image:
    """Draws the Windows icon at `size` pixels: the square as a rounded
    tile."""
    inset = size * WINDOWS_INSET
    side = size - 2 * inset
    return picture(
        size,
        [
            ([tile(inset, inset, side, side * WINDOWS_RADIUS)], mark.background),
            (mark.placed(inset, inset, side), mark.color),
        ],
    )


def windows_icons(mark: Mark) -> None:
    """Writes the Windows icon file, an image drawn for each size, and the
    picture the app shows."""
    images = []
    for size in WINDOWS_SIZES:
        data = BytesIO()
        windows_icon(mark, size).save(data, "PNG", optimize=True)
        images.append((size, data.getvalue()))
    offset = struct.calcsize(ICO_HEADER) + len(images) * struct.calcsize(ICO_ENTRY)
    icon = bytearray(struct.pack(ICO_HEADER, 0, ICO_TYPE, len(images)))
    for size, data in images:
        side = 0 if size == ICO_LARGEST else size
        icon += struct.pack(ICO_ENTRY, side, side, 0, 0, 1, ICO_BITS, len(data), offset)
        offset += len(data)
    for _, data in images:
        icon += data
    (WINDOWS_ASSETS / "qdp.ico").write_bytes(bytes(icon))
    save_png(windows_icon(mark, WINDOWS_PICTURE_SIZE), WINDOWS_ASSETS / "qdp.png")


def linux_icon(mark: Mark, size: int) -> Image.Image:
    """Draws the Linux icon at `size` pixels: the square as a rounded tile
    within the canvas's margin."""
    scale = size / LINUX_CANVAS
    inset = LINUX_INSET * scale
    side = size - 2 * inset
    return picture(
        size,
        [
            ([tile(inset, inset, side, LINUX_RADIUS * scale)], mark.background),
            (mark.placed(inset, inset, side), mark.color),
        ],
    )


def linux_icons(mark: Mark) -> None:
    """Writes the Linux app icon to the hicolor theme: scalable, and a
    picture at each size."""
    side = LINUX_CANVAS - 2 * LINUX_INSET
    scalable = LINUX_ICONS / "scalable/apps" / f"{LINUX_ICON_NAME}.svg"
    scalable.parent.mkdir(parents=True, exist_ok=True)
    scalable.write_text(
        f"""<?xml version="1.0" encoding="UTF-8"?>
<!-- QDP's app icon, the Orbit Q of branding/qdp-orbit-q.svg on a rounded
     tile. Written by branding/generate-icons.py. -->
<svg xmlns="http://www.w3.org/2000/svg" width="{LINUX_CANVAS:g}" height="{LINUX_CANVAS:g}" viewBox="0 0 {LINUX_CANVAS:g} {LINUX_CANVAS:g}">
  <rect x="{LINUX_INSET:g}" y="{LINUX_INSET:g}" width="{side:g}" height="{side:g}" rx="{LINUX_RADIUS:g}" fill="{mark.background}"/>
  <path transform="translate({LINUX_INSET:g} {LINUX_INSET:g}) scale({side / mark.side:.6f})" fill="{mark.color}" fill-rule="evenodd" d="{mark.path}"/>
</svg>
"""
    )
    for size in LINUX_SIZES:
        path = LINUX_ICONS / f"{size}x{size}/apps" / f"{LINUX_ICON_NAME}.png"
        path.parent.mkdir(parents=True, exist_ok=True)
        save_png(linux_icon(mark, size), path)


def main() -> None:
    mark = read_mark()
    ios_icons(mark)
    mac_icons(mark)
    android_foreground(mark)
    windows_icons(mark)
    linux_icons(mark)


if __name__ == "__main__":
    main()
