using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace TTJiang.Services;

/// <summary>
/// 剪贴板写入。直接用 Win32 API：WPF 的 Clipboard.SetText 在失败时内部会长时间重试，
/// 会把 UI 线程卡住（表现为按键无响应），这里改成快速失败 + 短重试。
/// </summary>
public static class ClipboardHelper
{
    private const uint CF_UNICODETEXT = 13;
    private const uint GMEM_MOVEABLE = 0x0002;

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool OpenClipboard(IntPtr hWndNewOwner);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool CloseClipboard();

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool EmptyClipboard();

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr SetClipboardData(uint uFormat, IntPtr hMem);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GlobalAlloc(uint uFlags, UIntPtr dwBytes);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GlobalLock(IntPtr hMem);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GlobalUnlock(IntPtr hMem);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GlobalFree(IntPtr hMem);

    /// <summary>
    /// 后台线程写剪贴板。占用重试最多 300ms，放后台执行可以避免界面冻结、Esc 与热键失灵。
    /// </summary>
    public static Task<bool> TrySetTextAsync(string text)
    {
        if (string.IsNullOrEmpty(text)) return Task.FromResult(false);
        return Task.Run(() => TrySetText(text));
    }

    public static bool TrySetText(string text)
    {
        if (string.IsNullOrEmpty(text)) return false;

        byte[] payload = Encoding.Unicode.GetBytes(text + "\0");

        for (int attempt = 0; attempt < 10; attempt++)
        {
            if (attempt > 0) Thread.Sleep(30);

            if (!OpenClipboard(IntPtr.Zero)) continue;

            IntPtr hMem = IntPtr.Zero;
            bool transferred = false;
            try
            {
                if (!EmptyClipboard()) continue;

                hMem = GlobalAlloc(GMEM_MOVEABLE, (UIntPtr)payload.Length);
                if (hMem == IntPtr.Zero) break;

                IntPtr target = GlobalLock(hMem);
                if (target == IntPtr.Zero) break;

                Marshal.Copy(payload, 0, target, payload.Length);
                GlobalUnlock(hMem);

                // 成功放入剪贴板后，内存所有权归系统，不能再释放
                if (SetClipboardData(CF_UNICODETEXT, hMem) != IntPtr.Zero)
                {
                    transferred = true;
                    return true;
                }
            }
            finally
            {
                if (!transferred && hMem != IntPtr.Zero) GlobalFree(hMem);
                CloseClipboard();
            }
        }

        ConfigStore.Log("复制失败：剪贴板长时间被其他程序占用");
        return false;
    }
}
