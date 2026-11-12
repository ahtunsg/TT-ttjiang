using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Threading;

namespace TTJiang.Services;

/// <summary>
/// 托盘图标：纯 Win32 Shell_NotifyIcon + 隐藏消息窗口 + 原生弹出菜单。
/// 不引用 System.Windows.Forms / System.Drawing，常驻程序不再加载这两套程序集。
/// </summary>
public sealed class TrayIcon : IDisposable
{
    public Action ShowRequested;
    public Action ScreenshotRequested;
    public Action AddRequested;
    public Action SettingsRequested;
    public Action ExportRequested;
    public Action ImportRequested;
    public Action HookRepairRequested;
    public Action ExitRequested;

    private const int TrayMessage = 0x8001;           // WM_APP + 1
    private const int WM_LBUTTONDBLCLK = 0x0203;
    private const int WM_RBUTTONUP = 0x0205;
    private const int WM_CONTEXTMENU = 0x007B;
    private const int WM_NULL = 0x0000;

    private const int NIM_ADD = 0;
    private const int NIM_MODIFY = 1;
    private const int NIM_DELETE = 2;
    private const int NIF_MESSAGE = 0x01;
    private const int NIF_ICON = 0x02;
    private const int NIF_TIP = 0x04;
    private const int NIF_INFO = 0x10;
    private const int NIIF_INFO = 0x01;

    // 开机自启时通知区域可能尚未就绪，NIM_ADD 会失败；短间隔重试补齐。
    private const int MaxIconRetries = 10;
    private const int RetryIntervalSeconds = 2;

    private const uint MF_STRING = 0x0000;
    private const uint MF_CHECKED = 0x0008;
    private const uint MF_SEPARATOR = 0x0800;
    private const uint TPM_RETURNCMD = 0x0100;
    private const uint TPM_RIGHTBUTTON = 0x0002;
    private const uint TPM_NONOTIFY = 0x0080;

    private const int IdOpen = 1;
    private const int IdAdd = 2;
    private const int IdSettings = 3;
    private const int IdEdit = 4;
    private const int IdExport = 5;
    private const int IdImport = 6;
    private const int IdRepair = 7;
    private const int IdAutoStart = 8;
    private const int IdExit = 9;
    private const int IdScreenshot = 10;

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NOTIFYICONDATA
    {
        public int cbSize;
        public IntPtr hWnd;
        public int uID;
        public int uFlags;
        public int uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public int dwState;
        public int dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public int uTimeoutOrVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public int dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT
    {
        public int X;
        public int Y;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool Shell_NotifyIcon(int dwMessage, ref NOTIFYICONDATA lpData);

    // Explorer 重启后广播此消息；自定义消息号必须运行时向系统申请。
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int RegisterWindowMessageW(string lpString);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern uint ExtractIconExW(string lpszFile, int nIconIndex,
        out IntPtr phiconLarge, out IntPtr phiconSmall, uint nIcons);

    [DllImport("user32.dll")]
    private static extern bool DestroyIcon(IntPtr hIcon);

    [DllImport("user32.dll")]
    private static extern IntPtr LoadIcon(IntPtr hInstance, IntPtr lpIconName);

    [DllImport("user32.dll")]
    private static extern IntPtr CreatePopupMenu();

    [DllImport("user32.dll")]
    private static extern bool DestroyMenu(IntPtr hMenu);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool AppendMenuW(IntPtr hMenu, uint uFlags, UIntPtr uIDNewItem, string lpNewItem);

    [DllImport("user32.dll")]
    private static extern int TrackPopupMenuEx(IntPtr hMenu, uint uFlags, int x, int y,
        IntPtr hWnd, IntPtr lptpm);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool PostMessage(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT pt);

    private HwndSource _source;
    private IntPtr _hwnd;
    private IntPtr _hIcon;
    private bool _ownsIcon;
    private bool _balloonShown;
    private readonly int _taskbarCreated;
    private DispatcherTimer _retryTimer;
    private int _retryCount;
    private bool _disposed;

    public TrayIcon()
    {
        var parameters = new HwndSourceParameters("TTJiangTrayHost")
        {
            Width = 1,
            Height = 1,
            PositionX = 0,
            PositionY = 0,
            // WS_POPUP + WS_EX_TOOLWINDOW：不可见、不进任务栏、不抢焦点
            WindowStyle = unchecked((int)0x80000000),
            ExtendedWindowStyle = 0x00000080
        };

        _source = new HwndSource(parameters);
        _hwnd = _source.Handle;
        _source.AddHook(WndProc);

        _hIcon = LoadSmallIcon();

        // Explorer 重启（崩溃、系统更新、手动结束进程）会清空通知区域的所有注册项，
        // 并向顶层窗口广播 TaskbarCreated。不处理它，进程仍在、热键仍灵，图标却永久消失。
        _taskbarCreated = RegisterWindowMessageW("TaskbarCreated");

        if (!AddIcon())
        {
            // 开机自启竞态：程序先于 Explorer 通知区域就绪时 NIM_ADD 会失败，定时重试补齐。
            ConfigStore.Log("托盘图标注册失败，错误码 " + Marshal.GetLastWin32Error() + "，启动自动重试");
            StartRetry();
        }
    }

    /// <summary>向通知区域注册图标，回调 / 图标 / 提示一次给全。</summary>
    private bool AddIcon()
    {
        var data = MakeData();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TrayMessage;
        return Shell_NotifyIcon(NIM_ADD, ref data);
    }

    /// <summary>Explorer 重建任务栏后旧注册项已失效：先删残留，再重新 NIM_ADD。</summary>
    private void ReRegister()
    {
        if (_disposed) return;

        StopRetry();
        try
        {
            var stale = MakeData();
            Shell_NotifyIcon(NIM_DELETE, ref stale); // 旧项可能已不存在，忽略返回值
        }
        catch
        {
            // 清理失败不阻断重新注册
        }

        if (AddIcon())
        {
            ConfigStore.Log("托盘图标已重新注册（资源管理器重启后自动恢复）");
            return;
        }

        // 广播到达时通知区域未必已具备接收条件，交给重试兜底。
        ConfigStore.Log("托盘图标重新注册失败，错误码 " + Marshal.GetLastWin32Error());
        StartRetry();
    }

    private void StartRetry()
    {
        if (_disposed) return;

        if (_retryTimer == null)
        {
            _retryTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(RetryIntervalSeconds) };
            _retryTimer.Tick += OnRetryTick;
        }

        _retryCount = 0;
        _retryTimer.Start();
    }

    private void StopRetry()
    {
        if (_retryTimer != null) _retryTimer.Stop();
    }

    private void OnRetryTick(object sender, EventArgs e)
    {
        if (_disposed)
        {
            StopRetry();
            return;
        }

        if (AddIcon())
        {
            StopRetry();
            ConfigStore.Log("托盘图标重试注册成功");
            return;
        }

        _retryCount++;
        if (_retryCount >= MaxIconRetries)
        {
            StopRetry();
            ConfigStore.Log("托盘图标重试 " + MaxIconRetries + " 次仍未成功，等待下一次资源管理器广播");
        }
    }

    private NOTIFYICONDATA MakeData()
    {
        return new NOTIFYICONDATA
        {
            cbSize = Marshal.SizeOf(typeof(NOTIFYICONDATA)),
            hWnd = _hwnd,
            uID = 1,
            hIcon = _hIcon,
            szTip = "TT酱 · 双击 Ctrl 唤起"
        };
    }

    private IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (_taskbarCreated != 0 && msg == _taskbarCreated)
        {
            ReRegister();
            handled = true;
            return IntPtr.Zero;
        }

        if (msg != TrayMessage) return IntPtr.Zero;

        int ev = (int)(lParam.ToInt64() & 0xFFFF);
        if (ev == WM_LBUTTONDBLCLK)
        {
            Invoke(ShowRequested);
        }
        else if (ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU)
        {
            // 菜单自带模态循环，异步弹出避免在消息回调里重入
            Dispatcher.CurrentDispatcher.BeginInvoke(new Action(ShowMenu));
        }

        handled = true;
        return IntPtr.Zero;
    }

    private void ShowMenu()
    {
        IntPtr menu = CreatePopupMenu();
        if (menu == IntPtr.Zero) return;

        try
        {
            AddItem(menu, IdScreenshot, "截图  (" + Capture.ScreenshotService.HotkeyText + ")", false);
            Separator(menu);
            AddItem(menu, IdOpen, "打开TT酱", false);
            AddItem(menu, IdAdd, "添加快捷方式…", false);
            AddItem(menu, IdSettings, "设置…", false);
            AddItem(menu, IdEdit, "编辑 shortcuts.json", false);
            Separator(menu);
            AddItem(menu, IdExport, "导出快捷方式…", false);
            AddItem(menu, IdImport, "导入快捷方式…", false);
            Separator(menu);
            AddItem(menu, IdRepair, "重新注册全局热键（双击 Ctrl 失效时）", false);
            Separator(menu);
            AddItem(menu, IdAutoStart, "开机自动启动", AutoStart.IsEnabled);
            Separator(menu);
            AddItem(menu, IdExit, "退出", false);

            POINT pt;
            GetCursorPos(out pt);

            SetForegroundWindow(_hwnd);
            int cmd = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                pt.X, pt.Y, _hwnd, IntPtr.Zero);
            // 系统要求补一条空消息，否则菜单在点击外部时可能不关闭
            PostMessage(_hwnd, WM_NULL, IntPtr.Zero, IntPtr.Zero);

            switch (cmd)
            {
                case IdScreenshot: Invoke(ScreenshotRequested); break;
                case IdOpen: Invoke(ShowRequested); break;
                case IdAdd: Invoke(AddRequested); break;
                case IdSettings: Invoke(SettingsRequested); break;
                case IdEdit: OpenConfigFile(); break;
                case IdExport: Invoke(ExportRequested); break;
                case IdImport: Invoke(ImportRequested); break;
                case IdRepair: Invoke(HookRepairRequested); break;
                case IdAutoStart: ToggleAutoStart(); break;
                case IdExit: Invoke(ExitRequested); break;
            }
        }
        finally
        {
            DestroyMenu(menu);
        }
    }

    private static void AddItem(IntPtr menu, int id, string text, bool isChecked)
    {
        uint flags = MF_STRING | (isChecked ? MF_CHECKED : 0u);
        AppendMenuW(menu, flags, new UIntPtr((uint)id), text);
    }

    private static void Separator(IntPtr menu)
    {
        AppendMenuW(menu, MF_SEPARATOR, UIntPtr.Zero, null);
    }

    private static void Invoke(Action action)
    {
        if (action != null) action();
    }

    /// <summary>从 exe 里取小图标，直接用 HICON，不经 System.Drawing.Icon。</summary>
    private IntPtr LoadSmallIcon()
    {
        try
        {
            string exe = AutoStart.ExePath;
            if (!string.IsNullOrEmpty(exe) && File.Exists(exe))
            {
                IntPtr large, small;
                uint count = ExtractIconExW(exe, 0, out large, out small, 1);
                if (count > 0)
                {
                    if (large != IntPtr.Zero) DestroyIcon(large);
                    if (small != IntPtr.Zero)
                    {
                        _ownsIcon = true;
                        return small;
                    }
                }
            }
        }
        catch
        {
            // 取不到就用系统默认图标
        }

        // IDI_APPLICATION 是共享图标，不能 DestroyIcon
        return LoadIcon(IntPtr.Zero, new IntPtr(32512));
    }

    private static void OpenConfigFile()
    {
        try
        {
            string path = ConfigStore.ConfigPath;
            if (!File.Exists(path)) ConfigStore.Save(new List<Models.ShortcutItem>());

            System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(path)
            {
                UseShellExecute = true
            });
        }
        catch (Exception ex)
        {
            ConfigStore.Log("打开配置文件失败：" + ex.Message);
        }
    }

    private static void ToggleAutoStart()
    {
        bool want = !AutoStart.IsEnabled;
        if (!AutoStart.SetEnabled(want))
        {
            MessageBox.Show("无法写入开机启动项，可能被系统策略或安全软件拦截。", "TT酱",
                MessageBoxButton.OK, MessageBoxImage.Warning);
        }
    }

    /// <summary>窗口收进托盘时提示一次，避免用户以为程序退出了。</summary>
    public void ShowHideHint()
    {
        if (_balloonShown) return;
        _balloonShown = true;

        try
        {
            var data = MakeData();
            data.uFlags = NIF_INFO;
            data.szInfoTitle = "TT酱仍在运行";
            data.szInfo = "双击 Ctrl 随时唤起窗口；右键托盘图标可退出。";
            data.dwInfoFlags = NIIF_INFO;
            Shell_NotifyIcon(NIM_MODIFY, ref data);
        }
        catch
        {
            // 通知失败不影响使用
        }
    }

    public void Dispose()
    {
        _disposed = true;
        StopRetry();

        try
        {
            var data = MakeData();
            Shell_NotifyIcon(NIM_DELETE, ref data);
        }
        catch
        {
            // 退出阶段忽略
        }

        try
        {
            if (_ownsIcon && _hIcon != IntPtr.Zero) DestroyIcon(_hIcon);
            _hIcon = IntPtr.Zero;

            if (_source != null)
            {
                _source.RemoveHook(WndProc);
                _source.Dispose();
                _source = null;
            }
        }
        catch
        {
            // 退出阶段忽略
        }
    }
}
