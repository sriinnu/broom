#!/usr/bin/env python3
"""Broom mark, second spin: ligne claire as *covers*, not emblems.

Round one produced four floating emblems on a paper tile. Hergé's album covers
are scenes: a flat sky, a ground line, a single bold subject, a tiny figure for
scale, and a straight-faced gag. This spin keeps the rules that define ligne
claire -- flat unmodulated fills, one uniform ink contour, no gradients, no
gloss, no bevels, no cast shadows, no alpha for "glass" -- and spends them on
compositions instead.

Writes four 1024px concepts into <out>/spin2/ and a contact sheet at
<out>/icon-tintin-spin2-preview.png.

    uv run --with pillow python3 -I scripts/icon-tintin-spin2.py packaging
"""
import math
import os
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFont

S = 4          # draw at 4096, downsample: smooth contours, still flat fills
OUT = 1024
N = OUT * S

INSET = 100
RADIUS = 185
LW = 14        # the single contour weight. Every shape, and only this.
PAD_D = 20     # local-layer bleed so a stroke never clips

INK = (26, 24, 22, 255)
CREAM = (247, 238, 216, 255)
PAPER = (252, 250, 244, 255)
INDIGO = (38, 52, 92, 255)
FLOOR = (26, 36, 66, 255)
SKY = (152, 198, 226, 255)
BRICK = (178, 64, 50, 255)
OCHRE = (216, 160, 54, 255)
OLIVE = (124, 140, 70, 255)
TEAL = (48, 124, 122, 255)
SLATE = (178, 188, 202, 255)
SLATE_D = (122, 134, 150, 255)
PERI = (74, 104, 168, 255)


# --- primitives (device space, then OUT space) --------------------------------
def px(v):
    return int(round(v * S))


def dpoly(img, pts, fill=None, stroke=None, w=LW):
    d = ImageDraw.Draw(img)
    if fill is not None:
        d.polygon(pts, fill=fill)
    if stroke is not None:
        d.line(list(pts) + [pts[0]], fill=stroke, width=px(w), joint="curve")


def poly(img, pts, fill=None, stroke=None, w=LW):
    dpoly(img, [(px(x), px(y)) for x, y in pts], fill, stroke, w)


def ddisc(img, cx, cy, r, fill=None, stroke=None, w=LW):
    d = ImageDraw.Draw(img)
    box = (cx - r, cy - r, cx + r, cy + r)
    if fill is not None:
        d.ellipse(box, fill=fill)
    if stroke is not None:
        d.ellipse(box, outline=stroke, width=px(w))


def disc(img, cx, cy, r, fill=None, stroke=None, w=LW):
    ddisc(img, px(cx), px(cy), px(r), fill, stroke, w)


def dellipse(img, cx, cy, rx, ry, fill=None, stroke=None, w=LW):
    d = ImageDraw.Draw(img)
    box = (cx - rx, cy - ry, cx + rx, cy + ry)
    if fill is not None:
        d.ellipse(box, fill=fill)
    if stroke is not None:
        d.ellipse(box, outline=stroke, width=px(w))


def rect(img, x0, y0, x1, y1, fill=None, stroke=None, rad=0, w=LW):
    d = ImageDraw.Draw(img)
    box = (px(x0), px(y0), px(x1), px(y1))
    if fill is not None:
        d.rounded_rectangle(box, px(rad), fill=fill)
    if stroke is not None:
        d.rounded_rectangle(box, px(rad), outline=stroke, width=px(w))


def dline(img, pts, w=LW, fill=INK):
    ImageDraw.Draw(img).line(pts, fill=fill, width=px(w))


def arc(img, x0, y0, x1, y1, start, end, w=LW, fill=INK):
    ImageDraw.Draw(img).arc((px(x0), px(y0), px(x1), px(y1)), start, end, fill=fill, width=px(w))


# --- panel ------------------------------------------------------------------
def squircle_mask():
    m = Image.new("L", (N, N), 0)
    ImageDraw.Draw(m).rounded_rectangle(
        (px(INSET), px(INSET), N - px(INSET), N - px(INSET)), px(RADIUS), fill=255
    )
    return m


def scene_bg(img, sky, ground=None, horizon=0.80):
    """A flat field, and optionally a flat ground with a contour line."""
    mask = squircle_mask()
    img.paste(Image.new("RGBA", (N, N), sky), (0, 0), mask)
    if ground is not None:
        y = px(INSET + (OUT - 2 * INSET) * horizon)
        gm = Image.new("L", (N, N), 0)
        ImageDraw.Draw(gm).rectangle((px(INSET), y, N - px(INSET), N - px(INSET)), fill=255)
        gm = ImageChops.multiply(gm, mask)
        img.paste(Image.new("RGBA", (N, N), ground), (0, 0), gm)
        dline(img, [(px(INSET), y), (N - px(INSET), y)])
    return INSET + (OUT - 2 * INSET) * horizon


def border(img):
    ImageDraw.Draw(img).rounded_rectangle(
        (px(INSET), px(INSET), N - px(INSET), N - px(INSET)), px(RADIUS),
        outline=INK, width=px(LW),
    )


def place(img, layer, cx, cy, angle=0.0):
    lay = layer.rotate(-angle, expand=True, resample=Image.BICUBIC) if angle else layer
    img.alpha_composite(lay, (int(px(cx) - lay.width / 2), int(px(cy) - lay.height / 2)))


# --- props, built in their own local space so they can be tilted ---------------
def layer_canvas(w, h):
    return Image.new("RGBA", (px(w) + 2 * px(PAD_D), px(h) + 2 * px(PAD_D)), (0, 0, 0, 0))


def folder_layer(fill, w=190, h=148, tabw=None, tabh=None):
    tabw = w * 0.42 if tabw is None else tabw
    tabh = h * 0.22 if tabh is None else tabh
    lay = layer_canvas(w, h)
    p = px(PAD_D)
    dpoly(lay, [
        (p, p + px(tabh)), (p + px(tabw), p + px(tabh)), (p + px(tabw), p),
        (p + px(w), p), (p + px(w), p + px(h)), (p, p + px(h)),
    ], fill=fill, stroke=INK)
    return lay


def photo_layer(w=176, h=138):
    lay = layer_canvas(w, h)
    p = px(PAD_D)
    dpoly(lay, [(p, p), (p + px(w), p), (p + px(w), p + px(h)), (p, p + px(h))],
          fill=CREAM, stroke=INK)
    i = px(w * 0.11)
    x0, y0, x1, y1 = p + i, p + i, p + px(w) - i, p + px(h) - i
    dpoly(lay, [(x0, y0), (x1, y0), (x1, y1), (x0, y1)], fill=SKY, stroke=INK)
    dpoly(lay, [(x0, y1), ((x0 + x1) // 2, int(y0 + (y1 - y0) * 0.40)), (x1, y1)],
          fill=OLIVE, stroke=INK)
    ddisc(lay, int(x1 - (x1 - x0) * 0.24), int(y0 + (y1 - y0) * 0.26), px(15),
          fill=OCHRE, stroke=INK)
    return lay


def ball_layer(r=64):
    lay = layer_canvas(r * 2, r * 2)
    p = px(PAD_D)
    ddisc(lay, p + px(r), p + px(r), px(r), fill=BRICK, stroke=INK)
    return lay


def brow_layer(w=382, h=70, rad=34):
    lay = layer_canvas(w, h)
    p = px(PAD_D)
    d = ImageDraw.Draw(lay)
    d.rounded_rectangle((p, p, p + px(w), p + px(h)), px(rad), fill=SLATE_D, outline=INK,
                        width=px(LW))
    return lay


# --- E: the drawer, erupting ---------------------------------------------------
def concept_drawer():
    """A filing cabinet with the top drawer blown open. Says "full of stuff",
    and gives the mark a joke."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    scene_bg(img, INDIGO, FLOOR, 0.80)

    # Bigger shapes, thinner contour, so ink stops swamping the colour field.
    rect(img, 336, 372, 688, 766, fill=SLATE, stroke=INK, rad=26, w=10)
    rect(img, 380, 404, 644, 500, fill=FLOOR, stroke=INK, rad=14, w=10)     # cavity
    for y0, y1 in ((520, 630), (664, 754)):
        rect(img, 380, y0, 644, y1, fill=SLATE_D, stroke=INK, rad=14, w=10)
        dline(img, [(px(486), px((y0 + y1) / 2)), (px(538), px((y0 + y1) / 2))], w=10)

    place(img, folder_layer(OCHRE, 210, 160), 396, 206, -16)
    place(img, photo_layer(150, 118), 608, 196, 12)
    place(img, ball_layer(52), 686, 306, 0)

    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


# --- F: the mountain -----------------------------------------------------------
def concept_mountain():
    """An album-cover landscape where the mountain is folders, and one very small
    explorer stands at the foot of it."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    base = scene_bg(img, SKY, OLIVE, 0.80)

    disc(img, 802, 250, 74, fill=OCHRE, stroke=INK)
    for cx, cy, s in ((298, 218, 1.0), (688, 336, 0.70)):
        for dx, dy, rr in ((-72, 10, 46), (0, 0, 58), (68, 12, 44)):
            disc(img, cx + dx * s, cy + dy * s, rr * s, fill=CREAM, stroke=INK)

    cx = 462
    y = base
    for w, c in ((644, OLIVE), (552, OCHRE), (456, BRICK), (356, TEAL), (252, CREAM)):
        h = 112
        rect(img, cx - w / 2, y - h, cx + w / 2, y, fill=c, stroke=INK, rad=16)
        rect(img, cx - w / 2, y - h - 26, cx - w / 2 + w * 0.30, y - h, fill=c,
             stroke=INK, rad=10)
        y -= h

    fx = 808
    poly(img, [(fx - 30, base), (fx + 30, base), (fx + 23, base - 86), (fx - 23, base - 86)],
         fill=BRICK, stroke=INK)
    disc(img, fx, base - 108, 28, fill=CREAM, stroke=INK)
    dellipse(img, px(fx), px(base - 126), px(54), px(15), fill=INK)
    d = ImageDraw.Draw(img)
    d.pieslice((px(fx - 31), px(base - 157), px(fx + 31), px(base - 95)), 180, 360, fill=INK)

    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


# --- G: the unblinking drive ----------------------------------------------------
def concept_eye():
    """The platter is the eye. One big shape, one pupil, a heavy lid, a flat mouth.
    Deadpan, and it still reads at 16px because there is only one thing to read."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    scene_bg(img, OCHRE)

    rect(img, 236, 296, 788, 764, fill=SLATE, stroke=INK, rad=64)

    disc(img, 512, 520, 172, fill=PAPER, stroke=INK)
    disc(img, 512, 520, 92, fill=PERI, stroke=INK)
    disc(img, 512, 520, 42, fill=INK, stroke=INK)
    disc(img, 484, 492, 19, fill=PAPER)                                    # flat, contourless

    place(img, brow_layer(), 512, 334, -6)

    dline(img, [(px(468), px(674)), (px(556), px(674))])
    for sx in (302, 722):
        disc(img, sx, 718, 19, fill=SLATE_D, stroke=INK)

    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


# --- H: exploded ---------------------------------------------------------------
def concept_exploded():
    """Hergé drew cutaways and exploded machines with real relish. Broom's whole
    job is taking a disk apart, so here the disk is already apart."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    scene_bg(img, CREAM)

    rect(img, 200, 650, 824, 790, fill=SLATE_D, stroke=INK, rad=44)         # base
    rect(img, 560, 676, 700, 758, fill=TEAL, stroke=INK, rad=12)           # chip
    for k in range(3):
        dline(img, [(px(248), px(692 + k * 26)), (px(432), px(692 + k * 26))])

    dellipse(img, px(512), px(500), px(250), px(90), fill=SLATE, stroke=INK)  # platter
    dellipse(img, px(512), px(500), px(176), px(63), fill=None, stroke=INK)
    dellipse(img, px(512), px(500), px(104), px(37), fill=None, stroke=INK)
    dellipse(img, px(512), px(500), px(46), px(17), fill=INK)               # spindle

    rect(img, 236, 210, 788, 356, fill=SLATE, stroke=INK, rad=44)           # cover
    for hx in (306, 718):
        for hy in (254, 314):
            disc(img, hx, hy, 18, fill=INK)

    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


CONCEPTS = [
    ("E", "The drawer, erupting", "a cabinet coming apart · the gag",
     concept_drawer, "drawer"),
    ("F", "The mountain", "a landscape made of folders · the cover",
     concept_mountain, "mountain"),
    ("G", "The unblinking drive", "one big eye, deadpan · the character",
     concept_eye, "eye"),
    ("H", "Exploded", "the drive taken apart · the cutaway",
     concept_exploded, "exploded"),
]


# --- Dock-legible 16/32px art, drawn rather than downscaled --------------------
def small_drawer():
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    scene_bg(img, INDIGO, FLOOR, 0.84)
    rect(img, 300, 410, 724, 830, fill=SLATE, stroke=INK, rad=30)
    rect(img, 344, 700, 680, 800, fill=SLATE_D, stroke=INK, rad=16)
    dline(img, [(px(360), px(560)), (px(664), px(560))])
    place(img, folder_layer(OCHRE, 250, 186), 462, 292, -12)
    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


def small_mountain():
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    base = scene_bg(img, SKY, OLIVE, 0.86)
    disc(img, 806, 300, 86, fill=OCHRE, stroke=INK)
    y = base
    for w, c in ((648, OLIVE), (468, OCHRE), (292, CREAM)):
        rect(img, 462 - w / 2, y - 152, 462 + w / 2, y, fill=c, stroke=INK, rad=18)
        y -= 152
    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


def small_eye():
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    scene_bg(img, OCHRE)
    rect(img, 200, 258, 824, 800, fill=SLATE, stroke=INK, rad=70)
    disc(img, 512, 536, 228, fill=PAPER, stroke=INK)
    disc(img, 512, 536, 120, fill=PERI, stroke=INK)
    disc(img, 512, 536, 52, fill=INK, stroke=INK)
    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


def small_exploded():
    """Needs real colour at 16px: plate, platter, and a coloured chip."""
    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    scene_bg(img, INDIGO)
    rect(img, 232, 190, 792, 330, fill=SLATE, stroke=INK, rad=44, w=10)
    dellipse(img, px(512), px(514), px(252), px(88), fill=OCHRE, stroke=INK, w=10)
    dellipse(img, px(512), px(514), px(118), px(41), fill=INK)
    rect(img, 196, 672, 828, 812, fill=SLATE_D, stroke=INK, rad=44, w=10)
    rect(img, 566, 700, 730, 786, fill=TEAL, stroke=INK, rad=14, w=10)
    border(img)
    return img.resize((OUT, OUT), Image.LANCZOS)


SMALL = {
    "drawer": small_drawer,
    "mountain": small_mountain,
    "eye": small_eye,
    "exploded": small_exploded,
}


# --- contact sheet -------------------------------------------------------------
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


BG = (24, 25, 29, 255)
FG = (233, 234, 237, 255)
MUTED = (142, 146, 155, 255)
RULE = (48, 50, 56, 255)
HEAD = 148
ROW = 326


def sheet(entries, shipped, out_path):
    W = 1600
    H = HEAD + ROW * len(entries) + 430
    img = Image.new("RGBA", (W, H), BG)
    d = ImageDraw.Draw(img)

    d.text((48, 40), "Broom — ligne claire, second spin", font=font(34, True), fill=FG)
    d.text((48, 86), "scenes, not emblems · flat fill · one contour weight · no gradients or gloss",
           font=font(16), fill=MUTED)
    d.text((1148, 52), "actual Dock sizes →", font=font(15), fill=MUTED)
    d.text((1148, 90), "← zoomed 4x / 6x", font=font(15), fill=MUTED)

    ladder = [256, 128, 64, 32, 16]
    ladder_x = [380, 676, 812, 884, 924]
    zoom_x = [1010, 1160]

    for row, (key, name, blurb, art, _slug) in enumerate(entries):
        top = HEAD + row * ROW
        d.text((48, top + 100), key, font=font(40, True), fill=FG)
        d.text((100, top + 100), name, font=font(23, True), fill=FG)
        d.text((100, top + 136), blurb, font=font(15), fill=MUTED)
        for size, x in zip(ladder, ladder_x):
            s = art.resize((size, size), resample())
            img.alpha_composite(s, (x, top + (ROW - size) // 2))
        for size, mult, x in ((32, 4, zoom_x[0]), (16, 6, zoom_x[1])):
            big = art.resize((size, size), resample()).resize((size * mult, size * mult), nearest())
            img.alpha_composite(big, (x, top + (ROW - big.height) // 2))
        d.line([(48, top + ROW - 1), (W - 48, top + ROW - 1)], fill=RULE, width=1)

    top = HEAD + ROW * len(entries)
    d.text((48, top + 34), "shipped", font=font(23, True), fill=FG)
    d.text((48, top + 68), "chrome, gel, glass, cast shadow\n— a different app's vocabulary",
           font=font(15), fill=MUTED)
    img.alpha_composite(shipped.resize((256, 256), resample()), (380, top + 60))

    d.text((700, top + 34), "concept G", font=font(23, True), fill=FG)
    d.text((700, top + 68), "one silhouette, one pupil\n— legible before it is clever",
           font=font(15), fill=MUTED)
    img.alpha_composite(entries[2][3].resize((256, 256), resample()), (700, top + 60))

    img.convert("RGB").save(out_path)
    return out_path


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else "."
    concepts_dir = os.path.join(out_dir, "spin2")
    os.makedirs(concepts_dir, exist_ok=True)

    entries = []
    for key, name, blurb, draw, slug in CONCEPTS:
        full = draw()
        art = SMALL[slug]() if False else full
        full.save(os.path.join(concepts_dir, f"spin2-{slug}.png"))
        SMALL[slug]().save(os.path.join(concepts_dir, f"spin2-{slug}-small.png"))
        entries.append((key, name, blurb, art, slug))
        print(f"wrote {concepts_dir}/spin2-{slug}.png (+ -small)")

    shipped = Image.open(os.path.join(out_dir, "AppIcon.png")).convert("RGBA")
    print(f"wrote {sheet(entries, shipped, os.path.join(out_dir, 'icon-tintin-spin2-preview.png'))}")


if __name__ == "__main__":
    main()