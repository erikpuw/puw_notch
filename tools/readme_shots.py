"""Turn the BMPs that `cargo test readme_shots -- --ignored` writes to target/readme into the
README's PNGs in assets/screenshots: premultiplied BGRA to straight RGBA, cropped around the
notch (it is drawn at the top centre of a 900 px wide window).

    python tools/readme_shots.py
"""
import os
import sys

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "target", "readme")
DST = os.path.join(ROOT, "assets", "screenshots")
CENTRE = 450

# name -> (width, height) of the crop; None height = the picture's own bottom edge plus a margin.
SIZES = {
    "pill-idle": (260, 44), "pill-music": (260, 44), "weather-pill": (260, 44), "weather-pill-music": (260, 44),
    "style-pill-idle": (260, 50), "style-pill-music": (260, 50),
    "page-music": (580, 218), "page-music-empty": (580, 218), "page-health": (580, 218), "page-weather": (580, 218),
    "page-tasks": (580, 218), "page-notifications": (580, 218),
    "page-usage-claude": (580, 218), "page-usage-codex": (580, 218), "page-usage-antigravity": (580, 218),
    "style-pill-page": (580, 224),
    "page-music-lyrics": (900, 218),
    "banner-approve-claude": (580, None), "banner-approve-antigravity": (580, None),
}


def load(path):
    data = open(path, "rb").read()
    w = int.from_bytes(data[18:22], "little")
    h = abs(int.from_bytes(data[22:26], "little", signed=True))
    return Image.frombuffer("RGBa", (w, h), data[54:54 + w * h * 4], "raw", "BGRa", 0, 1).convert("RGBA")


def main(names):
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
        im.crop((left, 0, left + width, height)).save(os.path.join(DST, name + ".png"), optimize=True)
        print("wrote", name, (width, height))


if __name__ == "__main__":
    main(sys.argv[1:] or list(SIZES))
