using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;
using System.Windows;
using TTJiang.Models;
using TTJiang.Services;

namespace TTJiang;

public partial class MainWindow
{
    // ---------------------------------------------------------------- 复制
    private async Task<bool> CopyTextAsync(string text)
    {
        if (string.IsNullOrEmpty(text)) return false;

        bool ok = await ClipboardHelper.TrySetTextAsync(text);
        if (!ok) ConfigStore.Log("复制失败，剪贴板被占用：" + text);
        return ok;
    }

    private void CopyText(string text)
    {
        _ = CopyTextAsync(text);
    }

    // ---------------------------------------------------------------- 打开
    /// <summary>
    /// 统一启动入口：Process 对象用完即释放，工作目录设成目标程序自己的目录，
    /// 避免老程序按相对路径找配置时出错。
    /// </summary>
    private static bool TryStart(string file, string arguments, string workingDir, out string error)
    {
        error = null;

        try
        {
            var psi = new ProcessStartInfo(file)
            {
                UseShellExecute = true,
                Arguments = arguments ?? "",
                WorkingDirectory = workingDir ?? ""
            };

            using (Process.Start(psi))
            {
                // 只为及时释放句柄
            }

            return true;
        }
        catch (Exception ex)
        {
            error = ex.Message;
            return false;
        }
    }

    private static string WorkingDirFor(string path)
    {
        try
        {
            if (string.IsNullOrWhiteSpace(path)) return "";
            string p = path.Trim().Trim('"');
            if (p.StartsWith("shell:", StringComparison.OrdinalIgnoreCase)) return "";

            string dir = Path.GetDirectoryName(p);
            if (!string.IsNullOrEmpty(dir) && Directory.Exists(dir)) return dir;
        }
        catch
        {
            // 路径非法就当没有工作目录
        }

        return "";
    }

    public static void OpenItem(ShortcutItem item)
    {
        if (item == null) return;

        string path = (item.Path ?? "").Trim();
        if (path.Length == 0) return;
        string args = (item.Args ?? "").Trim();

        if (path.StartsWith("shell:", StringComparison.OrdinalIgnoreCase))
        {
            string shellError;
            if (TryStart(path, "", "", out shellError)) return;

            string fallbackError;
            if (TryStart("explorer.exe", "\"" + path + "\"", "", out fallbackError)) return;

            ConfigStore.Log("打开 shell 目录失败：" + path + " -> " + shellError);
            MessageBox.Show("打开失败：" + path + Environment.NewLine + fallbackError, "TT酱",
                MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        string error;
        if (TryStart(path, args, WorkingDirFor(path), out error)) return;

        ConfigStore.Log("打开失败：" + path + " -> " + error);
        MessageBox.Show("打开失败：" + path + Environment.NewLine + error, "TT酱",
            MessageBoxButton.OK, MessageBoxImage.Warning);
    }

    private void OpenPath(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return;

        string error;
        if (TryStart(path, "", WorkingDirFor(path), out error)) return;

        ConfigStore.Log("打开文件失败：" + path + " -> " + error);
        MessageBox.Show("打开失败：" + path + Environment.NewLine + error, "TT酱",
            MessageBoxButton.OK, MessageBoxImage.Warning);
    }

    private void OpenLocation(ShortcutItem item)
    {
        string path = (item.Path ?? "").Trim();
        if (path.Length == 0) return;

        try
        {
            if (path.StartsWith("shell:", StringComparison.OrdinalIgnoreCase))
            {
                OpenItem(item);
                return;
            }

            OpenPathLocation(path);
        }
        catch (Exception ex)
        {
            ConfigStore.Log("打开所在位置失败：" + ex.Message);
        }
    }

    private static void OpenPathLocation(string path)
    {
        try
        {
            string arguments;
            string workingDir = "";

            if (File.Exists(path))
            {
                arguments = "/select,\"" + path + "\"";
                workingDir = WorkingDirFor(path);
            }
            else
            {
                string dir = Directory.Exists(path) ? path : Path.GetDirectoryName(path);
                if (string.IsNullOrEmpty(dir)) return;
                arguments = "\"" + dir + "\"";
            }

            string error;
            if (!TryStart("explorer.exe", arguments, workingDir, out error))
            {
                ConfigStore.Log("打开所在位置失败：" + path + " -> " + error);
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("打开所在位置失败：" + ex.Message);
        }
    }
}
