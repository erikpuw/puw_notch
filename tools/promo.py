"""Compose the TikTok promo (1080 x 1920, 30 fps: a 3 s intro, then 30 s of the notch).

The notch layer comes from the app's own painter: `cargo test -- --ignored promo_frames` writes
target/promo/notch-vi.mov and notch-en.mov (set PUW_FFMPEG and PUW_PREVIEW_COVER). This script puts
it at the top of a rounded "screen" holding the desktop wallpaper, adds the captions, a mouse
pointer that opens the panel and clicks through the pages, and the closing card.

    python -I tools/promo.py vi|en [ffmpeg] [wallpaper]

Writes target/promo/puw-notch-promo-<lang>.mp4 (H.264, no audio: TikTok adds the music).
"""
import os
import subprocess
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
W, H, FPS, SECONDS = 1080, 1920, 30, 30
LAYER_W, LAYER_H = 960, 420
SCREEN = (40, 300, 1040, 1060)  # the "screen": left, top, right, bottom
LAYER_X, LAYER_Y = (W - LAYER_W) // 2, SCREEN[1]
BOLD, SEMI, REGULAR = (os.path.join(os.environ["WINDIR"], "Fonts", f) for f in ("segoeuib.ttf", "seguisb.ttf", "segoeui.ttf"))

# (from, to, title, subtitle) per language.
CAPTIONS = {
    "vi": [
        (0.3, 3.8, "Windows của bạn thiếu một thứ…", ""),
        (4.0, 6.8, "Nhạc đang phát, ngay trên notch", "Ảnh bìa và sóng nhạc"),
        (7.0, 10.3, "Rê chuột để mở", "Lời bài hát, tua bài, nền theo màu ảnh bìa"),
        (10.5, 12.4, "Việc & lịch hẹn", "Gõ “Họp 20:00 mai” là xong"),
        (12.5, 14.4, "Thông báo Windows", "Bấm một dòng để mở app đã gửi"),
        (14.5, 16.4, "Nhắc uống nước, đứng dậy", "Chỉnh giờ và mức theo ý bạn"),
        (16.5, 18.6, "Thời tiết", "Gõ tên thành phố một lần"),
        (19.6, 23.2, "Banner cho mọi thông báo", "Âm lượng, pin, Wi-Fi, USB, ảnh chụp màn hình"),
        (23.8, 26.8, "Rời máy? Robot nhỏ trông giúp", "Quay lại thì nó cười chào"),
    ],
    "en": [
        (0.3, 3.8, "Your Windows is missing something…", ""),
        (4.0, 6.8, "Now playing, right in the notch", "Cover art and live waves"),
        (7.0, 10.3, "Hover to open", "Lyrics, seeking, colours from the cover"),
        (10.5, 12.4, "Tasks & appointments", "Just type “Call 8pm tomorrow”"),
        (12.5, 14.4, "Windows notifications", "Click a row to open its app"),
        (14.5, 16.4, "Water & stand-up reminders", "Your times, your amounts"),
        (16.5, 18.6, "Weather", "Type your city once"),
        (19.6, 23.2, "A banner for every alert", "Volume, battery, Wi-Fi, USB, screenshots"),
        (23.8, 26.8, "Away? A tiny robot keeps watch", "It grins when you come back"),
    ],
}
CARD = {
    "vi": ("Chiếc notch sống ngay trên Windows của bạn.", "Miễn phí trên GitHub · erikpuw/puw_notch"),
    "en": ("A notch that lives on your Windows.", "Free on GitHub · erikpuw/puw_notch"),
}
INTRO_LINE = {"vi": "Notch kiểu Dynamic Island cho Windows", "en": "A Dynamic Island–style notch for Windows"}
# Seconds of intro (the icon pops in, the name slides up) before the 30 s of the notch; its last
# INTRO_FADE seconds cross-fade into the screen. All times below are on the notch's clock.
INTRO, INTRO_FADE = 3.0, 0.6
CARD_AT = 27.0
# Pointer path: (time, x, y) keyframes, eased between; clicks at the page changes.
POINTER = [(6.3, 760, 980), (7.0, 540, 340), (9.6, 545, 345), (10.2, LAYER_X + 876, LAYER_Y + 184),
           (18.2, LAYER_X + 876, LAYER_Y + 184), (18.8, 780, 960)]
POINTER_SHOW = (6.3, 19.2)
CLICKS = [10.5, 12.5, 14.5, 16.5]


def ease(t):
    t = min(max(t, 0.0), 1.0)
    return 1 - (1 - t) ** 3


def ease_back(t):
    t = min(max(t, 0.0), 1.0) - 1
    return 1 + 2.4 * t ** 3 + 1.4 * t ** 2


def rgba(img):
    """PIL RGBA -> float32 premultiplied array."""
    a = np.asarray(img, dtype=np.float32) / 255.0
    a[..., :3] *= a[..., 3:4]
    return a


def over(dst, src, x, y, alpha=1.0):
    """Premultiplied `src` over `dst` (float RGB) at (x, y), clipped to the frame."""
    h, w = src.shape[:2]
    x0, y0, x1, y1 = max(x, 0), max(y, 0), min(x + w, dst.shape[1]), min(y + h, dst.shape[0])
    if x0 >= x1 or y0 >= y1:
        return
    s = src[y0 - y:y1 - y, x0 - x:x1 - x] * alpha
    d = dst[y0:y1, x0:x1]
    d[:] = s[..., :3] + d * (1 - s[..., 3:4])


def background(wallpaper):
    top, bottom = np.array([13, 13, 18]), np.array([24, 18, 34])
    ramp = np.linspace(0, 1, H)[:, None, None]
    bg = Image.fromarray((top + (bottom - top) * ramp).repeat(W, axis=1).astype(np.uint8))
    l, t, r, b = SCREEN
    sw, sh = r - l, b - t
    wp = Image.open(wallpaper).convert("RGB")
    k = max(sw / wp.width, sh / wp.height)
    wp = wp.resize((round(wp.width * k), round(wp.height * k)), Image.LANCZOS)
    wp = wp.crop(((wp.width - sw) // 2, 0, (wp.width - sw) // 2 + sw, sh))
    mask = Image.new("L", (sw, sh), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, sw - 1, sh - 1), 40, fill=255)
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).rounded_rectangle((l - 6, t - 6, r + 6, b + 6), 46, fill=(120, 90, 200, 70))
    bg.paste(glow.filter(ImageFilter.GaussianBlur(28)), (0, 0), glow.filter(ImageFilter.GaussianBlur(28)))
    bg.paste(wp, (l, t), mask)
    ImageDraw.Draw(bg).rounded_rectangle((l, t, r - 1, b - 1), 40, outline=(255, 255, 255, 30), width=2)
    return np.asarray(bg, dtype=np.float32) / 255.0, mask


def fit(path, size, text, width=W - 100):
    """The font at `size`, smaller while `text` would not fit in `width`."""
    font = ImageFont.truetype(path, size)
    while font.getlength(text) > width and size > 30:
        size -= 2
        font = ImageFont.truetype(path, size)
    return font


def text_block(title, subtitle):
    big, small = fit(BOLD, 64, title), fit(REGULAR, 38, subtitle)
    img = Image.new("RGBA", (W, 240), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.text((W / 2, 70), title, font=big, fill=(255, 255, 255, 255), anchor="mm")
    if subtitle:
        d.text((W / 2, 150), subtitle, font=small, fill=(200, 200, 215, 255), anchor="mm")
    return rgba(img)


def pointer_img():
    s = 3
    pts = [(0, 0), (0, 17), (4.5, 13), (7.5, 20), (10, 19), (7, 12.2), (12.5, 12.2)]
    img = Image.new("RGBA", (48 * s, 64 * s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    p = [(4 * s + x * 2.2 * s, 4 * s + y * 2.2 * s) for x, y in pts]
    d.polygon(p, fill=(0, 0, 0, 255))
    inner = [(4 * s + x * 2.2 * s, 4 * s + y * 2.2 * s) for x, y in [(1.6, 3.6), (1.6, 13.4), (4.9, 10.4), (8, 17.4), (8.4, 17.2), (5.4, 10.4), (9.3, 10.4)]]
    d.polygon(inner, fill=(255, 255, 255, 255))
    return img.resize((48, 64), Image.LANCZOS)


def intro(lang):
    """The intro's backdrop (dark, a violet glow in the middle) and its name + line, as arrays."""
    top, bottom = np.array([10, 10, 14]), np.array([22, 16, 32])
    ramp = np.linspace(0, 1, H)[:, None, None]
    bg = Image.fromarray((top + (bottom - top) * ramp).repeat(W, axis=1).astype(np.uint8))
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse((W / 2 - 330, 560, W / 2 + 330, 1220), fill=(120, 90, 220, 90))
    glow = glow.filter(ImageFilter.GaussianBlur(90))
    bg.paste(glow, (0, 0), glow)
    words = Image.new("RGBA", (W, 260), (0, 0, 0, 0))
    d = ImageDraw.Draw(words)
    d.text((W / 2, 70), "PUW Notch", font=ImageFont.truetype(BOLD, 120), fill=(255, 255, 255, 255), anchor="mm")
    d.text((W / 2, 180), INTRO_LINE[lang], font=fit(SEMI, 44, INTRO_LINE[lang]), fill=(205, 200, 225, 255), anchor="mm")
    return np.asarray(bg, dtype=np.float32) / 255.0, rgba(words)


def intro_frame(backdrop, icon, words, t):
    """The intro at `t` seconds: the icon pops in, then the name and the line slide up."""
    frame = backdrop.copy()
    s = ease_back((t - 0.15) / 0.6)
    if s > 0.01:
        side = max(2, round(260 * s))
        over(frame, rgba(icon.resize((side, side), Image.LANCZOS)), (W - side) // 2, 700 + (260 - side) // 2, alpha=min(1.0, (t - 0.15) / 0.25))
    k = ease((t - 0.6) / 0.5)
    over(frame, words, 0, 1010 + round(40 * (1 - k)), alpha=k)
    return frame


def card(lang):
    slogan, link = CARD[lang]
    icon = Image.open(os.path.join(ROOT, "assets", "puw_1024.png")).convert("RGBA").resize((240, 240), Image.LANCZOS)
    words = Image.new("RGBA", (W, 420), (0, 0, 0, 0))
    d = ImageDraw.Draw(words)
    d.text((W / 2, 60), "PUW Notch", font=ImageFont.truetype(BOLD, 104), fill=(255, 255, 255, 255), anchor="mm")
    d.text((W / 2, 170), slogan, font=fit(SEMI, 44, slogan), fill=(225, 225, 235, 255), anchor="mm")
    d.text((W / 2, 260), link, font=fit(REGULAR, 36, link), fill=(160, 160, 180, 255), anchor="mm")
    return icon, rgba(words)


def pointer_at(t):
    for (t0, x0, y0), (t1, x1, y1) in zip(POINTER, POINTER[1:]):
        if t <= t1:
            k = ease((t - t0) / (t1 - t0)) if t >= t0 else 0.0
            return x0 + (x1 - x0) * k, y0 + (y1 - y0) * k
    return POINTER[-1][1:]


def main():
    lang = sys.argv[1]
    ffmpeg = sys.argv[2] if len(sys.argv) > 2 else "ffmpeg"
    wallpaper = sys.argv[3] if len(sys.argv) > 3 else os.path.join(os.environ["APPDATA"], "Microsoft", "Windows", "Themes", "TranscodedWallpaper")
    src = os.path.join(ROOT, "target", "promo", f"notch-{lang}.mov")
    dst = os.path.join(ROOT, "target", "promo", f"puw-notch-promo-{lang}.mp4")
    base, screen_mask = background(wallpaper)
    clip = np.zeros((LAYER_H, LAYER_W, 1), np.float32)
    m = np.asarray(screen_mask, dtype=np.float32)[:, :, None] / 255.0
    clip[:] = m[:LAYER_H, LAYER_X - SCREEN[0]:LAYER_X - SCREEN[0] + LAYER_W]
    captions = [(a, b, text_block(t, s)) for a, b, t, s in CAPTIONS[lang]]
    pointer = pointer_img()
    cursor, pressed = rgba(pointer), rgba(pointer.resize((40, 53), Image.LANCZOS))
    icon, words = card(lang)
    backdrop, intro_words = intro(lang)
    big_icon = Image.open(os.path.join(ROOT, "assets", "puw_1024.png")).convert("RGBA").resize((260, 260), Image.LANCZOS)
    reader = subprocess.Popen([ffmpeg, "-loglevel", "error", "-i", src, "-f", "rawvideo", "-pix_fmt", "bgra", "-"], stdout=subprocess.PIPE)
    writer = subprocess.Popen([ffmpeg, "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
                               "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", dst], stdin=subprocess.PIPE)
    size = LAYER_W * LAYER_H * 4
    for i in range(round(FPS * (INTRO + SECONDS))):
        t = i / FPS - INTRO
        if t < 0:
            frame = intro_frame(backdrop, big_icon, intro_words, t + INTRO)
            k = ease((t + INTRO_FADE) / INTRO_FADE)
            if k > 0:
                frame = frame * (1 - k) + base * k
            writer.stdin.write((np.clip(frame, 0, 1) * 255).astype(np.uint8).tobytes())
            continue
        raw = reader.stdout.read(size)
        if len(raw) < size:
            break
        layer = np.frombuffer(raw, np.uint8).reshape(LAYER_H, LAYER_W, 4)[..., [2, 1, 0, 3]].astype(np.float32) / 255.0
        frame = base.copy()
        over(frame, layer * clip, LAYER_X, LAYER_Y)  # already premultiplied
        for a, b, block in captions:
            if a <= t <= b:
                k_in, k_out = ease((t - a) / 0.35), min(1.0, (b - t) / 0.25)
                over(frame, block, 0, 1150 + round(36 * (1 - k_in)), alpha=k_in * k_out)
        if POINTER_SHOW[0] <= t <= POINTER_SHOW[1]:
            x, y = pointer_at(t)
            fade = min(1.0, (t - POINTER_SHOW[0]) / 0.2, (POINTER_SHOW[1] - t) / 0.3)
            for c in CLICKS:
                if 0 <= t - c < 0.45:
                    k = (t - c) / 0.45
                    ring = Image.new("RGBA", (120, 120), (0, 0, 0, 0))
                    r = 12 + 44 * ease(k)
                    ImageDraw.Draw(ring).ellipse((60 - r, 60 - r, 60 + r, 60 + r), outline=(255, 255, 255, round(200 * (1 - k))), width=4)
                    over(frame, rgba(ring), round(x) - 60, round(y) - 60)
            cur = pressed if any(0 <= t - c < 0.12 for c in CLICKS) else cursor
            over(frame, cur, round(x) - 4, round(y) - 4, alpha=fade)
        if t >= CARD_AT:
            k = (t - CARD_AT) / 0.6
            frame *= 1 - 0.82 * ease(k)
            s = ease_back((t - CARD_AT - 0.1) / 0.6)
            if s > 0.01:
                side = max(2, round(240 * s))
                over(frame, rgba(icon.resize((side, side), Image.LANCZOS)), (W - side) // 2, 640 + (240 - side) // 2, alpha=min(1.0, k * 2))
            kw = ease((t - CARD_AT - 0.35) / 0.5)
            over(frame, words, 0, 920 + round(30 * (1 - kw)), alpha=kw)
        writer.stdin.write((np.clip(frame, 0, 1) * 255).astype(np.uint8).tobytes())
    writer.stdin.close()
    writer.wait()
    reader.wait()
    print(dst)


if __name__ == "__main__":
    main()
