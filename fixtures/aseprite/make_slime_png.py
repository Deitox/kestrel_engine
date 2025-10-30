"""
Utility script to regenerate `slime.png` so the fixtures stay self-contained.

Generates a 192x32 RGBA spritesheet with 6 colored frames that line up with
the metadata in `slime_idle.json`.
"""

import struct
import zlib
from pathlib import Path


WIDTH, HEIGHT = 192, 32
SEGMENT_WIDTH = 32
COLORS = [
    (0, 200, 83, 255),    # idle frame 0
    (86, 220, 120, 255),  # idle frame 1
    (255, 149, 0, 255),   # attack frame 0
    (255, 94, 58, 255),   # attack frame 1
    (255, 196, 0, 255),   # attack frame 2
    (120, 144, 156, 255), # hit frame
]


def build_rows() -> bytes:
    rows = []
    for _ in range(HEIGHT):
        row = bytearray()
        for x in range(WIDTH):
            color_index = min(x // SEGMENT_WIDTH, len(COLORS) - 1)
            row.extend(COLORS[color_index])
        rows.append(b"\x00" + bytes(row))  # PNG filter byte + pixels
    return b"".join(rows)


def chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def generate_png(path: Path) -> None:
    raw = build_rows()
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


if __name__ == "__main__":
    output = Path(__file__).with_name("slime.png")
    generate_png(output)
    print(f"Wrote placeholder spritesheet to {output}")
