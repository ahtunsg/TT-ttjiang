// PP-OCR (PaddleOCR) offline pipeline: text detection with DB + CTC recognition.
//
// Why this exists: Windows.Media.Ocr is fine on large clean text but drops
// glyphs on 11-14px screenshots and mangles decimal numbers. PP-OCRv6 tiny
// reads those reliably at a third of the model size. Everything stays local -
// two ONNX files plus the onnxruntime DLL, no service call, no Python.
//
// The pipeline mirrors the reference implementation closely enough that the
// same models give the same text: DB probability map -> connected components
// -> unclip to a padded box -> height-normalised crop -> CTC greedy decode.
// Character timesteps are mapped back to pixel positions so the existing
// Layout module still gets per-word rectangles for table reconstruction.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace TTJiang.Capture
{
    internal sealed class PaddleOcrEngine
    {
        // PP-OCRv6 keeps the v5 preprocessing contract (same mean/std, same
        // 48px recognition height). Its tiny detector is noticeably better on
        // low-contrast and dark backgrounds, so the 960 side limit stays.
        private const int DetMaxSide = 1280;
        private const float DetThreshold = 0.30f;
        private const float BoxThreshold = 0.50f;
        private const int RecHeight = 48;
        private const int RecMaxWidth = 640;
        private const int RecOverlap = 96;
        private const int RecMinSegment = 140;

        private static readonly object Gate = new object();
        private static readonly float[] DetMean = { 0.485f, 0.456f, 0.406f };
        private static readonly float[] DetStd = { 0.229f, 0.224f, 0.225f };
        // Recognition models are trained with 0.5/0.5 normalisation. Using the
        // detection constants here still yields plausible Chinese, just wrong.
        private static readonly float[] RecMean = { 0.5f, 0.5f, 0.5f };
        private static readonly float[] RecStd = { 0.5f, 0.5f, 0.5f };

        private static bool _tried;
        private static string _error;
        private static Ort.Session _det;
        private static Ort.Session _rec;
        private static string[] _dict;

        internal static string LastError { get { return _error; } }

        // Rectangles of the last run, for diagnostics only.
        internal static readonly List<Rectangle> LastBoxes = new List<Rectangle>();
        internal static string LastTiming = "";
        // Per-line space-detection trace, diagnostics only (--ocr-check).
        internal static readonly StringBuilder SpaceTrace = new StringBuilder();
        internal static string RuntimeVersion { get { return Ort.RuntimeVersion; } }

        // Loads the runtime and models once. Returns false (with LastError) when
        // the files are missing or the engine cannot start, so the caller can use
        // Windows OCR instead of failing the screenshot.
        internal static bool EnsureReady()
        {
            lock (Gate)
            {
                if (_tried) return _error == null;
                _tried = true;
                try
                {
                    string dir;
                    if (!Runtime.EnsureFiles(out dir))
                    {
                        _error = Runtime.LastError;
                        return false;
                    }
                    if (!Ort.EnsureLoaded(dir))
                    {
                        _error = Ort.LastFailure;
                        return false;
                    }
                    string det = Path.Combine(dir, "det.onnx");
                    string rec = Path.Combine(dir, "rec.onnx");
                    string keys = Path.Combine(dir, "keys.txt");
                    _dict = LoadDictionary(keys);
                    _det = new Ort.Session(det, 4);
                    // Two intra-op threads: boxes are decoded on a small worker
                    // pool instead, which measures 2x faster than one wide run.
                    _rec = new Ort.Session(rec, 2);
                    _error = null;
                    return true;
                }
                catch (Exception ex)
                {
                    _error = ex.Message;
                    return false;
                }
            }
        }

        // Pre-loads both sessions and warms the first inference so the first
        // screenshot does not pay model-load latency. Called on a background
        // thread at startup; also purges the payload that 3.1.0 unpacked into
        // %LOCALAPPDATA%. Failures fall back as usual.
        internal static void Warmup()
        {
            try
            {
                Runtime.CleanLegacy();
                if (!EnsureReady()) return;
                float[] detIn = new float[3 * 32 * 32];
                float[] recIn = new float[3 * RecHeight * 16];
                long[] shape;
                _det.Run(detIn, new long[] { 1, 3, 32, 32 }, out shape);
                _rec.Run(recIn, new long[] { 1, 3, RecHeight, 16 }, out shape);
            }
            catch (Exception) { }
        }

        private static string[] LoadDictionary(string path)
        {
            string[] lines = File.ReadAllLines(path, Encoding.UTF8);
            // CTC layout: 0 is blank, 1..N are the dictionary entries, N+1 is a
            // space class. The models emit N+2 logits (blank + entries + space);
            // the space class was missing here until 3.3.0, so every space had
            // to be guessed from pixel gaps and narrow glyphs got split apart.
            string[] chars = new string[lines.Length + 2];
            chars[0] = "";
            for (int i = 0; i < lines.Length; i++) chars[i + 1] = lines[i];
            chars[lines.Length + 1] = " ";
            return chars;
        }

        internal static OcrResult Recognize(Bitmap image)
        {
            if (image == null) throw new OcrException("选区图像为空");
            if (!EnsureReady()) throw new OcrException("本地 OCR 模型不可用：" + _error);

            System.Diagnostics.Stopwatch watch = System.Diagnostics.Stopwatch.StartNew();
            SpaceTrace.Length = 0;
            int dw, dh;
            float[] prob = RunDetector(image, out dw, out dh);
            long detMs = watch.ElapsedMilliseconds;
            List<Box> boxes = ExtractBoxes(prob, dw, dh, image.Width, image.Height);
            // Grey copy is computed once and shared by ink snapping and word
            // spacing; word spacing must look at real glyph ink, not at the
            // recognition model's timesteps.
            int grayBackground;
            byte[] gray = ToGray(image, out grayBackground);
            SnapToInk(image, gray, grayBackground, boxes);
            SortReadingOrder(boxes);
            LastBoxes.Clear();
            foreach (Box b in boxes) LastBoxes.Add(b.Rect);

            OcrResult result = new OcrResult();
            result.Language = "zh-Hans-CN";
            result.Engine = "pp-ocr-v6";
            result.MaxImageDimension = 0;
            RecogniseLines(image, gray, grayBackground, boxes, result);
            result.Text = Layout.ToPlainText(result);
            LastTiming = "det=" + detMs + "ms rec=" + (watch.ElapsedMilliseconds - detMs)
                + "ms lines=" + boxes.Count;
            return result;
        }

        // ---- detection -----------------------------------------------------------

        private static float[] RunDetector(Bitmap image, out int outW, out int outH)
        {
            double scale = Math.Min(1.0, DetMaxSide / (double)Math.Max(image.Width, image.Height));
            int sw = Math.Max(32, (int)Math.Round(image.Width * scale / 32.0) * 32);
            int sh = Math.Max(32, (int)Math.Round(image.Height * scale / 32.0) * 32);
            float[] data = ToChw(image, sw, sh, DetMean, DetStd);
            long[] shape;
            float[] prob = _det.Run(data, new long[] { 1, 3, sh, sw }, out shape);
            outH = shape.Length >= 4 ? (int)shape[2] : sh;
            outW = shape.Length >= 4 ? (int)shape[3] : sw;
            return prob;
        }

        // Renders the bitmap at the model resolution and converts to CHW floats.
        // GDI+ does the resampling in native code; the managed loop only packs
        // channels, which keeps 960x960 input well under 10 ms.
        private static float[] ToChw(Bitmap source, int width, int height, float[] mean, float[] std)
        {
            float[] data = new float[3 * width * height];
            int plane = width * height;
            using (Bitmap scaled = new Bitmap(width, height, PixelFormat.Format32bppArgb))
            {
                using (Graphics g = Graphics.FromImage(scaled))
                {
                    g.InterpolationMode = InterpolationMode.HighQualityBilinear;
                    g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                    g.CompositingMode = CompositingMode.SourceCopy;
                    g.DrawImage(source, new Rectangle(0, 0, width, height));
                }
                BitmapData bits = scaled.LockBits(new Rectangle(0, 0, width, height),
                    ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                try
                {
                    int stride = bits.Stride;
                    byte[] row = new byte[stride];
                    for (int y = 0; y < height; y++)
                    {
                        Marshal.Copy(IntPtr.Add(bits.Scan0, y * stride), row, 0, stride);
                        int rowOffset = y * width;
                        for (int x = 0; x < width; x++)
                        {
                            int p = x * 4;
                            float b = row[p] / 255f;
                            float g2 = row[p + 1] / 255f;
                            float r = row[p + 2] / 255f;
                            data[rowOffset + x] = (r - mean[0]) / std[0];
                            data[plane + rowOffset + x] = (g2 - mean[1]) / std[1];
                            data[2 * plane + rowOffset + x] = (b - mean[2]) / std[2];
                        }
                    }
                }
                finally { scaled.UnlockBits(bits); }
            }
            return data;
        }

        private struct Box
        {
            internal Rectangle Rect;
            internal float Score;
        }

        // Binarise, label 8-connected blobs, keep the ones that look like text.
        private static List<Box> ExtractBoxes(float[] prob, int pw, int ph,
            int imageWidth, int imageHeight)
        {
            List<Box> boxes = new List<Box>();
            if (pw <= 0 || ph <= 0) return boxes;
            byte[] mask = new byte[pw * ph];
            for (int i = 0; i < mask.Length; i++)
                mask[i] = prob[i] > DetThreshold ? (byte)1 : (byte)0;

            int[] stack = new int[pw * ph];
            double fx = (double)imageWidth / pw;
            double fy = (double)imageHeight / ph;

            for (int start = 0; start < mask.Length; start++)
            {
                if (mask[start] != 1) continue;
                int top = 0;
                stack[top++] = start;
                mask[start] = 2;
                int minX = pw, maxX = -1, minY = ph, maxY = -1;
                double sum = 0;
                int count = 0;
                while (top > 0)
                {
                    int index = stack[--top];
                    int x = index % pw;
                    int y = index / pw;
                    if (x < minX) minX = x;
                    if (x > maxX) maxX = x;
                    if (y < minY) minY = y;
                    if (y > maxY) maxY = y;
                    sum += prob[index];
                    count++;
                    for (int dy = -1; dy <= 1; dy++)
                    {
                        int ny = y + dy;
                        if (ny < 0 || ny >= ph) continue;
                        for (int dx = -1; dx <= 1; dx++)
                        {
                            int nx = x + dx;
                            if (nx < 0 || nx >= pw) continue;
                            int n = ny * pw + nx;
                            if (mask[n] == 1)
                            {
                                mask[n] = 2;
                                stack[top++] = n;
                            }
                        }
                    }
                }
                int w = maxX - minX + 1;
                int h = maxY - minY + 1;
                if (w < 3 || h < 3) continue;
                float score = (float)(sum / count);
                if (score < BoxThreshold) continue;
                // DB outputs the shrunken text kernel; grow it back so ascenders
                // and thin strokes are inside the crop.
                int modelH = h;
                int padX = Math.Max(5, (int)Math.Round(modelH * 0.6));
                int padY = Math.Max(3, (int)Math.Round(modelH * 0.45));
                int x1 = Math.Max(0, (int)(minX * fx) - padX);
                int y1 = Math.Max(0, (int)(minY * fy) - padY);
                int x2 = Math.Min(imageWidth, (int)((maxX + 1) * fx) + padX);
                int y2 = Math.Min(imageHeight, (int)((maxY + 1) * fy) + padY);
                Box box = new Box();
                box.Rect = Rectangle.FromLTRB(x1, y1, Math.Max(x1 + 1, x2), Math.Max(y1 + 1, y2));
                box.Score = score;
                boxes.Add(box);
            }
            return boxes;
        }

        // Grey copy of the source plus its modal (most common) luminance. The
        // mode is the background on light screenshots and on dark ones alike,
        // so ink can be detected as "deviates from the background".
        private static byte[] ToGray(Bitmap image, out int background)
        {
            int w = image.Width;
            int h = image.Height;
            byte[] gray = new byte[w * h];
            int[] hist = new int[256];
            BitmapData bits = image.LockBits(new Rectangle(0, 0, w, h),
                ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            try
            {
                int stride = bits.Stride;
                byte[] row = new byte[stride];
                for (int y = 0; y < h; y++)
                {
                    Marshal.Copy(IntPtr.Add(bits.Scan0, y * stride), row, 0, stride);
                    int offset = y * w;
                    for (int x = 0; x < w; x++)
                    {
                        int p = x * 4;
                        int lum = (row[p] * 29 + row[p + 1] * 150 + row[p + 2] * 77) >> 8;
                        gray[offset + x] = (byte)lum;
                        hist[lum]++;
                    }
                }
            }
            finally { image.UnlockBits(bits); }
            int mode = 255;
            int best = -1;
            for (int i = 0; i < 256; i++)
            {
                if (hist[i] > best) { best = hist[i]; mode = i; }
            }
            background = mode;
            return gray;
        }

        private const int InkDelta = 44;

        // The global background is the most common grey in the whole capture. A
        // dark panel (IDE, terminal, dark web page) next to a white page breaks
        // that assumption: the panel reads as solid ink, so every edge walk runs
        // into it and swallows the neighbouring lines. Sample the ring just
        // outside each box instead, which follows the local surface.
        private static int LocalBackground(byte[] gray, int width, int height,
            Rectangle rect, int fallback)
        {
            if (rect.Right <= 0 || rect.Bottom <= 0 || rect.Left >= width ||
                rect.Top >= height)
                return fallback;
            int[] hist = new int[256];
            int count = 0;
            int x1 = Math.Max(0, Math.Min(width - 1, rect.Left - 3));
            int x2 = Math.Max(x1, Math.Min(width - 1, rect.Right + 2));
            int y1 = Math.Max(0, Math.Min(height - 1, rect.Y - 3));
            int y2 = Math.Max(y1, Math.Min(height - 1, rect.Bottom + 2));
            for (int x = x1; x <= x2; x++)
            {
                hist[gray[y1 * width + x]]++;
                count++;
                if (y2 != y1) { hist[gray[y2 * width + x]]++; count++; }
            }
            for (int y = y1; y <= y2; y++)
            {
                hist[gray[y * width + x1]]++;
                count++;
                if (x2 != x1) { hist[gray[y * width + x2]]++; count++; }
            }
            if (count == 0) return fallback;
            int half = count / 2, acc = 0;
            for (int i = 0; i < 256; i++)
            {
                acc += hist[i];
                if (acc >= half) return i;
            }
            return fallback;
        }

        private static bool ColumnHasInk(byte[] gray, int width, int x, int y1, int y2,
            int background)
        {
            for (int y = y1; y < y2; y++)
            {
                int delta = gray[y * width + x] - background;
                if (delta > InkDelta || delta < -InkDelta) return true;
            }
            return false;
        }

        private static bool RowHasInk(byte[] gray, int width, int y, int x1, int x2,
            int background)
        {
            int offset = y * width;
            for (int x = x1; x < x2; x++)
            {
                int delta = gray[offset + x] - background;
                if (delta > InkDelta || delta < -InkDelta) return true;
            }
            return false;
        }

        // DB emits the shrunken text kernel. A fixed ratio pad recovers ascenders
        // but still clips glyphs when the kernel starts inside a stroke, or when a
        // rule or border touches the line ("TT-SPrint" under a header rule reads
        // as "L -SPrint" because the T bars end up outside the crop). Walk every
        // edge outwards while the neighbouring rows or columns still carry ink,
        // then add a small margin. Walking stops at the first blank row/column,
        // so a neighbouring line with normal spacing is never swallowed.
        private static void SnapToInk(Bitmap image, byte[] gray, int background,
            List<Box> boxes)
        {
            if (boxes.Count == 0) return;
            int w = image.Width;
            int hgt = image.Height;
            for (int i = 0; i < boxes.Count; i++)
            {
                Box b = boxes[i];
                Rectangle r = b.Rect;
                // The detector works on a downscaled copy and can hand back a box
                // that sticks out of the bitmap by a pixel or two; every probe
                // below indexes the grey buffer directly, so clamp once here.
                r.Intersect(new Rectangle(0, 0, w, hgt));
                if (r.Width < 2 || r.Height < 2) { b.Rect = Rectangle.Empty; boxes[i] = b; continue; }
                int h = Math.Max(1, r.Height);
                int localBg = LocalBackground(gray, w, hgt, r, background);
                // Keep the full vertical span: the top bar of a "T" or the dot of
                // an "i" only exists in the first rows, and trimming them makes
                // the probe blind exactly where it matters.
                int y1 = r.Y + 2;
                int y2 = r.Bottom - 2;
                if (y2 - y1 < 4) { y1 = r.Y; y2 = r.Bottom; }
                int maxGrow = Math.Max(6, Math.Min(h, 40));
                int left = r.Left;
                int right = r.Right;
                int steps = 0;
                while (left > 0 && steps < maxGrow
                    && ColumnHasInk(gray, w, left - 1, y1, y2, localBg))
                {
                    left--;
                    steps++;
                }
                if (left < r.Left) left = Math.Max(0, left - 2);
                steps = 0;
                while (right < w && steps < maxGrow
                    && ColumnHasInk(gray, w, right, y1, y2, localBg))
                {
                    right++;
                    steps++;
                }
                if (right > r.Right) right = Math.Min(w, right + 2);
                // Vertical: recover clipped ascenders/descenders and pull in a
                // rule that touches the line (the model needs it for context).
                int top = r.Y;
                int bottom = r.Bottom;
                steps = 0;
                while (top > 0 && steps < maxGrow
                    && RowHasInk(gray, w, top - 1, left, right, localBg))
                {
                    top--;
                    steps++;
                }
                if (top < r.Y) top = Math.Max(0, top - 2);
                steps = 0;
                while (bottom < hgt && steps < maxGrow
                    && RowHasInk(gray, w, bottom, left, right, localBg))
                {
                    bottom++;
                    steps++;
                }
                if (bottom > r.Bottom) bottom = Math.Min(hgt, bottom + 2);
                b.Rect = Rectangle.FromLTRB(left, top, Math.Max(left + 1, right),
                    Math.Max(top + 1, bottom));
                boxes[i] = b;
            }
        }

        // Top-to-bottom, left-to-right. Boxes are bucketed by vertical centre with a
        // tolerance proportional to their own height so mixed font sizes still read
        // in the right order.
        private static void SortReadingOrder(List<Box> boxes)
        {
            boxes.Sort(delegate(Box a, Box b)
            {
                int ha = Math.Max(1, a.Rect.Height);
                int hb = Math.Max(1, b.Rect.Height);
                int bandA = (int)Math.Round(a.Rect.Y / (ha * 0.6));
                int bandB = (int)Math.Round(b.Rect.Y / (hb * 0.6));
                int overlap = Math.Min(a.Rect.Bottom, b.Rect.Bottom)
                    - Math.Max(a.Rect.Y, b.Rect.Y);
                bool sameLine = overlap > Math.Min(ha, hb) * 0.5;
                if (sameLine) return a.Rect.X.CompareTo(b.Rect.X);
                if (bandA != bandB) return bandA.CompareTo(bandB);
                return a.Rect.Y.CompareTo(b.Rect.Y);
            });
        }

        // ---- recognition ---------------------------------------------------------

        // Diagnostics: TTSPRINT_DUMP=<dir> writes the exact tensor handed to the
        // recognition model so it can be compared against a reference run.
        private static int _dumpIndex;
        private static void DumpTensor(float[] data, int width, int height)
        {
            string dir = Environment.GetEnvironmentVariable("TTSPRINT_DUMP");
            if (string.IsNullOrEmpty(dir)) return;
            try
            {
                Directory.CreateDirectory(dir);
                string path = Path.Combine(dir, "rec" + (_dumpIndex++) + "_" + width + "x"
                    + height + ".f32");
                byte[] bytes = new byte[data.Length * 4];
                Buffer.BlockCopy(data, 0, bytes, 0, bytes.Length);
                File.WriteAllBytes(path, bytes);
            }
            catch (Exception) { }
        }

        private struct RecognisedChar
        {
            internal char Value;
            internal double X;
            internal double W;
            // True when the model emitted its space class right before this
            // glyph; BuildWords treats it as authoritative.
            internal bool Space;
        }

        // Recognition quality collapses once the input gets wide: the model is
        // trained around 48x320, and a full-width table row at 48x1200 turns
        // digits into noise. Long lines are therefore cut into overlapping
        // segments; only each segment's core range contributes characters, so the
        // overlap is context rather than duplicated text.
        //
        // Pipeline: (1) on this thread, crop and normalise every segment - GDI+
        // is not thread-safe, so all bitmap access stays serial and cheap;
        // (2) run the ONNX tensors on a small worker pool - sessions accept
        // concurrent Run calls; (3) merge characters in reading order. A 20-line
        // table measures ~185 ms serial vs ~90 ms parallel for the run stage.
        private static void RecogniseLines(Bitmap image, byte[] gray, int background,
            List<Box> boxes, OcrResult result)
        {
            List<RecJob> jobs = new List<RecJob>();
            int[] first = new int[boxes.Count + 1];
            for (int i = 0; i < boxes.Count; i++)
            {
                first[i] = jobs.Count;
                Rectangle rect = boxes[i].Rect;
                if (rect.Width < 6 || rect.Height < 6) continue;
                CollectSegments(image, rect, i, jobs);
            }
            first[boxes.Count] = jobs.Count;

            List<RecognisedChar>[] decoded = RunJobs(jobs);

            for (int i = 0; i < boxes.Count; i++)
            {
                List<RecognisedChar> chars = new List<RecognisedChar>();
                for (int j = first[i]; j < first[i + 1]; j++)
                {
                    List<RecognisedChar> part = decoded[j];
                    if (part == null) continue;
                    for (int k = 0; k < part.Count; k++) chars.Add(part[k]);
                }
                BuildWords(gray, image.Width, LocalBackground(gray, image.Width,
                    image.Height, boxes[i].Rect, background), boxes[i].Rect, chars, result);
            }
        }

        private struct RecJob
        {
            internal float[] Data;    // CHW tensor handed to the recognition model
            internal int Width;       // natural (48-height) width of the tensor
            internal int From;        // natural x of the tensor's left edge
            internal int Core0;       // natural range whose characters count
            internal int Core1;
            internal double Scale;    // natural units -> source pixels
            internal Rectangle Rect;  // source rectangle of the whole line
        }

        private static void CollectSegments(Bitmap image, Rectangle rect, int boxIndex,
            List<RecJob> jobs)
        {
            double scaleToSource = rect.Height / (double)RecHeight;
            int natural = (int)Math.Round(rect.Width / scaleToSource);
            if (natural < 16) natural = 16;

            int start = 0;
            while (start < natural)
            {
                int core1 = Math.Min(natural, start + RecMaxWidth);
                // Never leave a sliver at the end: a 20px-wide segment carries no
                // context and decodes to noise that would pollute the line.
                int remain = natural - core1;
                if (remain > 0 && remain < RecMinSegment) core1 = natural;
                int from = Math.Max(0, start - RecOverlap / 2);
                int to = Math.Min(natural, core1 + RecOverlap / 2);
                if (to - from >= 16) jobs.Add(MakeJob(image, rect, scaleToSource,
                    from, to, start, core1));
                if (core1 >= natural) break;
                start = core1;
            }
        }

        private static RecJob MakeJob(Bitmap image, Rectangle rect, double scaleToSource,
            int from, int to, int core0, int core1)
        {
            int targetW = to - from;
            int srcX = rect.X + (int)Math.Round(from * scaleToSource);
            int srcW = (int)Math.Round(targetW * scaleToSource);
            srcX = Math.Max(rect.X, Math.Min(rect.Right - 1, srcX));
            srcW = Math.Max(1, Math.Min(rect.Right - srcX, srcW));

            RecJob job = new RecJob();
            job.Width = targetW;
            job.From = from;
            job.Core0 = core0;
            job.Core1 = core1;
            job.Scale = scaleToSource;
            job.Rect = rect;
            using (Bitmap crop = new Bitmap(srcW, rect.Height, PixelFormat.Format32bppArgb))
            {
                using (Graphics g = Graphics.FromImage(crop))
                {
                    g.CompositingMode = CompositingMode.SourceCopy;
                    g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                    g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                    g.DrawImage(image, new Rectangle(0, 0, srcW, rect.Height),
                        srcX, rect.Y, srcW, rect.Height, GraphicsUnit.Pixel);
                }
                // Tested a bicubic-upscale + unsharp path for rows under 20 px:
                // it did not recover any misread glyph and it widened glyph edges
                // enough to swallow real word gaps ("foxjumps"). Kept the plain
                // resample, which measures better on both counts.
                job.Data = ToChw(crop, targetW, RecHeight, RecMean, RecStd);
            }
            return job;
        }

        private static List<RecognisedChar>[] RunJobs(List<RecJob> jobs)
        {
            List<RecognisedChar>[] decoded = new List<RecognisedChar>[jobs.Count];
            if (jobs.Count == 0) return decoded;
            int workers = Math.Min(4, Environment.ProcessorCount);
            if (workers > jobs.Count) workers = jobs.Count;
            if (workers <= 1)
            {
                for (int i = 0; i < jobs.Count; i++) decoded[i] = RunJob(jobs[i]);
                return decoded;
            }
            int next = 0;
            int remaining = workers;
            using (ManualResetEvent done = new ManualResetEvent(false))
            {
                for (int w = 0; w < workers; w++)
                {
                    ThreadPool.QueueUserWorkItem(delegate(object unused)
                    {
                        try
                        {
                            while (true)
                            {
                                int index = Interlocked.Increment(ref next) - 1;
                                if (index >= jobs.Count) break;
                                try { decoded[index] = RunJob(jobs[index]); }
                                catch (Exception) { decoded[index] = null; }
                            }
                        }
                        finally
                        {
                            if (Interlocked.Decrement(ref remaining) == 0) done.Set();
                        }
                    });
                }
                done.WaitOne();
            }
            return decoded;
        }

        private static List<RecognisedChar> RunJob(RecJob job)
        {
            DumpTensor(job.Data, job.Width, RecHeight);
            long[] shape;
            float[] output = _rec.Run(job.Data, new long[] { 1, 3, RecHeight, job.Width },
                out shape);
            List<RecognisedChar> chars = new List<RecognisedChar>();
            int steps = shape.Length >= 3 ? (int)shape[1] : 0;
            int classes = shape.Length >= 3 ? (int)shape[2] : 0;
            if (steps <= 0 || classes <= 0) return chars;

            double stepWidth = job.Width / (double)steps;
            // Last class is the space token: blank + dictionary entries + space.
            // The models do emit it, which is far more reliable than guessing a
            // word break from the gap between two narrow glyphs.
            int spaceClass = _dict.Length - 1;
            bool pendingSpace = false;
            int previous = -1;
            for (int t = 0; t < steps; t++)
            {
                int offset = t * classes;
                int best = 0;
                float bestScore = output[offset];
                for (int c = 1; c < classes; c++)
                {
                    float v = output[offset + c];
                    if (v > bestScore) { bestScore = v; best = c; }
                }
                if (best == 0 || best == previous) { previous = best; continue; }
                previous = best;

                double naturalX = job.From + (t + 0.5) * stepWidth;
                // Drop a sliver at the core edge: the same glyph can be decoded by
                // both neighbouring segments with slightly different x, which would
                // otherwise appear twice.
                if (naturalX < job.Core0 + 2 || naturalX >= job.Core1) continue;
                if (best == spaceClass) { pendingSpace = true; continue; }
                string glyph = best < _dict.Length ? _dict[best] : "";
                if (glyph.Length == 0) continue;
                RecognisedChar item = new RecognisedChar();
                item.Value = glyph[0];
                item.W = stepWidth * job.Scale;
                item.X = job.Rect.X + naturalX * job.Scale;
                item.Space = pendingSpace;
                pendingSpace = false;
                chars.Add(item);
            }
            return chars;
        }

        // Group characters into words. Chinese has no spaces, so a wide gap is the
        // only signal for a column boundary; an explicit space character also ends
        // a word. Gaps shorter than half a line height stay inside one word.
        private static void BuildWords(byte[] gray, int grayWidth, int background,
            Rectangle rect, List<RecognisedChar> chars, OcrResult result)
        {
            if (chars.Count == 0) return;
            // CTC timesteps are not glyph widths (one Chinese character spans
            // several steps), so derive each glyph box from the distance to its
            // neighbours and cap it at a plausible character width.
            double cap = rect.Height * 0.72;
            double fallback = rect.Height * 0.6;
            for (int i = 0; i < chars.Count; i++)
            {
                double pitch = double.MaxValue;
                if (i > 0) pitch = chars[i].X - chars[i - 1].X;
                if (i + 1 < chars.Count) pitch = Math.Min(pitch, chars[i + 1].X - chars[i].X);
                if (pitch == double.MaxValue || pitch <= 0) pitch = fallback;
                RecognisedChar item = chars[i];
                item.W = Math.Max(2.0, Math.Min(pitch, cap));
                chars[i] = item;
            }

            // The model reports most word breaks itself through its space class,
            // which is carried on the first glyph after the blank. Pixel
            // geometry only fills the gaps it stays silent about:
            //  - half a line height or more is a real blank (a full-width space,
            //    a table gutter) whatever the neighbouring glyphs are;
            //  - a CJK glyph followed by Latin or a digit is a typeset word
            //    boundary even without a space token.
            // Latin-to-Latin is never guessed: narrow glyphs leave 8-10 px of
            // air by design, which is what used to split "hello" into "hel lo".
            int[] spaceBefore, wideBefore;
            MeasureSpaces(gray, grayWidth, background, rect, chars,
                out spaceBefore, out wideBefore);

            // Split on the gap between glyph boxes: contiguous Chinese characters
            // touch, while table columns leave a wide blank.
            double gapLimit = rect.Height * 0.5;
            int index = 0;
            while (index < chars.Count)
            {
                if (chars[index].Value == ' ') { index++; continue; }
                StringBuilder text = new StringBuilder();
                double firstLeft = chars[index].X - chars[index].W * 0.5;
                double lastRight = firstLeft + chars[index].W;
                while (index < chars.Count)
                {
                    RecognisedChar c = chars[index];
                    if (c.Value == ' ') break;
                    double left = c.X - c.W * 0.5;
                    if (text.Length > 0 && left - lastRight > gapLimit) break;
                    if (text.Length > 0)
                    {
                        char previousChar = text[text.Length - 1];
                        bool space = c.Space || wideBefore[index] == 1
                            || (spaceBefore[index] == 1 && IsCjkChar(previousChar)
                                && !IsCjkChar(c.Value));
                        // The other direction stays closed up on purpose: "8,650元"
                        // and "120ms" read wrong with a blank in front of the unit.
                        if (space && previousChar != ' ' &&
                            !SuppressSpace(previousChar, c.Value) &&
                            !IsDecimalPoint(text, c.Value))
                            text.Append(' ');
                    }
                    text.Append(c.Value);
                    lastRight = c.X + c.W * 0.5;
                    index++;
                }
                if (text.Length == 0) continue;
                OcrWord word = new OcrWord();
                word.Text = text.ToString();
                word.X = Math.Max(rect.X, firstLeft);
                word.W = Math.Max(rect.Height * 0.35, lastRight - firstLeft);
                word.Y = rect.Y;
                word.H = rect.Height;
                result.Words.Add(word);
            }
        }

        // Flags index i when the ink gap between glyph i-1 and glyph i is wide
        // enough to be a real space. The model's own space class decides most
        // cases; this measurement only covers the blanks it does not mark.
        // wideBefore holds the unambiguous ones (half a line height or more),
        // which is the only geometric evidence trusted now.
        private static void MeasureSpaces(byte[] gray, int grayWidth, int background,
            Rectangle rect, List<RecognisedChar> chars,
            out int[] spaceBefore, out int[] wideBefore)
        {
            spaceBefore = new int[chars.Count];
            wideBefore = new int[chars.Count];
            List<int> runStart, runEnd;
            InkRuns(gray, grayWidth, background, rect, out runStart, out runEnd);
            if (runStart.Count < 2) return;

            int[] runOf = new int[chars.Count];
            List<int> gaps = new List<int>(chars.Count);
            for (int i = 0; i < chars.Count; i++)
            {
                runOf[i] = NearestRun(runStart, runEnd, (int)Math.Round(chars[i].X));
                if (i == 0) { gaps.Add(0); continue; }
                int prev = runOf[i - 1];
                int cur = runOf[i];
                gaps.Add(prev == cur || prev < 0 || cur < 0
                    ? 0
                    : Math.Max(0, runStart[cur] - runEnd[prev] - 1));
            }
            int threshold = SpaceThreshold(gaps, rect.Height);
            int wideLimit = (int)Math.Ceiling(Math.Max(threshold * 2.0, rect.Height * 0.45));
            int marked = MarkSpaces(gaps, chars, threshold, wideLimit, spaceBefore, wideBefore);
            // A dense mono-spaced or heavily anti-aliased line leaves a 2-3 px
            // blank between every glyph, which the base threshold reads as a
            // space and shreds into "ve r s ion". When a third of the glyphs
            // fire, the threshold is too low for this line: raise it and redo.
            if (chars.Count > 0 && marked * 3 > chars.Count)
            {
                threshold = (int)Math.Ceiling(threshold * 1.8);
                Array.Clear(spaceBefore, 0, spaceBefore.Length);
                Array.Clear(wideBefore, 0, wideBefore.Length);
                marked = MarkSpaces(gaps, chars, threshold, wideLimit, spaceBefore, wideBefore);
            }
            int wide = 0;
            for (int i = 0; i < wideBefore.Length; i++) if (wideBefore[i] == 1) wide++;
            SpaceTrace.Append("h=").Append(rect.Height)
                .Append(" thr=").Append(threshold)
                .Append(" sp=").Append(marked)
                .Append(" wide=").Append(wide)
                .Append('/').Append(chars.Count)
                .Append(" gaps=").Append(string.Join(",", gaps.ToArray()))
                .Append('\n');
        }

        private static int MarkSpaces(List<int> gaps, List<RecognisedChar> chars,
            int threshold, int wideLimit, int[] spaceBefore, int[] wideBefore)
        {
            int marked = 0;
            for (int i = 1; i < gaps.Count; i++)
            {
                if (gaps[i] >= wideLimit) wideBefore[i] = 1;
                int limit = threshold;
                // Two CJK glyphs are flush by design, so a blank between them is
                // loose kerning rather than a space; demand a much wider gap.
                if (IsCjkChar(chars[i - 1].Value) && IsCjkChar(chars[i].Value))
                    limit = (int)Math.Ceiling(threshold * 2.2);
                if (gaps[i] >= limit) { spaceBefore[i] = 1; marked++; }
            }
            return marked;
        }

        private static void InkRuns(byte[] gray, int grayWidth, int background,
            Rectangle rect, out List<int> starts, out List<int> ends)
        {
            starts = new List<int>(32);
            ends = new List<int>(32);
            int y1 = rect.Y + 1;
            int y2 = rect.Bottom - 1;
            if (y2 - y1 < 3) { y1 = rect.Y; y2 = rect.Bottom; }
            int x1 = Math.Max(0, rect.Left);
            int x2 = Math.Min(grayWidth, rect.Right);
            bool open = false;
            for (int x = x1; x < x2; x++)
            {
                if (ColumnHasInk(gray, grayWidth, x, y1, y2, background))
                {
                    if (!open) { starts.Add(x); open = true; }
                }
                else if (open)
                {
                    ends.Add(x - 1);
                    open = false;
                }
            }
            if (open) ends.Add(x2 - 1);
            while (ends.Count < starts.Count) ends.Add(x2 - 1);
        }

        private static int NearestRun(List<int> starts, List<int> ends, int x)
        {
            int lo = 0;
            int hi = starts.Count - 1;
            while (lo <= hi)
            {
                int mid = (lo + hi) / 2;
                if (x < starts[mid]) hi = mid - 1;
                else if (x > ends[mid]) lo = mid + 1;
                else return mid;
            }
            int right = Math.Min(lo, starts.Count - 1);
            int left = right - 1;
            if (left < 0) return right;
            if (right == left) return left;
            return (x - ends[left]) <= (starts[right] - x) ? left : right;
        }

        // Calibrated over Arial / Segoe UI / Tahoma / Calibri / Consolas / Courier
        // New / Microsoft YaHei / SimSun / DengXian at 12-28 px: every intra-word
        // gap stays below 1.3x the line's median gap plus a pixel or two, while a
        // real space is always wider, whichever font and size is on screen.
        private static int SpaceThreshold(List<int> gaps, int height)
        {
            // Blank width that separates words scales with the line height, not
            // with the distribution of gaps. Deriving it from the median fails
            // exactly where it matters: on a CJK line with spaces the median IS
            // a space, which pushes the threshold above every real gap and drops
            // them all. 0.18*H sits above the 0-2 px inter-glyph gap of both
            // Latin and CJK and below the 4-8 px blank of a 16-22 px line.
            return (int)Math.Ceiling(Math.Max(2.5, height * 0.18));
        }

        // A wide gap before closing punctuation, or after an opening bracket, is
        // glyph spacing rather than a word break.
        private static bool SuppressSpace(char previous, char next)
        {
            const string Closing = ".,;:!?%)]}>，。！？；：、）》】」』";
            const string Opening = "([{<（《【「『";
            if (Closing.IndexOf(next) >= 0) return true;
            if (Opening.IndexOf(previous) >= 0) return true;
            // "C:" + "\Users" measures as a real blank but is one token.
            if ((previous == ':' || previous == ';') && (next == '\\' || next == '/'))
                return true;
            return false;
        }

        private static bool IsCjkChar(char c)
        {
            return (c >= 0x2E80 && c <= 0x9FFF)
                || (c >= 0x3000 && c <= 0x303F)
                || (c >= 0xFF00 && c <= 0xFFEF);
        }

        // "25. 50" is one decimal number: a blank after the point that sits
        // between two digits is measurement noise, not a word break.
        private static bool IsDecimalPoint(StringBuilder text, char next)
        {
            int n = text.Length;
            if (n < 2 || text[n - 1] != '.') return false;
            return char.IsDigit(text[n - 2]) && char.IsDigit(next);
        }
    }
}
