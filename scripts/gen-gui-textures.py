#!/usr/bin/env python3
"""Generate assets/textures/gui/panel.png — the inventory's 9-slice UI sheet.

The UI is drawn from one texture so the whole panel costs a single egui
`TextureId` and one bind. Every region on the sheet is a 9-slice: the corners
are drawn at their authored size and only the edges and centre stretch, which
is what lets one 48x48 frame back a panel of any size without the border
thickness changing with it.

Committed as a generator rather than hand-painted art because the whole sheet
is flat fills and bevels — it is *defined* by the palette below, and a reviewer
should be able to see that definition rather than diff a binary. Re-run it and
commit the PNG when the palette changes:

    python3 scripts/gen-gui-textures.py

Region offsets and insets are mirrored in `src/ui/ninepatch.rs`; the two must
agree, so change them together.

Standard library only (zlib + struct), matching scripts/third-party-notices.py.
"""

import struct
import zlib
from pathlib import Path

SHEET = 128
OUT = Path(__file__).resolve().parent.parent / "assets" / "textures" / "gui" / "panel.png"

# The sketch's palette: a dark brown carcass, a mid-brown bed behind the grid,
# and light grey cells.
PANEL_FILL = (58, 46, 36, 255)
PANEL_LIGHT = (82, 66, 52, 255)
PANEL_DARK = (38, 29, 22, 255)

GRID_FILL = (122, 85, 47, 255)
GRID_LIGHT = (150, 107, 62, 255)
GRID_DARK = (92, 62, 33, 255)

SLOT_FILL = (226, 226, 226, 255)
SLOT_SHADOW = (168, 168, 168, 255)
SLOT_LIGHT = (245, 245, 245, 255)

SELECT_BORDER = (255, 255, 255, 255)

TIP_FILL = (26, 22, 20, 240)
TIP_BORDER = (150, 107, 62, 255)


class Image:
    def __init__(self, size):
        self.size = size
        self.px = [(0, 0, 0, 0)] * (size * size)

    def put(self, x, y, color):
        if 0 <= x < self.size and 0 <= y < self.size:
            self.px[y * self.size + x] = color

    def rect(self, x, y, w, h, color):
        for j in range(y, y + h):
            for i in range(x, x + w):
                self.put(i, j, color)

    def to_rgba(self):
        return b"".join(bytes(p) for p in self.px)


def bevelled(img, x, y, size, fill, light, dark, edge=None):
    """A raised panel: flat fill, a lit top/left and a shaded bottom/right.

    Both bevels are two pixels, so a 9-slice inset of 8 or more keeps them
    whole in the corners and never stretches them.
    """
    img.rect(x, y, size, size, fill)
    outer = edge if edge is not None else dark
    # One-pixel outline, so adjacent regions on the sheet read as separate.
    img.rect(x, y, size, 1, outer)
    img.rect(x, y + size - 1, size, 1, outer)
    img.rect(x, y, 1, size, outer)
    img.rect(x + size - 1, y, 1, size, outer)
    # Two-pixel bevel inside it.
    for d in (1, 2):
        img.rect(x + d, y + d, size - 2 * d, 1, light)
        img.rect(x + d, y + d, 1, size - 2 * d, light)
        img.rect(x + d, y + size - 1 - d, size - 2 * d, 1, dark)
        img.rect(x + size - 1 - d, y + d, 1, size - 2 * d, dark)


def sunken(img, x, y, size, fill, shadow, light):
    """A recessed cell: the bevel runs the other way, so it reads as a hole."""
    img.rect(x, y, size, size, fill)
    for d in (0, 1):
        img.rect(x + d, y + d, size - 2 * d, 1, shadow)
        img.rect(x + d, y + d, 1, size - 2 * d, shadow)
        img.rect(x + d, y + size - 1 - d, size - 2 * d, 1, light)
        img.rect(x + size - 1 - d, y + d, 1, size - 2 * d, light)


def write_png(path, size, rgba):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(
        b"\x00" + rgba[row * size * 4 : (row + 1) * size * 4] for row in range(size)
    )
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def main():
    img = Image(SHEET)

    # Panel frame, (0,0) 48x48, inset 16 — the dark carcass everything sits in.
    bevelled(img, 0, 0, 48, PANEL_FILL, PANEL_LIGHT, PANEL_DARK)

    # Grid bed, (48,0) 48x48, inset 16 — the mid-brown behind the slots.
    bevelled(img, 48, 0, 48, GRID_FILL, GRID_LIGHT, GRID_DARK)

    # Slot, (0,48) 24x24, inset 8.
    sunken(img, 0, 48, 24, SLOT_FILL, SLOT_SHADOW, SLOT_LIGHT)

    # Selected slot, (24,48) 24x24, inset 8 — same cell, white surround.
    sunken(img, 24, 48, 24, SLOT_FILL, SLOT_SHADOW, SLOT_LIGHT)
    img.rect(24, 48, 24, 2, SELECT_BORDER)
    img.rect(24, 48 + 22, 24, 2, SELECT_BORDER)
    img.rect(24, 48, 2, 24, SELECT_BORDER)
    img.rect(24 + 22, 48, 2, 24, SELECT_BORDER)

    # Tooltip, (48,48) 32x32, inset 12.
    bevelled(img, 48, 48, 32, TIP_FILL, TIP_BORDER, TIP_BORDER, edge=TIP_BORDER)

    write_png(OUT, SHEET, img.to_rgba())
    print(f"wrote {OUT.relative_to(Path.cwd())} ({SHEET}x{SHEET})")


if __name__ == "__main__":
    main()
