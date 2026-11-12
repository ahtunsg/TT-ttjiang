// Selection overlay, four-button action bar and non-focus toast.
// Everything is painted with GDI+ into a back buffer; no UI framework.
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.Drawing.Drawing2D;

namespace TTJiang.Capture
{
    internal static class Actions
    {
        internal const int Image = 0;
        internal const int Save = 1;
        internal const int Table = 2;
        internal const int Text = 3;
        internal const int Cancel = -1;
    }

    internal sealed class SelectionOverlay : WinWindow
    {
        private readonly Bitmap _screen;
        private readonly Action<Rectangle> _onDone;
        private readonly Action _onCancel;
        private readonly int _vx, _vy, _vw, _vh;
        private readonly Font _titleFont = Theme.MakeFont(12f, false);
        private readonly Font _smallFont = Theme.MakeFont(9f, false);
        private Point _start;
        private bool _dragging;
        private bool _hasRect;
        private Rectangle _rect;
        private Rectangle _paintedRect;
        private bool _painted;
        private int _lastDraw;
        private bool _finished;
        // Window content stays opaque; DWM applies one uniform alpha to the whole
        // layered window. This removes the full-screen GDI+ alpha blend per frame.
        private const byte OverlayAlpha = 148;
        private static readonly Color DimColor = Color.FromArgb(255, 10, 15, 26);

        internal SelectionOverlay(Bitmap screen, Action<Rectangle> onDone, Action onCancel)
        {
            _screen = screen;
            _onDone = onDone;
            _onCancel = onCancel;
            SystemInfo.VirtualScreen(out _vx, out _vy, out _vw, out _vh);
            CreateWindow(Native.WS_EX_TOPMOST | Native.WS_EX_LAYERED,
                Native.WS_POPUP, _vx, _vy, _vw, _vh);
            Native.SetLayeredWindowAttributes(Hwnd, 0, OverlayAlpha, Native.LWA_ALPHA);
        }

        internal new void Show(bool activate)
        {
            base.Show(true);
            Native.SetFocus(Hwnd);
        }

        internal void PaintOnce() { Repaint(); }

        protected override void Draw(Graphics g)
        {
            using (SolidBrush dim = new SolidBrush(DimColor))
                g.FillRectangle(dim, 0, 0, Width, Height);
            DrawHint(g);
            if (_hasRect && _rect.Width > 1 && _rect.Height > 1)
            {
                DrawSelection(g, _rect);
                _paintedRect = _rect;
                _painted = true;
            }
        }

        private void DrawHint(Graphics g)
        {
            string hint = "拖动鼠标框选区域    ·    Esc 取消";
            SizeF hintSize = g.MeasureString(hint, _titleFont);
            float hx = (Width - hintSize.Width) / 2f;
            using (SolidBrush brush = new SolidBrush(Color.FromArgb(230, 229, 231, 235)))
                g.DrawString(hint, _titleFont, brush, hx, 26f);
        }

        private void DrawSelection(Graphics g, Rectangle rect)
        {
            g.DrawImage(_screen, rect, rect, GraphicsUnit.Pixel);
            using (Pen pen = new Pen(Theme.Accent, 2f))
                g.DrawRectangle(pen, rect.X, rect.Y, rect.Width - 1, rect.Height - 1);
            string label = rect.Width + " × " + rect.Height;
            int ly = Math.Max(2, rect.Y - 24);
            Rectangle tag = new Rectangle(rect.X, ly, 96, 20);
            using (SolidBrush brush = new SolidBrush(Theme.Bg))
                g.FillRectangle(brush, tag);
            using (StringFormat sf = new StringFormat())
            {
                sf.Alignment = StringAlignment.Center;
                sf.LineAlignment = StringAlignment.Center;
                using (SolidBrush brush = new SolidBrush(Theme.Fg))
                    g.DrawString(label, _smallFont, brush, tag, sf);
            }
        }

        private Rectangle InflateForRestore(Rectangle rect)
        {
            int x = Math.Max(0, rect.X - 4);
            int y = Math.Max(0, rect.Y - 28);
            int right = Math.Min(Width, rect.Right + 100);
            int bottom = Math.Min(Height, rect.Bottom + 4);
            return new Rectangle(x, y, Math.Max(0, right - x), Math.Max(0, bottom - y));
        }

        // Incremental repaint: only the previous and current selection footprints
        // are touched, instead of alpha-blending the whole virtual screen each frame.
        private void UpdateSelection(Rectangle rect)
        {
            if (Buffer == null) return;
            bool hintTouched = false;
            Rectangle dirty = Rectangle.Empty;
            using (Graphics g = Graphics.FromImage(Buffer))
            {
                g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.NearestNeighbor;
                g.PixelOffsetMode = System.Drawing.Drawing2D.PixelOffsetMode.Half;
                if (_painted)
                {
                    Rectangle restore = InflateForRestore(_paintedRect);
                    if (restore.Width > 0 && restore.Height > 0)
                    {
                        using (SolidBrush dim = new SolidBrush(DimColor))
                            g.FillRectangle(dim, restore);
                        dirty = restore;
                        if (restore.Top < 60) hintTouched = true;
                    }
                }
                _painted = false;
                _paintedRect = rect;
                if (rect.Width > 1 && rect.Height > 1)
                {
                    DrawSelection(g, rect);
                    Rectangle footprint = InflateForRestore(rect);
                    dirty = dirty.Width > 0 ? Rectangle.Union(dirty, footprint) : footprint;
                    _painted = true;
                    if (rect.Y - 28 < 60) hintTouched = true;
                }
                if (hintTouched)
                {
                    g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
                    g.PixelOffsetMode = System.Drawing.Drawing2D.PixelOffsetMode.HighQuality;
                    DrawHint(g);
                    dirty = Rectangle.Union(dirty, new Rectangle(0, 0, Width, 60));
                }
            }
            FlushBuffer(dirty);
        }

        private Point LocalCursor()
        {
            Native.POINT pt;
            Native.GetCursorPos(out pt);
            int x = Math.Max(0, Math.Min(Width, pt.X - _vx));
            int y = Math.Max(0, Math.Min(Height, pt.Y - _vy));
            return new Point(x, y);
        }

        private static Rectangle MakeRect(Point a, Point b)
        {
            return new Rectangle(Math.Min(a.X, b.X), Math.Min(a.Y, b.Y),
                Math.Abs(a.X - b.X), Math.Abs(a.Y - b.Y));
        }

        protected override IntPtr OnMessage(uint msg, IntPtr wParam, IntPtr lParam)
        {
            if (msg == Native.WM_SETCURSOR)
            {
                Native.SetCursor(Native.LoadCursor(IntPtr.Zero, Native.IDC_CROSS));
                return new IntPtr(1);
            }
            if (msg == Native.WM_LBUTTONDOWN)
            {
                _start = LocalCursor();
                _dragging = true;
                _hasRect = false;
                UpdateSelection(Rectangle.Empty);
                return IntPtr.Zero;
            }
            if (msg == Native.WM_MOUSEMOVE && _dragging)
            {
                int now = Environment.TickCount;
                if (now - _lastDraw >= 16)
                {
                    _lastDraw = now;
                    _rect = MakeRect(_start, LocalCursor());
                    _hasRect = true;
                    UpdateSelection(_rect);
                }
                return IntPtr.Zero;
            }
            if (msg == Native.WM_LBUTTONUP && _dragging)
            {
                _dragging = false;
                Rectangle rect = MakeRect(_start, LocalCursor());
                _hasRect = false;
                if (rect.Width < 8 || rect.Height < 8)
                {
                    _hasRect = false;
                    UpdateSelection(Rectangle.Empty);
                    return IntPtr.Zero;
                }
                Finish();
                Rectangle screenRect = new Rectangle(rect.X + _vx, rect.Y + _vy,
                    rect.Width, rect.Height);
                if (_onDone != null) _onDone(screenRect);
                return IntPtr.Zero;
            }
            if (msg == Native.WM_RBUTTONDOWN)
            {
                Finish();
                if (_onCancel != null) _onCancel();
                return IntPtr.Zero;
            }
            if (msg == Native.WM_KEYDOWN && (int)wParam == Native.VK_ESCAPE)
            {
                Finish();
                if (_onCancel != null) _onCancel();
                return IntPtr.Zero;
            }
            return base.OnMessage(msg, wParam, lParam);
        }

        private void Finish()
        {
            if (_finished) return;
            _finished = true;
            Hide();
        }

        public override void Dispose()
        {
            _titleFont.Dispose();
            _smallFont.Dispose();
            base.Dispose();
        }
    }

    internal sealed class ActionBar : WinWindow
    {
        private const int Count = 4;
        private const int ButtonWidth = 46;
        private const int ButtonHeight = 38;
        private const int Padding = 7;
        private const int Gap = 4;
        private readonly Action<int> _onAction;
        private readonly Font _keyFont = Theme.MakeFont(7f, false);
        private int _hover = -1;
        private bool _finished;

        internal ActionBar(Action<int> onAction)
        {
            _onAction = onAction;
            int w = Padding * 2 + Count * ButtonWidth + (Count - 1) * Gap;
            int h = Padding * 2 + ButtonHeight;
            CreateWindow(Native.WS_EX_TOPMOST | Native.WS_EX_TOOLWINDOW,
                Native.WS_POPUP, 0, 0, w, h);
        }

        private Rectangle ButtonRect(int index)
        {
            return new Rectangle(Padding + index * (ButtonWidth + Gap), Padding,
                ButtonWidth, ButtonHeight);
        }

        internal void PlaceNear(Rectangle selection)
        {
            int left, top, right, bottom;
            SystemInfo.WorkArea(out left, out top, out right, out bottom);
            int x = Math.Max(left + 4, Math.Min(selection.Right - Width, right - Width - 12));
            int y = selection.Bottom + 10;
            if (y + Height > bottom - 8)
                y = Math.Max(top + 8, selection.Top - Height - 10);
            MoveTo(x, y);
        }

        internal new void Show(bool activate)
        {
            base.Show(true);
            Native.SetFocus(Hwnd);
        }

        internal void PaintOnce() { Repaint(); }

        protected override void Draw(Graphics g)
        {
            Theme.RoundRect(g, new Rectangle(0, 0, Width - 1, Height - 1), 8,
                Theme.Bg, Color.FromArgb(70, 75, 85, 99));
            for (int i = 0; i < Count; i++)
            {
                Rectangle r = ButtonRect(i);
                Color fill = (i == _hover) ? Theme.Accent : Theme.BgSoft;
                Theme.RoundRect(g, r, 5, fill, i == _hover ? Theme.Accent : Color.FromArgb(55, 65, 81));
                DrawIcon(g, i, r);
                using (StringFormat sf = new StringFormat())
                {
                    sf.Alignment = StringAlignment.Far;
                    sf.LineAlignment = StringAlignment.Far;
                    using (SolidBrush brush = new SolidBrush(Theme.Muted))
                        g.DrawString((i + 1).ToString(), _keyFont, brush,
                            new RectangleF(r.Left + 1, r.Top + 1, r.Width - 4, r.Height - 3), sf);
                }
            }
        }

        // Four line icons drawn with GDI+: no image resources, no text labels.
        // Order: copy image / save PNG / table OCR / text OCR.
        private static void DrawIcon(Graphics g, int index, Rectangle r)
        {
            int cx = r.Left + r.Width / 2;
            int cy = r.Top + r.Height / 2 - 1;
            using (Pen pen = new Pen(Theme.Fg, 1.7f))
            {
                pen.StartCap = LineCap.Round;
                pen.EndCap = LineCap.Round;
                pen.LineJoin = LineJoin.Round;
                if (index == Actions.Image)
                {
                    // Two stacked frames with a small landscape: copy image.
                    g.DrawRectangle(pen, cx - 9, cy - 8, 14, 13);
                    using (SolidBrush brush = new SolidBrush(Theme.BgSoft))
                        g.FillRectangle(brush, cx - 5, cy - 4, 14, 13);
                    g.DrawRectangle(pen, cx - 5, cy - 4, 14, 13);
                    g.DrawLine(pen, cx - 4, cy + 7, cx - 1, cy + 3);
                    g.DrawLine(pen, cx - 1, cy + 3, cx + 3, cy + 7);
                    g.DrawLine(pen, cx + 3, cy + 7, cx + 8, cy + 7);
                }
                else if (index == Actions.Save)
                {
                    // Down arrow onto a tray: save as PNG.
                    g.DrawLine(pen, cx, cy - 8, cx, cy + 3);
                    g.DrawLine(pen, cx - 5, cy - 2, cx, cy + 3);
                    g.DrawLine(pen, cx + 5, cy - 2, cx, cy + 3);
                    g.DrawLine(pen, cx - 8, cy + 8, cx + 8, cy + 8);
                }
                else if (index == Actions.Table)
                {
                    // 2x2 grid: recognise table.
                    g.DrawRectangle(pen, cx - 9, cy - 7, 18, 14);
                    g.DrawLine(pen, cx - 9, cy, cx + 9, cy);
                    g.DrawLine(pen, cx, cy - 7, cx, cy + 7);
                }
                else
                {
                    // Three text lines: recognise text.
                    g.DrawLine(pen, cx - 8, cy - 6, cx + 8, cy - 6);
                    g.DrawLine(pen, cx - 8, cy, cx + 8, cy);
                    g.DrawLine(pen, cx - 8, cy + 6, cx + 3, cy + 6);
                }
            }
        }

        private int HitTest(Point pt)
        {
            for (int i = 0; i < Count; i++)
                if (ButtonRect(i).Contains(pt)) return i;
            return -1;
        }

        protected override IntPtr OnMessage(uint msg, IntPtr wParam, IntPtr lParam)
        {
            if (msg == Native.WM_MOUSEMOVE)
            {
                int hit = HitTest(new Point(LowWord(lParam), HighWord(lParam)));
                if (hit != _hover) { _hover = hit; Repaint(); }
                return IntPtr.Zero;
            }
            if (msg == Native.WM_LBUTTONUP)
            {
                int hit = HitTest(new Point(LowWord(lParam), HighWord(lParam)));
                if (hit >= 0) Fire(hit);
                return IntPtr.Zero;
            }
            if (msg == Native.WM_KEYDOWN)
            {
                int vk = (int)wParam;
                if (vk == Native.VK_ESCAPE) { Fire(Actions.Cancel); return IntPtr.Zero; }
                if (vk == Native.VK_RETURN) { Fire(Actions.Image); return IntPtr.Zero; }
                if (vk >= Native.VK_1 && vk <= Native.VK_1 + 3)
                {
                    Fire(vk - Native.VK_1);
                    return IntPtr.Zero;
                }
            }
            return base.OnMessage(msg, wParam, lParam);
        }

        private void Fire(int action)
        {
            if (_finished) return;
            _finished = true;
            Hide();
            if (_onAction != null) _onAction(action);
        }

        public override void Dispose()
        {
            _keyFont.Dispose();
            base.Dispose();
        }
    }

    internal sealed class Toast : WinWindow
    {
        private const int ToastWidth = 470;
        private const int ToastHeight = 46;
        private readonly Font _font = Theme.MakeFont(9.5f, false);
        private string _text = "";
        private bool _ok = true;
        private bool _enabled;

        internal Toast(bool enabled)
        {
            _enabled = enabled;
            CreateWindow(Native.WS_EX_TOPMOST | Native.WS_EX_TOOLWINDOW | Native.WS_EX_NOACTIVATE,
                Native.WS_POPUP, 0, 0, ToastWidth, ToastHeight);
        }

        internal void ShowMessage(string text, bool ok, int durationMs)
        {
            if (!_enabled)
                return;
            _text = text ?? "";
            _ok = ok;
            int left, top, right, bottom;
            SystemInfo.WorkArea(out left, out top, out right, out bottom);
            MoveTo(Math.Max(left + 8, right - ToastWidth - 24), bottom - ToastHeight - 28);
            Repaint();
            Native.ShowWindow(Hwnd, Native.SW_SHOWNOACTIVATE);
            Native.SetWindowPos(Hwnd, new IntPtr(Native.HWND_TOPMOST), 0, 0, 0, 0,
                Native.SWP_NOACTIVATE | 0x0001 | 0x0002);
            Native.SetTimer(Hwnd, new IntPtr(1), (uint)Math.Max(300, durationMs), IntPtr.Zero);
        }

        internal void Dismiss()
        {
            Native.KillTimer(Hwnd, new IntPtr(1));
            Hide();
        }

        protected override void Draw(Graphics g)
        {
            Theme.RoundRect(g, new Rectangle(0, 0, Width - 1, Height - 1), 8,
                Theme.Bg, Color.FromArgb(70, 75, 85, 99));
            using (SolidBrush accent = new SolidBrush(_ok ? Theme.OkColor : Theme.Danger))
                g.FillRectangle(accent, 0, 8, 3, Height - 17);
            Rectangle textRect = new Rectangle(16, 4, Width - 26, Height - 8);
            using (StringFormat sf = new StringFormat())
            {
                sf.Alignment = StringAlignment.Near;
                sf.LineAlignment = StringAlignment.Center;
                sf.Trimming = StringTrimming.EllipsisCharacter;
                sf.FormatFlags = StringFormatFlags.NoWrap;
                using (SolidBrush brush = new SolidBrush(Theme.Fg))
                    g.DrawString(_text, _font, brush, textRect, sf);
            }
        }

        protected override IntPtr OnMessage(uint msg, IntPtr wParam, IntPtr lParam)
        {
            if (msg == Native.WM_TIMER && (int)wParam == 1)
            {
                Dismiss();
                return IntPtr.Zero;
            }
            return base.OnMessage(msg, wParam, lParam);
        }

        public override void Dispose()
        {
            _font.Dispose();
            base.Dispose();
        }
    }
}
