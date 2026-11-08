using System.Diagnostics;
using System.Runtime.InteropServices;

namespace TTJiang.Services;

/// <summary>
/// 低级键盘钩子，实现「双击 Ctrl」。必须在带消息泵的线程（UI 线程）安装，
/// 回调也在该线程触发。
/// </summary>
public sealed class KeyboardHook : IDisposable
{
    private const int WH_KEYBOARD_LL = 13;

    private const int WM_KEYDOWN = 0x0100;
    private const int WM_KEYUP = 0x0101;
    private const int WM_SYSKEYDOWN = 0x0104;
    private const int WM_SYSKEYUP = 0x0105;

    private const int VK_CONTROL = 0x11;
    private const int VK_LCONTROL = 0xA2;
    private const int VK_RCONTROL = 0xA3;

    private const long DoubleCtrlWindowMs = 600;

    /// <summary>用 Stopwatch 而不是 Environment.TickCount，避免 49.7 天回绕时误判。</summary>
    private static readonly Stopwatch Clock = Stopwatch.StartNew();

    private delegate IntPtr HookProc(int nCode, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetWindowsHookEx(int idHook, HookProc lpfn, IntPtr hMod, uint dwThreadId);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool UnhookWindowsHookEx(IntPtr hhk);

    [DllImport("user32.dll")]
    private static extern IntPtr CallNextHookEx(IntPtr hhk, int nCode, IntPtr wParam, IntPtr lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr GetModuleHandle(string lpModuleName);

    private readonly HookProc _proc;
    private IntPtr _handle = IntPtr.Zero;
    private bool _ctrlDown;
    private long _lastPressMs;

    /// <summary>双击 Ctrl 触发（间隔 600ms 内）。</summary>
    public event Action DoubleCtrl;

    public KeyboardHook()
    {
        _proc = HookCallback;
        Install();
    }

    public bool IsInstalled
    {
        get { return _handle != IntPtr.Zero; }
    }

    private bool Install()
    {
        _handle = SetWindowsHookEx(WH_KEYBOARD_LL, _proc, GetModuleHandle(null), 0);
        if (_handle == IntPtr.Zero)
        {
            ConfigStore.Log("全局键盘钩子安装失败，错误码 " + Marshal.GetLastWin32Error());
            return false;
        }

        return true;
    }

    /// <summary>
    /// 重新安装钩子。钩子被安全软件摘掉后（系统不会通知我们），
    /// 用户可以靠这个入口自愈，不必重启程序。
    /// </summary>
    public bool Reinstall()
    {
        try
        {
            if (_handle != IntPtr.Zero)
            {
                UnhookWindowsHookEx(_handle);
                _handle = IntPtr.Zero;
            }
        }
        catch
        {
            // 卸载失败也继续尝试重装
        }

        _ctrlDown = false;
        _lastPressMs = 0;

        bool ok = Install();
        ConfigStore.Log(ok ? "全局热键已重新注册" : "全局热键重新注册失败");
        return ok;
    }

    private IntPtr HookCallback(int nCode, IntPtr wParam, IntPtr lParam)
    {
        if (nCode >= 0)
        {
            int message = wParam.ToInt32();
            int vk = Marshal.ReadInt32(lParam);

            if (vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL)
            {
                if (message == WM_KEYDOWN || message == WM_SYSKEYDOWN)
                {
                    if (!_ctrlDown)
                    {
                        _ctrlDown = true;
                        long now = Clock.ElapsedMilliseconds;

                        if (_lastPressMs != 0 && now - _lastPressMs <= DoubleCtrlWindowMs)
                        {
                            _lastPressMs = 0;
                            var handler = DoubleCtrl;
                            if (handler != null) handler();
                        }
                        else
                        {
                            _lastPressMs = now;
                        }
                    }
                }
                else if (message == WM_KEYUP || message == WM_SYSKEYUP)
                {
                    _ctrlDown = false;
                }
            }
            else if (message == WM_KEYDOWN || message == WM_SYSKEYDOWN)
            {
                // 两次 Ctrl 之间夹了别的键（如 Ctrl+A、Ctrl+N），就不算双击，
                // 避免正常快捷键被误判成唤起热键。
                _lastPressMs = 0;
            }
        }

        return CallNextHookEx(_handle, nCode, wParam, lParam);
    }

    public void Dispose()
    {
        if (_handle != IntPtr.Zero)
        {
            UnhookWindowsHookEx(_handle);
            _handle = IntPtr.Zero;
        }
    }
}
