// Direct in-process Windows.Media.Ocr access. No PowerShell subprocess, no
// temporary PNG in the hot path, and the OcrEngine instance is reused.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading;
using Windows.Foundation;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;
using Windows.Storage;

namespace TTJiang.Capture
{
    internal sealed class OcrException : Exception
    {
        internal OcrException(string message) : base(message) { }
        internal OcrException(string message, Exception inner) : base(message, inner) { }
    }

    internal sealed class OcrWork
    {
        internal Bitmap Image;
        internal string Language;
        internal Action<OcrResult, Exception> Callback;
        internal OcrResult Result;
        internal Exception Error;
    }

    internal static class Ocr
    {
        // Set from config at startup; tests can flip it to compare engines.
        internal static bool PreferLocalModel = true;
        internal static string LastLocalFailure = "";

        private static readonly object Gate = new object();
        private static readonly Queue<OcrWork> Pending = new Queue<OcrWork>();
        private static readonly Queue<OcrWork> Completed = new Queue<OcrWork>();
        private static readonly ManualResetEvent Signal = new ManualResetEvent(false);
        private static Thread _worker;
        private static IntPtr _notifyHwnd;
        private static OcrEngine _engine;
        private static string _engineLanguage;
        private static volatile bool _stop;

        internal static void Start(IntPtr notifyHwnd)
        {            _notifyHwnd = notifyHwnd;
            _stop = false;
            _worker = new Thread(WorkerLoop);
            _worker.Name = "TTJiang.OcrWorker";
            _worker.IsBackground = true;
            _worker.SetApartmentState(ApartmentState.STA);
            _worker.Start();
        }

        internal static void Stop()
        {
            _stop = true;
            Signal.Set();
        }

        internal static bool Request(Bitmap image, string language, Action<OcrResult, Exception> callback)
        {
            OcrWork work = new OcrWork();
            work.Image = image;
            work.Language = language;
            work.Callback = callback;
            lock (Gate)
            {
                if (Pending.Count > 0) return false;
                Pending.Enqueue(work);
            }
            Signal.Set();
            return true;
        }

        // Called on the UI thread when APP_OCR_DONE arrives.
        internal static void DrainCompleted()
        {
            List<OcrWork> batch = new List<OcrWork>();
            lock (Gate)
            {
                while (Completed.Count > 0) batch.Add(Completed.Dequeue());
            }
            foreach (OcrWork work in batch)
            {
                if (work.Callback != null)
                    work.Callback(work.Result, work.Error);
            }
        }

        private static void WorkerLoop()
        {
            while (true)
            {
                Signal.WaitOne();
                if (_stop) return;
                OcrWork work = null;
                lock (Gate)
                {
                    if (Pending.Count > 0) work = Pending.Dequeue();
                    else Signal.Reset();
                }
                if (work == null) continue;
                try
                {
                    work.Result = Recognize(work.Image, work.Language);
                }
                catch (Exception ex)
                {
                    work.Error = ex;
                }
                finally
                {
                    if (work.Image != null) { work.Image.Dispose(); work.Image = null; }
                }
                lock (Gate) { Completed.Enqueue(work); }
                Native.PostMessage(_notifyHwnd, Native.APP_OCR_DONE, IntPtr.Zero, IntPtr.Zero);
            }
        }

        private static T Await<T>(IAsyncOperation<T> op)
        {
            using (ManualResetEvent done = new ManualResetEvent(false))
            {
                op.Completed = delegate(IAsyncOperation<T> a, AsyncStatus s) { done.Set(); };
                if (!done.WaitOne(30000))
                    throw new OcrException("OCR 引擎响应超时（30 秒）");
                if (op.Status != AsyncStatus.Completed)
                    throw new OcrException("OCR 引擎返回状态：" + op.Status);
                return op.GetResults();
            }
        }

        private static OcrEngine GetEngine(string language)
        {
            string wanted = string.IsNullOrEmpty(language) ? "" : language;
            if (_engine != null && _engineLanguage == wanted)
                return _engine;
            OcrEngine engine = null;
            if (wanted.Length > 0)
            {
                try { engine = OcrEngine.TryCreateFromLanguage(new Language(wanted)); }
                catch (Exception) { engine = null; }
            }
            if (engine == null)
                engine = OcrEngine.TryCreateFromUserProfileLanguages();
            if (engine == null)
                throw new OcrException("系统未安装可用的 OCR 语言包（设置 > 时间和语言 > 语言）");
            _engine = engine;
            _engineLanguage = wanted;
            return engine;
        }

        // Build a WinRT SoftwareBitmap straight from GDI+ scanlines (BGRA8).
        // The machine has no merged Windows.winmd, so AsBuffer() would produce an
        // IBuffer from a different metadata assembly. Writing through
        // IMemoryBufferByteAccess avoids that type mismatch entirely and also
        // avoids a PNG encode/decode round trip.
        private static unsafe SoftwareBitmap ToSoftwareBitmap(Bitmap source)
        {
            int w = source.Width;
            int h = source.Height;
            byte[] pixels = new byte[w * h * 4];
            using (Bitmap bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb))
            {
                using (Graphics g = Graphics.FromImage(bmp))
                {
                    g.CompositingMode = System.Drawing.Drawing2D.CompositingMode.SourceCopy;
                    g.DrawImageUnscaled(source, 0, 0);
                }
                BitmapData data = bmp.LockBits(new Rectangle(0, 0, w, h),
                    ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                try
                {
                    for (int y = 0; y < h; y++)
                        Marshal.Copy(IntPtr.Add(data.Scan0, y * data.Stride),
                            pixels, y * w * 4, w * 4);
                }
                finally { bmp.UnlockBits(data); }
            }
            SoftwareBitmap sb = new SoftwareBitmap(BitmapPixelFormat.Bgra8, w, h,
                BitmapAlphaMode.Premultiplied);
            using (BitmapBuffer buffer = sb.LockBuffer(BitmapBufferAccessMode.Write))
            {
                using (IMemoryBufferReference reference = buffer.CreateReference())
                {
                    byte* target;
                    uint capacity;
                    ((Native.IMemoryBufferByteAccess)reference).GetBuffer(out target, out capacity);
                    BitmapPlaneDescription plane = buffer.GetPlaneDescription(0);
                    for (int y = 0; y < h; y++)
                    {
                        IntPtr row = new IntPtr(target + plane.StartIndex + y * plane.Stride);
                        Marshal.Copy(pixels, y * w * 4, row, w * 4);
                    }
                }
            }
            return sb;
        }

        // Fallback for systems where IMemoryBufferByteAccess is not available.
        private static SoftwareBitmap ToSoftwareBitmapViaFile(Bitmap source)
        {
            string path = Path.Combine(Path.GetTempPath(),
                "ttsprint_ocr_" + Guid.NewGuid().ToString("N") + ".png");
            try
            {
                source.Save(path, ImageFormat.Png);
                StorageFile file = Await(StorageFile.GetFileFromPathAsync(path));
                var stream = Await(file.OpenAsync(FileAccessMode.Read));
                try
                {
                    BitmapDecoder decoder = Await(BitmapDecoder.CreateAsync(stream));
                    return Await(decoder.GetSoftwareBitmapAsync());
                }
                finally { try { stream.Dispose(); } catch (Exception) { } }
            }
            finally
            {
                try { File.Delete(path); }
                catch (Exception) { }
            }
        }

        private static Bitmap PrepareAt(Bitmap source, double wanted, int cap, out double scale)
        {
            int w = source.Width;
            int h = source.Height;
            int longEdge = Math.Max(w, h);
            scale = Math.Min(wanted, longEdge > 0 ? (double)cap / longEdge : 1.0);
            if (scale < 0.05) scale = 0.05;
            if (Math.Abs(scale - 1.0) < 0.000001)
                return new Bitmap(source);
            int nw = Math.Max(1, (int)(w * scale));
            int nh = Math.Max(1, (int)(h * scale));
            Bitmap resized = new Bitmap(nw, nh, PixelFormat.Format32bppArgb);
            using (Graphics g = Graphics.FromImage(resized))
            {
                g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
                g.PixelOffsetMode = System.Drawing.Drawing2D.PixelOffsetMode.HighQuality;
                g.DrawImage(source, new Rectangle(0, 0, nw, nh));
            }
            return resized;
        }

        private static OcrResult RecognizeAt(Bitmap image, OcrEngine engine, double wanted, int cap)
        {
            Bitmap prepared = null;
            SoftwareBitmap sb = null;
            double scale;
            try
            {
                prepared = PrepareAt(image, wanted, cap, out scale);
                try { sb = ToSoftwareBitmap(prepared); }
                catch (Exception)
                {
                    if (sb != null) { try { sb.Dispose(); } catch (Exception) { } sb = null; }
                    sb = ToSoftwareBitmapViaFile(prepared);
                }
                return Parse(Await(engine.RecognizeAsync(sb)), scale, engine);
            }
            finally
            {
                if (sb != null) { try { sb.Dispose(); } catch (Exception) { } }
                if (prepared != null) prepared.Dispose();
            }
        }

        // Windows OCR is very sensitive to glyph size: 32px text reads cleanly at
        // 1x while 11-24px text is far better at 2x/3x. Always run 1x, then only
        // when the text looks small or unreliable run the larger scales and keep
        // the best result. No ground truth is needed: broken glyphs and damaged
        // decimal numbers make a candidate score lower.
        internal static OcrResult Recognize(Bitmap image, string language)
        {
            if (image == null) throw new OcrException("选区图像为空");
            // The bundled PP-OCR model is markedly better on small screenshot text,
            // so it is tried first. Any failure (missing payload, unsupported CPU,
            // corrupt model) silently degrades to Windows OCR instead of failing
            // the user's screenshot.
            if (PreferLocalModel)
            {
                try
                {
                    if (PaddleOcrEngine.EnsureReady())
                        return PaddleOcrEngine.Recognize(image);
                }
                catch (Exception ex) { LastLocalFailure = ex.Message; }
            }
            OcrEngine engine = GetEngine(language);
            int maxDim = MaxDimension();
            if (maxDim <= 0) maxDim = 8000;
            int cap = Math.Min(maxDim, 8000);
            int longEdge = Math.Max(image.Width, image.Height);
            OcrResult best = null;
            double bestScore = double.NegativeInfinity;
            Exception last = null;
            OcrResult first = null;
            try { first = RecognizeAt(image, engine, 1.0, cap); }
            catch (Exception ex) { last = ex; }
            if (first != null)
            {
                best = first;
                bestScore = QualityScore(first);
            }

            string flat = first == null ? "" : Flat(first);
            double medianH = MedianWordHeight(first);
            bool small = medianH > 0 && medianH < 28;
            bool poor = first == null || CountSuspicious(flat) > 0 ||
                (HasDigit(flat) && CountDecimals(flat) == 0);
            if (small || poor)
            {
                double[] extras = (medianH > 0 && medianH < 12)
                    ? new double[] { 2.0, 3.0 } : new double[] { 2.0 };
                foreach (double scale in extras)
                {
                    if (longEdge * scale > cap + 1) continue;
                    try
                    {
                        OcrResult candidate = RecognizeAt(image, engine, scale, cap);
                        double score = QualityScore(candidate);
                        if (score > bestScore + 0.0001)
                        {
                            bestScore = score;
                            best = candidate;
                        }
                    }
                    catch (Exception ex) { last = ex; }
                }
            }
            if (best == null)
                throw new OcrException("OCR 识别失败：" +
                    (last == null ? "未知错误" : last.Message), last);
            return best;
        }

        private static string Flat(OcrResult result)
        {
            return Layout.Flatten(Layout.ToPlainText(result));
        }

        private static bool Allowed(char c)
        {
            if (c < 128)
                return char.IsLetterOrDigit(c) || ".,;:!?%()[]{}/\\-+=_".IndexOf(c) >= 0;
            if (c >= '\u4E00' && c <= '\u9FFF') return true;
            return "，。；：、（）【】".IndexOf(c) >= 0;
        }

        private static int CountSuspicious(string text)
        {
            int n = 0;
            foreach (char c in text) if (!Allowed(c)) n++;
            return n;
        }

        private static int CountDecimals(string text)
        {
            int n = 0;
            for (int i = 1; i + 1 < text.Length; i++)
                if (text[i] == '.' && char.IsDigit(text[i - 1]) && char.IsDigit(text[i + 1])) n++;
            return n;
        }

        private static bool HasDigit(string text)
        {
            foreach (char c in text) if (char.IsDigit(c)) return true;
            return false;
        }

        private static double MedianWordHeight(OcrResult result)
        {
            if (result == null) return 0;
            List<double> values = new List<double>();
            foreach (OcrWord w in result.Words) if (w.H > 0) values.Add(w.H);
            if (values.Count == 0) return 0;
            values.Sort();
            return values[values.Count / 2];
        }

        private static double QualityScore(OcrResult result)
        {
            string text = Flat(result);
            double avg = (double)text.Length / Math.Max(1, result.Words.Count);
            return text.Length - 7.0 * CountSuspicious(text)
                + 3.0 * CountDecimals(text) + 2.0 * avg;
        }

        private static OcrResult Parse(Windows.Media.Ocr.OcrResult raw, double scale, OcrEngine engine)
        {
            OcrResult result = new OcrResult();
            result.Text = raw.Text;
            result.Language = engine.RecognizerLanguage.LanguageTag;
            result.Engine = "windows-ocr";
            result.MaxImageDimension = (int)OcrEngine.MaxImageDimension;
            foreach (OcrLine line in raw.Lines)
            {
                foreach (Windows.Media.Ocr.OcrWord word in line.Words)
                {
                    string text = (word.Text == null ? "" : word.Text.Trim());
                    if (text.Length == 0) continue;
                    OcrWord w = new OcrWord();
                    w.Text = text;
                    w.X = word.BoundingRect.X / scale;
                    w.Y = word.BoundingRect.Y / scale;
                    w.W = word.BoundingRect.Width / scale;
                    w.H = word.BoundingRect.Height / scale;
                    result.Words.Add(w);
                }
            }
            return result;
        }

        internal static string[] AvailableLanguages()
        {
            List<string> tags = new List<string>();
            try
            {
                foreach (Language lang in OcrEngine.AvailableRecognizerLanguages)
                    tags.Add(lang.LanguageTag);
            }
            catch (Exception) { }
            return tags.ToArray();
        }

        internal static int MaxDimension()
        {
            try { return (int)OcrEngine.MaxImageDimension; }
            catch (Exception) { return 0; }
        }
    }
}
