"""Render the icon Windows actually pulls out of the exe, at real pixel size.

ExtractIconEx gives the shell's small (16) and large (32) icon; we draw them
with DrawIconEx onto a 32bpp DIB and save a PNG, then upscale 8x with NEAREST
so the pixels are visible for review.

    python tools/render_exe_icon.py target/release/ttjiang.exe out_dir
"""

import ctypes
import sys
from ctypes import wintypes
from pathlib import Path

from PIL import Image

user32 = ctypes.WinDLL('user32', use_last_error=True)
gdi32 = ctypes.WinDLL('gdi32', use_last_error=True)
shell32 = ctypes.WinDLL('shell32', use_last_error=True)

LRESULT = ctypes.c_ssize_t
shell32.ExtractIconExW.argtypes = [wintypes.LPCWSTR, ctypes.c_int,
                                  ctypes.POINTER(wintypes.HICON),
                                  ctypes.POINTER(wintypes.HICON), wintypes.UINT]
shell32.ExtractIconExW.restype = wintypes.UINT
user32.DestroyIcon.argtypes = [wintypes.HICON]
user32.DrawIconEx.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int,
                              wintypes.HICON, ctypes.c_int, ctypes.c_int,
                              wintypes.UINT, wintypes.HBRUSH, wintypes.UINT]
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateDIBSection.argtypes = [wintypes.HDC, ctypes.c_void_p, wintypes.UINT,
                                   ctypes.POINTER(ctypes.c_void_p),
                                   wintypes.HANDLE, wintypes.DWORD]
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [('biSize', wintypes.DWORD), ('biWidth', wintypes.LONG),
                ('biHeight', wintypes.LONG), ('biPlanes', wintypes.WORD),
                ('biBitCount', wintypes.WORD), ('biCompression', wintypes.DWORD),
                ('biSizeImage', wintypes.DWORD), ('biXPelsPerMeter', wintypes.LONG),
                ('biYPelsPerMeter', wintypes.LONG), ('biClrUsed', wintypes.DWORD),
                ('biClrImportant', wintypes.DWORD)]


def render(hicon, size):
    screen = user32.GetDC(None)
    mem = gdi32.CreateCompatibleDC(screen)
    bi = BITMAPINFOHEADER()
    bi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.biWidth = size
    bi.biHeight = -size          # top-down
    bi.biPlanes = 1
    bi.biBitCount = 32
    bi.biCompression = 0         # BI_RGB
    bits = ctypes.c_void_p()
    gdi32.CreateDIBSection.restype = wintypes.HBITMAP
    hbm = gdi32.CreateDIBSection(mem, ctypes.byref(bi), 0, ctypes.byref(bits), None, 0)
    gdi32.SelectObject(mem, hbm)
    user32.DrawIconEx(mem, 0, 0, hicon, size, size, 0, None, 3)  # DI_NORMAL
    gdi32.GdiFlush()
    raw = ctypes.string_at(bits, size * size * 4)
    img = Image.frombuffer('RGBA', (size, size), raw, 'raw', 'BGRA', 0, 1).convert('RGBA')
    gdi32.DeleteObject(hbm)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(None, screen)
    return img


def main():
    exe = Path(sys.argv[1]).resolve()
    out = Path(sys.argv[2] if len(sys.argv) > 2 else 'icon_render')
    out.mkdir(parents=True, exist_ok=True)

    large = wintypes.HICON()
    small = wintypes.HICON()
    n = shell32.ExtractIconExW(str(exe), 0, ctypes.byref(large), ctypes.byref(small), 1)
    print(f'ExtractIconEx -> {n} icon(s)')
    if not n:
        raise SystemExit(f'no icon in {exe}')

    for name, h, size in (('small16', small.value, 16), ('large32', large.value, 32)):
        img = render(h, size)
        img.save(out / f'{name}.png')
        img.resize((size * 8, size * 8), Image.NEAREST).save(out / f'{name}_8x.png')
        print(f'  {name}: {img.size} -> {out / f"{name}.png"}')

    # composite the two sizes on white, side by side, enlarged for review
    a = Image.open(out / 'small16_8x.png')
    b = Image.open(out / 'large32_8x.png')
    sheet = Image.new('RGBA', (a.width + b.width + 60, max(a.height, b.height) + 20),
                      (255, 255, 255, 255))
    sheet.paste(a, (10, 10), a)
    sheet.paste(b, (a.width + 40, 10), b)
    sheet.save(out / 'sheet.png')
    print(f'sheet -> {out / "sheet.png"}')

    if large.value:
        user32.DestroyIcon(large)
    if small.value:
        user32.DestroyIcon(small)


if __name__ == '__main__':
    main()
