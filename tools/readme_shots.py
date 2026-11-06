"""Turn the BMPs that `cargo test readme_shots -- --ignored` writes to target/readme into the
README's PNGs in assets/screenshots: premultiplied BGRA to straight RGBA, cropped around the
notch (it is drawn at the top centre of a 900 px wide window) and laid over assets/backdrop.jpg,
the picture a desktop would have behind it.

    python tools/readme_shots.py                 # Vietnamese set -> assets/screenshots
    python tools/readme_shots.py --lang en       # English set (PUW_README_LANG=en for the cargo test) -> assets/screenshots/en

The English set holds only the pictures with words in them (pages and banners); the pills, the robot and
the style pills read the same in both languages and are shared.
"""
import os
import sys

from PIL import Image, ImageEnhance

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ENGLISH = "--lang" in sys.argv and sys.argv[sys.argv.index("--lang") + 1] == "en"
SRC = os.path.join(ROOT, "target", "readme-en" if ENGLISH else "readme")
DST = os.path.join(ROOT, "assets", "screenshots", "en") if ENGLISH else os.path.join(ROOT, "assets", "screenshots")
CENTRE = 450
BACKDROP = os.path.join(ROOT, "assets", "backdrop.jpg")

# name -> (width, height) of the crop; None height = the picture's own bottom edge plus a margin.
SIZES = {
    "pill-idle": (260, 44), "pill-music": (260, 44), "weather-pill": (260, 44), "weather-pill-music": (260, 44),
    "robot": (260, 44), "robot-happy": (260, 44), "pill-focus": (260, 44),
    "style-pill-idle": (260, 50), "style-pill-music": (260, 50),
    "page-music": (580, 218), "page-focus": (580, 218), "page-weather-sun": (580, 218), "page-weather-night": (580, 218), "page-nav": (580, 236), "page-music-empty": (580, 218), "page-health": (580, 218), "page-weather": (580, 218),
    "page-tasks": (580, 218), "page-notifications": (580, 218), "page-calendar": (580, 218), "page-calendar-lunar": (580, 218),
    "page-usage-claude": (580, 218), "page-usage-codex": (580, 218), "page-usage-antigravity": (580, 218),
    "style-pill-page": (580, 224),
    "page-music-lyrics": (900, 218),
    "banner": (500, None), "banner-water": (500, None), "banner-water-goal": (500, None), "banner-standup": (500, None), "banner-wifi": (500, None), "banner-airplane-off": (500, None), "banner-airplane-on": (500, None), "banner-bluetooth": (500, None), "banner-approve-claude": (580, None), "banner-approve-antigravity": (580, None),
}


def load(path):
    data = open(path, "rb").read()
    w = int.from_bytes(data[18:22], "little")
    h = abs(int.from_bytes(data[22:26], "little", signed=True))
    return Image.frombuffer("RGBa", (w, h), data[54:54 + w * h * 4], "raw", "BGRa", 0, 1).convert("RGBA")


def backdrop(width, height):
    """The backdrop scaled to `width` and cut to `height` from its top."""
    pic = Image.open(BACKDROP).convert("RGBA")
    scale = max(width / pic.width, height / pic.height)
    pic = pic.resize((round(pic.width * scale), round(pic.height * scale)), Image.LANCZOS)
    # The notch is black, so the picture behind it is made light (brighter, a veil of white) to
    # let its edge stand out.
    pic = ImageEnhance.Brightness(pic.convert("RGB")).enhance(1.9)
    pic = Image.blend(pic, Image.new("RGB", pic.size, (255, 255, 255)), 0.22).convert("RGBA")
    top = min(round(pic.height * 0.15), pic.height - height)
    return pic.crop((0, top, width, top + height))


# The pictures that carry words (page titles, dates, sample content, buttons): each language has its own.
TEXTUAL = [n for n in SIZES if n.startswith(("page-", "banner-approve")) or n in ("style-pill-page", "banner", "banner-water", "banner-water-goal", "banner-standup", "banner-wifi", "banner-airplane-off", "banner-airplane-on", "banner-bluetooth")]


def main(names):
    os.makedirs(DST, exist_ok=True)
    for name in names:
        path = os.path.join(SRC, name + ".bmp")
        if not os.path.exists(path):
            print("missing", name)
            continue
        im = load(path)
        width, height = SIZES[name]
        if height is None:
            height = im.getchannel("A").getbbox()[3] + 18
        left = max(0, CENTRE - width // 2)
        shot = im.crop((left, 0, left + width, height))
        picture = backdrop(width, height)
        picture.alpha_composite(shot)
        picture.convert("RGB").save(os.path.join(DST, name + ".png"), optimize=True)
        print("wrote", name, (width, height))


if __name__ == "__main__":
    args = [a for i, a in enumerate(sys.argv[1:], 1) if a != "--lang" and sys.argv[i - 1] != "--lang"]
    main(args or (TEXTUAL if ENGLISH else list(SIZES)))
