#!/usr/bin/env python3
"""Check that an exe carries the brand icon as a PE resource (not just at runtime).

Usage: python tools/check_icon.py target/release/ttjiang.exe [assets/app.ico]

Explorer, shortcuts and Alt+Tab read the icon from the resource directory, so a
build that only creates an HICON at runtime looks blank there even though the
taskbar is fine. This walks RT_GROUP_ICON/RT_ICON and, when the source ICO is
given, compares every frame byte for byte.
"""
import struct
import sys


def sections(b):
    pe = struct.unpack_from('<I', b, 0x3c)[0]
    magic = struct.unpack_from('<H', b, pe + 24)[0]
    dd = pe + 24 + (112 if magic == 0x20b else 96)
    rva, size = struct.unpack_from('<II', b, dd + 16)
    nsec = struct.unpack_from('<H', b, pe + 6)[0]
    opt = struct.unpack_from('<H', b, pe + 20)[0]
    so = pe + 24 + opt
    secs = [struct.unpack_from('<IIII', b, so + i * 40 + 8) for i in range(nsec)]
    return rva, size, secs


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else 'target/release/ttjiang.exe'
    ico_path = sys.argv[2] if len(sys.argv) > 2 else None
    b = open(path, 'rb').read()
    rva, size, secs = sections(b)
    if not rva:
        print(f'{path}: NO resource directory -> Explorer shows the generic icon')
        return 1

    def r2o(r):
        for vs, va, rs, rp in secs:
            if va <= r < va + max(vs, rs):
                return r - va + rp
        raise ValueError(hex(r))

    base = r2o(rva)

    def dirents(o):
        named, ids = struct.unpack_from('<HH', b, base + o + 12)
        return [struct.unpack_from('<II', b, base + o + 16 + i * 8) for i in range(named + ids)]

    def blob(o):
        r, sz = struct.unpack_from('<II', b, base + o)
        return b[r2o(r):r2o(r) + sz]

    res = {}
    for tn, to in dirents(0):
        for n, to2 in dirents(to & 0x7fffffff):
            for _, dt in dirents(to2 & 0x7fffffff):
                res.setdefault(tn, {})[n] = blob(dt)

    icons = res.get(3, {})
    groups = res.get(14, {})
    print(f'{path}: resource rva={rva:#x} size={size} | RT_ICON ids={sorted(icons)} '
          f'| RT_GROUP_ICON ids={sorted(groups)}')
    if not groups:
        print('NO icon group -> Explorer shows the generic icon')
        return 1

    gid = sorted(groups)[0]
    grp = groups[gid]
    _, typ, cnt = struct.unpack_from('<HHH', grp, 0)
    print(f'group {gid}: type={typ} frames={cnt}')
    ok = typ == 1 and cnt > 0

    if ico_path:
        ico = open(ico_path, 'rb').read()
        for i in range(cnt):
            w, h, colors, _, planes, bits, gsz, nid = struct.unpack_from('<BBBBHHIH', grp, 6 + i * 14)
            iw, ih, _, _, _, ibits, isz, ioff = struct.unpack_from('<BBBBHHII', ico, 6 + i * 16)
            same = (icons.get(nid) == ico[ioff:ioff + isz] and gsz == isz
                    and (w, h) == (iw, ih) and bits == ibits)
            ok &= same
            print(f'  {w or 256}x{h or 256} {bits}bpp id={nid} {gsz}B {"ok" if same else "MISMATCH"}')
    else:
        for i in range(cnt):
            w, h, colors, _, planes, bits, gsz, nid = struct.unpack_from('<BBBBHHIH', grp, 6 + i * 14)
            ok &= nid in icons and len(icons[nid]) == gsz
            print(f'  {w or 256}x{h or 256} {bits}bpp id={nid} {gsz}B')
            ok &= nid in icons and len(icons[nid]) == gsz

    print('ICON RESOURCE OK' if ok else 'ICON RESOURCE BROKEN')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
