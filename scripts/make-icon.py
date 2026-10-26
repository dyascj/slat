"""Renders Slat's app icon from assets/icon.svg (a navy squircle with a row of colored slats running past both edges, the
focused one white and taller) into
assets/icon-1024.png and assets/AppIcon.icns. Edit the SVG, then run:
python3 scripts/make-icon.py  (needs cairosvg: pip install cairosvg)"""
import os, subprocess, tempfile
import cairosvg

root = os.path.join(os.path.dirname(__file__), "..", "assets")
svg = os.path.join(root, "icon.svg")


def render(path, px):
    # every size is drawn straight from the vector, so small sizes stay crisp
    cairosvg.svg2png(url=svg, write_to=path, output_width=px, output_height=px)


render(os.path.join(root, "icon-1024.png"), 1024)
with tempfile.TemporaryDirectory() as t:
    iconset = os.path.join(t, "AppIcon.iconset")
    os.mkdir(iconset)
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
            render(os.path.join(iconset, name), size * scale)
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(root, "AppIcon.icns")], check=True)
