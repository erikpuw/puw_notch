"""Compose the TikTok promo (1080 x 1920, 30 fps: a 2.6 s intro, then 30 s of the notch).

The notch layer comes from the app's own painter: `cargo test -- --ignored promo_frames` writes
target/promo/notch-vi.mov and notch-en.mov (set PUW_FFMPEG and PUW_PREVIEW_COVER). This script puts
it at the top of a rounded "screen" holding the desktop wallpaper, adds the captions, a mouse
pointer that opens the panel and clicks through the pages, and the closing card.

    python -I tools/promo.py vi|en [ffmpeg] [wallpaper]

Writes target/promo/puw-notch-promo-<lang>.mp4 (H.264 + AAC: a synthesised lo-fi bed and effects).
"""
import os
import subprocess
import sys
import wave

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
# Seconds of intro before the 30 s of the notch: the icon pops in, blinks, then shrinks into the
# thin line at the top of the screen that the notch grows from. All times below are on the notch's clock.
INTRO = 2.6
ICON_SIDE, ICON_Y = 260, 700
MINI_W, MINI_H = 88, 8  # the notch's first frame: the mini line (44 x 4) at 2x
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


def backdrop():
    """The intro's backdrop: dark, with a violet glow behind the icon."""
    top, bottom = np.array([10, 10, 14]), np.array([22, 16, 32])
    ramp = np.linspace(0, 1, H)[:, None, None]
    bg = Image.fromarray((top + (bottom - top) * ramp).repeat(W, axis=1).astype(np.uint8))
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    cy = ICON_Y + ICON_SIDE / 2
    ImageDraw.Draw(glow).ellipse((W / 2 - 330, cy - 330, W / 2 + 330, cy + 330), fill=(120, 90, 220, 90))
    glow = glow.filter(ImageFilter.GaussianBlur(90))
    bg.paste(glow, (0, 0), glow)
    return np.asarray(bg, dtype=np.float32) / 255.0


def icon_img(w, h, eyes):
    """The app icon (assets/puw_1024.png's shapes, on its 1024 grid) at w x h, eyes open 0..1."""
    s = 3
    kx, ky = w * s / 1024, h * s / 1024
    img = Image.new("RGBA", (max(1, round(w * s)), max(1, round(h * s))), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    k = min(kx, ky)
    d.rounded_rectangle((0, 0, 1023 * kx, 1023 * ky), 220 * k, fill=(28, 28, 34, 255))
    d.rounded_rectangle((67 * kx, 67 * ky, 957 * kx, 957 * ky), 155 * k, fill=(250, 250, 252, 255))
    half = max(10.0, 138.5 * eyes)
    for x0 in (261, 589):
        d.rounded_rectangle((x0 * kx, (512.5 - half) * ky, (x0 + 175) * kx, (512.5 + half) * ky), min(40, half) * k, fill=(20, 20, 24, 255))
    return img.resize((max(1, round(w)), max(1, round(h))), Image.LANCZOS)


def blink(t):
    """How open the eyes are at `t`: one blink at 0.85 s."""
    u = (t - 0.85) / 0.3
    return 1.0 if not 0 <= u <= 1 else abs(1 - 2 * u) ** 0.8


def intro_frame(back, base, t):
    """The intro at `t` seconds: the icon pops in on the dark backdrop and blinks; from MORPH it
    shrinks, turns black and flies up into the mini line at the top of the screen, which fades in."""
    morph0, morph1 = 1.45, 2.45
    m = ease((t - morph0) / (morph1 - morph0))
    k_bg = ease((t - morph0) / 0.8)
    frame = back * (1 - k_bg) + base * k_bg
    pop = ease_back((t - 0.1) / 0.55)
    if pop <= 0.01:
        return frame
    w = (ICON_SIDE + (MINI_W - ICON_SIDE) * m) * (pop if m == 0 else 1)
    h = (ICON_SIDE + (MINI_H - ICON_SIDE) * m) * (pop if m == 0 else 1)
    cy = ICON_Y + ICON_SIDE / 2 + (SCREEN[1] + MINI_H / 2 - ICON_Y - ICON_SIDE / 2) * m
    x, y = round(W / 2 - w / 2), round(cy - h / 2)
    alpha = min(1.0, (t - 0.1) / 0.2)
    # The black shape it becomes, under the icon that fades out in the first part of the morph.
    if m > 0:
        shape = Image.new("RGBA", (max(1, round(w)), max(1, round(h))), (0, 0, 0, 0))
        ImageDraw.Draw(shape).rounded_rectangle((0, 0, shape.width - 1, shape.height - 1), min(w, h) * (0.22 + 0.28 * m), fill=(0, 0, 0, 255))
        over(frame, rgba(shape), x, y)
    fade = 1 - ease(m / 0.45)
    if fade > 0.01:
        over(frame, rgba(icon_img(w, h, blink(t))), x, y, alpha=alpha * fade)
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


RATE = 48000
BPM = 100


def soundtrack(path):
    """The promo's sound, synthesised here (nothing licensed): a quiet lo-fi bed at BPM and an
    effect on every move of the video, written as 16-bit stereo WAV. Times are on the video's clock."""
    total = INTRO + SECONDS
    n = int(total * RATE)
    mix = np.zeros((n, 2), np.float32)
    rng = np.random.default_rng(7)

    def env(m, attack, release):
        x = np.arange(m) / RATE
        return np.minimum(1.0, x / max(attack, 1e-4)) * np.exp(-x / max(release, 1e-4))

    def put(sig, at, gain=1.0, pan=0.0):
        i = int(at * RATE)
        if i >= n:
            return
        sig = sig[: n - i] * gain
        mix[i:i + len(sig), 0] += sig * (1 - max(pan, 0))
        mix[i:i + len(sig), 1] += sig * (1 + min(pan, 0))

    def lowpass(x, cut):
        a = np.exp(-2 * np.pi * cut / RATE)
        y = np.empty_like(x)
        acc = 0.0
        for i, v in enumerate(x):  # ponytail: per-sample loop, fine for a few seconds of effects
            acc = (1 - a) * v + a * acc
            y[i] = acc
        return y

    def sweep(f0, f1, dur, attack=0.004, release=0.08):
        m = int(dur * RATE)
        f = np.geomspace(f0, f1, m)
        return np.sin(2 * np.pi * np.cumsum(f) / RATE) * env(m, attack, release)

    def bell(freq, dur=1.2, release=0.35):
        m = int(dur * RATE)
        x = np.arange(m) / RATE
        tone = np.sin(2 * np.pi * freq * x) + 0.35 * np.sin(2 * np.pi * freq * 2.76 * x) * np.exp(-x / 0.12)
        return tone * env(m, 0.002, release)

    def noise(dur, cut, attack, release):
        m = int(dur * RATE)
        return lowpass(rng.standard_normal(m).astype(np.float32), cut) * env(m, attack, release)

    def whoosh(dur, up=True):
        m = int(dur * RATE)
        x = np.linspace(0, 1, m)
        shape = np.sin(np.pi * (x if up else 1 - x) ** 0.7) ** 2
        return lowpass(rng.standard_normal(m).astype(np.float32), 2500) * shape

    def click():
        return noise(0.03, 6000, 0.0005, 0.006) * 0.8 + sweep(3200, 2400, 0.03, 0.0005, 0.008) * 0.3

    # The bed: Cmaj7 - Am7 - Fmaj7 - G6, a chord a bar, soft pad + kick + hats.
    beat = 60 / BPM
    chords = [[48, 55, 59, 64], [45, 52, 55, 60], [41, 48, 52, 57], [43, 50, 52, 59]]
    hz = lambda note: 440 * 2 ** ((note - 69) / 12)
    bar = 4 * beat
    m = int(bar * RATE)
    x = np.arange(m) / RATE
    pad_env = np.minimum(1, x / 0.35) * np.minimum(1, (bar - x) / 0.25)
    t0 = INTRO - 0.4  # the beat comes in as the notch appears
    k = 0
    while t0 + k * bar < total:
        notes = chords[k % 4]
        pad = sum(np.sin(2 * np.pi * hz(nn + 12) * x * d) for nn in notes for d in (0.997, 1.003))
        put(pad * pad_env / 8, t0 + k * bar, 0.10)
        put(np.sin(2 * np.pi * hz(notes[0] - 12) * x) * pad_env, t0 + k * bar, 0.10)
        for b in range(4):
            at = t0 + k * bar + b * beat
            if b % 2 == 0:
                put(sweep(120, 45, 0.35, 0.002, 0.12), at, 0.30)
            put(noise(0.05, 9000, 0.001, 0.012), at + beat / 2, 0.05, pan=0.3)
        k += 1
    # The intro's soft drone under the icon, before the beat.
    put(sum(np.sin(2 * np.pi * hz(nn + 12) * np.arange(int(INTRO * RATE)) / RATE) for nn in (48, 55, 64))
        * np.minimum(1, np.arange(int(INTRO * RATE)) / RATE / 0.6) / 3, 0, 0.05)

    v = lambda t: INTRO + t  # notch clock -> video clock
    put(sweep(320, 980, 0.18), 0.10, 0.45)                       # icon pops in
    put(click(), 0.85, 0.5)                                      # blink: eyes shut
    put(click(), 1.0, 0.35)                                      # and open
    put(whoosh(0.9), 1.45, 0.35, pan=-0.2)                       # flies up into the notch
    put(sweep(180, 420, 0.25, 0.01, 0.12), v(0.0), 0.35)         # the notch grows
    for f, d in ((784, 0), (1175, 0.09)):                        # music starts
        put(bell(f, 0.9, 0.3), v(4.0) + d, 0.18)
    put(whoosh(0.45), v(7.3), 0.3)                               # panel opens
    for c in CLICKS:
        put(click(), v(c), 0.6, pan=0.25)                         # pointer clicks
        put(whoosh(0.3), v(c) + 0.02, 0.15, pan=-0.25)            # page slides
    put(whoosh(0.4, up=False), v(18.6), 0.25)                    # panel closes
    for f, d in ((988, 0), (1319, 0.12)):                        # notification banner
        put(bell(f, 1.2, 0.4), v(19.6) + d, 0.28)
    for i, note in enumerate((84, 88, 91, 96)):                  # the robot shows up
        put(sweep(hz(note), hz(note) * 1.02, 0.09, 0.003, 0.05), v(23.8) + i * 0.07, 0.22)
    put(whoosh(0.8), v(CARD_AT) - 0.5, 0.25)                     # closing card
    for i, note in enumerate((72, 76, 79, 84, 88)):
        put(bell(hz(note), 2.2, 0.8), v(CARD_AT) + 0.05 + i * 0.06, 0.12, pan=(i - 2) * 0.15)

    fade = np.ones(n, np.float32)
    tail = int(1.2 * RATE)
    fade[-tail:] = np.linspace(1, 0, tail)
    mix *= fade[:, None]
    mix /= max(1.0, np.abs(mix).max() / 0.89)
    with wave.open(path, "wb") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(RATE)
        f.writeframes((mix * 32767).astype(np.int16).tobytes())


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
    back = backdrop()
    reader = subprocess.Popen([ffmpeg, "-loglevel", "error", "-i", src, "-f", "rawvideo", "-pix_fmt", "bgra", "-"], stdout=subprocess.PIPE)
    audio = os.path.join(ROOT, "target", "promo", "soundtrack.wav")
    soundtrack(audio)
    writer = subprocess.Popen([ffmpeg, "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-", "-i", audio,
                               "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-af", "loudnorm=I=-14:TP=-1.5:LRA=11", "-ar", str(RATE), "-c:a", "aac", "-b:a", "192k", "-shortest",
                               "-movflags", "+faststart", dst], stdin=subprocess.PIPE)
    size = LAYER_W * LAYER_H * 4
    for i in range(round(FPS * (INTRO + SECONDS))):
        t = i / FPS - INTRO
        if t < 0:
            frame = intro_frame(back, base, t + INTRO)
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
