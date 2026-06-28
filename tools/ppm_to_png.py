#!/usr/bin/env python3
"""Convert a binary PPM (P6) to PNG using only the Python standard library.

Usage: python tools/ppm_to_png.py <in.ppm> [out.png]
The PNG goes next to the PPM (same name, .png) if no output is given.
"""
import struct
import sys
import zlib


def ppm_to_png(ppm_path: str, png_path: str) -> tuple[int, int]:
    data = open(ppm_path, "rb").read()
    if data[:2] != b"P6":
        raise SystemExit(f"{ppm_path}: not a P6 PPM")
    # Header is three newline-separated fields: "P6", "W H", "maxval".
    newlines = [i for i, b in enumerate(data) if b == 10][:3]
    w, h = map(int, data[newlines[0] + 1 : newlines[1]].split())
    body = data[newlines[2] + 1 :]
    if len(body) < w * h * 3:
        raise SystemExit(f"{ppm_path}: truncated pixel data")

    def chunk(typ: bytes, payload: bytes) -> bytes:
        c = typ + payload
        return struct.pack(">I", len(payload)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    raw = bytearray()
    for y in range(h):
        raw.append(0)  # filter byte: none
        raw += body[y * w * 3 : (y + 1) * w * 3]

    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    open(png_path, "wb").write(png)
    return w, h


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    src = sys.argv[1]
    dst = sys.argv[2] if len(sys.argv) > 2 else src.rsplit(".", 1)[0] + ".png"
    w, h = ppm_to_png(src, dst)
    print(f"wrote {dst} ({w}x{h})")
