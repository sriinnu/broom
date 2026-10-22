#!/usr/bin/env python3
"""Draw Broom's app icon — a flower that is a hard drive — and build AppIcon.icns.

One physical platter, blooming, with a little depth and a little retro: a
chunky chrome-bevel rim with chassis screws, gel-plastic brooms set in a
recessed well under a glassy sheen, a sphere-lit spindle dome, and a chrome
arm reading from the lower-right with a blue head. Small sizes (16/32) get
their own simplified single-ring art instead of a downscale.

Usage: scripts/make-icon.py <output-dir>   (needs Pillow and macOS's iconutil)
"""
import colorsys
import math
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops, ImageDraw, ImageFilter

SIZE = 1024
SCALE = 4  # draw large, then downsample, for smooth edges

# Categorical ladder: rose, tangerine, gold, lime, teal, periwinkle. Lime and
# teal sit far enough apart to stay distinct at Dock sizes.
HUES = [0.99, 0.07, 0.13, 0.30, 0.50, 0.62]

STEEL = (154, 158, 166)
STEEL_HI = (206, 210, 218)
STEEL_LO = (96, 100, 108)
ACCENT = (79, 157, 255)
HOLE = (12, 13, 16)
EDGE = (52, 54, 60)
FLOOR = (26, 27, 32)


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
    # Background: a subtle warm-leaning vertical gradient.
    bg = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    bd = ImageDraw.Draw(bg)
    for y in range(inset, n - inset):
        t = (y - inset) / (n - 2 * inset)
        bd.line((inset, y, n - inset, y), fill=mix((47, 46, 52), (27, 26, 31), t))
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle((inset, inset, n - inset, n - inset), radius, fill=255)
    img.paste(bg, (0, 0), mask)


def annulus(n, cx, cy, r_in, r_out):
    """An L mask for the ring r_in..r_out (r_in 0 gives a full disc)."""
    m = Image.new("L", (n, n), 0)
    md = ImageDraw.Draw(m)
    md.ellipse((cx - r_out, cy - r_out, cx + r_out, cy + r_out), fill=255)
    if r_in:
        md.ellipse((cx - r_in, cy - r_in, cx + r_in, cy + r_in), fill=0)
    return m


def gradient_ring(n, cx, cy, r_in, r_out, top, bottom):
    """A ring with a vertical metal gradient, as its own layer."""
    grad = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    for y in range(n):
        gd.line((0, y, n, y), fill=mix(top, bottom, y / n))
    layer = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    layer.paste(grad, (0, 0), annulus(n, cx, cy, r_in, r_out))
    return layer


def ring(d, cx, cy, r0, r1, segments, rim=False, ramp_steps=16):
    """segments: list of (start_turn, end_turn, color). Drawn outer-edge-lit."""
    for start, end, color in segments:
        if rim:
            # Radial ramp: lit from the rim, painted as growing pieslices that
            # overpaint their inner predecessors.
            top = lighten(color, 0.10)
            for t in range(ramp_steps):
                r = r0 + (r1 - r0) * (t + 1) / ramp_steps
                box = (cx - r, cy - r, cx + r, cy + r)
                d.pieslice(box, start * 360 - 90, end * 360 - 90, fill=mix(color, top, t / max(ramp_steps - 1, 1)))
        else:
            box = (cx - r1, cy - r1, cx + r1, cy + r1)
            d.pieslice(box, start * 360 - 90, end * 360 - 90, fill=color)
    # Carve the inner edge and the seams, down to the well floor.
    d.ellipse((cx - r0, cy - r0, cx + r0, cy + r0), fill=FLOOR + (255,))
    for start, _, _ in segments:
        a = start * 2 * math.pi - math.pi / 2
        d.line(
            (cx + r0 * math.cos(a), cy + r0 * math.sin(a), cx + r1 * math.cos(a), cy + r1 * math.sin(a)),
            fill=FLOOR + (255,),
            width=5 * SCALE,
        )


def arcs(count, span, offset=0.0):
    """`count` evenly spaced arcs of `span` turns each."""
    pitch = 1.0 / count
    return [(offset + i * pitch, offset + i * pitch + span) for i in range(count)]


def bevel_rim(img, cx, cy, r_in, r_out):
    """A chunky chrome rim: dark cut at the edge, metal face, lit top lip."""
    n = img.width
    d = ImageDraw.Draw(img)
    d.ellipse((cx - r_out, cy - r_out, cx + r_out, cy + r_out), outline=EDGE + (255,), width=3 * SCALE)
    img.alpha_composite(gradient_ring(n, cx, cy, r_in, r_out - 3 * SCALE, STEEL_HI, STEEL_LO))
    # Light catches the top lip; shade pools along the bottom lip.
    lip = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ld = ImageDraw.Draw(lip)
    r_edge = r_out - 3 * SCALE
    ld.arc((cx - r_edge, cy - r_edge, cx + r_edge, cy + r_edge), 185, 355, fill=(255, 255, 255, 120), width=3 * SCALE)
    ld.arc((cx - r_edge, cy - r_edge, cx + r_edge, cy + r_edge), 5, 175, fill=(0, 0, 0, 120), width=3 * SCALE)
    img.alpha_composite(lip.filter(ImageFilter.GaussianBlur(1.5 * SCALE)))
    # The inner lip drops into the well.
    inner = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(inner).arc(
        (cx - r_in, cy - r_in, cx + r_in, cy + r_in), 5, 175, fill=(0, 0, 0, 140), width=3 * SCALE
    )
    img.alpha_composite(inner)


def screw(img, cx, cy, r, slot_deg):
    """A chassis screw: shaded head, a slot, a crescent of light."""
    n = img.width
    img.alpha_composite(gradient_ring(n, cx, cy, 0, r, STEEL_HI, STEEL_LO))
    d = ImageDraw.Draw(img)
    d.ellipse((cx - r, cy - r, cx + r, cy + r), outline=EDGE + (255,), width=2 * SCALE)
    a = math.radians(slot_deg)
    d.line(
        (cx - 0.62 * r * math.cos(a), cy - 0.62 * r * math.sin(a), cx + 0.62 * r * math.cos(a), cy + 0.62 * r * math.sin(a)),
        fill=(40, 42, 48, 255),
        width=2 * SCALE,
    )
    hl = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(hl).arc(
        (cx - r + 2 * SCALE, cy - r + 2 * SCALE, cx + r - 2 * SCALE, cy + r - 2 * SCALE), 185, 355,
        fill=(255, 255, 255, 90),
        width=2 * SCALE,
    )
    img.alpha_composite(hl)


def gloss(img, cx, cy, r_in, r_out):
    """A vertical sheen over an annulus — light from above, shade below — so
    the brooms read as glossy plastic under studio light."""
    n = img.width
    zone = annulus(n, cx, cy, r_in, r_out)
    for color, hi, lo in (((255, 255, 255), 76, 0), ((0, 0, 0), 0, 64)):
        rows = Image.new("L", (n, n), 0)
        rd = ImageDraw.Draw(rows)
        for y in range(n):
            t = y / n
            a = int(hi * max(0.0, 1 - 2 * t) + lo * max(0.0, 2 * t - 1))
            if a:
                rd.line((0, y, n, y), fill=a)
        veil = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        veil.paste(color + (255,), (0, 0), ImageChops.multiply(rows, zone))
        img.alpha_composite(veil)


def draw_tracks(img, cx, cy, radii):
    """Faint full-circle data tracks over the brooms (alpha, so a layer)."""
    overlay = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    od = ImageDraw.Draw(overlay)
    for r in radii:
        box = (cx - r, cy - r, cx + r, cy + r)
        od.arc(box, 0, 360, fill=(16, 17, 21, 80), width=2 * SCALE)
    img.alpha_composite(overlay)


def glass(img, cx, cy, r_mid, width):
    """A soft specular streak across the upper-left: a cover glass over the data."""
    spec = Image.new("RGBA", (img.width, img.width), (0, 0, 0, 0))
    ImageDraw.Draw(spec).arc(
        (cx - r_mid, cy - r_mid, cx + r_mid, cy + r_mid), 195, 265, fill=(255, 255, 255, 36), width=width
    )
    img.alpha_composite(spec.filter(ImageFilter.GaussianBlur(4 * SCALE)))


def draw_spindle(img, d, cx, cy, ring_r_in, ring_r_out, dome_r, hole_r):
    """The hub: metal collar, then a dome lit from the top-left, punched hole."""
    n = img.width
    img.alpha_composite(gradient_ring(n, cx, cy, ring_r_in, ring_r_out, STEEL_HI, STEEL_LO))
    # The dome sits proud of the floor: a contact shadow first.
    sh = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    pad = 5 * SCALE
    ImageDraw.Draw(sh).ellipse((cx - dome_r - pad, cy - dome_r - pad, cx + dome_r + pad, cy + dome_r + pad), fill=(0, 0, 0, 90))
    img.alpha_composite(sh.filter(ImageFilter.GaussianBlur(4 * SCALE)))
    # Sphere shading: dark rim first, then circles growing brighter and
    # drifting toward the light. The punch stays small enough that the lit
    # arc survives around it.
    for t in range(28):
        k = t / 27
        rr = dome_r * (1 - 0.72 * k)
        off = 12 * SCALE * k
        d.ellipse(
            (cx - rr - 0.7 * off, cy - rr - 0.7 * off, cx + rr - 0.7 * off, cy + rr - 0.7 * off),
            fill=mix((56, 58, 66), (212, 216, 224), k),
        )
    # A punch through the dome, with a crescent of light on its upper edge.
    d.ellipse((cx - hole_r, cy - hole_r, cx + hole_r, cy + hole_r), fill=HOLE + (255,))
    overlay = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(overlay).arc(
        (cx - hole_r, cy - hole_r, cx + hole_r, cy + hole_r), 180, 360, fill=(255, 255, 255, 60), width=2 * SCALE
    )
    img.alpha_composite(overlay)
    # And one tight specular on the dome itself.
    dot = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    r_dot = 0.16 * dome_r
    dx, dy = cx - 0.42 * dome_r, cy - 0.42 * dome_r
    ImageDraw.Draw(dot).ellipse((dx - r_dot, dy - r_dot, dx + r_dot, dy + r_dot), fill=(255, 255, 255, 190))
    img.alpha_composite(dot.filter(ImageFilter.GaussianBlur(2 * SCALE)))


def draw_arm(img, cx, cy, r_head, r_pivot):
    """The actuator arm: chrome, floating above the platter, pivot on a screw."""
    ang = math.radians(38)  # sweeping in from the lower-right
    dx, dy = math.cos(ang), math.sin(ang)
    p1 = (cx + dx * r_pivot, cy + dy * r_pivot)  # pivot
    p2 = (cx + dx * r_head, cy + dy * r_head)  # head
    px, py = -dy, dx  # unit perpendicular
    w1, w2 = 24 * SCALE, 15 * SCALE  # half-widths: taper to the head
    ends = [(p1, w1), (p2, w2)]
    poly = [
        (p1[0] + px * w1, p1[1] + py * w1),
        (p2[0] + px * w2, p2[1] + py * w2),
        (p2[0] - px * w2, p2[1] - py * w2),
        (p1[0] - px * w1, p1[1] - py * w1),
    ]

    shape = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shape)
    sd.polygon(poly, fill=(255, 255, 255, 255))
    for (pxc, pyc), w in ends:
        sd.ellipse((pxc - w, pyc - w, pxc + w, pyc + w), fill=(255, 255, 255, 255))

    # Its shadow on the platter — it floats a little higher now.
    shadow = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 125), (0, 0), shape.split()[3])
    shadow = ImageChops.offset(shadow, 7 * SCALE, 13 * SCALE)
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(7 * SCALE)))

    # The arm itself: chrome body — a vertical gradient cut to its shape.
    grad = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    for y in range(img.height):
        gd.line((0, y, img.width, y), fill=mix(STEEL_HI, STEEL, y / img.height))
    arm = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    arm.paste(grad, (0, 0), shape.split()[3])
    ad = ImageDraw.Draw(arm)
    ad.line((poly[0][0], poly[0][1], poly[1][0], poly[1][1]), fill=(255, 255, 255, 220), width=2 * SCALE)
    ad.line((poly[3][0], poly[3][1], poly[2][0], poly[2][1]), fill=(40, 42, 48, 200), width=2 * SCALE)
    # The head, reading: a chrome block with the app's blue orb.
    hb = w2 + 4 * SCALE
    head_mask = Image.new("L", (img.width, img.height), 0)
    ImageDraw.Draw(head_mask).ellipse((p2[0] - hb, p2[1] - hb, p2[0] + hb, p2[1] + hb), fill=255)
    arm.paste(grad, (0, 0), head_mask)
    ad.ellipse((p2[0] - hb, p2[1] - hb, p2[0] + hb, p2[1] + hb), outline=EDGE + (255,), width=2 * SCALE)
    orb = 8 * SCALE
    ad.ellipse((p2[0] - orb, p2[1] - orb, p2[0] + orb, p2[1] + orb), fill=ACCENT + (255,))
    img.alpha_composite(arm)
    # The pivot: a screw, machined like the rim's.
    screw(img, p1[0], p1[1], w1, 20)
    # A specular on the blue orb, cut to its circle.
    orb_hi = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
    od = ImageDraw.Draw(orb_hi)
    r_hi = 0.4 * orb
    od.ellipse((p2[0] - orb + 2 * SCALE, p2[1] - orb + 2 * SCALE, p2[0] - orb + 2 * SCALE + 2 * r_hi,
                p2[1] - orb + 2 * SCALE + 2 * r_hi), fill=(255, 255, 255, 210))
    img.alpha_composite(orb_hi.filter(ImageFilter.GaussianBlur(1.5 * SCALE)))


def draw_icon() -> Image.Image:
    n = SIZE * SCALE
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    draw_squircle(img, 100 * SCALE, 185 * SCALE)

    cx = cy = n / 2
    brooms = [(a, b, hsl(HUES[i % 6], 0.64, 0.60)) for i, (a, b) in enumerate(arcs(6, 1.0 / 6 - 0.055, -(1.0 / 6 - 0.055) / 2))]

    # The platter's shadow, then its bevelled rim and chassis screws.
    shadow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).ellipse((cx - 336 * SCALE, cy - 328 * SCALE, cx + 336 * SCALE, cy + 344 * SCALE), fill=(0, 0, 0, 100))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(12 * SCALE)))
    bevel_rim(img, cx, cy, 296 * SCALE, 338 * SCALE)
    for k in range(4):
        a = math.radians(45 + 90 * k)
        screw(img, cx + 317 * SCALE * math.cos(a), cy + 317 * SCALE * math.sin(a), 13 * SCALE, 15 + 20 * k)

    # The well floor, with the rim's shadow cast onto it from above.
    d.ellipse((cx - 296 * SCALE, cy - 296 * SCALE, cx + 296 * SCALE, cy + 296 * SCALE), fill=FLOOR + (255,))
    cast = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    cr = 289 * SCALE
    ImageDraw.Draw(cast).arc((cx - cr, cy - cr, cx + cr, cy + cr), 185, 355, fill=(0, 0, 0, 110), width=8 * SCALE)
    img.alpha_composite(cast.filter(ImageFilter.GaussianBlur(4 * SCALE)))

    ring(d, cx, cy, 152 * SCALE, 288 * SCALE, brooms, rim=True)
    draw_tracks(img, cx, cy, [190 * SCALE, 224 * SCALE, 258 * SCALE])
    gloss(img, cx, cy, 152 * SCALE, 288 * SCALE)
    glass(img, cx, cy, 225 * SCALE, 52 * SCALE)
    draw_spindle(img, d, cx, cy, 108 * SCALE, 132 * SCALE, 108 * SCALE, 20 * SCALE)
    draw_arm(img, cx, cy, 176 * SCALE, 368 * SCALE)

    return img.resize((SIZE, SIZE), Image.LANCZOS)


def draw_icon_small() -> Image.Image:
    """Dock-legible 16/32 px art: platter rim, one bold broom ring, spindle dot."""
    n = SIZE * SCALE
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    draw_squircle(img, 100 * SCALE, 185 * SCALE)

    cx = cy = n / 2
    brooms = [(a, b, hsl(HUES[i % 6], 0.65, 0.68)) for i, (a, b) in enumerate(arcs(6, 1.0 / 6 - 0.05))]
    d.ellipse((cx - 336 * SCALE, cy - 336 * SCALE, cx + 336 * SCALE, cy + 336 * SCALE), outline=STEEL + (255,), width=18 * SCALE)
    ring(d, cx, cy, 150 * SCALE, 310 * SCALE, brooms)
    d.ellipse((cx - 126 * SCALE, cy - 126 * SCALE, cx + 126 * SCALE, cy + 126 * SCALE), fill=(44, 46, 54, 255))
    d.ellipse((cx - 30 * SCALE, cy - 30 * SCALE, cx + 30 * SCALE, cy + 30 * SCALE), fill=HOLE + (255,))

    return img.resize((SIZE, SIZE), Image.LANCZOS)


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out_dir, exist_ok=True)
    icon = draw_icon()
    small = draw_icon_small()
    icon.save(os.path.join(out_dir, "AppIcon.png"))
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
