"""Draws SimLab's app icon (the OffGrid ring on a warm dark tile) as a 1024 px PNG.
Run: mesher/.venv/bin/python scripts/make_icon.py app/src-tauri/icons/source.png"""
import struct, sys, zlib
import numpy as np

N, SS = 1024, 4  # size, supersampling
y, x = np.mgrid[0:N * SS, 0:N * SS].astype(np.float64) / SS
# macOS tile: 824 px rounded square centred, radius 185
m, r = 100, 185
dx = np.maximum(np.maximum(m + r - x, x - (N - m - r)), 0)
dy = np.maximum(np.maximum(m + r - y, y - (N - m - r)), 0)
tile = (dx ** 2 + dy ** 2) <= r ** 2
# ring geometry from the brand mark (200-unit box), scaled to 560 px, centred
s = 560 / 200
ox, oy = (N - 200 * s) / 2, (N - 200 * s) / 2 + 10
cx, cy, R, w = ox + 100 * s, oy + 108 * s, 58 * s, 22 * s / 2
px, py = x - cx, y - cy
ang = np.degrees(np.arctan2(px, -py))  # 0 at top, clockwise positive
gap = 25.0  # degrees each side of the top left open
dist = np.abs(np.hypot(px, py) - R)
ring = (dist <= w) & (np.abs(ang) >= gap)
for a in (gap, -gap):  # round caps
    ex, ey = cx + R * np.sin(np.radians(a)), cy - R * np.cos(np.radians(a))
    ring |= np.hypot(x - ex, y - ey) <= w
dot = np.hypot(x - (ox + 100 * s), y - (oy + 40 * s)) <= 17 * s
img = np.zeros((N * SS, N * SS, 4))
img[tile] = (0x14 / 255, 0x11 / 255, 0x0d / 255, 1)
img[tile & (ring | dot)] = (1, 0x6a / 255, 0, 1)
img = img.reshape(N, SS, N, SS, 4).mean(axis=(1, 3))
rgba = (img * 255 + 0.5).astype(np.uint8)
raw = b"".join(b"\x00" + rgba[i].tobytes() for i in range(N))
def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
