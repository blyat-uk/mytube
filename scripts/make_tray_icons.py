#!/usr/bin/env python3
"""Renders the tray icon set from the app icon.

Run once; the PNGs it writes are checked in and embedded into the binary by
`src-tauri/src/tray.rs` with `include_bytes!`. Kept so the badge can be
restyled without redrawing eleven files by hand.

    python3 scripts/make_tray_icons.py

Output (src-tauri/icons/): tray.png, tray-1.png .. tray-9.png, tray-9plus.png.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"
SOURCE = ICONS / "icon.png"

# 128px rather than the 32px a Plasma tray shows: the panel downscales, and
# handing it a small image instead makes a HiDPI or large-panel tray blurry.
SIZE = 128

BADGE_FILL = (229, 72, 77, 255)  # red, against an icon that has none of it
BADGE_TEXT = (255, 255, 255, 255)
# A ring in the icon's own background colour, so the badge stays readable when
# it lands on top of a light part of the artwork.
BADGE_RING = (24, 24, 27, 255)
RING_WIDTH = SIZE // 32

# The badge is the only part of the icon anyone actually reads at panel size
# (a 24px tray cell leaves a 9px badge at 0.52), so it gets the space rather
# than the artwork, which stays recognisable from its silhouette alone.
BADGE_DIAMETER = round(SIZE * 0.66)
# Fraction of the badge's inner diameter the label's bounding box may span,
# measured on the diagonal. 1.0 would touch the ring.
LABEL_FILL = 0.94

FONT_CANDIDATES = [
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
]


def font_path() -> str:
    for path in FONT_CANDIDATES:
        if Path(path).exists():
            return path
    raise SystemExit(
        "no bold sans font found; add one to FONT_CANDIDATES:\n  "
        + "\n  ".join(FONT_CANDIDATES)
    )


def load_font(size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(font_path(), size)


def fit_font(draw: ImageDraw.ImageDraw, label: str, inner: float) -> ImageFont.FreeTypeFont:
    """Largest font size whose inked bounding box still fits the badge circle.

    A w*h box fits a circle of diameter D exactly when sqrt(w^2 + h^2) <= D, so
    one measurement at a reference size scales linearly to the answer. Measuring
    beats a hand-tuned ratio because the ink box is what the eye reads: digits
    have no descender and "9+" is nearly twice as wide as "1", and picking one
    ratio for both is what made the digits small.
    """
    ref = 100
    left, top, right, bottom = draw.textbbox((0, 0), label, font=load_font(ref), anchor="lt")
    diag = ((right - left) ** 2 + (bottom - top) ** 2) ** 0.5
    return load_font(max(1, round(ref * inner * LABEL_FILL / diag)))


def base_icon() -> Image.Image:
    if not SOURCE.exists():
        raise SystemExit(f"missing source icon: {SOURCE}")
    return Image.open(SOURCE).convert("RGBA").resize((SIZE, SIZE), Image.LANCZOS)


def with_badge(base: Image.Image, label: str) -> Image.Image:
    """Draws `label` in a circle over the bottom-right corner of `base`.

    Supersampled 4x and downscaled, because PIL has no antialiased ellipse and
    a hard-edged circle looks broken next to the antialiased panel icons.
    """
    scale = 4
    big = base.resize((SIZE * scale, SIZE * scale), Image.LANCZOS)
    draw = ImageDraw.Draw(big)

    d = BADGE_DIAMETER * scale
    # Flush to the corner: the tray crops nothing, and insetting it costs
    # diameter that the digits need.
    right, bottom = SIZE * scale, SIZE * scale
    box = (right - d, bottom - d, right, bottom)
    draw.ellipse(box, fill=BADGE_FILL, outline=BADGE_RING, width=RING_WIDTH * scale)

    # Inside the ring, which is drawn inward from the ellipse's edge.
    font = fit_font(draw, label, d - 2 * RING_WIDTH * scale)
    cx, cy = (box[0] + box[2]) / 2, (box[1] + box[3]) / 2
    # Centred on the ink, not on anchor="mm" — that one splits the ascender and
    # descender, so a digit (which has neither) drifts high, and at this fill
    # ratio the drift is enough to graze the ring.
    x0, y0, x1, y1 = draw.textbbox((0, 0), label, font=font, anchor="lt")
    draw.text(
        (cx - (x0 + x1) / 2, cy - (y0 + y1) / 2),
        label,
        font=font,
        fill=BADGE_TEXT,
        anchor="lt",
    )

    return big.resize((SIZE, SIZE), Image.LANCZOS)


def main() -> int:
    base = base_icon()
    written = []

    plain = ICONS / "tray.png"
    base.save(plain)
    written.append(plain)

    for n in range(1, 10):
        path = ICONS / f"tray-{n}.png"
        with_badge(base, str(n)).save(path)
        written.append(path)

    plus = ICONS / "tray-9plus.png"
    with_badge(base, "9+").save(plus)
    written.append(plus)

    for path in written:
        print(f"wrote {path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
