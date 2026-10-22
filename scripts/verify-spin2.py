#!/usr/bin/env python3
"""Probe the spin-2 renders against the palette they actually use.

Round one asserted the six ACCENT_HUES; spin 2 is an album palette, so that
check is meaningless here. What matters instead: real contour presence, flat
fill (no gradient smearing, and NO alpha pretending to be glass), the album
colours surviving, and 16px legibility for each concept AND its small art.
"""
import os
import sys
from collections import Counter

from PIL import Image

INK_MAX = 92

ALBUM = [
    ("cream", (247, 238, 216)),
    ("paper", (252, 250, 244)),
    ("indigo", (38, 52, 92)),
    ("floor", (26, 36, 66)),
    ("sky", (152, 198, 226)),
    ("brick", (178, 64, 50)),
    ("ochre", (216, 160, 54)),
    ("olive", (124, 140, 70)),
    ("teal", (48, 124, 122)),
    ("slate", (178, 188, 202)),
    ("slate_d", (122, 134, 150)),
    ("peri", (74, 104, 168)),
]
# A concept should carry at least this many album colours to read as a scene.
MIN_ALBUM = 4


def near(a, b, tol=24):
    return all(abs(x - y) <= tol for x, y in zip(a[:3], b[:3]))


def probe(path):
    img = Image.open(path).convert("RGBA")
    w, h = img.size
    px = img.load()

    lo, hi = int(w * 0.15), int(w * 0.85)
    ink = opaque = alpha_edge = 0
    album = Counter()
    for y in range(lo, hi):
        for x in range(lo, hi):
            r, g, b, a = px[x, y]
            if a < 250:
                alpha_edge += 1
                continue
            opaque += 1
            if max(r, g, b) < INK_MAX:
                ink += 1
            for name, ref in ALBUM:
                if near((r, g, b), ref):
                    album[name] += 1
                    break

    # Flatness: share of the panel covered by its 40 commonest quantised colours.
    counts = Counter()
    for y in range(lo, hi, 2):
        for x in range(lo, hi, 2):
            r, g, b, a = px[x, y]
            if a >= 250:
                counts[(r // 8, g // 8, b // 8)] += 1
    total = sum(counts.values()) or 1
    flat = 100 * sum(c for _, c in counts.most_common(40)) / total

    small = img.resize((16, 16), Image.LANCZOS)
    sp = small.load()
    distinct = len({sp[x, y][:3] for y in range(16) for x in range(16)})
    ink16 = sum(1 for y in range(16) for x in range(16) if max(sp[x, y][:3]) < 115)
    # Does anything but background survive? count how many of the 256 cells are
    # neither the most common colour nor within 12 of it.
    common = Counter(sp[x, y][:3] for y in range(16) for x in range(16)).most_common(1)
    bg = common[0][0] if common else (0, 0, 0)
    nonbg = sum(1 for y in range(16) for x in range(16)
                if not near(sp[x, y][:3], bg, 12))

    hues = sum(1 for n, _ in ALBUM if album[n] > 400)
    problems = []
    if ink / max(opaque, 1) < 0.03:
        problems.append("NO CONTOUR")
    if flat < 55:
        problems.append(f"NOT FLAT ({flat:.0f}%)")
    if alpha_edge / max(opaque + alpha_edge, 1) > 0.01:
        problems.append(f"ALPHA GLASS ({100*alpha_edge/max(opaque+alpha_edge,1):.1f}%)")
    if hues < MIN_ALBUM:
        problems.append(f"PALETTE THIN ({hues}/{MIN_ALBUM})")
    if nonbg < 45 or ink16 < 8:
        problems.append(f"16PX MUSH (nonbg={nonbg} ink={ink16})")
    return {
        "ink": 100 * ink / max(opaque, 1),
        "flat": flat,
        "alpha": 100 * alpha_edge / max(opaque + alpha_edge, 1),
        "album": hues,
        "distinct16": distinct,
        "ink16": ink16,
        "nonbg": nonbg,
        "problems": problems,
    }


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "packaging/spin2"
    files = sorted(f for f in os.listdir(base) if f.endswith(".png"))
    bad = 0
    for f in files:
        r = probe(os.path.join(base, f))
        tag = "ok" if not r["problems"] else "FAIL: " + "; ".join(r["problems"])
        if r["problems"]:
            bad += 1
        print(f"{f}")
        print(f"  ink {r['ink']:.1f}% · flat {r['flat']:.1f}% · alpha {r['alpha']:.2f}% "
              f"· album {r['album']}/12 · 16px nonbg {r['nonbg']} ink {r['ink16']} "
              f"distinct {r['distinct16']}  -> {tag}")
    print("\nOK" if not bad else f"\n{bad} file(s) flagged")


if __name__ == "__main__":
    main()