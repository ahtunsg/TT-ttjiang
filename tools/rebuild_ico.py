"""Derive every ICO frame from the single 256px master artwork.

The shipped app.ico used to mix two art styles: 256/128 were the smooth line
drawing, while 64/48/32/24 were hand-pixelled (much heavier rabbit) and 16 was
an unreadable red blob. Explorer, the taskbar and the window title all pick
from the small frames, so the icon "looked wrong" next to the original art.

Now there is exactly one source of truth: the 256px master. Every other frame
is regrown from it with the same pipeline, so all sizes share one drawing:

  1. project the master onto the BG->INK axis to get an ink-coverage map;
  2. optionally dilate by 1px so a hairline stroke still lands on a pixel at
     small sizes (0.5px of coverage LANCZOS-downscales into invisible grey);
  3. Lanczos-downscale coverage and disc mask to the target size;
  4. re-quantise coverage onto the two artwork colours with a contrast window,
     keeping a soft alpha rim from the disc mask.

Only frames under 64px get the 1px dilation; 64px and up scale straight down,
which is what keeps their line weight identical to the master.

    python tools/rebuild_ico.py assets/app.ico            # rewrite in place
    python tools/rebuild_ico.py assets/app.ico --check    # report only
    python tools/rebuild_ico.py assets/app.ico --preview DIR   # dump frames
"""

import io
import struct
import sys
from pathlib import Path

from PIL import Image, ImageFilter

MASTER = 256

# size -> (dilate radius at master res, contrast lo, contrast hi)
SPEC = {
    256: (0, 0.22, 0.55),
    128: (0, 0.22, 0.55),
    64: (0, 0.22, 0.55),
    48: (1, 0.26, 0.58),
    32: (1, 0.26, 0.58),
    24: (1, 0.26, 0.58),
    16: (1, 0.26, 0.58),
}
ORDER = [256, 128, 64, 48, 32, 24, 16]

BG = (253, 240, 240)   # pale pink disc
INK = (150, 24, 26)    # deep red stroke


def read_frames(path):
    b = Path(path).read_bytes()
    _, _, count = struct.unpack_from('<HHH', b, 0)
    out = {}
    for i in range(count):
        w, h, cl, rsv, planes, bits, size, off = struct.unpack_from('<BBBBHHII', b, 6 + i * 16)
        out[w or 256] = b[off:off + size]
    return out


def master_rgba(blobs):
    img = Image.open(io.BytesIO(blobs[MASTER]))
    img.load()
    return img.convert('RGBA')


def coverage(src):
    """(ink, disc) as 8-bit L images at master resolution."""
    px = src.load()
    w, h = src.size
    ink = Image.new('L', (w, h), 0)
    disc = Image.new('L', (w, h), 0)
    ip, dp = ink.load(), disc.load()
    dv = ((INK[0] - BG[0]) ** 2 + (INK[1] - BG[1]) ** 2 + (INK[2] - BG[2]) ** 2)
    for y in range(h):
        for x in range(w):
            r, g, b, a = px[x, y]
            if a < 9:
                continue
            dp[x, y] = 255
            # projection along BG->INK tells how much ink covers the pixel
            t = ((r - BG[0]) * (INK[0] - BG[0]) + (g - BG[1]) * (INK[1] - BG[1])
                 + (b - BG[2]) * (INK[2] - BG[2])) / dv
            if t > 0.02:
                ip[x, y] = max(0, min(255, int(t * 255)))
    return ink, disc


def synth(ink, disc, size, grow, lo, hi):
    if grow > 0:
        ink = ink.filter(ImageFilter.MaxFilter(2 * grow + 1))
    i_s = ink.resize((size, size), Image.LANCZOS)
    d_s = disc.resize((size, size), Image.LANCZOS)
    ip, dp = i_s.load(), d_s.load()
    out = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    op = out.load()
    for y in range(size):
        for x in range(size):
            if dp[x, y] < 70:
                continue
            c = max(0.0, min(1.0, (ip[x, y] / 255.0 - lo) / (hi - lo)))
            r = round(BG[0] + (INK[0] - BG[0]) * c)
            g = round(BG[1] + (INK[1] - BG[1]) * c)
            b = round(BG[2] + (INK[2] - BG[2]) * c)
            op[x, y] = (r, g, b, min(255, int(dp[x, y] * 1.2)))
    return out


def encode(img):
    buf = io.BytesIO()
    img.save(buf, format='PNG', optimize=True)
    return buf.getvalue()


def write_ico(blobs, path):
    entries = [(n, blobs[n]) for n in ORDER if n in blobs]
    header = struct.pack('<HHH', 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dirs, body = b'', b''
    for n, data in entries:
        d = 0 if n >= 256 else n
        dirs += struct.pack('<BBBBHHII', d, d, 0, 0, 1, 32, len(data), offset + len(body))
        body += data
    Path(path).write_bytes(header + dirs + body)


def frames_from_master(path):
    blobs = read_frames(path)
    if MASTER not in blobs:
        raise SystemExit(f'{path}: no {MASTER}px frame to derive from')
    src = master_rgba(blobs)
    ink, disc = coverage(src)
    out = {MASTER: blobs[MASTER]}
    for size in ORDER:
        if size == MASTER:
            continue
        out[size] = encode(synth(ink, disc, size, *SPEC[size]))
    return out


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    path = Path(sys.argv[1])

    if '--check' in sys.argv:
        blobs = read_frames(path)
        print(f'{path}: {len(blobs)} frames {sorted(blobs, reverse=True)}')
        for n in sorted(blobs, reverse=True):
            d = blobs[n]
            kind = 'PNG' if d[:4] == b'\x89PNG' else 'BMP'
            print(f'  {n:3}x{n:<3} {len(d):6}B {kind}')
        return

    out = frames_from_master(path)

    if '--preview' in sys.argv:
        dest = Path(sys.argv[sys.argv.index('--preview') + 1])
        dest.mkdir(parents=True, exist_ok=True)
        for n in ORDER:
            (dest / f'{n}.png').write_bytes(out[n])
            print(f'  preview {n:3}px -> {dest / f"{n}.png"}')
        return

    backup = path.with_name(path.stem + '.ico.orig')
    if not backup.exists():
        backup.write_bytes(path.read_bytes())
        print(f'backup -> {backup}')

    write_ico(out, path)
    print(f'{path}: {path.stat().st_size} bytes, frames {ORDER}')


if __name__ == '__main__':
    main()
