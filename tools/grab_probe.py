"""Decisive control: same GDI calls from CPython/ctypes in this session."""
import ctypes
from ctypes import wintypes

u32 = ctypes.windll.user32
g32 = ctypes.windll.gdi32
k32 = ctypes.windll.kernel32

u32.GetDC.argtypes = [wintypes.HWND]
u32.GetDC.restype = wintypes.HDC
u32.GetSystemMetrics.argtypes = [ctypes.c_int]
u32.GetSystemMetrics.restype = ctypes.c_int
u32.SetProcessDPIAware.argtypes = []
u32.SetProcessDPIAware.restype = wintypes.BOOL
u32.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]

g32.CreateCompatibleDC.argtypes = [wintypes.HDC]
g32.CreateCompatibleDC.restype = wintypes.HDC
g32.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
g32.CreateCompatibleBitmap.restype = wintypes.HBITMAP
g32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
g32.SelectObject.restype = wintypes.HGDIOBJ
g32.GetPixel.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
g32.GetPixel.restype = wintypes.DWORD
g32.GetDeviceCaps.argtypes = [wintypes.HDC, ctypes.c_int]
g32.GetDeviceCaps.restype = ctypes.c_int
g32.BitBlt.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int,
                       ctypes.c_int, ctypes.c_int, wintypes.HDC,
                       ctypes.c_int, ctypes.c_int, wintypes.DWORD]
g32.BitBlt.restype = wintypes.BOOL
g32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
g32.DeleteDC.argtypes = [wintypes.HDC]
k32.GetLastError.argtypes = []
k32.GetLastError.restype = wintypes.DWORD
k32.SetLastError.argtypes = [wintypes.DWORD]

u32.SetProcessDPIAware()
vw = u32.GetSystemMetrics(78)
vh = u32.GetSystemMetrics(79)
print("virtual =", vw, "x", vh)

screen = u32.GetDC(None)
print("screen =", hex(screen or 0), "horzres =", g32.GetDeviceCaps(screen, 8))
print("GetPixel(10,10) =", hex(g32.GetPixel(screen, 10, 10)))

mem = g32.CreateCompatibleDC(screen)
bmp = g32.CreateCompatibleBitmap(screen, vw, vh)
old = g32.SelectObject(mem, bmp)
k32.SetLastError(0)
ok = g32.BitBlt(mem, 0, 0, vw, vh, screen, 0, 0, 0x00CC0020)
err = ctypes.get_last_error() if hasattr(ctypes, 'get_last_error') else k32.GetLastError()
print("BitBlt ok =", ok, "err =", err)
print("BitBlt last-error after =", k32.GetLastError())

g32.SelectObject(mem, old)
g32.DeleteObject(bmp)
g32.DeleteDC(mem)
u32.ReleaseDC(None, screen)
