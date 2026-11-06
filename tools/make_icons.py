"""Generate Keyscene tray and app icons.

Run: python -I tools/make_icons.py   (from the project folder)
Logo: a toggle switch (press once to turn a scene on, again to turn it back).
Tray icons: icons/tray/<size>/<state>-<theme>[-mic].png, drawn at 256 px and downscaled to
16/20/24/32 px (the tray size at 100/125/150/200% scale), so Windows never has to shrink them.
  state: base, paused (shortcuts paused: dimmed logo + pause badge)
  theme: light (dark glyph for a light taskbar), dark (white glyph for a dark taskbar)
  -mic: microphone muted (red badge; replaces the pause badge)
"""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SIZE = 32  # drawing units
SCALE = 8
TRAY_SIZES = (16, 20, 24, 32)
S = SIZE * SCALE
CLEAR = (0, 0, 0, 0)


def px(v):
    return int(round(v * SCALE))


def switch(draw, color, k=SCALE):
    """The logo in a 32-unit box: a rounded track with the knob on the right ("on")."""
    draw.rounded_rectangle([round(2.5 * k), round(9 * k), round(29.5 * k), round(23 * k)],
                           radius=round(7 * k), outline=color, width=round(2.6 * k))
    draw.ellipse([round(16.8 * k), round(11.8 * k), round(26.2 * k), round(21.2 * k)], fill=color)


def pause_badge(img, fg):
    d = ImageDraw.Draw(img)
    cx, cy, r = 24, 24, 8
    d.ellipse([px(cx - r), px(cy - r), px(cx + r), px(cy + r)], fill=CLEAR)
    d.ellipse([px(cx - r + 1.2), px(cy - r + 1.2), px(cx + r - 1.2), px(cy + r - 1.2)], fill=(*fg, 255))
    inv = (255, 255, 255) if sum(fg) < 300 else (26, 26, 26)
    d.rectangle([px(cx - 3), px(cy - 3.5), px(cx - 1), px(cy + 3.5)], fill=(*inv, 255))
    d.rectangle([px(cx + 1), px(cy - 3.5), px(cx + 3), px(cy + 3.5)], fill=(*inv, 255))


def mic_badge(img):
    d = ImageDraw.Draw(img)
    cx, cy, r = 24, 24, 8.4
    d.ellipse([px(cx - r), px(cy - r), px(cx + r), px(cy + r)], fill=CLEAR)
    r2 = r - 1.0
    d.ellipse([px(cx - r2), px(cy - r2), px(cx + r2), px(cy + r2)], fill=(196, 43, 28, 255))
    w = (255, 255, 255, 255)
    d.rounded_rectangle([px(cx - 1.9), px(cy - 5), px(cx + 1.9), px(cy + 1.2)], radius=px(1.9), fill=w)
    d.arc([px(cx - 3.8), px(cy - 4.2), px(cx + 3.8), px(cy + 3.2)], 20, 160, fill=w, width=px(1.2))
    d.line([px(cx), px(cy + 3.2), px(cx), px(cy + 5)], fill=w, width=px(1.2))


def make(state, theme, mic):
    img = Image.new("RGBA", (S, S), CLEAR)
    d = ImageDraw.Draw(img)
    fg = (26, 26, 26) if theme == "light" else (255, 255, 255)
    alpha = 110 if state == "paused" else 255
    switch(d, (*fg, alpha))
    if mic:
        mic_badge(img)
    elif state == "paused":
        pause_badge(img, fg)
    return img


def app_icon():
    sizes = [16, 24, 32, 48, 64, 128, 256]
    big = Image.new("RGBA", (1024, 1024), CLEAR)
    d = ImageDraw.Draw(big)
    d.rounded_rectangle([40, 40, 984, 984], radius=220, fill=(0, 95, 184, 255))
    glyph = Image.new("RGBA", (640, 640), CLEAR)
    switch(ImageDraw.Draw(glyph), (255, 255, 255, 255), k=20)
    big.alpha_composite(glyph, (192, 192))
    out = ROOT / "icons"
    out.mkdir(exist_ok=True)
    big.resize((512, 512), Image.LANCZOS).save(out / "icon.png")
    big.resize((32, 32), Image.LANCZOS).save(out / "32x32.png")
    big.resize((128, 128), Image.LANCZOS).save(out / "128x128.png")
    big.save(out / "icon.ico", sizes=[(s, s) for s in sizes])


def main():
    for size in TRAY_SIZES:
        folder = ROOT / "icons" / "tray" / str(size)
        folder.mkdir(parents=True, exist_ok=True)
        for old in folder.glob("*.png"):
            old.unlink()
    for state in ("base", "paused"):
        for theme in ("light", "dark"):
            for mic in (False, True):
                name = f"{state}-{theme}{'-mic' if mic else ''}.png"
                big = make(state, theme, mic)
                for size in TRAY_SIZES:
                    big.resize((size, size), Image.LANCZOS).save(ROOT / "icons" / "tray" / str(size) / name)
    app_icon()
    print("icons written to", ROOT / "icons")


if __name__ == "__main__":
    main()
