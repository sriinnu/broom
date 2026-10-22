#!/usr/bin/env python3
"""Ligne-claire (Tintin-style) icon concepts for Broom.

Flat unmodulated colour, one uniform black contour weight, no gradients, no
gloss, no bevels, no cast shadows. Depth comes from overlap and line, not from
light. The six hues are ACCENT_HUES' positions (0.99, 0.07, 0.13, 0.30, 0.50,
0.62) pushed to album-print saturation, so the mark still reads as Broom's
categorical ladder.

Writes four 1024px concepts into <out>/concepts/ and a contact sheet at
<out>/icon-tintin-preview.png: each concept at real Dock sizes, at 4x/6x zoom,
and beside the shipped chrome icon.

Needs Pillow and macOS's iconutil:
    uv run --with pillow python3 scripts/icon-tintin.py packaging
"""
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

S = 4          # draw at 4096, downsample to 1024: smooth contours, flat fills
OUT = 1024
N = OUT * S

INSET = 100
RADIUS = 185

# The single contour weight. Every shape in every concept uses this and only
# this: that uniformity is the whole look.
LW = 13

INK = (30, 27, 25, 255)
PAPER = (245, 238, 220, 255)
SILVER = (214, 221, 232, 255)
SKY = (186, 224, 238, 255)
WOOD = (198, 140, 62, 255)
FACE = (247, 231, 196, 255)
HUB = (238, 242, 248, 255)
BLUE = (79, 157, 255, 255)
GREEN = (92, 168, 52, 255)

# ACCENT_HUES' six hue positions, pushed to ligne-claire saturation.
HUES = [
    (214, 62, 58, 255),    # rose      0.99
    (233, 131, 40, 255),   # tangerine 0.07
    (242, 197, 22, 255),   # gold      0.13
    (92, 168, 52, 255),    # lime      0.30
    (40, 156, 156, 255),   # teal      0.50
    (58, 108, 196, 255),   # periwinkle 0.62
]


def px(v):
    return int(round(v * S))


def disc(img, cx, cy, r, color):
    ImageDraw.Draw(img).ellipse((px(cx - r), px(cy - r), px(cx + r), px(cy + r)), fill=color)


def ring(img, cx, cy, r, w=LW, color=INK):
    ImageDraw.Draw(img).ellipse(
        (px(cx - r), px(cy - r), px(cx + r), px(cy + r)), outline=color, width=px(w)
    )


def poly(img, pts, color=None, stroke=None, w=LW):
    xy = [(px(x), px(y)) for x, y in pts]
    if color is not None:
        ImageDraw.Draw(img).polygon(xy, fill=color)
    if stroke is not None:
        ImageDraw.Draw(img).line(xy + [xy[0]], fill=stroke, width=px(w), joint="curve")


def squircle(img, fill=PAPER, stroke=INK):
    """A flat album panel: paper inside, one uniform ink border."""
    d = ImageDraw.Draw(img)
    box = (px(INSET), px(INSET), N - px(INSET), N - px(INSET))
    d.rounded_rectangle(box, px(RADIUS), fill=fill)
    d.rounded_rectangle(box, px(RADIUS), outline=stroke, width=px(LW))


def broom_pts(cx, cy, phi_deg, r0, r1, half_deg, steps=72):
    """A leaf-shaped broom running outward along `phi_deg` from r0 to r1."""
    phi = math.radians(phi_deg)
    half = math.radians(half_deg)
    side_a, side_b = [], []
    for i in range(steps + 1):
        t = i / steps
        r = r0 + (r1 - r0) * t
        # Bowed sides, pointed base, rounded tip.
        hw = half * (math.sin(math.pi * t) ** 0.72)
        side_a.append((cx + r * math.cos(phi - hw), cy + r * math.sin(phi - hw)))
        side_b.append((cx + r * math.cos(phi + hw), cy + r * math.sin(phi + hw)))
    return side_a + side_b[::-1]


def broom_ring(img, cx, cy, r0, r1, half_deg, colors=HUES, count=6, offset=-90.0):
    """Six brooms, splits showing paper between them. Fills first, then every
    contour: that ordering is what keeps the line weight uniform where shapes
    meet."""
    shapes = [
        broom_pts(cx, cy, offset + i * (360.0 / count), r0, r1, half_deg)
        for i in range(count)
    ]
    for shape, color in zip(shapes, colors):
        poly(img, shape, color=color)
    for shape in shapes:
        poly(img, shape, stroke=INK)


def arm(img, cx, cy, ang_deg, r_pivot, r_head, w1, w2, fill, head_dot=None):
    """A tapered bar from pivot to head, both ends rounded, one contour."""
    a = math.radians(ang_deg)
    dx, dy = math.cos(a), math.sin(a)
    p1 = (cx + dx * r_pivot, cy + dy * r_pivot)
    p2 = (cx + dx * r_head, cy + dy * r_head)
    ux, uy = -dy, dx
    quad = [
        (p1[0] + ux * w1, p1[1] + uy * w1),
        (p2[0] + ux * w2, p2[1] + uy * w2),
        (p2[0] - ux * w2, p2[1] - uy * w2),
        (p1[0] - ux * w1, p1[1] - uy * w1),
    ]
    d = ImageDraw.Draw(img)
    d.polygon([(px(x), px(y)) for x, y in quad], fill=fill)
    for (qx, qy), w in ((p1, w1), (p2, w2)):
        disc(img, qx, qy, w, fill)
    # Contour: two long sides, plus each cap as its own circle.
    d.line(
        [(px(quad[0][0]), px(quad[0][1])), (px(quad[1][0]), px(quad[1][1]))],
        fill=INK, width=px(LW),
    )
    d.line(
        [(px(quad[3][0]), px(quad[3][1])), (px(quad[2][0]), px(quad[2][1]))],
        fill=INK, width=px(LW),
    )
    ring(img, p1[0], p1[1], w1)
    ring(img, p2[0], p2[1], w2)
    if head_dot is not None:
        disc(img, p2[0], p2[1], head_dot, BLUE)
        ring(img, p2[0], p2[1], head_dot)


# --- Concept A: the shipped idea, drawn flat ---------------------------------
def concept_a():
    """Platter, broom ring, spindle, actuator arm — the current composition,
    with the chrome and the glass taken out."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    squircle(img)
    cx = cy = 512.0

    disc(img, cx, cy, 360, SILVER)
    ring(img, cx, cy, 360)

    broom_ring(img, cx, cy, 158, 306, 25)

    disc(img, cx, cy, 164, HUB)
    ring(img, cx, cy, 164)
    disc(img, cx, cy, 42, INK)

    arm(img, cx, cy, 44, 322, 186, 34, 22, SILVER, head_dot=20)
    return img.resize((OUT, OUT), Image.LANCZOS)


# --- Concept B: the investigator ---------------------------------------------
def concept_b():
    """A magnifier over the rosette. Tintin's trade was looking closely at
    things; so is Broom's."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    squircle(img)
    cx, cy = 486.0, 470.0

    # Handle first, then the lens over its inner end, so the lens contour
    # closes cleanly against the glass.
    arm(img, cx, cy, 45, 300, 540, 40, 30, WOOD)

    disc(img, cx, cy, 300, SKY)
    ring(img, cx, cy, 300, w=16)

    broom_ring(img, cx, cy, 128, 250, 25)
    disc(img, cx, cy, 134, PAPER)
    ring(img, cx, cy, 134)
    disc(img, cx, cy, 34, INK)

    # The glass catches a flat, hard-edged wedge of light. Ligne claire has no
    # soft sheen, so this is a shape, not a blur.
    a0, a1 = math.radians(196), math.radians(268)
    wedge = [(cx, cy)]
    for i in range(17):
        a = a0 + (a1 - a0) * i / 16
        wedge.append((cx + 288 * math.cos(a), cy + 288 * math.sin(a)))
    poly(img, wedge, color=(255, 255, 255, 46))
    return img.resize((OUT, OUT), Image.LANCZOS)


# --- Concept C: the mark ------------------------------------------------------
def concept_c():
    """Brooms only. The in-app rosette given a contour — the tightest mark, and
    the one with the best chance at 16px."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    squircle(img)
    cx = cy = 512.0
    broom_ring(img, cx, cy, 96, 372, 26)
    disc(img, cx, cy, 102, PAPER)
    ring(img, cx, cy, 102)
    disc(img, cx, cy, 46, INK)
    return img.resize((OUT, OUT), Image.LANCZOS)


# --- Concept D: the platter, awake -------------------------------------------
def concept_d():
    """The fun one: a platter with a face, the actuator as a stem, a leaf. Still
    flat, still one line weight."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    squircle(img)
    cx, cy = 512.0, 430.0

    # Stem and leaf first, then the head over them.
    poly(img, [(cx - 26, cy + 120), (cx + 26, cy + 120), (cx + 26, 858), (cx - 26, 858)],
         color=GREEN, stroke=INK)
    poly(img, [(cx - 8, 730), (cx - 150, 660), (cx - 196, 726), (cx - 40, 792)],
         color=GREEN, stroke=INK)

    broom_ring(img, cx, cy, 148, 330, 25)

    disc(img, cx, cy, 176, FACE)
    ring(img, cx, cy, 176)
    disc(img, cx - 58, cy - 26, 27, INK)
    disc(img, cx + 58, cy - 26, 27, INK)
    ImageDraw.Draw(img).arc(
        (px(cx - 74), px(cy - 16), px(cx + 74), px(cy + 92)),
        20, 160, fill=INK, width=px(LW),
    )
    return img.resize((OUT, OUT), Image.LANCZOS)


CONCEPTS = [
    ("A", "The platter, flat", "the shipped composition, chrome removed", concept_a),
    ("B", "The investigator", "a magnifier over the rosette", concept_b),
    ("C", "The mark", "brooms only · the in-app rosette, contoured", concept_c),
    ("D", "The platter, awake", "the fun one · a face, the arm as a stem", concept_d),
]


# --- contact sheet ------------------------------------------------------------
def font(size, bold=False):
    names = (
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNS.ttf",
    ) if bold else (
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNS.ttf",
    )
    for path in names:
        try:
            return ImageFont.truetype(path, size)
        except Exception:
            pass
    return ImageFont.load_default()


def resample():
    return getattr(Image, "Resampling", Image).LANCZOS


def nearest():
    return getattr(Image, "Resampling", Image).NEAREST


BG = (26, 27, 31, 255)
FG = (233, 234, 237, 255)
MUTED = (140, 144, 153, 255)
RULE = (48, 50, 56, 255)


def sheet(concepts, shipped, out_path):
    W = 1600
    HEAD = 148
    ROW = 326
    FOOT = 430
    H = HEAD + ROW * len(concepts) + FOOT
    img = Image.new("RGBA", (W, H), BG)
    d = ImageDraw.Draw(img)

    d.text((48, 44), "Broom — ligne claire concepts", font=font(34, True), fill=FG)
    d.text((48, 90), "flat colour · one contour weight · no gradients, gloss or bevel",
           font=font(16), fill=MUTED)
    d.text((1150, 52), "actual Dock sizes →", font=font(15), fill=MUTED)
    d.text((1150, 90), "← zoomed 4x / 6x", font=font(15), fill=MUTED)

    ladder = [256, 128, 64, 32, 16]
    ladder_x = [380, 676, 812, 884, 924]
    zoom_x = [1010, 1160]

    for row, (key, name, blurb, art) in enumerate(concepts):
        top = HEAD + row * ROW
        d.text((48, top + 100), key, font=font(40, True), fill=FG)
        d.text((100, top + 104), name, font=font(23, True), fill=FG)
        d.text((100, top + 138), blurb, font=font(15), fill=MUTED)

        for size, x in zip(ladder, ladder_x):
            small = art.resize((size, size), resample())
            img.alpha_composite(small, (x, top + (ROW - size) // 2))

        for size, mult, x in ((32, 4, zoom_x[0]), (16, 6, zoom_x[1])):
            small = art.resize((size, size), resample())
            big = small.resize((size * mult, size * mult), nearest())
            img.alpha_composite(big, (x, top + (ROW - big.height) // 2))

        d.line([(48, top + ROW - 1), (W - 48, top + ROW - 1)], fill=RULE, width=1)

    top = HEAD + ROW * len(concepts)
    d.text((48, top + 34), "shipped", font=font(23, True), fill=FG)
    d.text((48, top + 68), "chrome, gel, glass, cast shadows\n— a different app's vocabulary",
           font=font(15), fill=MUTED)
    img.alpha_composite(shipped.resize((256, 256), resample()), (380, top + 60))

    d.text((700, top + 34), "concept A", font=font(23, True), fill=FG)
    d.text((700, top + 68), "same subject, flat and contoured\n— sits beside the real UI",
           font=font(15), fill=MUTED)
    img.alpha_composite(concepts[0][3].resize((256, 256), resample()), (700, top + 60))

    img.convert("RGB").save(out_path)
    return out_path


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    concepts_dir = os.path.join(out_dir, "concepts")
    os.makedirs(concepts_dir, exist_ok=True)

    art = []
    for key, name, blurb, draw in CONCEPTS:
        image = draw()
        slug = {"A": "a-platter", "B": "b-investigator", "C": "c-mark", "D": "d-awake"}[key]
        image.save(os.path.join(concepts_dir, f"tintin-{slug}.png"))
        art.append((key, name, blurb, image))
        print(f"wrote {concepts_dir}/tintin-{slug}.png")

    shipped_path = os.path.join(out_dir, "AppIcon.png")
    shipped = Image.open(shipped_path).convert("RGBA")

    path = sheet(art, shipped, os.path.join(out_dir, "icon-tintin-preview.png"))
    print(f"wrote {path}")


if __name__ == "__main__":
    main()