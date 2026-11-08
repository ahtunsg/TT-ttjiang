using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace TTJiang.Services;

/// <summary>
/// 取快捷方式对应的程序/文件/文件夹图标。
/// 先按 48×48 从 exe 资源里直接抽（PrivateExtractIcons），抽不到再用 Shell 的大图标。
/// 取不到时返回 null，界面用首字占位。
/// </summary>
public static class IconLoader
{
    private const int IconSize = 48;

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint PrivateExtractIcons(string szFileName, int nIconIndex, int cxIcon, int cyIcon,
        IntPtr[] phicon, uint[] piconid, uint nIcons, uint flags);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool DestroyIcon(IntPtr hIcon);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr SHGetFileInfo(string pszPath, uint dwFileAttributes, ref SHFILEINFO psfi,
        uint cbFileInfo, uint uFlags);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct SHFILEINFO
    {
        public IntPtr hIcon;
        public int iIcon;
        public uint dwAttributes;

        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)]
        public string szDisplayName;

        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)]
        public string szTypeName;
    }

    private const uint SHGFI_ICON = 0x000000100;
    private const uint SHGFI_LARGEICON = 0x000000000;
    private const uint SHGFI_USEFILEATTRIBUTES = 0x000000010;
    private const uint FILE_ATTRIBUTE_NORMAL = 0x00000080;
    private const uint FILE_ATTRIBUTE_DIRECTORY = 0x00000010;

    private static readonly Dictionary<string, Entry> Cache =
        new Dictionary<string, Entry>(StringComparer.OrdinalIgnoreCase);

    private static readonly object CacheLock = new object();

    /// <summary>抽取失败的结果只短暂保留，避免可移动盘拔插后永远显示占位图标。</summary>
    private static readonly TimeSpan FailTtl = TimeSpan.FromSeconds(5);

    private sealed class Entry
    {
        public ImageSource Image;
        public string Stamp = "";
        public DateTime FailedAt = DateTime.MinValue;
    }

    public static ImageSource Load(string rawPath)
    {
        string key = rawPath ?? "";
        string stamp = StampOf(key);

        lock (CacheLock)
        {
            Entry cached;
            if (Cache.TryGetValue(key, out cached))
            {
                // 成功结果要求文件版本戳一致；程序升级换了图标会重新抽取
                if (cached.Image != null && cached.Stamp == stamp) return cached.Image;

                // 失败结果在短期内不重试，避免每次重绘都扫一遍 PATH
                if (cached.Image == null && DateTime.UtcNow - cached.FailedAt < FailTtl) return null;
            }
        }

        ImageSource result = null;

        try
        {
            string resolved = Resolve(rawPath);
            if (!string.IsNullOrEmpty(resolved))
            {
                bool isDir = Directory.Exists(resolved);
                IntPtr hIcon = Extract(resolved, isDir);
                if (hIcon != IntPtr.Zero)
                {
                    try
                    {
                        BitmapSource source = Imaging.CreateBitmapSourceFromHIcon(
                            hIcon, Int32Rect.Empty, BitmapSizeOptions.FromEmptyOptions());
                        source.Freeze();
                        result = source;
                    }
                    finally
                    {
                        DestroyIcon(hIcon);
                    }
                }
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("读取图标失败：" + rawPath + " -> " + ex.Message);
        }

        lock (CacheLock)
        {
            Cache[key] = result != null
                ? new Entry { Image = result, Stamp = stamp }
                : new Entry { Image = null, FailedAt = DateTime.UtcNow };
        }

        return result;
    }

    /// <summary>文件版本戳：修改时间 + 长度，用来判断缓存的图标是否过期。</summary>
    private static string StampOf(string rawPath)
    {
        if (string.IsNullOrEmpty(rawPath)) return "";

        try
        {
            string p = rawPath.Trim().Trim('"');
            if (p.StartsWith("shell:", StringComparison.OrdinalIgnoreCase)) return "shell";

            if (File.Exists(p))
            {
                var info = new FileInfo(p);
                return info.LastWriteTimeUtc.Ticks.ToString() + ":" + info.Length;
            }
        }
        catch
        {
            // 路径非法按无版本戳处理
        }

        return "";
    }

    /// <summary>把配置里的 path 变成真实存在的路径；shell: 特殊目录做有限映射。</summary>
    private static string Resolve(string rawPath)
    {
        if (string.IsNullOrWhiteSpace(rawPath)) return null;

        string p = rawPath.Trim().Trim('"');
        if (p.Length == 0) return null;

        if (p.StartsWith("shell:", StringComparison.OrdinalIgnoreCase))
        {
            string key = p.Substring(6).Trim().TrimEnd('\\', '/');
            if (key.Equals("Personal", StringComparison.OrdinalIgnoreCase))
                return Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments);
            if (key.Equals("Desktop", StringComparison.OrdinalIgnoreCase))
                return Environment.GetFolderPath(Environment.SpecialFolder.Desktop);
            if (key.Equals("Downloads", StringComparison.OrdinalIgnoreCase))
                return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Downloads");
            if (key.Equals("Profile", StringComparison.OrdinalIgnoreCase))
                return Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
            return null;
        }

        if (File.Exists(p) || Directory.Exists(p)) return p;

        // 没写全路径的命令行（notepad.exe、calc.exe）在 PATH 里找一次，只为拿图标
        if (p.IndexOf('\\') < 0 && p.IndexOf('/') < 0)
        {
            string pathVar = Environment.GetEnvironmentVariable("PATH") ?? "";
            foreach (string dir in pathVar.Split(';'))
            {
                if (string.IsNullOrWhiteSpace(dir)) continue;
                try
                {
                    string candidate = Path.Combine(dir.Trim().Trim('"'), p);
                    if (File.Exists(candidate)) return candidate;
                    if (File.Exists(candidate + ".exe")) return candidate + ".exe";
                }
                catch
                {
                    // 跳过非法 PATH 项
                }
            }
        }

        return null;
    }

    private static IntPtr Extract(string resolved, bool isDir)
    {
        if (!isDir)
        {
            var handles = new IntPtr[1];
            var ids = new uint[1];
            uint count = PrivateExtractIcons(resolved, 0, IconSize, IconSize, handles, ids, 1, 0);
            if (count > 0 && handles[0] != IntPtr.Zero) return handles[0];
        }

        var info = new SHFILEINFO();
        uint flags = SHGFI_ICON | SHGFI_LARGEICON;
        uint attributes = FILE_ATTRIBUTE_NORMAL;

        if (isDir)
        {
            attributes = FILE_ATTRIBUTE_DIRECTORY;
            flags |= SHGFI_USEFILEATTRIBUTES;
        }

        IntPtr ok = SHGetFileInfo(resolved, attributes, ref info,
            (uint)Marshal.SizeOf(typeof(SHFILEINFO)), flags);

        if (ok != IntPtr.Zero && info.hIcon != IntPtr.Zero) return info.hIcon;
        return IntPtr.Zero;
    }
}
