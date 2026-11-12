// Lightweight Win32 window base with a GDI+ back buffer (no WinForms).
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.Runtime.InteropServices;

namespace TTJiang.Capture
{
    internal static class Theme
    {
        internal static readonly Color Bg = Color.FromArgb(17, 24, 39);
        internal static readonly Color BgSoft = Color.FromArgb(31, 41, 55);
        internal static readonly Color Fg = Color.FromArgb(249, 250, 251);
        internal static readonly Color Muted = Color.FromArgb(156, 163, 175);
        internal static readonly Color Accent = Color.FromArgb(59, 130, 246);
        internal static readonly Color Danger = Color.FromArgb(248, 113, 113);
        internal static readonly Color OkColor = Color.FromArgb(52, 211, 153);

        internal static Font MakeFont(float size, bool bold)
        {
            return new Font("Microsoft YaHei UI", size,
                bold ? FontStyle.Bold : FontStyle.Regular, GraphicsUnit.Point);
        }

        internal static void Prepare(Graphics g)
        {
            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.TextRenderingHint = TextRenderingHint.ClearTypeGridFit;
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
        }

        internal static void RoundRect(Graphics g, Rectangle r, int radius, Color fill, Color? border)
        {
            using (GraphicsPath path = new GraphicsPath())
            {
                int d = radius * 2;
                path.AddArc(r.Left, r.Top, d, d, 180, 90);
                path.AddArc(r.Right - d, r.Top, d, d, 270, 90);
                path.AddArc(r.Right - d, r.Bottom - d, d, d, 0, 90);
                path.AddArc(r.Left, r.Bottom - d, d, d, 90, 90);
                path.CloseFigure();
                using (SolidBrush brush = new SolidBrush(fill))
                    g.FillPath(brush, path);
                if (border.HasValue)
                {
                    using (Pen pen = new Pen(border.Value, 1f))
                        g.DrawPath(pen, path);
                }
            }
        }
    }

    internal abstract class WinWindow : IDisposable
    {
        private const string ClassName = "TTJiangCaptureWindow";
        private static readonly Dictionary<IntPtr, WinWindow> Map = new Dictionary<IntPtr, WinWindow>();
        private static readonly Native.WndProc Router = Route;
        private static bool _registered;

        protected IntPtr Hwnd;
        protected int Width;
        protected int Height;
        private Bitmap _buffer;
        private IntPtr _memDc;
        private IntPtr _hbm;
        private IntPtr _oldBm;
        private bool _disposed;

        internal IntPtr Handle { get { return Hwnd; } }

        // GDI+ wraps the very same memory as the GDI DIB section, so WM_PAINT can
        // BitBlt the finished pixels straight out without any GDI+ copy/convert.
        protected Bitmap Buffer { get { return _buffer; } }

        internal void SaveBuffer(string path)
        {
            _buffer.Save(path, ImageFormat.Png);
        }

        protected void FlushBuffer()
        {
            if (Hwnd != IntPtr.Zero)
                Native.InvalidateRect(Hwnd, IntPtr.Zero, false);
        }

        // Invalidate only the touched pixels: WM_PAINT then blits just that rect.
        protected void FlushBuffer(Rectangle dirty)
        {
            if (Hwnd == IntPtr.Zero || dirty.Width <= 0 || dirty.Height <= 0)
                return;
            Native.RECT rc = new Native.RECT();
            rc.Left = Math.Max(0, dirty.Left);
            rc.Top = Math.Max(0, dirty.Top);
            rc.Right = Math.Min(Width, dirty.Right);
            rc.Bottom = Math.Min(Height, dirty.Bottom);
            if (rc.Right > rc.Left && rc.Bottom > rc.Top)
                Native.InvalidateRectRect(Hwnd, ref rc, false);
        }

        protected WinWindow()
        {
            EnsureClass();
        }

        private static void EnsureClass()
        {
            if (_registered)
                return;
            Native.WNDCLASSEX wc = new Native.WNDCLASSEX();
            wc.cbSize = Marshal.SizeOf(typeof(Native.WNDCLASSEX));
            wc.style = Native.CS_HREDRAW | Native.CS_VREDRAW | Native.CS_DBLCLKS;
            wc.lpfnWndProc = Router;
            wc.hInstance = Native.GetModuleHandle(null);
            wc.hCursor = Native.LoadCursor(IntPtr.Zero, Native.IDC_ARROW);
            wc.hIcon = IntPtr.Zero;
            wc.hbrBackground = IntPtr.Zero;
            wc.lpszClassName = ClassName;
            wc.hIconSm = IntPtr.Zero;
            if (Native.RegisterClassEx(ref wc) == 0)
                throw new InvalidOperationException("RegisterClassEx failed");
            _registered = true;
        }

        protected void CreateWindow(int exStyle, int style, int x, int y, int w, int h)
        {
            Width = Math.Max(1, w);
            Height = Math.Max(1, h);
            _memDc = Native.CreateCompatibleDC(IntPtr.Zero);
            Native.BITMAPINFO bmi = new Native.BITMAPINFO();
            bmi.biSize = Marshal.SizeOf(typeof(Native.BITMAPINFO));
            bmi.biWidth = Width;
            bmi.biHeight = -Height;              // top-down DIB
            bmi.biPlanes = 1;
            bmi.biBitCount = 32;
            bmi.biCompression = 0;               // BI_RGB
            IntPtr bits;
            _hbm = Native.CreateDIBSection(_memDc, ref bmi, 0, out bits, IntPtr.Zero, 0);
            if (_hbm == IntPtr.Zero || bits == IntPtr.Zero)
                throw new InvalidOperationException("CreateDIBSection failed");
            _oldBm = Native.SelectObject(_memDc, _hbm);
            _buffer = new Bitmap(Width, Height, Width * 4,
                PixelFormat.Format32bppPArgb, bits);
            Hwnd = Native.CreateWindowEx(exStyle, ClassName, "TT酱截图", style,
                x, y, Width, Height, IntPtr.Zero, IntPtr.Zero,
                Native.GetModuleHandle(null), IntPtr.Zero);
            if (Hwnd == IntPtr.Zero)
                throw new InvalidOperationException("CreateWindowEx failed");
            Map[Hwnd] = this;
        }

        private static IntPtr Route(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam)
        {
            WinWindow target;
            if (Map.TryGetValue(hwnd, out target))
            {
                try { return target.OnMessage(msg, wParam, lParam); }
                catch (Exception) { return Native.DefWindowProc(hwnd, msg, wParam, lParam); }
            }
            return Native.DefWindowProc(hwnd, msg, wParam, lParam);
        }

        protected virtual IntPtr OnMessage(uint msg, IntPtr wParam, IntPtr lParam)
        {
            if (msg == Native.WM_PAINT)
            {
                Native.PAINTSTRUCT ps;
                IntPtr hdc = Native.BeginPaint(Hwnd, out ps);
                if (hdc != IntPtr.Zero && _memDc != IntPtr.Zero)
                {
                    int x = Math.Max(0, ps.rcPaint.Left);
                    int y = Math.Max(0, ps.rcPaint.Top);
                    int w = Math.Min(ps.rcPaint.Right, Width) - x;
                    int h = Math.Min(ps.rcPaint.Bottom, Height) - y;
                    if (w > 0 && h > 0)
                        Native.BitBlt(hdc, x, y, w, h, _memDc, x, y, Native.SRCCOPY);
                }
                Native.EndPaint(Hwnd, ref ps);
                return IntPtr.Zero;
            }
            if (msg == Native.WM_ERASEBKGND)
                return new IntPtr(1);
            if (msg == Native.WM_DESTROY)
            {
                Map.Remove(Hwnd);
                return IntPtr.Zero;
            }
            return Native.DefWindowProc(Hwnd, msg, wParam, lParam);
        }

        protected void Repaint()
        {
            if (_buffer == null || Hwnd == IntPtr.Zero)
                return;
            using (Graphics g = Graphics.FromImage(_buffer))
            {
                Theme.Prepare(g);
                g.Clear(Color.Transparent);
                Draw(g);
            }
            Native.InvalidateRect(Hwnd, IntPtr.Zero, false);
        }

        protected abstract void Draw(Graphics g);

        protected void Show(bool activate)
        {
            Native.ShowWindow(Hwnd, activate ? Native.SW_SHOW : Native.SW_SHOWNOACTIVATE);
            if (activate)
                Native.SetForegroundWindow(Hwnd);
            Repaint();
        }

        protected void Hide()
        {
            if (Hwnd != IntPtr.Zero)
                Native.ShowWindow(Hwnd, Native.SW_HIDE);
        }

        protected void MoveTo(int x, int y)
        {
            Native.SetWindowPos(Hwnd, new IntPtr(Native.HWND_TOPMOST), x, y,
                Width, Height, Native.SWP_NOACTIVATE | Native.SWP_NOZORDER | Native.SWP_SHOWWINDOW);
        }

        protected static int LowWord(IntPtr value)
        {
            return (short)((int)value.ToInt64() & 0xFFFF);
        }

        protected static int HighWord(IntPtr value)
        {
            return (short)(((int)value.ToInt64() >> 16) & 0xFFFF);
        }

        public virtual void Dispose()
        {
            if (_disposed)
                return;
            _disposed = true;
            if (Hwnd != IntPtr.Zero)
            {
                Map.Remove(Hwnd);
                Native.DestroyWindow(Hwnd);
                Hwnd = IntPtr.Zero;
            }
            if (_buffer != null) { _buffer.Dispose(); _buffer = null; }
            if (_memDc != IntPtr.Zero)
            {
                if (_oldBm != IntPtr.Zero) Native.SelectObject(_memDc, _oldBm);
                Native.DeleteDC(_memDc);
                _memDc = IntPtr.Zero;
            }
            if (_hbm != IntPtr.Zero) { Native.DeleteObject(_hbm); _hbm = IntPtr.Zero; }
        }
    }
}
