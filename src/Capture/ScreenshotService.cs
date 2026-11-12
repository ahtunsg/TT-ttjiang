// TT酱 3.0 的截图与离线 OCR 编排层。
//
// 截图管线（抓屏、框选遮罩、动作条、提示条）沿用 TT-SPrint 的原生 Win32 实现；
// 区别是这里不再自己跑 GetMessage 循环，也不再注册托盘 —— 原生窗口挂在 WPF 的
// UI 线程上，由 Dispatcher 的消息泵统一派发，托盘入口交给 TT酱 现有的 TrayIcon。
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Threading;
using TTJiang.Services;

namespace TTJiang.Capture
{
    internal sealed class HostWindow : WinWindow
    {
        private readonly Action<uint, IntPtr, IntPtr> _dispatch;

        internal HostWindow(Action<uint, IntPtr, IntPtr> dispatch)
        {
            _dispatch = dispatch;
            CreateWindow(0, 0, 0, 0, 1, 1);
        }

        protected override void Draw(Graphics g) { }

        protected override IntPtr OnMessage(uint msg, IntPtr wParam, IntPtr lParam)
        {
            if (_dispatch != null)
                _dispatch(msg, wParam, lParam);
            return base.OnMessage(msg, wParam, lParam);
        }
    }

    public sealed class ScreenshotService : IDisposable
    {
        private const int TimerHotkeyRetry = 2;
        private const int HotkeyId = 1;

        private static ScreenshotService _current;

        internal static ScreenshotService Current { get { return _current; } }

        internal static string HotkeyText
        {
            get { return _current == null ? "Ctrl+1" : _current._config.Hotkey; }
        }

        private readonly Config _config;
        private readonly Func<bool> _beforeCapture;
        private HostWindow _host;
        private Toast _toast;
        private SelectionOverlay _overlay;
        private ActionBar _bar;
        private Bitmap _screen;
        private Bitmap _selection;
        private Rectangle _selectionRect;
        private bool _hotkeyOk;
        private bool _busy;
        private bool _pending;
        private int _hotkeyMods;
        private int _hotkeyVk;
        private bool _disposed;

        private ScreenshotService(Config config, Func<bool> beforeCapture)
        {
            _config = config;
            _beforeCapture = beforeCapture;
        }

        /// <summary>
        /// 必须在 WPF UI 线程调用：原生遮罩窗口要与 Dispatcher 共用消息泵。
        /// beforeCapture 返回 true 表示它隐藏了 TT酱 自己的窗口，需要等一帧再抓屏。
        /// </summary>
        internal static bool Start(Func<bool> beforeCapture)
        {
            if (_current != null) return true;

            try
            {
                ScreenshotService service = new ScreenshotService(Config.Load(), beforeCapture);
                service.Init();
                _current = service;
                ConfigStore.Log("截图模块已启动，热键 " + service._config.Hotkey);
                return true;
            }
            catch (Exception ex)
            {
                ConfigStore.Log("截图模块启动失败：" + ex.GetType().Name + ": " + ex.Message);
                return false;
            }
        }

        private void Init()
        {
            _host = new HostWindow(Dispatch);
            IntPtr hwnd = _host.Handle;

            ParseHotkey(_config.Hotkey, out _hotkeyMods, out _hotkeyVk);
            _toast = new Toast(_config.ShowToast);
            Ocr.PreferLocalModel = _config.OcrEngine != "system";
            Ocr.Start(hwnd);

            // Pay extraction + model load in the background: the first screenshot
            // should not pay for 29 MB of file writes and two session builds.
            bool warmLocal = Ocr.PreferLocalModel;
            System.Threading.ThreadPool.QueueUserWorkItem(delegate(object unused)
            {
                if (warmLocal) PaddleOcrEngine.Warmup();
            });

            _hotkeyOk = Native.RegisterHotKey(hwnd, HotkeyId, _hotkeyMods, _hotkeyVk);
            if (!_hotkeyOk)
            {
                ConfigStore.Log("截图热键 " + _config.Hotkey + " 被占用，每 3 秒重试");
                Native.SetTimer(hwnd, new IntPtr(TimerHotkeyRetry), 3000, IntPtr.Zero);
            }
        }

        /// <summary>托盘菜单和热键共用这一个入口。</summary>
        internal void CaptureNow()
        {
            if (_disposed) return;

            if (!System.Windows.Application.Current.Dispatcher.CheckAccess())
            {
                System.Windows.Application.Current.Dispatcher.BeginInvoke(new Action(CaptureNow));
                return;
            }

            BeginCapture();
        }

        private void Dispatch(uint msg, IntPtr wParam, IntPtr lParam)
        {
            if (msg == Native.WM_HOTKEY && (int)wParam == HotkeyId)
            {
                BeginCapture();
                return;
            }

            if (msg == Native.APP_OCR_DONE)
            {
                Ocr.DrainCompleted();
                return;
            }

            if (msg == Native.WM_TIMER && (int)wParam == TimerHotkeyRetry)
            {
                RetryHotkey();
            }
        }

        private void RetryHotkey()
        {
            if (_hotkeyOk || _host == null) return;
            _hotkeyOk = Native.RegisterHotKey(_host.Handle, HotkeyId, _hotkeyMods, _hotkeyVk);
            if (_hotkeyOk)
            {
                Native.KillTimer(_host.Handle, new IntPtr(TimerHotkeyRetry));
                _toast.ShowMessage("已接管截图快捷键 " + _config.Hotkey, true, 2200);
                ConfigStore.Log("截图热键已接管：" + _config.Hotkey);
            }
        }

        // ---- capture flow ----

        private void BeginCapture()
        {
            if (_overlay != null || _bar != null || _pending) return;
            _pending = true;

            bool hidden = _beforeCapture != null && _beforeCapture();
            if (!hidden)
            {
                GrabAndShow();
                return;
            }

            // 启动器刚隐藏时 DWM 还差一帧才把它从桌面移除，立刻抓屏会截到它自己。
            DispatcherTimer delay = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(140) };
            delay.Tick += delegate(object sender, EventArgs e)
            {
                delay.Stop();
                GrabAndShow();
            };
            delay.Start();
        }

        private void GrabAndShow()
        {
            _pending = false;
            if (_disposed || _overlay != null || _bar != null) return;

            try
            {
                _screen = Capture.GrabVirtualScreen();
                _overlay = new SelectionOverlay(_screen, OnSelected, OnCancelled);
                _overlay.Show(true);
            }
            catch (Exception ex)
            {
                ReleaseCapture();
                _toast.ShowMessage("截图失败：" + Short(ex.Message), false, 3000);
            }
        }

        private void OnCancelled()
        {
            ReleaseCapture();
        }

        private void OnSelected(Rectangle screenRect)
        {
            int vx, vy, vw, vh;
            SystemInfo.VirtualScreen(out vx, out vy, out vw, out vh);
            try
            {
                _selectionRect = screenRect;
                _selection = Capture.Crop(_screen, screenRect, vx, vy);
            }
            catch (Exception ex)
            {
                ReleaseCapture();
                _toast.ShowMessage("选区无效：" + Short(ex.Message), false, 2600);
                return;
            }

            if (_overlay != null) { _overlay.Dispose(); _overlay = null; }
            if (_screen != null) { _screen.Dispose(); _screen = null; }

            _bar = new ActionBar(OnActionChosen);
            _bar.PlaceNear(screenRect);
            _bar.Show(true);
        }

        private void OnActionChosen(int action)
        {
            if (_bar != null) { _bar.Dispose(); _bar = null; }

            if (action == Actions.Cancel)
            {
                ReleaseCapture();
                return;
            }
            if (_selection == null) return;

            try
            {
                if (action == Actions.Image)
                {
                    Capture.CopyImage(_selection);
                    _toast.ShowMessage("图片已复制到剪贴板  (" + _selection.Width + " × " +
                        _selection.Height + ")", true, 2000);
                }
                else if (action == Actions.Save)
                {
                    string path = Capture.SavePng(_selection, _config.EffectiveSaveDir());
                    _toast.ShowMessage("已保存：" + Path.GetFileName(path), true, 2400);
                }
                else if (action == Actions.Text || action == Actions.Table)
                {
                    if (_busy)
                    {
                        _toast.ShowMessage("上一次识别还在进行中…", false, 1800);
                    }
                    else
                    {
                        _busy = true;
                        _toast.ShowMessage("正在离线识别…", true, 3000);
                        int mode = action;
                        Bitmap copy = new Bitmap(_selection);
                        if (!Ocr.Request(copy, _config.OcrLanguage, delegate(OcrResult r, Exception e)
                        {
                            OnOcrDone(mode, r, e);
                        }))
                        {
                            copy.Dispose();
                            _busy = false;
                            _toast.ShowMessage("识别任务排队失败", false, 2200);
                        }
                    }
                }
            }
            catch (Exception ex)
            {
                _toast.ShowMessage(Short(ex.Message), false, 3200);
            }
            finally
            {
                if (_selection != null) { _selection.Dispose(); _selection = null; }
            }
        }

        private void OnOcrDone(int mode, OcrResult result, Exception error)
        {
            _busy = false;

            if (error != null)
            {
                _toast.ShowMessage("识别失败：" + Short(error.Message), false, 3600);
                return;
            }

            try
            {
                string text = mode == Actions.Text
                    ? Layout.ToPlainText(result)
                    : Layout.ToTsv(result);

                if (string.IsNullOrEmpty(text.Trim()))
                {
                    _toast.ShowMessage("选区内未识别到文字", false, 2600);
                    return;
                }

                Capture.CopyText(text);

                if (mode == Actions.Text)
                    _toast.ShowMessage("文字已复制到剪贴板（" + Layout.LineCount(text) + " 行）", true, 2200);
                else
                    _toast.ShowMessage("表格已复制：可粘贴进 Excel / WPS（" +
                        Layout.ColumnCount(text) + " 列）", true, 2400);

                if (!string.IsNullOrEmpty(result.Language) &&
                    _config.RecentLanguage != result.Language)
                {
                    _config.RecentLanguage = result.Language;
                    _config.Save();
                }
            }
            catch (Exception ex)
            {
                _toast.ShowMessage("写入剪贴板失败：" + Short(ex.Message), false, 3000);
            }
        }

        private void ReleaseCapture()
        {
            if (_bar != null) { _bar.Dispose(); _bar = null; }
            if (_overlay != null) { _overlay.Dispose(); _overlay = null; }
            if (_screen != null) { _screen.Dispose(); _screen = null; }
            if (_selection != null) { _selection.Dispose(); _selection = null; }
        }

        private static string Short(string text)
        {
            if (string.IsNullOrEmpty(text)) return "";
            string flat = text.Replace("\r", " ").Replace("\n", " ").Trim();
            return flat.Length <= 90 ? flat : flat.Substring(0, 89) + "…";
        }

        public void Dispose()
        {
            if (_disposed) return;
            _disposed = true;

            try { Ocr.Stop(); }
            catch (Exception) { }

            try
            {
                if (_host != null)
                {
                    Native.KillTimer(_host.Handle, new IntPtr(TimerHotkeyRetry));
                    if (_hotkeyOk) Native.UnregisterHotKey(_host.Handle, HotkeyId);
                }
            }
            catch (Exception) { }

            ReleaseCapture();

            try { if (_toast != null) { _toast.Dispose(); _toast = null; } }
            catch (Exception) { }

            try { if (_host != null) { _host.Dispose(); _host = null; } }
            catch (Exception) { }

            if (_current == this) _current = null;
        }

        // ---- hotkey parsing ----

        internal static void ParseHotkey(string spec, out int mods, out int vk)
        {
            mods = 0;
            vk = Native.VK_1;
            if (string.IsNullOrEmpty(spec)) return;

            string[] parts = spec.Split('+');
            for (int i = 0; i < parts.Length; i++)
            {
                string part = parts[i].Trim();
                if (part.Length == 0) continue;
                if (part.Equals("ctrl", StringComparison.OrdinalIgnoreCase)) mods |= Native.MOD_CONTROL;
                else if (part.Equals("alt", StringComparison.OrdinalIgnoreCase)) mods |= Native.MOD_ALT;
                else if (part.Equals("shift", StringComparison.OrdinalIgnoreCase)) mods |= Native.MOD_SHIFT;
                else if (part.Equals("win", StringComparison.OrdinalIgnoreCase)) mods |= Native.MOD_WIN;
                else vk = KeyToVk(part);
            }
            if (mods == 0) mods = Native.MOD_CONTROL;
        }

        private static int KeyToVk(string key)
        {
            if (key.Length == 1)
            {
                char c = char.ToUpperInvariant(key[0]);
                if (c >= 'A' && c <= 'Z') return (int)c;
                if (c >= '0' && c <= '9') return (int)c;
            }
            if (key.StartsWith("F", StringComparison.OrdinalIgnoreCase))
            {
                int n;
                if (int.TryParse(key.Substring(1), NumberStyles.Integer,
                    CultureInfo.InvariantCulture, out n) && n >= 1 && n <= 24)
                    return 0x70 + n - 1;
            }
            return Native.VK_1;
        }

        // ---- headless diagnostics ----

        private static Bitmap MakeProbeImage()
        {
            Bitmap bmp = new Bitmap(560, 140, System.Drawing.Imaging.PixelFormat.Format32bppArgb);
            using (Graphics g = Graphics.FromImage(bmp))
            {
                g.Clear(Color.White);
                using (Font font = new Font("Arial", 68f, FontStyle.Regular, GraphicsUnit.Pixel))
                using (SolidBrush brush = new SolidBrush(Color.Black))
                    g.DrawString("TEXT 123", font, brush, 18f, 22f);
            }
            return bmp;
        }

        private static void RecognizeInto(Dictionary<string, string> fields, Bitmap bmp, string language)
        {
            System.Diagnostics.Stopwatch watch = System.Diagnostics.Stopwatch.StartNew();
            OcrResult result = Ocr.Recognize(bmp, language);
            fields["ocrMs"] = watch.ElapsedMilliseconds.ToString(CultureInfo.InvariantCulture);
            fields["ocrWords"] = result.Words.Count.ToString(CultureInfo.InvariantCulture);
            fields["ocrText"] = result.Text;
            fields["table"] = Layout.ToTsv(result);
            fields["text"] = Layout.ToPlainText(result);
            fields["language"] = result.Language;
            fields["ocrEngine"] = result.Engine;
            fields["localFailure"] = Ocr.LastLocalFailure ?? "";
            fields["ocrTiming"] = PaddleOcrEngine.LastTiming;
            fields["spaceTrace"] = PaddleOcrEngine.SpaceTrace.ToString();
            StringBuilder boxTrace = new StringBuilder();
            foreach (Rectangle r in PaddleOcrEngine.LastBoxes)
                boxTrace.Append(r.X).Append(',').Append(r.Y).Append(',')
                    .Append(r.Width).Append(',').Append(r.Height).Append(' ');
            fields["boxTrace"] = boxTrace.ToString();
        }

        /// <summary>不启动界面的自检：验证 ocr 目录里的模型能找到并真的跑出结果。</summary>
        internal static int RunCheck(string outputPath, string imagePath)
        {
            Dictionary<string, string> fields = new Dictionary<string, string>();
            fields["clr"] = Environment.Version.ToString();
            fields["exe"] = SystemInfo.ExePath();
            fields["languages"] = string.Join(",", Ocr.AvailableLanguages());
            fields["localModel"] = PaddleOcrEngine.EnsureReady()
                ? "ready " + PaddleOcrEngine.RuntimeVersion
                : "unavailable " + PaddleOcrEngine.LastError;
            // Which folder the models were loaded from - tells a missing ocr
            // directory apart from a broken one.
            fields["payload"] = Runtime.Dir ?? "(missing)";
            fields["ocrWords"] = "0";
            fields["ocrText"] = "";
            fields["table"] = "";
            fields["text"] = "";
            fields["ocrEngine"] = "";
            fields["ok"] = "true";
            fields["error"] = "";

            try
            {
                if (!string.IsNullOrEmpty(imagePath) && File.Exists(imagePath))
                {
                    using (Bitmap bmp = new Bitmap(imagePath))
                    {
                        fields["probe"] = "file:" + Path.GetFileName(imagePath);
                        RecognizeInto(fields, bmp, "");
                        RecognizeInto(fields, bmp, "");
                        fields["ocrWarmMs"] = fields["ocrMs"];
                    }
                }
                else
                {
                    using (Bitmap bmp = MakeProbeImage())
                    {
                        fields["probe"] = "generated";
                        RecognizeInto(fields, bmp, "zh-Hans-CN");
                    }
                }
            }
            catch (Exception ex)
            {
                fields["ok"] = "false";
                fields["error"] = ex.Message;
            }

            // Same cleanup the startup warmup performs: once the models resolve
            // from the ocr folder, the copy 3.1.0 unpacked is dead weight. Safe
            // to run here too - it refuses to delete the folder in active use.
            Runtime.CleanLegacy();

            WriteJson(outputPath, fields);
            return fields["ok"] == "true" ? 0 : 1;
        }

        /// <summary>
        /// 界面自检：在 WPF 的 UI 线程上建原生隐藏窗口、抓屏、画遮罩与动作条。
        /// 用来确认混合渲染没有坏，不显示遮罩也不动剪贴板。
        /// </summary>
        internal static int RunSmoke(string outputPath)
        {
            Dictionary<string, string> fields = new Dictionary<string, string>();
            fields["ok"] = "true";
            fields["error"] = "";
            fields["wpf"] = System.Windows.Application.Current != null ? "yes" : "no";
            fields["uiThread"] = System.Windows.Application.Current != null &&
                System.Windows.Application.Current.Dispatcher.CheckAccess() ? "yes" : "no";

            try
            {
                using (HostWindow host = new HostWindow(delegate(uint m, IntPtr w, IntPtr l) { }))
                {
                    if (host.Handle == IntPtr.Zero)
                        throw new InvalidOperationException("宿主隐藏窗口创建失败");
                    fields["hostWindow"] = "ok";

                    int vx, vy, vw, vh;
                    SystemInfo.VirtualScreen(out vx, out vy, out vw, out vh);
                    fields["virtualScreen"] = vw + "x" + vh;

                    Bitmap full = Capture.GrabVirtualScreen();
                    try
                    {
                        if (full.Width < 64 || full.Height < 64)
                            throw new InvalidOperationException("抓屏返回空图");
                        fields["grab"] = full.Width + "x" + full.Height;

                        using (Bitmap crop = Capture.Crop(full,
                            new Rectangle(vx + 10, vy + 10, 120, 80), vx, vy))
                        {
                            if (crop.Width != 120 || crop.Height != 80)
                                throw new InvalidOperationException("裁剪尺寸不符");
                        }
                        fields["crop"] = "ok";

                        using (SelectionOverlay overlay = new SelectionOverlay(full,
                            delegate(Rectangle r) { }, delegate() { }))
                        {
                            overlay.PaintOnce();
                            fields["overlay"] = "ok";
                        }

                        using (ActionBar bar = new ActionBar(delegate(int a) { }))
                        {
                            bar.PlaceNear(new Rectangle(vx + 40, vy + 40, 300, 200));
                            bar.PaintOnce();
                            fields["actionBar"] = "ok";
                        }
                    }
                    finally { full.Dispose(); }

                    using (Toast toast = new Toast(true))
                    {
                        toast.ShowMessage("TT酱 截图自检", true, 300);
                        toast.Dismiss();
                        fields["toast"] = "ok";
                    }

                    int mods, vk;
                    ParseHotkey("Ctrl+Alt+Shift+F9", out mods, out vk);
                    if (mods != (Native.MOD_CONTROL | Native.MOD_ALT | Native.MOD_SHIFT))
                        throw new InvalidOperationException("热键解析不符");
                    fields["hotkey"] = "ok";
                }
            }
            catch (Exception ex)
            {
                fields["ok"] = "false";
                fields["error"] = ex.GetType().Name + ": " + ex.Message;
            }

            WriteJson(outputPath, fields);
            return fields["ok"] == "true" ? 0 : 1;
        }

        private static void WriteJson(string path, Dictionary<string, string> fields)
        {
            StringBuilder sb = new StringBuilder();
            sb.AppendLine("{");
            int i = 0;
            foreach (KeyValuePair<string, string> pair in fields)
            {
                i++;
                sb.Append("  " + Json.Quote(pair.Key) + ": " + Json.Quote(pair.Value));
                if (i < fields.Count) sb.Append(",");
                sb.AppendLine();
            }
            sb.AppendLine("}");
            File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));
        }
    }
}
