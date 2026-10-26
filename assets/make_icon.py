"""PUW Notch app icon (run: python make_icon.py <assets dir> <preview dir>; needs Pillow): the idle
robot's two square eyes, black on white, inside a dark rounded border. Drawn at 1024 px, downsampled."""
import sys
import io, struct
from PIL import Image, ImageDraw
N = 1024
icon = Image.new('RGBA', (N, N), (0, 0, 0, 0))
d = ImageDraw.Draw(icon)
# white rounded square inside a dark border
border, radius = N * 0.065, N * 0.225
d.rounded_rectangle((0, 0, N - 1, N - 1), radius=radius, fill=(28, 28, 34, 255))
d.rounded_rectangle((border, border, N - 1 - border, N - 1 - border), radius=radius - border, fill=(250, 250, 252, 255))
# two square eyes
ew, eh, gap, rad = N * 0.17, N * 0.27, N * 0.15, N * 0.035
cy = N * 0.5
for x0 in ((N - gap) / 2 - ew, (N + gap) / 2):
    d.rounded_rectangle((x0, cy - eh / 2, x0 + ew, cy + eh / 2), radius=rad, fill=(20, 20, 24, 255))
out = sys.argv[1]
icon.save(out + '/puw_1024.png')
sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
frames = {s: icon.resize((s, s), Image.LANCZOS) for s in sizes}
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
