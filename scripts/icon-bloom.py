#!/usr/bin/env python3
"""Concept B — "the drive, blooming".

A literal hard drive (mirror platter, chrome arm, spindle) whose disc is
shedding brooms: five categorical-hue brooms peel off the spinning edge and
swirl up and away, growing as they fly. The inverse of the shipped concept
(a flower drawn as a platter): here a real drive grows a flower.

Writes AppIcon-v4.png/.icns next to the current AppIcon.* for side-by-side
approval. Needs Pillow (uv run --with pillow) and macOS's iconutil.
"""
import colorsys
import math
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
SCALE = 4  # draw at 4096, downsample

# Broom colours along the swirl: rose, tangerine, gold, lime, teal.
HUES = [0.99, 0.07, 0.13, 0.30, 0.50]

STEEL = (154, 158, 166)
STEEL_HI = (198, 202, 210)
STEEL_LO = (102, 106, 114)
ACCENT = (79, 157, 255)
HOLE = (12, 13, 16)

# Broom tips must stay inside this box (the squircle's inset is 100).
SAFE = 112


def hsl(h, s, l):
    r, g, b = colorsys.hls_to_rgb(h % 1.0, l, s)
    return (int(r * 255), int(g * 255), int(b * 255), 255)


def mix(c0, c1, t):
    return tuple(int(a + (b - a) * t) for a, b in zip(c0[:3], c1[:3])) + (255,)


def lighten(c, dl):
    h, l, s = colorsys.rgb_to_hls(c[0] / 255, c[1] / 255, c[2] / 255)
    return hsl(h, s, min(l + dl, 0.85))


def draw_squircle(img, inset, radius):
    n = img.width
    shadow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        (inset, inset + 12 * SCALE, n - inset, n - inset + 12 * SCALE), radius, fill=(0, 0, 0, 110)
    )
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(18 * SCALE)))
    bg = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    bd = ImageDraw.Draw(bg)
    for y in range(inset, n - inset):
        t = (y - inset) / (n - 2 * inset)
        bd.line((inset, y, n - inset, y), fill=mix((44, 46, 54), (24, 25, 30), t))
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle((inset, inset, n - inset, n - inset), radius, fill=255)
    img.paste(bg, (0, 0), mask)


def disc_mask(n, cx, cy, r_out, r_in=0.0):
    m = Image.new("L", (n, n), 0)
    md = ImageDraw.Draw(m)
    md.ellipse((cx - r_out, cy - r_out, cx + r_out, cy + r_out), fill=255)
    if r_in:
        md.ellipse((cx - r_in, cy - r_in, cx + r_in, cy + r_in), fill=0)
    return m


def gradient_layer(n, top, bottom, mask):
    grad = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    for y in range(n):
        gd.line((0, y, n, y), fill=mix(top, bottom, y / n))
    layer = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    layer.paste(grad, (0, 0), mask)
    return layer


def draw_drive(img, cx, cy, r):
    """A mirror platter: metal face, faint tracks, a specular streak, spindle."""
    n = img.width
    shadow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).ellipse((cx - r - 6, cy - r + 8, cx + r + 6, cy + r + 22), fill=(0, 0, 0, 110))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(16 * SCALE)))

    img.alpha_composite(gradient_layer(n, STEEL_HI, STEEL_LO, disc_mask(n, cx, cy, r)))

    # The edge falls off: a soft darkened rim.
    rim = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(rim).ellipse((cx - r, cy - r, cx + r, cy + r), outline=(0, 0, 0, 90), width=int(0.10 * r))
    img.alpha_composite(rim.filter(ImageFilter.GaussianBlur(6 * SCALE)))

    overlay = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    od = ImageDraw.Draw(overlay)
    # Faint data tracks.
    for tr in (0.54 * r, 0.73 * r):
        od.arc((cx - tr, cy - tr, cx + tr, cy + tr), 0, 360, fill=(16, 17, 21, 60), width=3 * SCALE)
    img.alpha_composite(overlay)

    # A specular streak catching the light across the upper-left.
    spec = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    sr = 0.80 * r
    ImageDraw.Draw(spec).arc((cx - sr, cy - sr, cx + sr, cy + sr), 195, 262, fill=(255, 255, 255, 30), width=int(0.22 * r))
    img.alpha_composite(spec.filter(ImageFilter.GaussianBlur(5 * SCALE)))

    # Spindle: chrome collar, punched hole, a crescent of light on the punch.
    img.alpha_composite(gradient_layer(n, STEEL_HI, STEEL_LO, disc_mask(n, cx, cy, 0.22 * r, 0.11 * r)))
    d = ImageDraw.Draw(img)
    hr = 0.11 * r
    d.ellipse((cx - hr, cy - hr, cx + hr, cy + hr), fill=HOLE + (255,))
    glow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(glow).arc((cx - hr, cy - hr, cx + hr, cy + hr), 180, 360, fill=(255, 255, 255, 70), width=2 * SCALE)
    img.alpha_composite(glow)


def draw_arm(img, pivot, head, w1, w2):
    """Chrome actuator: pivot on the chassis outside the platter, head over it."""
    dx, dy = head[0] - pivot[0], head[1] - pivot[1]
    length = math.hypot(dx, dy)
    ux, uy = dx / length, dy / length
    px, py = -uy, ux
    poly = [
        (pivot[0] + px * w1, pivot[1] + py * w1),
        (head[0] + px * w2, head[1] + py * w2),
        (head[0] - px * w2, head[1] - py * w2),
        (pivot[0] - px * w1, pivot[1] - py * w1),
    ]
    shape = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shape)
    sd.polygon(poly, fill=(255, 255, 255, 255))
    for (qx, qy), w in ((pivot, w1), (head, w2)):
        sd.ellipse((qx - w, qy - w, qx + w, qy + w), fill=(255, 255, 255, 255))

    from PIL import ImageChops

    shadow = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 110), (0, 0), shape.split()[3])
    shadow = ImageChops.offset(shadow, 5 * SCALE, 9 * SCALE)
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(5 * SCALE)))

    arm = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    ad = ImageDraw.Draw(arm)
    ad.polygon(poly, fill=STEEL + (255,))
    for (qx, qy), w in ((pivot, w1), (head, w2)):
        ad.ellipse((qx - w, qy - w, qx + w, qy + w), fill=STEEL + (255,))
    ad.line((poly[0][0], poly[0][1], poly[1][0], poly[1][1]), fill=STEEL_HI + (255,), width=2 * SCALE)
    ad.line((poly[3][0], poly[3][1], poly[2][0], poly[2][1]), fill=STEEL_LO + (255,), width=2 * SCALE)
    # Machined pivot with its screw; the head reading, with the app's blue dot.
    ad.ellipse((pivot[0] - w1, pivot[1] - w1, pivot[0] + w1, pivot[1] + w1), fill=STEEL_LO + (255,))
    screw = 0.5 * w1
    ad.ellipse((pivot[0] - screw, pivot[1] - screw, pivot[0] + screw, pivot[1] + screw), fill=HOLE + (255,))
    hb = w2 + 7 * SCALE
    ad.ellipse((head[0] - hb, head[1] - hb, head[0] + hb, head[1] + hb), fill=STEEL_HI + (255,))
    dot = 10 * SCALE
    ad.ellipse((head[0] - dot, head[1] - dot, head[0] + dot, head[1] + dot), fill=ACCENT + (255,))
    img.alpha_composite(arm)


def broom_points(length, width, steps=44):
    """A broom outline pointing along +x: pointed base at 0, rounded tip at length."""
    pts = []
    for i in range(steps + 1):
        t = i / steps
        pts.append((t * length, -width * math.sin(math.pi * t) ** 0.85))
    for i in range(steps, -1, -1):
        t = i / steps
        pts.append((t * length, width * math.sin(math.pi * t) ** 0.85))
    return pts


def broom_layer(color, length, width):
    """One broom as its own layer: gradient body, a pale vein, a shaded edge."""
    pad = 10 * SCALE
    w = int(length * SCALE) + 2 * pad
    h = int(2 * width * SCALE) + 2 * pad
    yoff = width * SCALE + pad  # centre the outline: y spans ±width*SCALE
    lay = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    pts = [(x * SCALE + pad, y * SCALE + yoff) for x, y in broom_points(length, width)]
    ImageDraw.Draw(lay).polygon(pts, fill=color)

    # Light falls on the base, the tip deepens.
    mask = lay.split()[3]
    grad = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    for x in range(w):
        gd.line((x, 0, x, h), fill=mix(lighten(color, 0.16), color, x / w))
    lay.paste(grad, (0, 0), mask)

    detail = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    dd = ImageDraw.Draw(detail)
    vein = [(x * SCALE + pad, y * SCALE + yoff) for x, y in broom_points(length * 0.94, width * 0.20)]
    dd.polygon(vein, fill=lighten(color, 0.22)[:3] + (80,))
    dd.line(pts, fill=mix(color, HOLE, 0.45)[:3] + (70,), width=2 * SCALE, joint="curve")
    lay.alpha_composite(detail)
    return lay


def place_broom(img, color, bx, by, angle_deg, length, width, fly):
    """Rotate a broom so its tip points along screen `angle_deg`, base at (bx, by).

    `fly` (0..1) lifts it off the disc: higher brooms throw longer, softer shadows.
    """
    # Keep the tip (and base) inside the safe box; shrink rather than clip.
    a = math.radians(angle_deg)
    tip = (bx + length * math.cos(a), by + length * math.sin(a))
    while length > 40 and not all(SAFE <= v <= SIZE - SAFE for v in (*tip, bx, by)):
        length *= 0.93
        tip = (bx + length * math.cos(a), by + length * math.sin(a))

    lay = broom_layer(color, length, width)
    rot = lay.rotate(-angle_deg, expand=True, resample=Image.BICUBIC)
    base_local = (
        rot.width / 2 - (length * SCALE / 2) * math.cos(a),
        rot.height / 2 - (length * SCALE / 2) * math.sin(a),
    )
    topleft = (int(bx * SCALE - base_local[0]), int(by * SCALE - base_local[1]))

    shadow = Image.new("RGBA", rot.size, (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 95), (0, 0), rot.split()[3])
    ox, oy = int((7 + 4 * fly) * SCALE), int((10 + 6 * fly) * SCALE)
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur((4 + 3 * fly) * SCALE)), (topleft[0] + ox, topleft[1] + oy))
    img.alpha_composite(rot, topleft)


def draw_icon():
    n = SIZE * SCALE
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    draw_squircle(img, 100 * SCALE, 185 * SCALE)

    cx, cy, r = 400 * SCALE, 580 * SCALE, 280 * SCALE
    draw_drive(img, cx, cy, r)
    draw_arm(img, (595 * SCALE, 858 * SCALE), (533 * SCALE, 503 * SCALE), 26 * SCALE, 17 * SCALE)

    # The bloom: brooms peel off the spinning edge and swirl away, growing.
    lengths = [115, 140, 168, 196, 222]
    for i, (hue, length) in enumerate(zip(HUES, lengths)):
        phi = math.radians(-110 + 22 * i)  # where it leaves the disc
        base = (400 + (310 + 34 * i) * math.cos(phi), 580 + (310 + 34 * i) * math.sin(phi))
        place_broom(img, hsl(hue, 0.62, 0.60), *base, -110 + 22 * i + 90, length, 0.62 * length, i / 4)

    return img.resize((SIZE, SIZE), Image.LANCZOS)


def draw_icon_small():
    """Dock-legible 16/32 px art: platter, two brooms, the blue dot."""
    n = SIZE * SCALE
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    draw_squircle(img, 100 * SCALE, 185 * SCALE)

    cx, cy, r = 400 * SCALE, 570 * SCALE, 285 * SCALE
    d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=STEEL + (255,))
    d.ellipse((cx - r, cy - r, cx + r, cy + r), outline=STEEL_LO + (255,), width=6 * SCALE)
    hr = 40 * SCALE
    d.ellipse((cx - hr, cy - hr, cx + hr, cy + hr), fill=HOLE + (255,))

    place_broom(img, hsl(0.07, 0.65, 0.66), 600, 330, -8, 250, 150, 0)
    place_broom(img, hsl(0.99, 0.65, 0.66), 430, 260, -34, 215, 130, 0)

    dot = 17 * SCALE
    d.ellipse((533 * SCALE - dot, 503 * SCALE - dot, 533 * SCALE + dot, 503 * SCALE + dot), fill=ACCENT + (255,))
    return img.resize((SIZE, SIZE), Image.LANCZOS)


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out_dir, exist_ok=True)
    icon = draw_icon()
    small = draw_icon_small()
    icon.save(os.path.join(out_dir, "AppIcon-v4.png"))
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "AppIcon.iconset")
        os.makedirs(iconset)
        for size in (16, 32, 128, 256, 512):
            art = small if size <= 32 else icon
            art.resize((size, size), Image.LANCZOS).save(os.path.join(iconset, f"icon_{size}x{size}.png"))
            art.resize((size * 2, size * 2), Image.LANCZOS).save(os.path.join(iconset, f"icon_{size}x{size}@2x.png"))
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(out_dir, "AppIcon-v4.icns")], check=True)
    print(f"wrote {out_dir}/AppIcon-v4.png and AppIcon-v4.icns")


if __name__ == "__main__":
    main()
