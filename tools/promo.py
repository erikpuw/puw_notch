"""Compose the TikTok promo (1080 x 1920, 30 fps, 33 s: a 1.6 s intro, then the notch, then the card).

The notch layer comes from the app's own painter: `cargo test -- --ignored promo_frames` writes
target/promo/notch-vi.mov and notch-en.mov (1680 x 680 at 2x; set PUW_FFMPEG). This script puts it
at the top of a rounded "screen" holding the desktop wallpaper, with a camera that frames whatever
the notch is showing and punches in on every cut, plus captions, a pointer, the version, the
closing card, a synthesised 128 BPM track with effects, and a voice-over from the local TTS
service at http://127.0.0.1:8082 (nothing leaves the machine).

    python -I tools/promo.py vi|en [ffmpeg] [wallpaper]

Writes target/promo/puw-notch-promo-<lang>.mp4 (H.264 + AAC, -14 LUFS).
"""
import hashlib
import json
import os
import re
import subprocess
import sys
import urllib.request
import wave

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont
from scipy.signal import butter, lfilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
W, H, FPS = 1080, 1920, 30
INTRO, SECONDS = 1.6, 31.4  # the notch layer's clock starts after the intro
LAYER_W, LAYER_H = 1680, 680
CANVAS_H = 1400  # the virtual desktop behind the notch, at the layer's 2x
SCREEN = (40, 250, 1040, 1090)
SW, SH = SCREEN[2] - SCREEN[0], SCREEN[3] - SCREEN[1]
BOLD, SEMI, REGULAR = (os.path.join(os.environ["WINDIR"], "Fonts", f) for f in ("segoeuib.ttf", "seguisb.ttf", "segoeui.ttf"))
ACCENT = ((150, 110, 255), (70, 215, 255))
TTS = "http://127.0.0.1:8082"
VOICE = "Adam bựa"
VO_TEMPO = 1.08


def version():
    with open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8") as f:
        return "v" + re.search(r'^version\s*=\s*"([^"]+)"', f.read(), re.M).group(1)


# Times below are on the notch's clock; they match the events in src/render.rs promo_layer.
CLICKS = [7.2, 8.5, 9.8, 11.1, 12.4, 13.7, 15.0]
CARD_AT = 28.4
PUNCHES = [0.0, 1.6, 4.3, 4.9, *CLICKS, 16.8, 17.2, 17.9, 18.6, 19.3, 20.4, 21.8, 23.3, 24.7, 26.3]
FLASHES = [0.0, 4.3, 17.2, 20.4, 23.3, 26.3]
POINTER = [(3.4, 1500, 1100), (4.0, 840, 40), (6.5, 846, 44), (7.0, 1290, 194), (16.4, 1290, 194), (16.9, 1460, 900)]
POINTER_SHOW = (3.4, 17.0)

CAPTIONS = {
    "vi": [
        (0.1, 1.5, "Windows thiếu một thứ…", ""),
        (1.6, 4.1, "Nhạc ngay trên notch", "Bìa đĩa, aurora theo màu bìa"),
        (4.3, 7.0, "Lời bài hát chạy theo nhạc", "Rê chuột vào là mở"),
        (7.2, 8.4, "Ghi chú & nhắc việc", "Gõ “Họp 20:00 mai” là xong"),
        (8.5, 9.7, "Tập trung", "Pomodoro ngay trên notch"),
        (9.8, 11.0, "Thông báo", "Bấm một dòng để mở app"),
        (11.1, 12.3, "Sức khỏe", "Nhắc uống nước, đứng dậy"),
        (12.4, 13.6, "Thời tiết", "Nhiệt độ từng giờ, mặt trời, mặt trăng"),
        (13.7, 14.9, "Lịch", "Dương lịch và âm lịch"),
        (15.0, 16.7, "Giới hạn AI", "Claude · Codex · Antigravity"),
        (17.2, 20.0, "Wi-Fi · Bluetooth · Máy bay · Sạc", "Có gì là notch báo liền"),
        (20.4, 22.9, "Copy, di chuyển, xóa, build", "Tiến độ ngay trên notch, có cả +N"),
        (23.3, 25.9, "AI xin duyệt?", "Bấm Đồng ý ngay trên notch"),
        (26.3, 28.2, "Rời máy? Robot trông giúp", "Quay lại là nó cười chào"),
    ],
    "en": [
        (0.1, 1.5, "Windows is missing something…", ""),
        (1.6, 4.1, "Music, right in the notch", "Cover art, aurora in its colours"),
        (4.3, 7.0, "Lyrics that follow along", "Hover to open"),
        (7.2, 8.4, "Notes & reminders", "Type “Call 8pm tomorrow”, done"),
        (8.5, 9.7, "Focus", "Pomodoro in the notch"),
        (9.8, 11.0, "Notifications", "Click a row to open its app"),
        (11.1, 12.3, "Health", "Water and stand-up reminders"),
        (12.4, 13.6, "Weather", "Hourly temps, sun and moon"),
        (13.7, 14.9, "Calendar", "Solar and lunar"),
        (15.0, 16.7, "AI limits", "Claude · Codex · Antigravity"),
        (17.2, 20.0, "Wi-Fi · Bluetooth · Airplane · Charging", "It tells you right away"),
        (20.4, 22.9, "Copy, move, delete, build", "Progress in the notch, with +N"),
        (23.3, 25.9, "AI needs approval?", "Allow it from the notch"),
        (26.3, 28.2, "Away? A robot keeps watch", "It grins when you come back"),
    ],
}
# (video seconds, line): each line starts at its time or just after the previous one ends.
VO = {
    "vi": [
        (0.15, "Windows của anh thiếu một thứ. PUW Notch đây!"),
        (3.3, "Nhạc đang phát? Hiện ngay trên notch."),
        (6.0, "Rê chuột vào, lời bài hát chạy theo luôn."),
        (8.85, "Ghi chú,"),
        (10.15, "Pomodoro,"),
        (11.45, "thông báo,"),
        (12.75, "nhắc uống nước,"),
        (14.05, "thời tiết,"),
        (15.35, "lịch âm dương,"),
        (16.65, "cả hạn mức AI!"),
        (18.9, "Wi-Fi, Bluetooth, cắm sạc, notch báo liền."),
        (22.1, "Copy file hay build code, có tiến độ luôn."),
        (25.0, "AI xin quyền? Duyệt ngay trên notch."),
        (28.0, "Đi đâu thì có robot trông nhà."),
        (30.2, "PUW Notch, miễn phí trên GitHub!"),
    ],
    "en": [
        (0.15, "Windows is missing something. Meet PUW Notch!"),
        (3.3, "Music? Right up here."),
        (6.0, "Hover in, the lyrics roll along."),
        (8.85, "Notes,"),
        (10.15, "Pomodoro,"),
        (11.45, "notifications,"),
        (12.75, "water breaks,"),
        (14.05, "weather,"),
        (15.35, "calendar,"),
        (16.65, "even your AI limits!"),
        (18.9, "Wi-Fi, Bluetooth, charging, instantly."),
        (22.1, "Copying or building? Live progress."),
        (25.0, "AI needs permission? Approve it here."),
        (28.0, "Step away, the robot keeps watch."),
        (30.2, "PUW Notch. Free on GitHub!"),
    ],
}
CARD = {
    "vi": ("Chiếc notch sống ngay trên Windows của bạn.", "Miễn phí trên GitHub · erikpuw/puw_notch"),
    "en": ("A notch that lives on your Windows.", "Free on GitHub · erikpuw/puw_notch"),
}
ICON_SIDE, ICON_Y = 280, 680
MINI_W, MINI_H = 88, 8  # the notch's first frame: the mini line (44 x 4) at 2x


def ease(t):
    t = min(max(t, 0.0), 1.0)
    return 1 - (1 - t) ** 3


def ease_back(t):
    t = min(max(t, 0.0), 1.0) - 1
    return 1 + 2.7 * t ** 3 + 1.7 * t ** 2


def faded(img, k):
    """`img` (RGBA) with its alpha scaled by k."""
    if k >= 0.999:
        return img
    out = img.copy()
    out.putalpha(img.getchannel("A").point(lambda v: round(v * max(k, 0.0))))
    return out


def paste(frame, img, x, y, k=1.0):
    if k > 0.01:
        img = faded(img, k)
        frame.paste(img, (round(x), round(y)), img)


def fit(path, size, text, width=W - 100):
    """The font at `size`, smaller while `text` would not fit in `width`."""
    font = ImageFont.truetype(path, size)
    while font.getlength(text) > width and size > 30:
        size -= 2
        font = ImageFont.truetype(path, size)
    return font


def gradient(w, h):
    x = np.linspace(0, 1, w)[None, :, None]
    a, b = np.array(ACCENT[0]), np.array(ACCENT[1])
    return Image.fromarray(np.repeat(a + (b - a) * x, h, axis=0).astype(np.uint8))


def rounded(img, radius):
    out = img.convert("RGBA")
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, img.width - 1, img.height - 1), radius, fill=255)
    out.putalpha(mask)
    return out


def chip(text, size):
    """The version chip: `text` on the accent gradient, rounded."""
    font = ImageFont.truetype(BOLD, size)
    w, h = round(font.getlength(text) + size * 1.1), round(size * 1.6)
    img = rounded(gradient(w, h), h // 2)
    ImageDraw.Draw(img).text((w / 2, h / 2), text, font=font, fill=(255, 255, 255, 255), anchor="mm")
    return img


def caption(title, subtitle):
    big, small = fit(BOLD, 76, title), fit(SEMI, 40, subtitle)
    img = Image.new("RGBA", (W, 230), (0, 0, 0, 0))
    shadow = Image.new("RGBA", img.size, (0, 0, 0, 0))
    ImageDraw.Draw(shadow).text((W / 2, 70), title, font=big, fill=(0, 0, 0, 200), anchor="mm")
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(10)))
    d = ImageDraw.Draw(img)
    d.text((W / 2, 70), title, font=big, fill=(255, 255, 255, 255), anchor="mm")
    if subtitle:
        d.text((W / 2, 170), subtitle, font=small, fill=(205, 205, 222, 255), anchor="mm")
    return img, round(big.getlength(title))


def pointer_img():
    s = 3
    pts = [(0, 0), (0, 17), (4.5, 13), (7.5, 20), (10, 19), (7, 12.2), (12.5, 12.2)]
    img = Image.new("RGBA", (48 * s, 64 * s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.polygon([(4 * s + x * 2.2 * s, 4 * s + y * 2.2 * s) for x, y in pts], fill=(0, 0, 0, 255))
    inner = [(1.6, 3.6), (1.6, 13.4), (4.9, 10.4), (8, 17.4), (8.4, 17.2), (5.4, 10.4), (9.3, 10.4)]
    d.polygon([(4 * s + x * 2.2 * s, 4 * s + y * 2.2 * s) for x, y in inner], fill=(255, 255, 255, 255))
    return img.resize((48, 64), Image.LANCZOS)


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


def blink(t, at=0.55):
    u = (t - at) / 0.25
    return 1.0 if not 0 <= u <= 1 else abs(1 - 2 * u) ** 0.8


def backdrop(glow_y):
    """Dark gradient with a violet glow centred at `glow_y`."""
    top, bottom = np.array([9, 9, 14]), np.array([22, 15, 34])
    ramp = np.linspace(0, 1, H)[:, None, None]
    bg = Image.fromarray((top + (bottom - top) * ramp).repeat(W, axis=1).astype(np.uint8)).convert("RGBA")
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse((W / 2 - 420, glow_y - 420, W / 2 + 420, glow_y + 420), fill=(120, 90, 230, 80))
    bg.alpha_composite(glow.filter(ImageFilter.GaussianBlur(110)))
    return bg.convert("RGB")


def desktop(wallpaper):
    """The wallpaper covering the virtual desktop (LAYER_W x CANVAS_H) as uint8 RGB."""
    wp = Image.open(wallpaper).convert("RGB")
    k = max(LAYER_W / wp.width, CANVAS_H / wp.height)
    wp = wp.resize((round(wp.width * k), round(wp.height * k)), Image.LANCZOS)
    x = (wp.width - LAYER_W) // 2
    return np.asarray(wp.crop((x, 0, x + LAYER_W, CANVAS_H)))


def camera_target(layer):
    """How far to zoom so the notch's current width fills most of the screen."""
    cols = np.nonzero(layer[::4, ::4, 3].max(axis=0) > 0.05)[0]
    width = (cols[-1] - cols[0] + 1) * 4 if len(cols) else 300
    return float(np.clip(880 / width, 0.62, 1.55))


def screen_view(wp, layer, k, shake_x):
    """What the screen shows at zoom k: the top-centre of the desktop with the notch layer over it."""
    cw, ch = min(LAYER_W, round(SW / k)), min(CANVAS_H, round(SH / k))
    x0 = int(np.clip(round(LAYER_W / 2 - cw / 2 + shake_x / k), 0, LAYER_W - cw))
    crop = wp[:ch, x0:x0 + cw].copy()
    if layer is not None:
        lh = min(LAYER_H, ch)
        lay = layer[:lh, x0:x0 + cw]
        crop[:lh] = (lay[..., :3] * 255 + crop[:lh] * (1 - lay[..., 3:4])).astype(np.uint8)
    return Image.fromarray(crop).resize((SW, SH), Image.BILINEAR), x0


class Camera:
    """A critically damped spring on the zoom, kicked on every cut."""

    def __init__(self, k):
        self.k, self.v = k, 0.0

    def step(self, target, dt, kick=0.0):
        self.v += kick
        omega = 11.0
        for _ in range(4):
            h = dt / 4
            self.v += (omega * omega * (target - self.k) - 2 * omega * self.v) * h
            self.k += self.v * h
        return self.k


def impulse(t, times, decay):
    """exp decay since the latest of `times` before t (0 when none)."""
    past = [t - s for s in times if 0 <= t - s < decay * 6]
    return float(np.exp(-min(past) / decay)) if past else 0.0


def pointer_at(t):
    for (t0, x0, y0), (t1, x1, y1) in zip(POINTER, POINTER[1:]):
        if t <= t1:
            k = ease((t - t0) / (t1 - t0)) if t >= t0 else 0.0
            return x0 + (x1 - x0) * k, y0 + (y1 - y0) * k
    return POINTER[-1][1:]


RATE = 48000
BPM = 128
DROP = INTRO  # the beat comes in as the notch appears


def lowpass(x, cut, order=2):
    b, a = butter(order, min(cut / (RATE / 2), 0.99))
    return lfilter(b, a, x).astype(np.float32)


def highpass(x, cut, order=2):
    b, a = butter(order, cut / (RATE / 2), btype="high")
    return lfilter(b, a, x).astype(np.float32)


def bandpass(x, lo, hi):
    b, a = butter(2, [lo / (RATE / 2), hi / (RATE / 2)], btype="band")
    return lfilter(b, a, x).astype(np.float32)


def read_wav(path):
    with wave.open(path, "rb") as f:
        assert f.getsampwidth() == 2 and f.getframerate() == RATE, path
        data = np.frombuffer(f.readframes(f.getnframes()), np.int16).astype(np.float32) / 32768
        return data.reshape(-1, f.getnchannels()).mean(axis=1)


def tts_ready():
    try:
        with urllib.request.urlopen(f"{TTS}/tts/health", timeout=3) as r:
            return json.load(r).get("ready", False)
    except OSError:
        return False


def voice_over(lang, ffmpeg, total):
    """The VO lines from the local TTS service, cached under target/promo/vo, laid on the video's clock."""
    cache = os.path.join(ROOT, "target", "promo", "vo")
    os.makedirs(cache, exist_ok=True)
    track = np.zeros(int(total * RATE), np.float32)
    end = 0.0
    for at, text in VO[lang]:
        key = hashlib.sha1(f"{VOICE}|{VO_TEMPO}|{text}".encode()).hexdigest()[:12]
        path = os.path.join(cache, f"{lang}-{key}.wav")
        if not os.path.exists(path):
            req = urllib.request.Request(f"{TTS}/tts/bytes", json.dumps({"text": text, "voice": VOICE}).encode(), {"Content-Type": "application/json"})
            raw = path + ".raw.wav"
            with urllib.request.urlopen(req, timeout=120) as r, open(raw, "wb") as f:
                f.write(r.read())
            subprocess.run([ffmpeg, "-y", "-loglevel", "error", "-i", raw, "-af", f"atempo={VO_TEMPO},silenceremove=start_periods=1:start_threshold=-45dB",
                            "-ac", "1", "-ar", str(RATE), "-c:a", "pcm_s16le", path], check=True)
            os.remove(raw)
        line = read_wav(path)
        start = max(at, end + 0.08)
        i = int(start * RATE)
        line = line[: len(track) - i]
        track[i:i + len(line)] += line
        end = start + len(line) / RATE
        print(f"  vo {start:5.2f}-{end:5.2f}  {text}")
    return track


def soundtrack(path, lang, ffmpeg):
    """The promo's sound, synthesised here (nothing licensed): a 128 BPM track, an effect on every
    move of the video and the voice-over with the music ducked under it. 16-bit stereo WAV."""
    total = INTRO + SECONDS
    n = int(total * RATE)
    music, sfx = np.zeros((n, 2), np.float32), np.zeros((n, 2), np.float32)
    rng = np.random.default_rng(7)

    def env(m, attack, release):
        x = np.arange(m) / RATE
        return np.minimum(1.0, x / max(attack, 1e-4)) * np.exp(-x / max(release, 1e-4))

    def put(bus, sig, at, gain=1.0, pan=0.0):
        i = int(at * RATE)
        if i >= n:
            return
        sig = sig[: n - i] * gain
        bus[i:i + len(sig), 0] += sig * (1 - max(pan, 0))
        bus[i:i + len(sig), 1] += sig * (1 + min(pan, 0))

    def sweep(f0, f1, dur, attack=0.004, release=0.08):
        m = int(dur * RATE)
        return np.sin(2 * np.pi * np.cumsum(np.geomspace(f0, f1, m)) / RATE) * env(m, attack, release)

    def bell(freq, dur=1.2, release=0.35):
        m = int(dur * RATE)
        x = np.arange(m) / RATE
        tone = np.sin(2 * np.pi * freq * x) + 0.35 * np.sin(2 * np.pi * freq * 2.76 * x) * np.exp(-x / 0.12)
        return tone * env(m, 0.002, release)

    def noise(dur):
        return rng.standard_normal(int(dur * RATE)).astype(np.float32)

    def whoosh(dur, up=True):
        x = np.linspace(0, 1, int(dur * RATE))
        return lowpass(noise(dur), 3000) * np.sin(np.pi * (x if up else 1 - x) ** 0.6) ** 2

    def click():
        return highpass(noise(0.03), 3000) * env(int(0.03 * RATE), 0.0005, 0.006) * 0.8 + sweep(3200, 2400, 0.03, 0.0005, 0.008) * 0.3

    def kick():
        body, tick = sweep(160, 42, 0.32, 0.001, 0.13), click()
        body[:len(tick)] += tick * 0.25
        return body

    def impact(big=1.0):
        boom = sweep(110, 32, 1.0, 0.001, 0.35)
        crash = highpass(noise(1.0), 4000) * env(int(1.0 * RATE), 0.001, 0.3 * big)
        return boom * 0.9 + crash * 0.25 * big

    def saw(freq, dur, harmonics=8):
        x = np.arange(int(dur * RATE)) / RATE
        return sum(np.sin(2 * np.pi * freq * h * x) / h for h in range(1, harmonics + 1))

    hz = lambda note: 440 * 2 ** ((note - 69) / 12)
    beat = 60 / BPM
    bar = 4 * beat
    chords = [[57, 60, 64, 67], [53, 57, 60, 64], [48, 52, 55, 59], [55, 59, 62, 66]]  # Am7 Fmaj7 Cmaj7 G
    v = lambda t: INTRO + t  # notch clock -> video clock
    card = v(CARD_AT)
    montage = (v(CLICKS[0]), v(16.8))

    # The track: a pumping pad, offbeat bass, kick on every beat, claps on 2 and 4, hats; 16th hats
    # and a plucked arpeggio while the pages fly by; it stops a beat before the card for the riser.
    pump = np.tile(1 - 0.75 * np.exp(-np.arange(int(beat * RATE)) / RATE / 0.08), 5)
    k = 0
    while DROP + k * bar < card - beat:
        t0 = DROP + k * bar
        notes = chords[k % 4]
        m = int(bar * RATE)
        pad = sum(saw(hz(nn), bar, 6) + saw(hz(nn) * 1.006, bar, 6) for nn in notes)
        pad = lowpass(pad, 1600) * pump[:m] * np.minimum(1, np.arange(m) / RATE / 0.02)
        put(music, pad / 10, t0, 0.55, pan=-0.15)
        put(music, pad / 10, t0 + 0.012, 0.55, pan=0.15)
        for b in range(4):
            at = t0 + b * beat
            if at + beat > card - 0.01:
                break
            put(music, kick(), at, 0.75)
            bass = lowpass(saw(hz(notes[0] - 24), beat / 2, 6), 420) * env(int(beat / 2 * RATE), 0.004, 0.16)
            put(music, bass, at + beat / 2, 0.55)
            if b % 2 == 1:
                put(music, bandpass(noise(0.2), 900, 3200) * env(int(0.2 * RATE), 0.001, 0.07), at, 0.38)
            busy = montage[0] <= at < montage[1]
            steps = 4 if busy else 2
            for s in range(steps):
                hat = highpass(noise(0.04), 7000) * env(int(0.04 * RATE), 0.0005, 0.012)
                put(music, hat, at + s * beat / steps, 0.12 if s == steps // 2 else 0.06, pan=0.3)
            if busy:
                for s in range(4):
                    f = hz(notes[(b + s) % 4] + 12)
                    m4 = int(0.18 * RATE)
                    pluck = (np.sin(2 * np.pi * f * np.arange(m4) / RATE) + 0.3 * saw(f, 0.18, 4)) * env(m4, 0.002, 0.05)
                    put(music, pluck, at + s * beat / 4, 0.07, pan=-0.35 if s % 2 else 0.35)
        k += 1

    m_intro = int(INTRO * RATE)
    x = np.arange(m_intro) / RATE
    rise = (x / INTRO) ** 2
    put(music, highpass(noise(INTRO), 1500) * rise * 0.5 + np.sin(2 * np.pi * np.cumsum(np.geomspace(200, 1600, m_intro)) / RATE) * rise * 0.15, 0, 0.6)
    put(music, sum(np.sin(2 * np.pi * hz(nn) * x) for nn in (45, 52, 57)) / 3 * np.minimum(1, x / 0.3), 0, 0.12)
    put(music, highpass(noise(0.8), 1200) * np.linspace(0, 1, int(0.8 * RATE)) ** 2, card - 0.8, 0.35)

    put(sfx, impact(0.6), 0.05, 0.5)                              # icon slams in
    put(sfx, click(), 0.55, 0.5)                                  # blink
    put(sfx, click(), 0.68, 0.35)
    put(sfx, whoosh(0.6), 0.95, 0.45, pan=-0.2)                   # flies up into the notch
    put(sfx, impact(1.0), DROP, 0.75)                             # the drop
    for f, d in ((784, 0), (1175, 0.09)):                         # music starts
        put(sfx, bell(f, 0.9, 0.3), v(1.6) + d, 0.22)
    put(sfx, whoosh(0.35), v(4.0), 0.35)                          # panel opens
    put(sfx, impact(0.3), v(4.3), 0.35)
    put(sfx, sweep(500, 1400, 0.2, 0.004, 0.08), v(4.9), 0.25)     # lyrics slide out
    for c in CLICKS:
        put(sfx, click(), v(c), 0.7, pan=0.25)                    # pointer clicks
        put(sfx, whoosh(0.25), v(c) + 0.02, 0.3, pan=-0.25)        # page slides
        put(sfx, sweep(120, 50, 0.25, 0.001, 0.08), v(c), 0.35)    # and lands
    put(sfx, whoosh(0.4, up=False), v(16.8), 0.35)                # panel closes
    for i, s in enumerate((17.2, 17.9, 18.6, 19.3)):              # status banners
        put(sfx, sweep(380 * 1.26 ** i, 1100 * 1.26 ** i, 0.14, 0.003, 0.06), v(s), 0.35)
        put(sfx, impact(0.2), v(s), 0.25)
    put(sfx, whoosh(0.35), v(20.4) - 0.1, 0.35)                   # copy starts
    for i in range(9):
        put(sfx, click(), v(20.5) + i * 0.15, 0.18, pan=0.2 * (-1) ** i)
    for f, d in ((1047, 0), (1568, 0.1)):                         # build finished
        put(sfx, bell(f, 1.0, 0.35), v(21.8) + d, 0.25)
    for s in (23.3, 24.7):                                        # approval banners
        for f, d in ((988, 0), (1319, 0.12)):
            put(sfx, bell(f, 1.0, 0.3), v(s) + d, 0.28)
        put(sfx, impact(0.3), v(s), 0.3)
    for i, note in enumerate((84, 88, 91, 96)):                   # the robot shows up
        put(sfx, sweep(hz(note), hz(note) * 1.02, 0.09, 0.003, 0.05), v(26.3) + i * 0.07, 0.25)
    put(sfx, impact(1.2), card, 0.8)                              # closing card
    for i, note in enumerate((69, 72, 76, 79, 84)):
        put(sfx, bell(hz(note), 2.4, 0.9), card + 0.05 + i * 0.05, 0.14, pan=(i - 2) * 0.15)

    vo = voice_over(lang, ffmpeg, total)
    talk = lowpass(np.abs(vo), 6, order=1)
    duck = lowpass(np.clip(talk / (talk.max() + 1e-6) * 8, 0, 1), 3, order=1)
    mix = music * (1 - 0.6 * duck)[:, None] + sfx * (1 - 0.3 * duck)[:, None] + vo[:, None] * 1.5
    tail = int(0.8 * RATE)
    mix[-tail:] *= np.linspace(1, 0, tail)[:, None]
    mix /= max(1.0, np.abs(mix).max() / 0.89)
    with wave.open(path, "wb") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(RATE)
        f.writeframes((mix * 32767).astype(np.int16).tobytes())


def intro_frame(back, base, tag, t, k0):
    """The intro at `t`: the icon slams in on the dark backdrop with the version under it and blinks;
    then it shrinks, turns black and flies up into the notch's first frame at the top of the screen."""
    morph0, morph1 = 0.95, 1.55
    m = ease((t - morph0) / (morph1 - morph0))
    frame = Image.blend(back, base, ease((t - morph0) / 0.55))
    pop = ease_back((t - 0.05) / 0.3)
    dx = np.sin(t * 90) * 16 * np.exp(-(t - 0.2) / 0.06) if t >= 0.2 else 0.0
    if pop > 0.01:
        mw, mh = MINI_W * k0, MINI_H * k0
        w = (ICON_SIDE + (mw - ICON_SIDE) * m) * (pop if m == 0 else 1)
        h = (ICON_SIDE + (mh - ICON_SIDE) * m) * (pop if m == 0 else 1)
        cy = ICON_Y + ICON_SIDE / 2 + (SCREEN[1] + mh / 2 - ICON_Y - ICON_SIDE / 2) * m
        x, y = W / 2 - w / 2 + dx, cy - h / 2
        if m > 0:
            shape = Image.new("RGBA", (max(1, round(w)), max(1, round(h))), (0, 0, 0, 0))
            ImageDraw.Draw(shape).rounded_rectangle((0, 0, shape.width - 1, shape.height - 1), min(w, h) * (0.22 + 0.28 * m), fill=(0, 0, 0, 255))
            paste(frame, shape, x, y)
        paste(frame, icon_img(w, h, blink(t)), x, y, min(1.0, (t - 0.05) / 0.12) * (1 - ease(m / 0.45)))
    s = ease_back((t - 0.3) / 0.3)
    if s > 0.01:
        img = tag.resize((max(1, round(tag.width * s)), max(1, round(tag.height * s))), Image.BILINEAR)
        paste(frame, img, (W - img.width) / 2 + dx, ICON_Y + ICON_SIDE + 60 + (tag.height - img.height) / 2, 1 - ease((t - morph0) / 0.25))
    return frame


def main():
    lang = sys.argv[1]
    ffmpeg = sys.argv[2] if len(sys.argv) > 2 else "ffmpeg"
    wallpaper = sys.argv[3] if len(sys.argv) > 3 else os.path.join(os.environ["APPDATA"], "Microsoft", "Windows", "Themes", "TranscodedWallpaper")
    if not tts_ready():
        sys.exit(f"The TTS service at {TTS} is not running: start it, then run this again.")
    ver = version()
    src = os.path.join(ROOT, "target", "promo", f"notch-{lang}.mov")
    dst = os.path.join(ROOT, "target", "promo", f"puw-notch-promo-{lang}.mp4")
    audio = os.path.join(ROOT, "target", "promo", f"soundtrack-{lang}.wav")
    soundtrack(audio, lang, ffmpeg)

    wp = desktop(wallpaper)
    bg = backdrop(SCREEN[1] + SH / 2).convert("RGBA")
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    l, t, r, b = SCREEN
    ImageDraw.Draw(glow).rounded_rectangle((l - 8, t - 8, r + 8, b + 8), 48, fill=(130, 100, 230, 90))
    bg.alpha_composite(glow.filter(ImageFilter.GaussianBlur(30)))
    bg = bg.convert("RGB")
    mask = Image.new("L", (SW, SH), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, SW - 1, SH - 1), 40, fill=255)
    border = Image.new("RGBA", (SW, SH), (0, 0, 0, 0))
    ImageDraw.Draw(border).rounded_rectangle((0, 0, SW - 1, SH - 1), 40, outline=(255, 255, 255, 40), width=2)
    header = Image.new("RGBA", (W, 140), (0, 0, 0, 0))
    name_font = ImageFont.truetype(BOLD, 50)
    tag = chip(ver, 30)
    name_w = name_font.getlength("PUW Notch")
    hx = (W - name_w - 18 - tag.width) / 2
    ImageDraw.Draw(header).text((hx, 70), "PUW Notch", font=name_font, fill=(255, 255, 255, 255), anchor="lm")
    header.alpha_composite(tag, (round(hx + name_w + 18), 70 - tag.height // 2))

    captions = [(a, c, caption(ti, su)) for a, c, ti, su in CAPTIONS[lang]]
    pointer = pointer_img()
    pressed = pointer.resize((40, 53), Image.LANCZOS)
    back = backdrop(ICON_Y + ICON_SIDE / 2)
    big_tag = chip(ver, 44)
    slogan, link = CARD[lang]
    card_icon = Image.open(os.path.join(ROOT, "assets", "puw_1024.png")).convert("RGBA").resize((260, 260), Image.LANCZOS)
    card_words = Image.new("RGBA", (W, 520), (0, 0, 0, 0))
    d = ImageDraw.Draw(card_words)
    d.text((W / 2, 70), "PUW Notch", font=ImageFont.truetype(BOLD, 116), fill=(255, 255, 255, 255), anchor="mm")
    card_words.alpha_composite(big_tag, ((W - big_tag.width) // 2, 160))
    d.text((W / 2, 300), slogan, font=fit(SEMI, 46, slogan), fill=(225, 225, 235, 255), anchor="mm")
    d.text((W / 2, 380), link, font=fit(REGULAR, 38, link), fill=(165, 165, 185, 255), anchor="mm")
    white, black = Image.new("RGB", (W, H), (255, 255, 255)), Image.new("RGB", (W, H), (0, 0, 0))

    reader = subprocess.Popen([ffmpeg, "-loglevel", "error", "-i", src, "-f", "rawvideo", "-pix_fmt", "bgra", "-"], stdout=subprocess.PIPE)
    writer = subprocess.Popen([ffmpeg, "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-", "-i", audio,
                               "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-af", "loudnorm=I=-14:TP=-1.5:LRA=11", "-ar", str(RATE),
                               "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart", dst], stdin=subprocess.PIPE)
    size = LAYER_W * LAYER_H * 4
    read = lambda raw: np.frombuffer(raw, np.uint8).reshape(LAYER_H, LAYER_W, 4)[..., [2, 1, 0, 3]].astype(np.float32) / 255.0
    layer = read(reader.stdout.read(size))
    camera = Camera(camera_target(layer))
    k0 = camera.k
    base0 = bg.copy()
    view0, _ = screen_view(wp, None, k0, 0)
    base0.paste(view0, SCREEN[:2], mask)
    base0.paste(border, SCREEN[:2], border)
    first = True
    for i in range(round(FPS * (INTRO + SECONDS))):
        t = i / FPS - INTRO
        if t < 0:
            writer.stdin.write(intro_frame(back, base0, big_tag, t + INTRO, k0).tobytes())
            continue
        if not first:
            raw = reader.stdout.read(size)
            if len(raw) < size:
                break
            layer = read(raw)
        first = False
        kick = sum(1.6 if s in FLASHES else 0.9 for s in PUNCHES if 0 <= t - s < 1 / FPS - 1e-6)
        k = camera.step(camera_target(layer), 1 / FPS, kick)
        jolt = impulse(t, PUNCHES, 0.07)
        shake = (np.sin(t * 93) * 14 * jolt, np.cos(t * 71) * 9 * jolt)
        view, x0 = screen_view(wp, layer, k, shake[0])
        frame = bg.copy()
        frame.paste(view, (SCREEN[0], SCREEN[1] + round(shake[1])), mask)
        frame.paste(border, (SCREEN[0], SCREEN[1] + round(shake[1])), border)
        paste(frame, header, 0, 80, ease(t / 0.4))
        for a, c, (block, tw) in captions:
            if a <= t <= c:
                u = (t - a) / 0.22
                s = 1 + 0.4 * (1 - ease_back(u))
                out = ease((t - (c - 0.15)) / 0.15)
                img = block if s < 1.002 else block.resize((round(W * s), round(230 * s)), Image.BILINEAR)
                paste(frame, img, (W - img.width) / 2 - 140 * out + shake[0] * 0.5, 1180 - (img.height - 230) / 2, min(1.0, u * 3) * (1 - out))
                bar_w = round((tw + 40) * ease((t - a - 0.08) / 0.3))
                if bar_w > 8:
                    paste(frame, rounded(gradient(bar_w, 8), 4), (W - bar_w) / 2 - 140 * out, 1300, 1 - out)
        if POINTER_SHOW[0] <= t <= POINTER_SHOW[1]:
            cx, cy = pointer_at(t)
            x, y = SCREEN[0] + (cx - x0) * k, SCREEN[1] + cy * k
            fade = min(1.0, (t - POINTER_SHOW[0]) / 0.2, (POINTER_SHOW[1] - t) / 0.3)
            for c in CLICKS:
                if 0 <= t - c < 0.4:
                    q = (t - c) / 0.4
                    ring = Image.new("RGBA", (140, 140), (0, 0, 0, 0))
                    rr = 12 + 52 * ease(q)
                    ImageDraw.Draw(ring).ellipse((70 - rr, 70 - rr, 70 + rr, 70 + rr), outline=(255, 255, 255, round(220 * (1 - q))), width=5)
                    paste(frame, ring, x - 70, y - 70)
            paste(frame, pressed if any(0 <= t - c < 0.12 for c in CLICKS) else pointer, x - 4, y - 4, fade)
        flash = impulse(t, FLASHES, 0.06) * 0.45
        if flash > 0.01:
            frame = Image.blend(frame, white, flash)
        if t >= CARD_AT:
            q = (t - CARD_AT) / 0.5
            frame = Image.blend(frame, black, 0.85 * ease(q))
            if t - CARD_AT < 0.2:
                frame = Image.blend(frame, white, 0.5 * (1 - (t - CARD_AT) / 0.2))
            s = ease_back((t - CARD_AT - 0.05) / 0.45)
            if s > 0.01:
                side = max(2, round(260 * s))
                paste(frame, card_icon.resize((side, side), Image.LANCZOS), (W - side) / 2, 560 + (260 - side) / 2, min(1.0, q * 2))
            kw = ease((t - CARD_AT - 0.3) / 0.45)
            paste(frame, card_words, 0, 880 + 40 * (1 - kw), kw)
        writer.stdin.write(frame.tobytes())
    writer.stdin.close()
    writer.wait()
    reader.wait()
    print(dst)


if __name__ == "__main__":
    main()
