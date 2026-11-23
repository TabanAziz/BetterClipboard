"""Draws the BetterClipboard icon: a rust-coloured clipboard on the Better tile, split by a bolt.

    py tools/make_icon.py

writes assets/bcb.ico, assets/bcb-paused.ico, assets/bcb.png and assets/bcb-preview.png
(git-ignored). Every size is drawn on its own, so the small ones stay readable.
The tile, gradients and bolt match the Better Task Manager icon, so the apps read as a set.
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageOps

ASSETS = Path(__file__).resolve().parent.parent / "assets"
SIZES = [16, 20, 24, 28, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 128, 256]
OVERSAMPLE = 8

TILE = [(0.0, (86, 58, 47)), (1.0, (30, 21, 19))]
BOARD = [(0.0, (255, 170, 72)), (0.45, (232, 93, 28)), (1.0, (150, 44, 14))]
PAPER = [(0.0, (255, 236, 214)), (1.0, (246, 203, 160))]
CLIP = [(0.0, (120, 84, 70)), (1.0, (62, 42, 36))]
BOLT = [(0.0, (255, 255, 250)), (1.0, (255, 218, 128))]

BOLT_SHAPE = [(0.600, 0.25), (0.330, 0.640), (0.495, 0.640), (0.430, 0.935), (0.725, 0.530),
              (0.555, 0.530)]
BOLT_SMALL = [(0.64, 0.22), (0.27, 0.66), (0.49, 0.66), (0.40, 0.98), (0.79, 0.52), (0.58, 0.52)]


def gradient(size, stops):
    strip = Image.new("RGBA", (1, size))
    for y in range(size):
        t = y / max(size - 1, 1)
        for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
            if t0 <= t <= t1:
                k = (t - t0) / (t1 - t0)
                strip.putpixel((0, y), tuple(round(a + (b - a) * k) for a, b in zip(c0, c1)) + (255,))
                break
    return strip.resize((size, size))


def draw(size):
    s = size * OVERSAMPLE
    small = size <= 24
    pad = 0 if small else s * 0.035
    inner = s - 2 * pad

    def box(x0, y0, x1, y1):
        return (pad + x0 * inner, pad + y0 * inner, pad + x1 * inner, pad + y1 * inner)

    def points(shape):
        return [(pad + x * inner, pad + y * inner) for x, y in shape]

    def mask(paint):
        m = Image.new("L", (s, s), 0)
        paint(ImageDraw.Draw(m))
        return m

    tile = mask(lambda d: d.rounded_rectangle((pad, pad, s - pad, s - pad), radius=inner * 0.22, fill=255))
    icon = Image.new("RGBA", (s, s))
    icon.paste(gradient(s, TILE), (0, 0), tile)

    # Clipboard board, paper sheet and the clip on top.
    if small:
        board_box, paper_box, clip_box = box(0.14, 0.16, 0.86, 0.97), None, box(0.32, 0.06, 0.68, 0.24)
    else:
        board_box, paper_box, clip_box = box(0.17, 0.17, 0.83, 0.92), box(0.25, 0.27, 0.75, 0.85), box(0.35, 0.09, 0.65, 0.25)
    board = mask(lambda d: d.rounded_rectangle(board_box, radius=inner * (0.07 if small else 0.09), fill=255))
    if not small:
        shadow = board.filter(ImageFilter.GaussianBlur(s / 40)).point(lambda v: int(v * 0.6))
        icon.paste((20, 6, 0, 255), (0, int(s * 0.012)), ImageChops.multiply(shadow, tile))
    icon.paste(gradient(s, BOARD), (0, 0), board)

    if paper_box:
        paper = mask(lambda d: d.rounded_rectangle(paper_box, radius=inner * 0.035, fill=255))
        icon.paste(gradient(s, PAPER), (0, 0), paper)
        lines = mask(lambda d: [
            d.rounded_rectangle(box(0.31, y, 0.31 + w, y + 0.035), radius=inner * 0.017, fill=255)
            for y, w in ((0.36, 0.30), (0.45, 0.38), (0.54, 0.22))
        ])
        icon.paste((226, 150, 104, 255), (0, 0), lines)

    clip = mask(lambda d: d.rounded_rectangle(clip_box, radius=inner * 0.05, fill=255))
    icon.paste(gradient(s, CLIP), (0, 0), clip)
    if not small:
        hole = mask(lambda d: d.ellipse(box(0.465, 0.125, 0.535, 0.175), fill=255))
        icon.paste((255, 206, 128, 255), (0, 0), hole)

    shape = points(BOLT_SMALL if small else BOLT_SHAPE)
    bolt = mask(lambda d: d.polygon(shape, fill=255))
    if not small:
        shadow = bolt.filter(ImageFilter.GaussianBlur(s / 38)).point(lambda v: int(v * 0.55))
        icon.paste((40, 8, 0, 255), (0, 0), ImageChops.multiply(shadow, tile))
    # A rust outline keeps the bolt readable on the light paper.
    grow = max(3, (s // 64) | 1)
    outline = bolt.filter(ImageFilter.MaxFilter(grow if not small else max(3, (s // 40) | 1)))
    icon.paste((122, 34, 10, 255), (0, 0), ImageChops.multiply(outline, tile))
    icon.paste(gradient(s, BOLT), (0, 0), bolt)
    return icon.resize((size, size), Image.LANCZOS)


def paused(frame):
    """Grey version for the tray while capture is paused."""
    grey = ImageOps.grayscale(frame.convert("RGB")).convert("RGBA")
    grey.putalpha(frame.getchannel("A").point(lambda v: int(v * 0.85)))
    return grey


def main():
    ASSETS.mkdir(exist_ok=True)
    frames = [draw(size) for size in SIZES]
    frames[-1].save(ASSETS / "bcb.ico", sizes=[(n, n) for n in SIZES], append_images=frames[:-1])
    frames[-1].save(ASSETS / "bcb.png")
    grey = [paused(f) for f in frames]
    grey[-1].save(ASSETS / "bcb-paused.ico", sizes=[(n, n) for n in SIZES], append_images=grey[:-1])

    shown = [256, 128, 64, 48, 32, 24, 20, 16]
    width = sum(shown) + 18 * (len(shown) + 1)
    preview = Image.new("RGBA", (width, 2 * 292), (32, 32, 32, 255))
    preview.paste((243, 243, 243, 255), (0, 292, width, 584))
    x = 18
    for n in shown:
        frame = frames[SIZES.index(n)]
        preview.alpha_composite(frame, (x, 18))
        preview.alpha_composite(frame, (x, 292 + 18))
        x += n + 18
    preview.save(ASSETS / "bcb-preview.png")
    print("icon written")


if __name__ == "__main__":
    main()
