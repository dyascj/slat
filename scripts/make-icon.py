"""Draws Slat's app icon (assets/AppIcon.icns): a black squircle with four
slats, the third one solid and taller. Run: python3 scripts/make-icon.py"""
import os, subprocess, tempfile
from PIL import Image, ImageDraw

S = 1024 * 4  # supersample, then downscale for smooth edges
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
k = S / 1024
# macOS icon grid: 824 px body centred in 1024, ~185 px corner radius
m = 100 * k
d.rounded_rectangle([m, m, S - m, S - m], radius=185 * k, fill=(0, 0, 0, 255))
# hairline rim so the icon reads on dark docks
d.rounded_rectangle([m, m, S - m, S - m], radius=185 * k, outline=(255, 255, 255, 38), width=int(4 * k))

w, gap = 86 * k, 62 * k
total = 4 * w + 3 * gap
x0 = (S - total) / 2
for i in range(4):
    x = x0 + i * (w + gap)
    tall = i == 2
    h = (500 if tall else 380) * k
    y = (S - h) / 2
    box = [x, y, x + w, y + h]
    r = 26 * k
    if tall:
        d.rounded_rectangle(box, radius=r, fill=(255, 255, 255, 255))
    else:
        d.rounded_rectangle(box, radius=r, outline=(255, 255, 255, 255), width=int(10 * k))

icon = img.resize((1024, 1024), Image.LANCZOS)
root = os.path.join(os.path.dirname(__file__), "..", "assets")
icon.save(os.path.join(root, "icon-1024.png"))
with tempfile.TemporaryDirectory() as t:
    iconset = os.path.join(t, "AppIcon.iconset")
    os.mkdir(iconset)
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = size * scale
            name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
            icon.resize((px, px), Image.LANCZOS).save(os.path.join(iconset, name))
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(root, "AppIcon.icns")], check=True)
