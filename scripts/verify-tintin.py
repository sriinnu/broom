#!/usr/bin/env python3
"""Probe the rendered concepts rather than trusting the render log.

Checks, per concept: the ink contour is actually present at the expected weight,
the colour field is genuinely flat (ligne claire, not gradients), the six
ladder hues all survive, and the mark is legible at 16 px.
"""
import os
import sys
from collections import Counter

from PIL import Image, ImageDraw

INK_MAX = 90          # a pixel this dark is contour
LADDER = [
    ("rose", (214, 62, 58)),
    ("tangerine", (233, 131, 40)),
    ("gold", (242, 197, 22)),
    ("lime", (92, 168, 52)),
    ("teal", (40, 156, 156)),
    ("periwinkle", (58, 108, 196)),
]


def near(a, b, tol=26):
    return all(abs(x - y) <= tol for x, y in zip(a[:3], b[:3]))


def probe(path):
    img = Image.open(path).convert("RGBA")
    w, h = img.size
    px = img.load()

    ink = 0
    opaque = 0
    hues = Counter()
    # Interior only: skip the squircle border band so we measure the mark.
    lo, hi = int(w * 0.16), int(w * 0.84)
    for y in range(lo, hi):
        for x in range(lo, hi):
            r, g, b, a = px[x, y]
            if a < 128:
                continue
            opaque += 1
            if max(r, g, b) < INK_MAX:
                ink += 1
            for name, ref in LADDER:
                if near((r, g, b), ref):
                    hues[name] += 1
                    break

    # Flatness: how much of the mark is covered by its 40 commonest colours.
    # Gradients smear this out; flat fills keep it high.
    counts = Counter()
    for y in range(lo, hi, 2):
        for x in range(lo, hi, 2):
            r, g, b, a = px[x, y]
            if a >= 128:
                counts[(r // 8, g // 8, b // 8)] += 1
    total = sum(counts.values()) or 1
    top40 = sum(c for _, c in counts.most_common(40)) / total

    # 16px legibility: does anything but background survive the downscale?
    small = img.resize((16, 16), Image.LANCZOS)
    sp = small.load()
    distinct = len({sp[x, y][:3] for y in range(16) for x in range(16)})
    ink16 = sum(1 for y in range(16) for x in range(16) if max(sp[x, y][:3]) < 110)

    return {
        "size": (w, h),
        "ink_pct": 100 * ink / max(opaque, 1),
        "hues": sum(1 for n, _ in LADDER if hues[n] > 400),
        "missing": [n for n, _ in LADDER if hues[n] <= 400],
        "hue_pct": {n: round(100 * hues[n] / max(opaque, 1), 1) for n, _ in LADDER},
        "top40_flatness": round(100 * top40, 1),
        "distinct_16": distinct,
        "ink_16": ink16,
    }


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "packaging/concepts"
    files = sorted(f for f in os.listdir(base) if f.endswith(".png"))
    ok = True
    for f in files:
        r = probe(os.path.join(base, f))
        flat = r["top40_flatness"]
        verdict = []
        if r["ink_pct"] < 3.0:
            verdict.append("NO CONTOUR")
        if flat < 55.0:
            verdict.append(f"NOT FLAT ({flat}%)")
        if r["hues"] < 6:
            verdict.append(f"MISSING HUES {r['missing']}")
        if r["distinct_16"] < 4 or r["ink_16"] < 6:
            verdict.append(f"16PX MUSH (distinct={r['distinct_16']} ink={r['ink_16']})")
        status = "ok" if not verdict else "FAIL: " + "; ".join(verdict)
        if verdict:
            ok = False
        print(f"{f}")
        print(f"  ink {r['ink_pct']:.1f}% · flatness(top40) {flat}% · hues {r['hues']}/6 "
              f"· 16px distinct {r['distinct_16']} ink {r['ink_16']}  -> {status}")
        print(f"  hue coverage: {r['hue_pct']}")

    # The preview must actually contain the shipped icon and the concepts side
    # by side: probe two known regions.
    sheet = "packaging/icon-tintin-preview.png"
    if os.path.exists(sheet):
        im = Image.open(sheet).convert("RGB")
        print(f"\n{sheet} {im.size}")
        d = ImageDraw.Draw(im)
        # Footer row starts at HEAD + ROW*4; the two 256px tiles sit at
        # (380, +60) and (700, +60).
        foot = 148 + 326 * 4 + 60
        counts = Counter(im.crop((380, foot, 636, foot + 256)).getdata())
        shipped_steel = sum(c for (r, g, b), c in counts.items()
                            if abs(r - g) < 14 and abs(g - b) < 16 and 120 < r < 230)
        print(f"  shipped tile: chrome/steel pixels {shipped_steel} "
              f"(expect >4000 for the bevelled icon)")
        counts = Counter(im.crop((700, foot, 956, foot + 256)).getdata())
        flat = sum(c for (r, g, b), c in counts.items() if max(r, g, b) < 95)
        hueful = sum(c for (r, g, b), c in counts.items()
                     if max(r, g, b) - min(r, g, b) > 60)
        print(f"  concept A tile: contour pixels {flat}, saturated pixels {hueful}")
    print("\nOK" if ok else "\nPROBLEMS FOUND")


if __name__ == "__main__":
    main()