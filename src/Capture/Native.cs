// Win32 interop. No WinForms, no WPF: the resident process only loads
// mscorlib / System / System.Core / System.Drawing / WinRT projection.
using System;
using System.Runtime.InteropServices;

namespace TTJiang.Capture
{
    internal static class Native
    {
        internal const int WM_CREATE = 0x0001;
        internal const int WM_DESTROY = 0x0002;
        internal const int WM_CLOSE = 0x0010;
        internal const int WM_PAINT = 0x000F;
        internal const int WM_ERASEBKGND = 0x0014;
        internal const int WM_KEYDOWN = 0x0100;
        internal const int WM_KEYUP = 0x0101;
        internal const int WM_CHAR = 0x0102;
        internal const int WM_LBUTTONDOWN = 0x0201;
        internal const int WM_LBUTTONUP = 0x0202;
        internal const int WM_MOUSEMOVE = 0x0200;
        internal const int WM_RBUTTONDOWN = 0x0204;
        internal const int WM_TIMER = 0x0113;
        internal const int WM_HOTKEY = 0x0312;
        internal const int WM_COMMAND = 0x0111;
        internal const int WM_APP = 0x8000;
        internal const int WM_MOUSELEAVE = 0x02A3;
        internal const int WM_ACTIVATE = 0x0006;
        internal const int WM_SETCURSOR = 0x0020;
        internal const int WM_DISPLAYCHANGE = 0x007E;

        internal const int APP_TRAY = WM_APP + 1;
        internal const int APP_OCR_DONE = WM_APP + 2;
        internal const int APP_HOTKEY_RETRY = WM_APP + 3;
        internal const int APP_SMOKE_EXIT = WM_APP + 4;

        internal const int WS_OVERLAPPED = 0x00000000;
        internal const int WS_POPUP = unchecked((int)0x80000000);
        internal const int WS_VISIBLE = 0x10000000;
        internal const int WS_CLIPCHILDREN = 0x02000000;
        internal const int WS_CLIPSIBLINGS = 0x04000000;
        internal const int WS_EX_TOOLWINDOW = 0x00000080;
        internal const int WS_EX_TOPMOST = 0x00000008;
        internal const int WS_EX_NOACTIVATE = 0x08000000;
        internal const int WS_EX_LAYERED = 0x00080000;
        internal const int WS_EX_TRANSPARENT = 0x00000020;

        internal const int SW_SHOW = 5;
        internal const int SW_SHOWNOACTIVATE = 4;
        internal const int SW_HIDE = 0;
        internal const int SWP_NOZORDER = 0x0004;
        internal const int SWP_NOACTIVATE = 0x0010;
        internal const int SWP_SHOWWINDOW = 0x0040;
        internal const int HWND_TOPMOST = -1;
        internal const int GWL_STYLE = -16;
        internal const int GWL_EXSTYLE = -20;

        internal const int CS_HREDRAW = 0x0002;
        internal const int CS_VREDRAW = 0x0001;
        internal const int CS_DBLCLKS = 0x0008;

        internal const int IDC_ARROW = 32512;
        internal const int IDC_CROSS = 32515;
        internal const int IDI_APPLICATION = 32512;
        internal const int IMAGE_ICON = 1;
        internal const int LR_LOADFROMFILE = 0x0010;
        internal const int LR_DEFAULTSIZE = 0x0040;

        internal const int NIM_ADD = 0x00000000;
        internal const int NIM_MODIFY = 0x00000001;
        internal const int NIM_DELETE = 0x00000002;
        internal const int NIF_MESSAGE = 0x00000001;
        internal const int NIF_ICON = 0x00000002;
        internal const int NIF_TIP = 0x00000004;
        internal const int NIM_SETVERSION = 0x00000004;
        internal const int NOTIFYICON_VERSION_4 = 4;

        internal const int WM_RBUTTONUP = 0x0205;
        internal const int WM_LBUTTONUP_TRAY = 0x0202;
        internal const int TPM_RIGHTBUTTON = 0x0002;
        internal const int TPM_RETURNCMD = 0x0100;
        internal const int TPM_NONOTIFY = 0x0080;
        internal const int MF_STRING = 0x00000000;
        internal const int MF_SEPARATOR = 0x00000800;
        internal const int MF_CHECKED = 0x00000008;
        internal const int MF_UNCHECKED = 0x00000000;
        internal const int MF_BYCOMMAND = 0x00000000;
        internal const int MF_GRAYED = 0x00000001;

        internal const int MOD_ALT = 0x0001;
        internal const int MOD_CONTROL = 0x0002;
        internal const int MOD_SHIFT = 0x0004;
        internal const int MOD_WIN = 0x0008;
        internal const int MOD_NOREPEAT = 0x4000;
        internal const int VK_ESCAPE = 0x1B;
        internal const int VK_RETURN = 0x0D;
        internal const int VK_1 = 0x31;

        internal const int SM_XVIRTUALSCREEN = 76;
        internal const int SM_YVIRTUALSCREEN = 77;
        internal const int SM_CXVIRTUALSCREEN = 78;
        internal const int SM_CYVIRTUALSCREEN = 79;
        internal const int SM_CXSCREEN = 0;
        internal const int SM_CYSCREEN = 1;
        internal const int SPI_GETWORKAREA = 0x0030;

        internal const int CF_UNICODETEXT = 13;
        internal const int CF_DIB = 8;
        internal const int GMEM_MOVEABLE = 0x0002;
        internal const int GMEM_ZEROINIT = 0x0040;

        internal const int SRCCOPY = 0x00CC0020;

        [StructLayout(LayoutKind.Sequential)]
        internal struct POINT
        {
            internal int X;
            internal int Y;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct RECT
        {
            internal int Left;
            internal int Top;
            internal int Right;
            internal int Bottom;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct MSG
        {
            internal IntPtr hwnd;
            internal uint message;
            internal IntPtr wParam;
            internal IntPtr lParam;
            internal uint time;
            internal POINT pt;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct PAINTSTRUCT
        {
            internal IntPtr hdc;
            internal int fErase;
            internal RECT rcPaint;
            internal int fRestore;
            internal int fIncUpdate;
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 32)]
            internal byte[] rgbReserved;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        internal struct WNDCLASSEX
        {
            internal int cbSize;
            internal int style;
            internal WndProc lpfnWndProc;
            internal int cbClsExtra;
            internal int cbWndExtra;
            internal IntPtr hInstance;
            internal IntPtr hIcon;
            internal IntPtr hCursor;
            internal IntPtr hbrBackground;
            internal string lpszMenuName;
            internal string lpszClassName;
            internal IntPtr hIconSm;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        internal struct NOTIFYICONDATA
        {
            internal int cbSize;
            internal IntPtr hWnd;
            internal int uID;
            internal int uFlags;
            internal int uCallbackMessage;
            internal IntPtr hIcon;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
            internal string szTip;
            internal int dwState;
            internal int dwStateMask;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
            internal string szInfo;
            internal int uVersion;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
            internal string szInfoTitle;
            internal int dwInfoFlags;
            internal Guid guidItem;
            internal IntPtr hBalloonIcon;
        }

        [UnmanagedFunctionPointer(CallingConvention.Winapi)]
        internal delegate IntPtr WndProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern ushort RegisterClassEx(ref WNDCLASSEX wc);

        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern IntPtr CreateWindowEx(int exStyle, string className, string windowName,
            int style, int x, int y, int width, int height, IntPtr parent, IntPtr menu,
            IntPtr instance, IntPtr param);

        [DllImport("user32.dll")]
        internal static extern bool DestroyWindow(IntPtr hwnd);

        [DllImport("user32.dll")]
        internal static extern IntPtr DefWindowProc(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll")]
        internal static extern bool ShowWindow(IntPtr hwnd, int cmd);

        [DllImport("user32.dll")]
        internal static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y,
            int cx, int cy, uint flags);

        internal const int LWA_ALPHA = 0x00000002;

        [DllImport("user32.dll")]
        internal static extern bool SetLayeredWindowAttributes(IntPtr hwnd, uint key, byte alpha, uint flags);

        [DllImport("user32.dll")]
        internal static extern bool UpdateWindow(IntPtr hwnd);

        [DllImport("user32.dll", EntryPoint = "InvalidateRect")]
        internal static extern bool InvalidateRectRect(IntPtr hwnd, ref RECT rect, bool erase);

        [DllImport("user32.dll")]
        internal static extern bool InvalidateRect(IntPtr hwnd, IntPtr rect, bool erase);

        [DllImport("user32.dll")]
        internal static extern IntPtr BeginPaint(IntPtr hwnd, out PAINTSTRUCT ps);

        [DllImport("user32.dll")]
        internal static extern bool EndPaint(IntPtr hwnd, ref PAINTSTRUCT ps);

        [DllImport("user32.dll")]
        internal static extern int GetMessage(out MSG msg, IntPtr hwnd, uint min, uint max);

        [ComImport]
        [Guid("5B0D3235-4DBA-4D44-865E-8F1D0E4FD04D")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        internal unsafe interface IMemoryBufferByteAccess
        {
            void GetBuffer(out byte* buffer, out uint capacity);
        }

        [DllImport("user32.dll")]
        internal static extern bool TranslateMessage(ref MSG msg);

        [DllImport("user32.dll")]
        internal static extern IntPtr DispatchMessage(ref MSG msg);

        [DllImport("user32.dll")]
        internal static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll")]
        internal static extern IntPtr SendMessage(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

        [DllImport("user32.dll")]
        internal static extern void PostQuitMessage(int exitCode);

        [DllImport("user32.dll")]
        internal static extern IntPtr SetTimer(IntPtr hwnd, IntPtr id, uint elapse, IntPtr callback);

        [DllImport("user32.dll")]
        internal static extern bool KillTimer(IntPtr hwnd, IntPtr id);

        [DllImport("user32.dll")]
        internal static extern bool RegisterHotKey(IntPtr hwnd, int id, int modifiers, int vk);

        [DllImport("user32.dll")]
        internal static extern bool UnregisterHotKey(IntPtr hwnd, int id);

        [DllImport("user32.dll")]
        internal static extern bool GetCursorPos(out POINT pt);

        [DllImport("user32.dll")]
        internal static extern bool SetCursorPos(int x, int y);

        [DllImport("user32.dll")]
        internal static extern IntPtr SetCursor(IntPtr cursor);

        [DllImport("user32.dll")]
        internal static extern IntPtr LoadCursor(IntPtr instance, int name);

        [DllImport("user32.dll")]
        internal static extern IntPtr LoadIcon(IntPtr instance, int name);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        internal static extern IntPtr LoadImage(IntPtr instance, string name, int type,
            int cx, int cy, int flags);

        [DllImport("user32.dll")]
        internal static extern bool DestroyIcon(IntPtr icon);

        [DllImport("user32.dll")]
        internal static extern IntPtr SetFocus(IntPtr hwnd);

        [DllImport("user32.dll")]
        internal static extern bool SetForegroundWindow(IntPtr hwnd);

        [DllImport("user32.dll")]
        internal static extern bool GetClientRect(IntPtr hwnd, out RECT rect);

        [DllImport("user32.dll")]
        internal static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);

        [DllImport("user32.dll")]
        internal static extern bool ScreenToClient(IntPtr hwnd, ref POINT pt);

        [DllImport("user32.dll")]
        internal static extern bool SystemParametersInfo(int action, int param, ref RECT rect, int flags);

        [DllImport("user32.dll")]
        internal static extern int GetSystemMetrics(int index);

        [DllImport("user32.dll")]
        internal static extern IntPtr GetDC(IntPtr hwnd);

        [DllImport("user32.dll")]
        internal static extern int ReleaseDC(IntPtr hwnd, IntPtr dc);

        [DllImport("gdi32.dll")]
        internal static extern bool BitBlt(IntPtr dest, int x, int y, int w, int h,
            IntPtr src, int sx, int sy, int op);

        [DllImport("gdi32.dll")]
        internal static extern IntPtr CreateCompatibleDC(IntPtr hdc);

        [DllImport("gdi32.dll")]
        internal static extern IntPtr CreateDIBSection(IntPtr hdc, ref BITMAPINFO info,
            uint usage, out IntPtr bits, IntPtr section, uint offset);

        [DllImport("gdi32.dll")]
        internal static extern IntPtr SelectObject(IntPtr hdc, IntPtr obj);

        [DllImport("gdi32.dll")]
        internal static extern bool DeleteObject(IntPtr obj);

        [DllImport("gdi32.dll")]
        internal static extern bool DeleteDC(IntPtr hdc);

        [StructLayout(LayoutKind.Sequential)]
        internal struct BITMAPINFO
        {
            internal int biSize;
            internal int biWidth;
            internal int biHeight;
            internal short biPlanes;
            internal short biBitCount;
            internal int biCompression;
            internal int biSizeImage;
            internal int biXPelsPerMeter;
            internal int biYPelsPerMeter;
            internal int biClrUsed;
            internal int biClrImportant;
        }

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        internal static extern bool SetProcessDPIAware();

        [DllImport("user32.dll")]
        internal static extern bool SetProcessDpiAwarenessContext(IntPtr value);

        [DllImport("shcore.dll")]
        internal static extern int SetProcessDpiAwareness(int value);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
        internal static extern IntPtr GetModuleHandle(string name);

        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        internal static extern bool Shell_NotifyIcon(int message, ref NOTIFYICONDATA data);

        [DllImport("user32.dll")]
        internal static extern IntPtr CreatePopupMenu();

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        internal static extern bool AppendMenu(IntPtr menu, int flags, IntPtr id, string text);

        [DllImport("user32.dll")]
        internal static extern int TrackPopupMenuEx(IntPtr menu, int flags, int x, int y,
            IntPtr hwnd, IntPtr param);

        [DllImport("user32.dll")]
        internal static extern bool DestroyMenu(IntPtr menu);

        [DllImport("user32.dll")]
        internal static extern bool CheckMenuItem(IntPtr menu, int id, int flags);

        [DllImport("user32.dll")]
        internal static extern bool EnableMenuItem(IntPtr menu, int id, int flags);

        [DllImport("user32.dll")]
        internal static extern bool OpenClipboard(IntPtr hwnd);

        [DllImport("user32.dll")]
        internal static extern bool CloseClipboard();

        [DllImport("user32.dll")]
        internal static extern bool EmptyClipboard();

        [DllImport("user32.dll")]
        internal static extern IntPtr SetClipboardData(int format, IntPtr handle);

        [DllImport("kernel32.dll")]
        internal static extern IntPtr GlobalAlloc(int flags, IntPtr bytes);

        [DllImport("kernel32.dll")]
        internal static extern IntPtr GlobalLock(IntPtr handle);

        [DllImport("kernel32.dll")]
        internal static extern bool GlobalUnlock(IntPtr handle);

        [DllImport("kernel32.dll")]
        internal static extern IntPtr GlobalFree(IntPtr handle);

        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        internal static extern IntPtr ShellExecute(IntPtr hwnd, string verb, string file,
            string args, string dir, int show);

        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        internal static extern IntPtr ExtractIcon(IntPtr instance, string file, int index);

        [DllImport("gdi32.dll")]
        internal static extern int GetObject(IntPtr handle, int size, IntPtr buffer);
    }
}
