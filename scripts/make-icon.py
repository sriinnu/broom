#!/usr/bin/env python3
"""Draw Broom's app icon and build AppIcon.icns.

The investigator from `src/comic.rs` (flat cap, red scarf, camel coat), face
first, holding a magnifier up to one eye so the lens makes it huge. A broom
rests over the other shoulder. Ligne claire: flat fills, one ink contour, no
gradients, no gloss, no cast shadows. Colours are the character's own.

Usage: scripts/make-icon.py <output-dir>   (needs Pillow and macOS's iconutil)

    uv run --with pillow python3 -I scripts/make-icon.py packaging
"""
import math
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw

OUT = 1024
S = 4
N = OUT * S
LW = 14  # the single contour weight, in output pixels

# The character's palette, from src/comic.rs.
INK = (29, 26, 23, 255)
CREAM = (243, 234, 216, 255)
LENS = (191, 227, 242, 255)
WHITE = (255, 255, 255, 255)
HANDLE = (181, 117, 46, 255)
COAT = (201, 164, 106, 255)
CAP = (90, 107, 122, 255)
SCARF = (216, 81, 100, 255)
SKIN = (244, 207, 168, 255)
STRAW = (222, 184, 92, 255)
SKY = (166, 206, 228, 255)
GROUND = (124, 140, 70, 255)


def px(v):
    return v * S


def poly(d, pts, fill=None, stroke=INK, w=LW):
    pts = [(px(x), px(y)) for x, y in pts]
    if fill:
        d.polygon(pts, fill=fill)
    if stroke:
        d.line(pts + [pts[0]], fill=stroke, width=px(w), joint="curve")


def disc(d, cx, cy, r, fill=None, stroke=INK, w=LW):
    box = [px(cx - r), px(cy - r), px(cx + r), px(cy + r)]
    if fill:
        d.ellipse(box, fill=fill)
    if stroke:
        d.ellipse(box, outline=stroke, width=px(w))


def ellipse(d, cx, cy, rx, ry, fill=None, stroke=INK, w=LW):
    box = [px(cx - rx), px(cy - ry), px(cx + rx), px(cy + ry)]
    if fill:
        d.ellipse(box, fill=fill)
    if stroke:
        d.ellipse(box, outline=stroke, width=px(w))


def stroke(d, pts, w=LW, color=INK):
    d.line([(px(x), px(y)) for x, y in pts], fill=color, width=px(w), joint="curve")
    for x, y in (pts[0], pts[-1]):
        r = px(w) / 2
        d.ellipse([px(x) - r, px(y) - r, px(x) + r, px(y) + r], fill=color)


def thick(d, a, b, w, fill):
    """A rounded bar from a to b: fill plus the contour."""
    stroke(d, [a, b], w + LW * 2, INK)
    stroke(d, [a, b], w, fill)


def squircle_mask(inset=100, radius=185):
    m = Image.new("L", (N, N), 0)
    ImageDraw.Draw(m).rounded_rectangle([px(inset), px(inset), px(OUT - inset), px(OUT - inset)], radius=px(radius), fill=255)
    return m


def draw_icon():
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # Scene: flat sky, a ground line low down.
    d.rectangle([0, 0, N, N], fill=SKY)
    d.rectangle([0, px(790), N, N], fill=GROUND)
    d.line([(0, px(790)), (N, px(790))], fill=INK, width=px(LW))

    # Broom over the near shoulder: head down on the left, handle up behind
    # the cap and out past it. Drawn first so the body covers its middle.
    hx0, hy0 = 670, 160
    hx1, hy1 = 265, 690
    thick(d, (hx0, hy0), (hx1, hy1), 26, HANDLE)
    ang = math.atan2(hy1 - hy0, hx1 - hx0)
    ux, uy = math.cos(ang), math.sin(ang)
    nx, ny = -uy, ux
    bx, by = hx1 + ux * 10, hy1 + uy * 10
    head = [
        (bx + nx * 46, by + ny * 46),
        (bx - nx * 46, by - ny * 46),
        (bx - nx * 100 + ux * 135, by - ny * 100 + uy * 135),
        (bx + nx * 100 + ux * 135, by + ny * 100 + uy * 135),
    ]
    poly(d, head, fill=STRAW)
    stroke(d, [(bx + nx * 50, by + ny * 50), (bx - nx * 50, by - ny * 50)], 22, SCARF)
    for t in (-0.6, -0.2, 0.2, 0.6):
        sx, sy = bx + nx * 70 * t + ux * 45, by + ny * 70 * t + uy * 45
        ex, ey = bx + nx * 90 * t + ux * 122, by + ny * 90 * t + uy * 122
        stroke(d, [(sx, sy), (ex, ey)], 8, INK)

    # Coat: shoulders wide, cut off by the frame bottom.
    poly(d, [(300, 1024 + 40), (330, 620), (700, 620), (730, 1024 + 40)], fill=COAT)
    stroke(d, [(515, 640), (515, 1024)], LW, INK)  # the coat's front edge
    # Scarf: a wrapped band with a tail off to the right.
    ellipse(d, 512, 600, 150, 60, fill=SCARF)
    poly(d, [(640, 590), (760, 700), (690, 740), (610, 640)], fill=SCARF)

    # Head.
    disc(d, 512, 420, 190, fill=SKIN)
    # Flat cap: a dome over the crown, a peak to the left.
    d.pieslice([px(512 - 200), px(240 - 10), px(512 + 200), px(420 + 150)], 180, 360, fill=CAP)
    d.pieslice([px(512 - 200), px(240 - 10), px(512 + 200), px(420 + 150)], 180, 360, outline=INK, width=px(LW))
    poly(d, [(330, 312), (190, 328), (196, 384), (340, 372)], fill=CAP)
    stroke(d, [(318, 325), (706, 325)], LW + 6, INK)  # band
    d.rectangle([px(318), px(325), px(706), px(345)], fill=CAP)
    stroke(d, [(318, 325), (706, 325)], LW, INK)

    # Face: a dot eye on the left, a mouth, an ear.
    disc(d, 438, 430, 14, fill=INK, stroke=None)
    stroke(d, [(470, 520), (520, 528), (560, 512)], LW, INK)
    disc(d, 700, 440, 26, fill=SKIN)

    # The magnifier, held up to the right eye. Lens big, the eye inside it
    # drawn large: that's the whole gag.
    lx, ly, lr = 600, 440, 118
    thick(d, (lx + lr * 0.72, ly + lr * 0.72), (820, 690), 30, HANDLE)
    # The hand on the handle.
    disc(d, 765, 640, 44, fill=SKIN)
    disc(d, lx, ly, lr + 10, fill=CREAM)  # rim
    disc(d, lx, ly, lr - 12, fill=LENS)
    # Magnified eye: white, iris, pupil, and the blink-proof highlight.
    disc(d, lx, ly + 4, 58, fill=WHITE)
    disc(d, lx + 6, ly + 8, 30, fill=INK, stroke=None)
    disc(d, lx + 18, ly - 6, 10, fill=WHITE, stroke=None)
    # Lens shine.
    stroke(d, [(lx - 70, ly - 40), (lx - 40, ly - 76)], 12, WHITE)

    # Downsample, then cut the squircle.
    icon = img.resize((OUT, OUT), Image.LANCZOS)
    mask = squircle_mask().resize((OUT, OUT), Image.LANCZOS)
    out = Image.new("RGBA", (OUT, OUT), (0, 0, 0, 0))
    out.paste(icon, (0, 0), mask)
    # Contour around the squircle, like every other shape.
    ring = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    ImageDraw.Draw(ring).rounded_rectangle([px(100), px(100), px(924), px(924)], radius=px(185), outline=INK, width=px(LW))
    out.alpha_composite(ring.resize((OUT, OUT), Image.LANCZOS))
    return out


def draw_icon_small():
    """At 16 and 32 px the magnifier and cap are all that reads: keep those."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, N, N], fill=SKY)
    d.rectangle([0, px(760), N, N], fill=GROUND)
    disc(d, 440, 470, 230, fill=SKIN, w=24)
    d.pieslice([px(440 - 240), px(230), px(440 + 240), px(470 + 180)], 180, 360, fill=CAP, outline=INK, width=px(24))
    poly(d, [(190, 330), (90, 350), (90, 395), (230, 380)], fill=CAP, w=24)
    lx, ly, lr = 600, 500, 170
    thick(d, (lx + lr * 0.7, ly + lr * 0.7), (880, 800), 48, HANDLE)
    disc(d, lx, ly, lr, fill=CREAM, w=24)
    disc(d, lx, ly, lr - 30, fill=LENS, w=24)
    disc(d, lx + 8, ly + 10, 60, fill=INK, stroke=None)
    icon = img.resize((OUT, OUT), Image.LANCZOS)
    mask = squircle_mask().resize((OUT, OUT), Image.LANCZOS)
    out = Image.new("RGBA", (OUT, OUT), (0, 0, 0, 0))
    out.paste(icon, (0, 0), mask)
    return out


def preview(icon, small, path):
    """Dock sizes side by side, on dark and light, so the mark can be judged."""
    sizes = (512, 256, 128, 64, 32, 16)
    w = sum(sizes) + 40 * (len(sizes) + 1)
    sheet = Image.new("RGBA", (w, 1200), (24, 24, 26, 255))
    ImageDraw.Draw(sheet).rectangle([0, 600, w, 1200], fill=(236, 233, 226, 255))
    for row, y0 in ((0, 40), (1, 640)):
        x = 40
        for s in sizes:
            art = small if s <= 32 else icon
            sheet.alpha_composite(art.resize((s, s), Image.LANCZOS), (x, y0 + (512 - s) // 2))
            x += s + 40
    sheet.save(path)


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out_dir, exist_ok=True)
    icon = draw_icon()
    small = draw_icon_small()
    icon.save(os.path.join(out_dir, "AppIcon.png"))
    # The About card embeds this one (src/app.rs); keep it small.
    icon.resize((256, 256), Image.LANCZOS).save(os.path.join(out_dir, "AppIcon-256.png"), optimize=True)
    preview(icon, small, os.path.join(out_dir, "icon-sizes-preview.png"))
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "AppIcon.iconset")
        os.makedirs(iconset)
        for size in (16, 32, 128, 256, 512):
            art = small if size <= 32 else icon
            art.resize((size, size), Image.LANCZOS).save(os.path.join(iconset, f"icon_{size}x{size}.png"))
            art.resize((size * 2, size * 2), Image.LANCZOS).save(os.path.join(iconset, f"icon_{size}x{size}@2x.png"))
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(out_dir, "AppIcon.icns")], check=True)
    print(f"wrote {out_dir}/AppIcon.icns")


if __name__ == "__main__":
    main()
