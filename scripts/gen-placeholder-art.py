#!/usr/bin/env python3
"""Generate placeholder art for the biome/progression content.

Every block, item and mob the ring biomes, underground layers and boss loop
introduce needs *some* art before an artist gets to it — otherwise it renders
as the magenta missing-texture marker. This derives each one from art already
in `assets/` (recoloured stone, dirt, flint, a pickaxe, ...) and writes:

    assets/textures/blocks/<id>.png          256x256 RGBA, plus
    assets/models/blocks/<id>.json           a full cube (`stone.json` re-pointed)
    assets/textures/items/<id>.png           32x32 RGBA sprite, plus
    assets/models/items/<id>.json            an `item/generated` stub, or a copy
                                             of a hand-modelled tool re-pointed
    assets/textures/entity/<id>/<id>.png     64x64 quadruped skin sheet

Re-run it after changing a recipe below and commit the output; replace a file
with real art at any time (and drop its entry here so the script stops
overwriting it):

    python3 scripts/gen-placeholder-art.py

Standard library only (zlib + struct), matching scripts/gen-gui-textures.py.
"""

import json
import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEXTURES = ROOT / "assets" / "textures" / "blocks"
MODELS = ROOT / "assets" / "models" / "blocks"


# ---- PNG I/O ----------------------------------------------------------------


def read_png(path):
    """Decode an 8-bit, non-interlaced RGB/RGBA PNG into (w, h, [[r,g,b,a]])."""
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    pos, idat, palette = 8, b"", []
    width = height = channels = None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8 and interlace == 0, f"{path}: unsupported PNG"
            channels = {2: 3, 3: 1, 6: 4}[color]
        elif kind == b"PLTE":
            palette = [list(body[i : i + 3]) + [255] for i in range(0, len(body), 3)]
        elif kind == b"tRNS":
            for i, alpha in enumerate(body):
                palette[i][3] = alpha
        elif kind == b"IDAT":
            idat += body
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1 : start + 1 + stride])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + a) & 0xFF
            elif kind == 2:
                line[i] = (line[i] + b) & 0xFF
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 0xFF
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        rows.append(line)
        prev = line
    pixels = []
    for line in rows:
        for x in range(width):
            px = list(line[x * channels : (x + 1) * channels])
            if channels == 1:
                pixels.append(list(palette[px[0]]))
            else:
                pixels.append(px + [255] if channels == 3 else px)
    return width, height, pixels


def write_png(path, width, height, pixels):
    raw = b"".join(
        b"\x00" + bytes(v for px in pixels[y * width : (y + 1) * width] for v in px)
        for y in range(height)
    )

    def chunk(kind, body):
        crc = zlib.crc32(kind + body) & 0xFFFFFFFF
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


# ---- colour operations ------------------------------------------------------


def clamp(v):
    return max(0, min(255, int(round(v))))


def luma(px):
    return 0.299 * px[0] + 0.587 * px[1] + 0.114 * px[2]


def tinted(px, tint, brightness=1.0):
    """Greyscale the pixel, then multiply by `tint` (an RGB colour)."""
    l = luma(px) / 255.0 * brightness
    return [clamp(l * c) for c in tint] + [px[3]]


def hash2(x, y, salt):
    h = (x * 374761393 + y * 668265263 + salt * 2147483647) & 0xFFFFFFFF
    h = ((h ^ (h >> 13)) * 1274126177) & 0xFFFFFFFF
    return (h ^ (h >> 16)) / 0xFFFFFFFF


def blotch(x, y, cell, salt):
    """Smooth-ish value noise in 0..1 on a grid of `cell` pixels."""
    gx, gy = x / cell, y / cell
    x0, y0 = int(gx), int(gy)
    fx, fy = gx - x0, gy - y0
    fx, fy = fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)
    a, b = hash2(x0, y0, salt), hash2(x0 + 1, y0, salt)
    c, d = hash2(x0, y0 + 1, salt), hash2(x0 + 1, y0 + 1, salt)
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy


def ore_mask(ore):
    """The coal texture's nuggets: its darkest pixels. Borrowing the coal
    layout gives every new ore the same hand-drawn vein shapes."""
    return [luma(px) < 62 for px in ore]


# ---- recipes ----------------------------------------------------------------


def recolour(src, tint, brightness=1.0):
    return lambda tex: [tinted(px, tint, brightness) for px in tex[src]]


def ore(host, host_tint, host_brightness, nugget):
    def build(tex):
        mask = ore_mask(tex["coal_ore"])
        out = []
        for px, is_ore in zip(tex[host], mask):
            if is_ore:
                l = luma(px) / 255.0
                out.append([clamp(c * (0.55 + 0.6 * l)) for c in nugget] + [255])
            else:
                out.append(tinted(px, host_tint, host_brightness))
        return out

    return build


def mossy(tex):
    size = int(math.sqrt(len(tex["cobblestone"])))
    out = []
    for i, px in enumerate(tex["cobblestone"]):
        x, y = i % size, i // size
        moss = blotch(x, y, 24, 7) * 0.8 + blotch(x, y, 7, 3) * 0.2
        if moss > 0.55 and luma(px) > 70:
            out.append(tinted(px, (96, 150, 64), 1.15))
        else:
            out.append(px)
    return out


def rune_stone(glow, base_tint, base_brightness, pattern_salt):
    """A dark stone face carved with a glowing geometric rune."""

    def build(tex):
        size = int(math.sqrt(len(tex["stone"])))
        centre = size / 2
        out = []
        for i, px in enumerate(tex["stone"]):
            x, y = i % size, i // size
            dx, dy = (x - centre) / size, (y - centre) / size
            r = math.hypot(dx, dy)
            ring = abs(r - 0.32) < 0.018
            spokes = False
            for k in range(3 + pattern_salt % 3):
                ang = math.pi * 2 * k / (3 + pattern_salt % 3) + pattern_salt * 0.3
                px_ = math.cos(ang) * dx + math.sin(ang) * dy
                py_ = -math.sin(ang) * dx + math.cos(ang) * dy
                if abs(py_) < 0.014 and 0.05 < px_ < 0.3:
                    spokes = True
            dot = r < 0.05
            if ring or spokes or dot:
                out.append(list(glow) + [255])
            else:
                edge = x < size * 0.04 or y < size * 0.04 or x > size * 0.96 or y > size * 0.96
                out.append(tinted(px, base_tint, base_brightness * (0.7 if edge else 1.0)))
        return out

    return build


BLOCKS = {
    "snow": recolour("sand", (240, 246, 255), 1.45),
    "deepstone": recolour("stone", (120, 128, 150), 0.75),
    "mud": recolour("dirt", (110, 84, 62), 0.85),
    "packed_snow": recolour("stone", (210, 225, 245), 1.35),
    "basalt": recolour("stone", (80, 76, 78), 0.8),
    "ash": recolour("gravel", (150, 140, 136), 0.95),
    "mossy_cobblestone": mossy,
    "tin_ore": ore("stone", (255, 255, 255), 1.0, (205, 208, 214)),
    "silver_ore": ore("stone", (225, 232, 245), 1.0, (240, 248, 255)),
    "cinder_ore": ore("stone", (80, 76, 78), 0.8, (255, 128, 40)),
    "wayrune": rune_stone((110, 235, 255), (140, 150, 160), 0.7, 3),
    "elder_altar": rune_stone((255, 214, 90), (120, 150, 100), 0.75, 5),
}

SOURCES = ["stone", "dirt", "sand", "gravel", "cobblestone", "coal_ore"]


# ---- crafting stations ------------------------------------------------------
#
# A station has a distinct top, side and (optionally) front, so it is written
# as several textures and a cube whose faces name them, rather than the one
# re-pointed `stone.json` the plain blocks above get.

PLANK = (176, 132, 82)


def planks(size, boards=4, salt=11):
    """Horizontal oak boards with grain and dark seams."""
    out = []
    board = size // boards
    for i in range(size * size):
        x, y = i % size, i // size
        row = y // board
        grain = blotch(x + row * 97, y * 6, 20, salt + row) * 0.25
        streak = hash2(x // 3 + row * 50, row, salt) * 0.12
        seam = y % board < max(2, size // 64)
        # Stagger the butt joints so the boards read as boards.
        joint = (x + row * board * 3 // 2) % (size // 2) < max(2, size // 128)
        shade = 0.55 if seam or joint else 0.85 + grain + streak
        out.append([clamp(c * shade) for c in PLANK] + [255])
    return out


def workbench_top(tex):
    size = int(math.sqrt(len(tex["stone"])))
    out = planks(size, boards=4, salt=5)
    edge = size // 16
    for i, px in enumerate(out):
        x, y = i % size, i // size
        border = x < edge or y < edge or x >= size - edge or y >= size - edge
        # A 3x3 crafting grid scored into the middle of the top.
        cell = size // 6
        gx, gy = x - (size - 3 * cell) // 2, y - (size - 3 * cell) // 2
        inside = 0 <= gx <= 3 * cell and 0 <= gy <= 3 * cell
        line = inside and (gx % cell < 3 or gy % cell < 3)
        if border or line:
            out[i] = [clamp(c * 0.5) for c in px[:3]] + [255]
    return out


def workbench_side(tex):
    size = int(math.sqrt(len(tex["stone"])))
    out = planks(size, boards=4, salt=9)
    leg = size // 6
    for i, px in enumerate(out):
        x, y = i % size, i // size
        # The open space between the legs, under the tabletop.
        under = y > size * 0.35 and leg <= x < size - leg
        if under:
            out[i] = [clamp(c * 0.3) for c in px[:3]] + [255]
    return out


def forge_stone(tex):
    return [tinted(px, (150, 140, 136), 0.75) for px in tex["cobblestone"]]


def forge_front(tex):
    size = int(math.sqrt(len(tex["cobblestone"])))
    out = forge_stone(tex)
    cx, top, bottom, half = size / 2, size * 0.42, size * 0.86, size * 0.27
    for i in range(len(out)):
        x, y = i % size, i // size
        dx = abs(x - cx)
        # A rounded arch: a rectangle below `top`, a semicircle above it.
        arch = (y >= top and dx < half) or (math.hypot(dx, y - top) < half)
        if arch and y < bottom and y > top - half:
            depth = (y - (top - half)) / (bottom - (top - half))
            flicker = blotch(x, y, 10, 17) * 0.35
            heat = min(1.0, 0.35 + depth * 0.8 + flicker)
            out[i] = [clamp(255 * heat), clamp(170 * heat * heat), clamp(40 * heat**3), 255]
    return out


def forge_top(tex):
    size = int(math.sqrt(len(tex["cobblestone"])))
    out = forge_stone(tex)
    lo, hi = size * 0.3, size * 0.7
    for i in range(len(out)):
        x, y = i % size, i // size
        if lo <= x < hi and lo <= y < hi:
            bar = (x - int(lo)) % (size // 10) < 4
            glow = 0.6 + blotch(x, y, 12, 23) * 0.4
            out[i] = [60, 50, 48, 255] if bar else [clamp(255 * glow), clamp(120 * glow), 30, 255]
    return out


# station id -> the textures it needs and which face reads which one.
STATIONS = {
    "workbench": {
        "textures": {"top": workbench_top, "side": workbench_side},
        "faces": {"up": "top", "down": "top", "side": "side"},
    },
    "forge": {
        "textures": {"top": forge_top, "side": forge_front},
        "faces": {"up": "top", "down": "top", "side": "side"},
    },
}


def station_model(block_id, spec):
    """A full cube whose faces name the station's own textures."""
    names = sorted(spec["textures"])
    ref = {name: f"../../textures/blocks/{block_id}_{name}" for name in names}
    key = {name: str(i) for i, name in enumerate(names)}
    faces = {}
    for face in ["north", "east", "south", "west", "up", "down"]:
        name = spec["faces"].get(face, spec["faces"]["side"])
        faces[face] = {"uv": [0, 0, 16, 16], "texture": f"#{key[name]}", "cullface": face}
    textures = {key[name]: ref[name] for name in names}
    textures["particle"] = ref[spec["faces"]["side"]]
    return {
        "format_version": "1.21.11",
        "credit": "Generated by scripts/gen-placeholder-art.py",
        "textures": textures,
        "elements": [{"name": "block", "from": [0, 0, 0], "to": [16, 16, 16], "faces": faces}],
    }


def generate_stations(textures):
    size = 256
    for block_id, spec in STATIONS.items():
        for name, build in spec["textures"].items():
            write_png(TEXTURES / f"{block_id}_{name}.png", size, size, build(textures))
        model = station_model(block_id, spec)
        (MODELS / f"{block_id}.json").write_text(json.dumps(model, indent="\t") + "\n")
        print(f"wrote station {block_id}")


def upscale(width, pixels, size):
    """Nearest-neighbour to `size`, the same rule the game applies at load."""
    assert size % width == 0, f"{width}px does not divide {size}"
    k = size // width
    return [pixels[(y // k) * width + (x // k)] for y in range(size) for x in range(size)]


ITEMS_DIR = ROOT / "assets" / "textures" / "items"
ITEM_MODELS = ROOT / "assets" / "models" / "items"
ENTITY_DIR = ROOT / "assets" / "textures" / "entity"


def saturation(px):
    return max(px[:3]) - min(px[:3])


def sprite(src, tint, brightness=1.0):
    """Recolour an item sprite, keeping its silhouette (alpha)."""
    return lambda items: [tinted(px, tint, brightness) for px in items[src]]


def two_tone(src, grey_tint, colour_tint, brightness=1.0):
    """Recolour a tool: its grey head one way, its coloured handle another."""

    def build(items):
        out = []
        for px in items[src]:
            if px[3] == 0:
                out.append(px)
            elif saturation(px) < 28:
                out.append(tinted(px, grey_tint, brightness))
            else:
                out.append(tinted(px, colour_tint, 1.0))
        return out

    return build


# Item id -> (sprite recipe, model): `None` writes an `item/generated` stub;
# a model name copies that hand-modelled item's .json, re-pointed.
ITEMS = {
    "antler_shard": (sprite("flint", (236, 226, 200), 1.6), None),
    "raw_venison": (sprite("raw_beef", (190, 70, 60), 1.2), None),
    "cooked_venison": (sprite("cooked_beef", (170, 110, 60), 1.15), None),
    "stag_effigy": (sprite("leather", (150, 200, 110), 1.3), None),
    "elder_antler": (sprite("flint", (250, 214, 110), 1.8), None),
    "elder_stag_trophy": (sprite("leather", (90, 150, 70), 1.4), None),
    "antler_pickaxe": (
        two_tone("stone_pickaxe", (240, 230, 205), (150, 110, 70), 1.5),
        "stone_pickaxe",
    ),
}
ITEM_SOURCES = ["flint", "raw_beef", "cooked_beef", "leather", "stone_pickaxe"]


def fur_sheet(base, spots, eyes, eye_colour, head_front):
    """A 64x64 quadruped sheet: mottled fur everywhere (every unwrap region
    reads as fur, wherever an entity's `*_uv` puts it) plus two eyes on the
    head's front face, `head_front` = its top-left texel."""
    out = []
    for y in range(64):
        for x in range(64):
            n = blotch(x, y, 5, 11) * 0.6 + hash2(x, y, 4) * 0.4
            colour = spots if n > 0.72 else base
            shade = 0.8 + 0.3 * n
            out.append([clamp(c * shade) for c in colour] + [255])
    fx, fy = head_front
    for ex, ey in eyes:
        out[(fy + ey) * 64 + fx + ex] = list(eye_colour) + [255]
    return out


# Mob skin sheets: id -> fur recipe. Eye positions are relative to the head's
# front face, whose corner is `(head depth, head depth)` for `head_uv = [0, 0]`.
SKINS = {
    "deer": lambda: fur_sheet((150, 104, 62), (226, 206, 170), [(1, 2), (4, 2)], (20, 16, 12), (6, 6)),
    # Humanoid: the player unwrap, whose head front starts at (8, 8).
    "thornling": lambda: fur_sheet((70, 96, 44), (120, 70, 40), [(1, 3), (5, 3)], (255, 220, 90), (8, 8)),
    "elder_stag": lambda: fur_sheet(
        (70, 84, 52), (140, 170, 96), [(1, 3), (6, 3)], (150, 255, 170), (9, 9)
    ),
}


def generate_blocks():
    size = 256
    textures = {}
    for name in SOURCES:
        w, h, px = read_png(TEXTURES / f"{name}.png")
        assert w == h, name
        textures[name] = upscale(w, px, size)
    template = json.loads((MODELS / "stone.json").read_text())
    for block_id, build in BLOCKS.items():
        write_png(TEXTURES / f"{block_id}.png", size, size, build(textures))
        model = json.loads(json.dumps(template))
        ref = f"../../textures/blocks/{block_id}"
        model["textures"] = {key: ref for key in model["textures"]}
        (MODELS / f"{block_id}.json").write_text(json.dumps(model, indent="\t") + "\n")
        print(f"wrote block {block_id}")
    generate_stations(textures)


def generate_items():
    size = 32
    sprites = {}
    for name in ITEM_SOURCES:
        w, _, px = read_png(ITEMS_DIR / f"{name}.png")
        sprites[name] = upscale(w, px, size)
    for item_id, (build, modelled) in ITEMS.items():
        write_png(ITEMS_DIR / f"{item_id}.png", size, size, build(sprites))
        ref = f"../../textures/items/{item_id}"
        if modelled is None:
            stub = {"parent": "item/generated", "textures": {"layer0": ref}}
            text = json.dumps(stub, indent="\t") + "\n"
        else:
            model = json.loads((ITEM_MODELS / f"{modelled}.json").read_text())
            model["textures"] = {key: ref for key in model["textures"]}
            text = json.dumps(model, indent="\t") + "\n"
        (ITEM_MODELS / f"{item_id}.json").write_text(text)
        print(f"wrote item {item_id}")


def generate_skins():
    for skin_id, build in SKINS.items():
        folder = ENTITY_DIR / skin_id
        folder.mkdir(exist_ok=True)
        write_png(folder / f"{skin_id}.png", 64, 64, build())
        print(f"wrote skin {skin_id}")


def main():
    generate_blocks()
    generate_items()
    generate_skins()


if __name__ == "__main__":
    main()
