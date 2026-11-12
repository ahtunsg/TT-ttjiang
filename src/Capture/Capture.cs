// Screen grab, clipboard (CF_DIB / CF_UNICODETEXT) and PNG persistence.
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace TTJiang.Capture
{
    internal static class Capture
    {
        internal static Bitmap GrabVirtualScreen()
        {
            int vx, vy, vw, vh;
            SystemInfo.VirtualScreen(out vx, out vy, out vw, out vh);
            Bitmap bmp = new Bitmap(vw, vh, PixelFormat.Format32bppArgb);
            IntPtr screenDc = Native.GetDC(IntPtr.Zero);
            Graphics g = Graphics.FromImage(bmp);
            IntPtr hdc = g.GetHdc();
            try
            {
                Native.BitBlt(hdc, 0, 0, vw, vh, screenDc, vx, vy, Native.SRCCOPY);
            }
            finally
            {
                g.ReleaseHdc(hdc);
                g.Dispose();
                Native.ReleaseDC(IntPtr.Zero, screenDc);
            }
            return bmp;
        }

        // rect is in virtual-screen coordinates.
        internal static Bitmap Crop(Bitmap full, Rectangle rect, int vx, int vy)
        {
            Rectangle local = new Rectangle(rect.X - vx, rect.Y - vy, rect.Width, rect.Height);
            local.Intersect(new Rectangle(0, 0, full.Width, full.Height));
            if (local.Width <= 0 || local.Height <= 0)
                throw new InvalidOperationException("选区为空");
            Bitmap crop = new Bitmap(local.Width, local.Height, PixelFormat.Format32bppArgb);
            using (Graphics g = Graphics.FromImage(crop))
                g.DrawImage(full, new Rectangle(0, 0, local.Width, local.Height),
                    local, GraphicsUnit.Pixel);
            return crop;
        }

        private static void WithClipboard(Action action)
        {
            Exception last = null;
            for (int attempt = 0; attempt < 8; attempt++)
            {
                if (Native.OpenClipboard(IntPtr.Zero))
                {
                    try
                    {
                        Native.EmptyClipboard();
                        action();
                        return;
                    }
                    finally { Native.CloseClipboard(); }
                }
                last = new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
                if (attempt < 7) Thread.Sleep(60);
            }
            throw new InvalidOperationException("无法访问剪贴板：" + (last == null ? "" : last.Message));
        }

        private static IntPtr ToGlobal(byte[] data)
        {
            IntPtr handle = Native.GlobalAlloc(Native.GMEM_MOVEABLE | Native.GMEM_ZEROINIT,
                new IntPtr(data.Length));
            if (handle == IntPtr.Zero)
                throw new OutOfMemoryException("GlobalAlloc failed");
            IntPtr ptr = Native.GlobalLock(handle);
            if (ptr == IntPtr.Zero)
            {
                Native.GlobalFree(handle);
                throw new InvalidOperationException("GlobalLock failed");
            }
            try { Marshal.Copy(data, 0, ptr, data.Length); }
            finally { Native.GlobalUnlock(handle); }
            return handle;
        }

        internal static void CopyText(string text)
        {
            byte[] bytes = Encoding.Unicode.GetBytes(text + "\0");
            WithClipboard(delegate
            {
                IntPtr handle = ToGlobal(bytes);
                if (Native.SetClipboardData(Native.CF_UNICODETEXT, handle) == IntPtr.Zero)
                    Native.GlobalFree(handle);
            });
        }

        internal static void CopyImage(Bitmap image)
        {
            byte[] dib;
            using (Bitmap rgb = new Bitmap(image.Width, image.Height, PixelFormat.Format24bppRgb))
            {
                using (Graphics g = Graphics.FromImage(rgb))
                    g.DrawImageUnscaled(image, 0, 0);
                using (MemoryStream ms = new MemoryStream())
                {
                    rgb.Save(ms, ImageFormat.Bmp);
                    byte[] bmp = ms.ToArray();
                    dib = new byte[bmp.Length - 14];
                    Array.Copy(bmp, 14, dib, 0, dib.Length);
                }
            }
            WithClipboard(delegate
            {
                IntPtr handle = ToGlobal(dib);
                if (Native.SetClipboardData(Native.CF_DIB, handle) == IntPtr.Zero)
                    Native.GlobalFree(handle);
            });
        }

        internal static string SavePng(Bitmap image, string directory)
        {
            Directory.CreateDirectory(directory);
            string name = "TTJiang_" + DateTime.Now.ToString("yyyyMMdd_HHmmss_fff",
                CultureInfo.InvariantCulture) + ".png";
            string path = Path.Combine(directory, name);
            image.Save(path, ImageFormat.Png);
            return path;
        }
    }
}
