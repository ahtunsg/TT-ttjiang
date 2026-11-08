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

        var data = MakeData();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TrayMessage;
        if (!Shell_NotifyIcon(NIM_ADD, ref data))
        {
            ConfigStore.Log("托盘图标注册失败，错误码 " + Marshal.GetLastWin32Error());
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
