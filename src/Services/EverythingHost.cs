using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using Microsoft.Win32;

namespace TTJiang.Services;

/// <summary>
/// 内置 Everything 的安装与生命周期。
/// 程序、配置、索引全部落在 exe 同级目录：everything 子目录放可执行文件与 Everything.ini / .db。
/// 这里不做任何"到系统里找 Everything"的动作，只用自己释放出来的这一份。
/// </summary>
public static class EverythingHost
{
    /// <summary>独立实例名，避免和用户自己装的 Everything 抢同一个实例与索引。</summary>
    public const string InstanceName = "TTJiang";

    private const string ResourceName = "TTJiang.Everything.zip";
    private const string VersionFile = "components.txt";
    private const string ComponentVersion = "Everything-1.5a + ES-1.1.0.38 + SDK3-3.0.1.12";

    public static string AppDir
    {
        get { return AppDomain.CurrentDomain.BaseDirectory.TrimEnd('\\'); }
    }

    public static string ToolsDir
    {
        get { return Path.Combine(AppDir, "everything"); }
    }

    public static string EverythingExe
    {
        get { return Path.Combine(ToolsDir, "Everything64.exe"); }
    }

    public static string EsPath
    {
        get { return Path.Combine(ToolsDir, "es.exe"); }
    }

    /// <summary>官方 SDK3 DLL：走命名管道直接查询，不再每次输入启动 es.exe。</summary>
    public static string Sdk3Path
    {
        get { return Path.Combine(ToolsDir, "Everything3_x64.dll"); }
    }

    public static string IniPath
    {
        get { return Path.Combine(ToolsDir, "TTJiang.ini"); }
    }

    public static string DbPath
    {
        get { return Path.Combine(ToolsDir, "TTJiang.db"); }
    }

    /// <summary>最近一次失败的原因，给界面提示用。</summary>
    public static string LastError { get; private set; } = "";

    // ---------------------------------------------------------------- 释放内置组件

    /// <summary>释放动作是否已经在本次进程内成功完成过。</summary>
    private static volatile bool _toolsReady;
    private static readonly object ToolsSync = new object();

    /// <summary>
    /// 进程内缓存释放结果：稳态搜索只做一次轻量存在性检查，不再重复读版本文件。
    /// 目录被手工删掉时缓存自动失效，会重新释放一次。
    /// </summary>
    public static bool EnsureTools()
    {
        if (_toolsReady && File.Exists(EsPath) && File.Exists(Sdk3Path)) return true;

        lock (ToolsSync)
        {
            if (_toolsReady && File.Exists(EsPath) && File.Exists(Sdk3Path)) return true;

            _toolsReady = false;
            if (!EnsureToolsCore()) return false;

            _toolsReady = true;
            return true;
        }
    }

    private static bool EnsureToolsCore()
    {
        LastError = "";

        string marker = Path.Combine(ToolsDir, VersionFile);
        try
        {
            if (File.Exists(EverythingExe) && File.Exists(EsPath) && File.Exists(Sdk3Path)
                && File.Exists(marker) && File.ReadAllText(marker).Trim() == ComponentVersion)
            {
                return true;
            }

            Directory.CreateDirectory(ToolsDir);

            using (Stream packed = typeof(EverythingHost).Assembly.GetManifestResourceStream(ResourceName))
            {
                if (packed == null)
                {
                    LastError = "安装包不完整：缺少内置搜索组件";
                    return false;
                }

                using (var archive = new ZipArchive(packed, ZipArchiveMode.Read))
                {
                    foreach (ZipArchiveEntry entry in archive.Entries)
                    {
                        if (string.IsNullOrEmpty(entry.Name)) continue;

                        string target = Path.Combine(ToolsDir, entry.Name);
                        entry.ExtractToFile(target, true);
                    }
                }
            }

            File.WriteAllText(marker, ComponentVersion);
            return true;
        }
        catch (Exception ex)
        {
            LastError = "释放内置搜索组件失败：" + ex.Message;
            ConfigStore.Log(LastError);
            return false;
        }
    }

    // ---------------------------------------------------------------- 客户端界面收敛

    /// <summary>
    /// 内置实例只作为后台查询服务存在，界面全部由 TT酱 自己提供。
    /// show_tray_icon / show_in_taskbar 是 Everything 的 ini 隐藏设置：
    /// 前者控制通知区域图标，后者控制任务栏按钮；run_in_background=1 保证没有窗口时进程不退出。
    /// </summary>
    private static readonly string[] StealthKeys =
    {
        "run_in_background", "show_in_taskbar", "show_tray_icon", "minimize_to_tray"
    };

    private static readonly string[] StealthValues = { "1", "0", "0", "0" };

    /// <summary>
    /// Everything 会在退出时用它内存里的值重写 ini，运行期间的手工修改会丢失。
    /// 所以只在实例没运行时写，且每次拉起客户端前检查一次。
    /// apply=false 只判断是否需要改，用于决定要不要先收掉旧实例。
    /// </summary>
    private static bool EnsureStealthConfig(bool apply)
    {
        try
        {
            bool existed = File.Exists(IniPath);
            byte[] raw = existed ? File.ReadAllBytes(IniPath) : Array.Empty<byte>();
            bool bom = raw.Length >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF;
            string text = raw.Length == 0
                ? ""
                : Encoding.UTF8.GetString(raw, bom ? 3 : 0, raw.Length - (bom ? 3 : 0));

            string nl = text.IndexOf("\r\n", StringComparison.Ordinal) >= 0 ? "\r\n" : "\n";
            var lines = new List<string>(text.Replace("\r\n", "\n").Split('\n'));
            while (lines.Count > 0 && lines[lines.Count - 1].Length == 0)
                lines.RemoveAt(lines.Count - 1);

            int head = lines.FindIndex(l => l.Trim().Equals("[Everything]", StringComparison.OrdinalIgnoreCase));
            if (head < 0)
            {
                lines.Add("[Everything]");
                head = lines.Count - 1;
            }

            int tail = lines.FindIndex(head + 1, l => l.TrimStart().StartsWith("[", StringComparison.Ordinal));
            if (tail < 0) tail = lines.Count;

            bool changed = !existed;
            for (int k = 0; k < StealthKeys.Length; k++)
            {
                string wanted = StealthKeys[k] + "=" + StealthValues[k];
                string prefix = StealthKeys[k] + "=";
                int at = -1;
                for (int i = head + 1; i < tail; i++)
                {
                    if (lines[i].StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) { at = i; break; }
                }

                if (at >= 0)
                {
                    if (lines[at] == wanted) continue;
                    lines[at] = wanted;
                }
                else
                {
                    lines.Insert(tail, wanted);
                    tail++;
                }
                changed = true;
            }

            if (!changed || !apply) return changed;

            Directory.CreateDirectory(ToolsDir);
            File.WriteAllText(IniPath, string.Join(nl, lines) + nl, new UTF8Encoding(bom));
            return true;
        }
        catch (Exception ex)
        {
            // 配置写不进去不影响搜索可用性，最坏情况只是 Everything 的托盘图标还在
            ConfigStore.Log("收敛内置实例界面失败：" + ex.Message);
            return false;
        }
    }

    // ---------------------------------------------------------------- 客户端进程

    private static string ClientPipeName
    {
        get { return "Everything IPC (" + InstanceName + ")"; }
    }

    /// <summary>
    /// 按名字定向查询管道是否存在。原先枚举 \\.\pipe\ 下全部条目再 LINQ 过滤，
    /// 本机实测常有 200+ 项，而每次搜索都会走到这里。
    /// </summary>
    public static bool IsClientRunning()
    {
        try
        {
            return Directory.GetFiles(@"\\.\pipe\", ClientPipeName).Length > 0;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>按需拉起内置实例；已经在跑就直接返回。</summary>
    public static bool StartClient(int waitMs = 8000)
    {
        LastError = "";

        bool running = IsClientRunning();

        // 旧版本拉起的实例带着托盘图标，且退出时会用旧值覆盖 ini。
        // 先收掉它再写配置，然后重新拉起，用户无需手工重启或删 ini。
        if (running && EnsureStealthConfig(false))
        {
            StopClient();
            running = false;
            // 管道关闭不等于进程已退出：ini 是退出时才落盘的，必须等进程真的没了再写
            for (int i = 0; i < 40 && (IsClientRunning() || IsOwnClientAlive()); i++)
                Thread.Sleep(100);
        }
        if (!running) EnsureStealthConfig(true);

        if (running) return true;
        if (!File.Exists(EverythingExe))
        {
            LastError = "内置搜索组件缺失，请重新解压完整的 TT酱 压缩包";
            return false;
        }

        var psi = new ProcessStartInfo(EverythingExe)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = ToolsDir,
            Arguments = "-instance " + InstanceName
                        + " -startup -config " + EverythingSearch.QuoteArg(IniPath)
                        + " -db " + EverythingSearch.QuoteArg(DbPath)
        };

        try
        {
            using (Process proc = Process.Start(psi))
            {
                // 进程句柄只为确认启动成功，真正的就绪状态看管道
                if (proc == null)
                {
                    LastError = "无法启动内置搜索进程";
                    return false;
                }
            }
        }
        catch (Exception ex)
        {
            LastError = "无法启动内置搜索进程：" + ex.Message;
            ConfigStore.Log(LastError);
            return false;
        }

        return WaitForClient(waitMs);
    }

    private static bool WaitForClient(int waitMs)
    {
        int waited = 0;
        while (waited < waitMs)
        {
            if (IsClientRunning()) return true;
            Thread.Sleep(150);
            waited += 150;
        }

        LastError = "内置搜索进程没有响应";
        return false;
    }

    private const uint ProcessQueryLimitedInformation = 0x1000;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, int pid);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool QueryFullProcessImageNameW(IntPtr handle, uint flags,
                                                          StringBuilder name, ref uint size);

    /// <summary>
    /// 本会话里是否还有自己这份 Everything 客户端进程。服务在 Session 0，按 SessionId 排掉。
    /// 本程序是 32 位、Everything64.exe 是 64 位，Process.MainModule 跨位数读不到，
    /// 所以用 QueryFullProcessImageNameW（PROCESS_QUERY_LIMITED_INFORMATION 可跨 WOW64）。
    /// </summary>
    private static bool IsOwnClientAlive()
    {
        try
        {
            int session = Process.GetCurrentProcess().SessionId;
            foreach (Process p in Process.GetProcessesByName("Everything64"))
            {
                using (p)
                {
                    if (p.SessionId != session) continue;

                    IntPtr handle = OpenProcess(ProcessQueryLimitedInformation, false, p.Id);
                    if (handle == IntPtr.Zero) continue;
                    try
                    {
                        var name = new StringBuilder(1024);
                        uint size = (uint)name.Capacity;
                        if (QueryFullProcessImageNameW(handle, 0, name, ref size)
                            && string.Equals(name.ToString(), EverythingExe,
                                             StringComparison.OrdinalIgnoreCase)) return true;
                    }
                    finally { CloseHandle(handle); }
                }
            }
        }
        catch { }
        return false;
    }

    /// <summary>退出本程序时一起收掉实例，保证删目录前没有残留进程占用文件。</summary>
    public static void StopClient()
    {
        try
        {
            if (!IsClientRunning()) return;

            RunOwn("-instance " + InstanceName + " -exit", 4000);
        }
        catch (Exception ex)
        {
            ConfigStore.Log("结束内置搜索进程失败：" + ex.Message);
        }
    }

    private static void RunOwn(string arguments, int waitMs)
    {
        if (!File.Exists(EverythingExe)) return;

        var psi = new ProcessStartInfo(EverythingExe, arguments)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = ToolsDir
        };

        using (Process proc = Process.Start(psi))
        {
            if (proc != null) proc.WaitForExit(waitMs);
        }
    }

    // ---------------------------------------------------------------- 索引服务

    /// <summary>
    /// 判断索引服务是否已经装好。服务名由 Everything 按实例生成，这里不去猜名字，
    /// 直接按 ImagePath 是否指向本目录的 Everything64.exe 来认。
    /// </summary>
    public static bool IsServiceInstalled()
    {
        return FindServiceName() != null;
    }

    /// <summary>
    /// 系统里是否已经有任意一份 Everything 索引服务（不管装在哪）。
    /// Everything 的服务按版本命名，全机共用一条管道，所以有的话直接借用即可，
    /// 不能再装一次——否则会把这个服务改指向我们的目录，把用户原来那份弄坏。
    /// </summary>
    public static bool HasAnyService()
    {
        try
        {
            using (RegistryKey services = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services"))
            {
                if (services == null) return false;

                foreach (string name in services.GetSubKeyNames())
                {
                    if (name.IndexOf("Everything", StringComparison.OrdinalIgnoreCase) < 0) continue;

                    using (RegistryKey key = services.OpenSubKey(name))
                    {
                        string image = key == null ? null : key.GetValue("ImagePath") as string;
                        if (!string.IsNullOrEmpty(image)) return true;
                    }
                }
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("读取服务列表失败：" + ex.Message);
        }

        return false;
    }

    /// <summary>搜索是否已经具备可用条件：自己的服务，或系统里已有的服务，都算。</summary>
    public static bool IsSearchReady()
    {
        return IsServiceInstalled() || HasAnyService();
    }

    public static string FindServiceName()
    {
        try
        {
            using (RegistryKey services = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services"))
            {
                if (services == null) return null;

                foreach (string name in services.GetSubKeyNames())
                {
                    if (name.IndexOf("Everything", StringComparison.OrdinalIgnoreCase) < 0) continue;

                    using (RegistryKey key = services.OpenSubKey(name))
                    {
                        string image = key == null ? null : key.GetValue("ImagePath") as string;
                        if (image != null && image.IndexOf(EverythingExe, StringComparison.OrdinalIgnoreCase) >= 0)
                        {
                            return name;
                        }
                    }
                }
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("读取服务列表失败：" + ex.Message);
        }

        return null;
    }

    /// <summary>安装索引服务；需要管理员授权，取消授权会返回 false 并给出原因。</summary>
    public static bool InstallService(out string message)
    {
        // 已经有服务（可能是用户自己装的 Everything）就直接共用，绝不覆盖它的安装路径
        if (HasAnyService())
        {
            message = "已检测到系统里已有的 Everything 索引服务，TT酱 直接共用，不重复安装";
            return true;
        }

        return RunElevated("-instance " + InstanceName + " -install-service", true, out message);
    }

    public static bool UninstallService(out string message)
    {
        return RunElevated("-instance " + InstanceName + " -uninstall-service", false, out message);
    }

    private static bool RunElevated(string arguments, bool expectInstalled, out string message)
    {
        message = "";

        if (!File.Exists(EverythingExe))
        {
            message = "内置搜索组件缺失，请重新解压完整的 TT酱 压缩包";
            return false;
        }

        var psi = new ProcessStartInfo(EverythingExe, arguments)
        {
            UseShellExecute = true,
            Verb = "runas",
            WorkingDirectory = ToolsDir
        };

        try
        {
            using (Process proc = Process.Start(psi))
            {
                if (proc == null)
                {
                    message = "无法启动索引服务安装程序";
                    return false;
                }

                if (!proc.WaitForExit(120000))
                {
                    message = "安装程序超时未结束";
                    return false;
                }

                if (proc.ExitCode != 0)
                {
                    message = "安装程序返回错误码 " + proc.ExitCode;
                    return false;
                }
            }
        }
        catch (System.ComponentModel.Win32Exception ex) when (ex.NativeErrorCode == 1223)
        {
            message = "已取消管理员授权，索引服务没有安装";
            return false;
        }
        catch (Exception ex)
        {
            message = "安装索引服务失败：" + ex.Message;
            ConfigStore.Log(message);
            return false;
        }

        // 服务注册/注销要几百毫秒才反映到服务列表
        for (int i = 0; i < 25; i++)
        {
            if (IsServiceInstalled() == expectInstalled) return true;
            Thread.Sleep(200);
        }

        message = expectInstalled
            ? "安装程序已结束，但系统里没有找到对应服务"
            : "卸载程序已结束，但系统里仍能查到对应服务";
        return false;
    }
}
