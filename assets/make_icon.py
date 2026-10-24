"""PUW Notch app icon (run: python make_icon.py <assets dir> <preview dir>; needs Pillow): rounded square, indigo->violet gradient, black iPhone-style notch hanging
from the top edge with a green battery ring and white equaliser bars. Drawn at 1024 px, downsampled."""
import sys, math
from PIL import Image, ImageDraw, ImageFilter
N = 1024
def lerp(a, b, t): return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))
# background gradient (diagonal)
bg = Image.new('RGB', (N, N))
top, bot = (49, 46, 129), (124, 58, 237)
px = bg.load()
for y in range(N):
    for x in range(N):
        px[x, y] = lerp(top, bot, (x * 0.35 + y * 0.65) / N)
# soft light from the top-left
glow = Image.new('L', (N, N), 0)
ImageDraw.Draw(glow).ellipse((-N * 0.3, -N * 0.45, N * 0.9, N * 0.55), fill=70)
glow = glow.filter(ImageFilter.GaussianBlur(N * 0.08))
bg = Image.composite(Image.new('RGB', (N, N), (255, 255, 255)), bg, glow.point(lambda v: v // 3))
# rounded-square mask
mask = Image.new('L', (N, N), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, N - 1, N - 1), radius=int(N * 0.225), fill=255)
icon = Image.new('RGBA', (N, N), (0, 0, 0, 0))
icon.paste(bg, (0, 0), mask)
d = ImageDraw.Draw(icon)
# notch hanging from the top edge, concave ears
w, h, r, ear = N * 0.70, N * 0.30, N * 0.11, N * 0.06
l, rr = (N - w) / 2, (N + w) / 2
d.rounded_rectangle((l, -r, rr, h), radius=r, fill=(0, 0, 0, 255))
d.rectangle((l - ear, 0, rr + ear, ear), fill=(0, 0, 0, 255))
for cx in (l - ear, rr + ear):   # carve the ears back out
    d.ellipse((cx - ear, 0, cx + ear, 2 * ear), fill=(0, 0, 0, 0))
# re-clip ears to the rounded square background (the carve punched transparent holes)
holes = Image.new('RGBA', (N, N), (0, 0, 0, 0))
holes.paste(bg, (0, 0), mask)
for cx in (l - ear, rr + ear):
    m = Image.new('L', (N, N), 0)
    ImageDraw.Draw(m).ellipse((cx - ear, 0, cx + ear, 2 * ear), fill=255)
    icon.paste(holes, (0, 0), Image.composite(m, Image.new('L', (N, N), 0), mask))
# battery ring (green, ~75%) on the left of the notch
cy = h * 0.52
rc = (l + N * 0.10, cy); rad = N * 0.062; st = int(N * 0.022)
d.ellipse((rc[0] - rad, rc[1] - rad, rc[0] + rad, rc[1] + rad), outline=(70, 70, 76, 255), width=st)
d.arc((rc[0] - rad, rc[1] - rad, rc[0] + rad, rc[1] + rad), start=-90, end=-90 + 270, fill=(52, 214, 74, 255), width=st)
# equaliser bars (white) on the right
bw = N * 0.03
for i, k in enumerate((0.45, 0.85, 0.6, 1.0)):
    bh = N * 0.12 * k
    x = rr - N * 0.27 + i * (bw + N * 0.022)
    d.rounded_rectangle((x, cy - bh / 2, x + bw, cy + bh / 2), radius=bw / 2, fill=(255, 255, 255, 255))
# "PUW" wordmark below the notch: only on sizes where it stays readable
from PIL import ImageFont
import io, struct
plain = icon.copy()
marked = icon.copy()
font = ImageFont.truetype(r'C:\Windows\Fonts\segoeuib.ttf', int(N * 0.22))
md = ImageDraw.Draw(marked)
tw = md.textlength("PUW", font=font)
md.text(((N - tw) / 2, N * 0.56), "PUW", font=font, fill=(255, 255, 255, 235))
out = sys.argv[1]
marked.save(out + '/puw_1024.png')
sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
frames = {s: (marked if s >= 48 else plain).resize((s, s), Image.LANCZOS) for s in sizes}
pngs = {}
for s, im in frames.items():
    b = io.BytesIO(); im.save(b, 'PNG'); pngs[s] = b.getvalue()
# .ico: PNG-compressed entries
ico = struct.pack('<HHH', 0, 1, len(sizes)); off = 6 + 16 * len(sizes); body = b''
for s in sizes:
    data = pngs[s]
    ico += struct.pack('<BBBBHHII', s % 256, s % 256, 0, 0, 1, 32, len(data), off + len(body)); body += data
open(out + '/puw.ico', 'wb').write(ico + body)
# .res for the MSVC linker: RT_ICON per size (ids 1..n) + RT_GROUP_ICON id 1
def entry(rtype, name, data):
    hdr = struct.pack('<HH', 0xFFFF, rtype) + struct.pack('<HH', 0xFFFF, name)
    hdr += struct.pack('<IHHII', 0, 0x1030, 0x0409, 0, 0)
    head = struct.pack('<II', len(data), 8 + len(hdr)) + hdr
    pad = (4 - len(data) % 4) % 4
    return head + data + b'\0' * pad
# leading empty resource header
res = struct.pack('<II', 0, 32) + struct.pack('<HHHH', 0xFFFF, 0, 0xFFFF, 0) + struct.pack('<IHHII', 0, 0, 0, 0, 0)
group = struct.pack('<HHH', 0, 1, len(sizes))
for n, s in enumerate(sizes, 1):
    res += entry(3, n, pngs[s])
    group += struct.pack('<BBBBHHIH', s % 256, s % 256, 0, 0, 1, 32, len(pngs[s]), n)
res += entry(14, 1, group)
open(out + '/puw.res', 'wb').write(res)
prev = Image.new('RGBA', (sum(sizes) + 12 * len(sizes), 270), (40, 40, 40, 255)); x = 6
for s in sizes:
    prev.alpha_composite(frames[s], (x, 270 - s - 6)); x += s + 12
prev.save(sys.argv[2] + '/icon_preview.png')
print('ok')
