#!/usr/bin/env python3
"""Draws the ntls application icon and writes every form the systems want.

One drawing, scaled: the sonar arcs the Ping tool is drawn with, on the same
blue the interface uses for anything that is running. It is drawn rather than
traced from an SVG so that a check-out needs nothing installed to rebuild it —
Pillow and the standard library are the whole of it.

    python3 packaging/icon/make-icons.py

Writes packaging/icon/png/*.png, ntls.ico, ntls.iconset/ and, where `iconutil`
exists, ntls.icns.
"""

import math
import os
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent

# The interface's own accent, and a slightly deeper shade under it so the tile
# has somewhere to sit at small sizes.
TOP = (94, 162, 255, 255)
BOTTOM = (11, 107, 203, 255)
MARK = (255, 255, 255, 255)

# Drawn eight times the largest size it is shown at and scaled down, which is
# what keeps the arcs smooth at 16 pixels.
CANVAS = 2048


def rounded_mask(size: int, radius: float) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return mask


def tile(size: int) -> Image.Image:
    """The rounded square, with the gradient the mark sits on."""
    gradient = Image.new("RGBA", (1, size))
    for y in range(size):
        t = y / max(size - 1, 1)
        gradient.putpixel(
            (0, y),
            tuple(round(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(4)),
        )
    body = gradient.resize((size, size))
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(body, (0, 0), rounded_mask(size, size * 0.2237))
    return out


def arc(draw: ImageDraw.ImageDraw, box, start, end, width):
    draw.arc(box, start=start, end=end, fill=MARK, width=width)


def mark(image: Image.Image) -> None:
    """The sonar: a dot, and two pairs of arcs opening left and right."""
    size = image.size[0]
    d = ImageDraw.Draw(image)
    centre = size / 2
    unit = size / 16.0
    width = round(unit * 1.15)

    d.ellipse(
        [centre - unit * 1.15, centre - unit * 1.15, centre + unit * 1.15, centre + unit * 1.15],
        fill=MARK,
    )
    for radius, spread in ((3.2, 52), (5.5, 46)):
        r = unit * radius
        box = [centre - r, centre - r, centre + r, centre + r]
        arc(d, box, 180 - spread, 180 + spread, width)
        arc(d, box, -spread, spread, width)


def render(size: int) -> Image.Image:
    big = tile(CANVAS)
    mark(big)
    return big.resize((size, size), Image.LANCZOS)


def main() -> int:
    png_dir = HERE / "png"
    png_dir.mkdir(parents=True, exist_ok=True)

    sizes = [16, 32, 48, 64, 128, 256, 512, 1024]
    images = {size: render(size) for size in sizes}
    for size, image in images.items():
        image.save(png_dir / f"ntls-{size}.png")
    images[512].save(HERE / "ntls.png")

    # Windows wants them in one file, smallest first.
    images[256].save(
        HERE / "ntls.ico",
        format="ICO",
        sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)],
    )

    # macOS wants a directory of exact names, which `iconutil` turns into an
    # .icns. Everything else can be built anywhere; this last step only works
    # on a Mac, so a check-out elsewhere keeps the .icns that is committed.
    iconset = HERE / "ntls.iconset"
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir()
    for size in (16, 32, 128, 256, 512):
        render(size).save(iconset / f"icon_{size}x{size}.png")
        render(size * 2).save(iconset / f"icon_{size}x{size}@2x.png")

    if shutil.which("iconutil"):
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(HERE / "ntls.icns")],
            check=True,
        )
    else:
        print("iconutil is a Mac program; keeping the .icns that is already here", file=sys.stderr)

    print(f"wrote {png_dir}, ntls.ico and ntls.icns")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
