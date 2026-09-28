#!/usr/bin/env python3
"""Generate icons/icon.png — the Bluetooth rune on a rounded blue tile.

Tauri's codegen embeds a window icon at compile time and errors out if the file
is missing, so the icon has to exist in the tree. Generating it from a script
keeps a reviewable source for it instead of an opaque binary, and needs nothing
beyond the standard library.

The rune is five strokes: a vertical stem, an X across it, and the X's two
right-hand tips carried up and down to the stem's ends.
"""

import math
import struct
import zlib

SIZE = 512
SS = 3  # supersampling factor, for antialiasing
BG = (0x00, 0x82, 0xFC)  # Bluetooth SIG blue
FG = (0xFF, 0xFF, 0xFF)

STROKES = [
    ((0.50, 0.10), (0.50, 0.90)),
    ((0.30, 0.31), (0.70, 0.69)),
    ((0.30, 0.69), (0.70, 0.31)),
    ((0.70, 0.31), (0.50, 0.10)),
    ((0.70, 0.69), (0.50, 0.90)),
]
STROKE_HALF_WIDTH = 0.043
TILE_INSET = 0.055
TILE_RADIUS = 0.225


def dist_to_segment(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    span = dx * dx + dy * dy
    t = 0.0 if span == 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / span))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def inside_rounded_tile(x, y):
    """Signed-distance test for a rounded square in unit coordinates."""
    lo, hi = TILE_INSET, 1.0 - TILE_INSET
    r = TILE_RADIUS
    cx = min(max(x, lo + r), hi - r)
    cy = min(max(y, lo + r), hi - r)
    if lo <= x <= hi and lo <= y <= hi and (cx == x or cy == y):
        return True
    return math.hypot(x - cx, y - cy) <= r


def sample(x, y):
    """Return (r, g, b, a) for one unit-space sample."""
    if not inside_rounded_tile(x, y):
        return (0, 0, 0, 0)
    for (ax, ay), (bx, by) in STROKES:
        if dist_to_segment(x, y, ax, ay, bx, by) <= STROKE_HALF_WIDTH:
            return FG + (255,)
    return BG + (255,)


def render():
    rows = []
    for py in range(SIZE):
        row = bytearray()
        for px in range(SIZE):
            acc = [0, 0, 0, 0]
            for sy in range(SS):
                for sx in range(SS):
                    x = (px + (sx + 0.5) / SS) / SIZE
                    y = (py + (sy + 0.5) / SS) / SIZE
                    r, g, b, a = sample(x, y)
                    # Premultiply so that averaging across the tile edge does
                    # not drag the colour toward black where alpha is zero.
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            n = SS * SS
            alpha = acc[3] // n
            if alpha == 0:
                row += b"\0\0\0\0"
            else:
                row += bytes((acc[0] // acc[3], acc[1] // acc[3], acc[2] // acc[3], alpha))
        rows.append(bytes(row))
    return rows


def png(rows):
    raw = b"".join(b"\0" + r for r in rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    with open("icons/icon.png", "wb") as f:
        f.write(png(render()))
    print(f"icons/icon.png written ({SIZE}x{SIZE})")
