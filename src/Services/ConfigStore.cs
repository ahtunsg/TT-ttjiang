using System.Globalization;
using System.IO;
using System.Text;
using TTJiang.Models;

namespace TTJiang.Services;

/// <summary>
/// shortcuts.json 读写。文件只跟随 exe 所在目录，保持"删文件夹即删干净"。
/// </summary>
public static class ConfigStore
{
    internal sealed class Root
    {
        public List<ShortcutItem> shortcuts { get; set; } = new List<ShortcutItem>();

        public AppSettings settings { get; set; } = new AppSettings();
    }

    /// <summary>一次读取拿到的完整配置，避免启动时把同一个文件读两遍。</summary>
    public sealed class Snapshot
    {
        public List<ShortcutItem> Items { get; set; } = new List<ShortcutItem>();
        public AppSettings Settings { get; set; } = new AppSettings();
    }

    private const long MaxLogBytes = 1024 * 1024;

    private static readonly object Sync = new object();
    private static readonly object LogSync = new object();
    private static readonly string StorageDir = ResolveStorageDir();

    private static AppSettings _settings = new AppSettings();

    public static string BaseDir
    {
        get { return StorageDir; }
    }

    public static string ConfigPath
    {
        get { return Path.Combine(StorageDir, "shortcuts.json"); }
    }

    public static string LogPath
    {
        get { return Path.Combine(StorageDir, "ttjiang.log"); }
    }

    /// <summary>
    /// 读取快捷方式与设置。首次运行（没有 shortcuts.json）时生成一份默认配置并落盘，
    /// 这样单独复制一个 exe 到新机器也能直接用。
    /// </summary>
    public static Snapshot LoadAll()
    {
        var snapshot = new Snapshot();

        Root root = ReadRoot();
        if (root == null)
        {
            snapshot.Settings = Normalize(_settings);
            snapshot.Items = SeedDefaults();
            return snapshot;
        }

        _settings = Normalize(root.settings);
        snapshot.Settings = _settings.Clone();
        snapshot.Items = Sanitize(root.shortcuts);
        return snapshot;
    }

    /// <summary>只读设置，不改动磁盘。</summary>
    public static AppSettings LoadSettings()
    {
        Root root = ReadRoot();
        if (root != null) _settings = Normalize(root.settings);
        return _settings.Clone();
    }

    public static bool SaveSettings(AppSettings settings)
    {
        _settings = Normalize(settings);
        Root root = ReadRoot() ?? new Root();
        root.settings = _settings;
        return WriteRoot(root);
    }

    /// <summary>只写快捷方式，保留已有的 settings 段。</summary>
    public static bool Save(List<ShortcutItem> items)
    {
        Root root = ReadRoot() ?? new Root();
        root.shortcuts = items;
        root.settings = Normalize(root.settings ?? _settings);
        return WriteRoot(root);
    }

    /// <summary>把当前配置文件复制一份带时间戳的备份，返回备份路径（失败或没有配置时返回 null）。</summary>
    public static string BackupConfig(string reason)
    {
        try
        {
            if (!File.Exists(ConfigPath)) return null;

            string name = "shortcuts-" + DateTime.Now.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture) + ".bak";
            string path = Path.Combine(StorageDir, name);
            File.Copy(ConfigPath, path, true);
            Log("备份配置（" + reason + "）到 " + name);
            return path;
        }
        catch (Exception ex)
        {
            Log("备份配置失败：" + ex.Message);
            return null;
        }
    }

    private static List<ShortcutItem> Sanitize(List<ShortcutItem> source)
    {
        var list = new List<ShortcutItem>();
        if (source == null) return list;

        foreach (var it in source)
        {
            if (it == null) continue;
            if (it.Name == null) it.Name = "";
            if (it.Path == null) it.Path = "";
            if (it.Args == null) it.Args = "";
            if (it.Category == null) it.Category = "";
            if (string.IsNullOrWhiteSpace(it.Path)) continue;
            list.Add(it);
        }

        return list;
    }

    private static AppSettings Normalize(AppSettings s)
    {
        if (s == null) return new AppSettings();
        if (s.EverythingPath == null) s.EverythingPath = "";
        s.EverythingPath = s.EverythingPath.Trim();
        // SearchResultLimit 是旧版遗留字段，不再 clamp：结果总数由引擎决定
        if (s.SearchSortKey < 0 || s.SearchSortKey > 2) s.SearchSortKey = 0;
        if (s.SearchSortAscending != 0 && s.SearchSortAscending != 1) s.SearchSortAscending = 1;
        if (s.SearchMatchFullPath != 0 && s.SearchMatchFullPath != 1) s.SearchMatchFullPath = 0;
        return s;
    }

    // ---------------------------------------------------------------- 目录

    /// <summary>
    /// 配置与日志只放 exe 同级目录：删掉整个文件夹就等于删干净，不往用户目录里散东西。
    /// 目录不可写时由启动流程直接提示，不做任何降级。
    /// </summary>
    private static string ResolveStorageDir()
    {
        return AppContext.BaseDirectory.TrimEnd('\\');
    }

    public static bool CanWrite(string dir)
    {
        try
        {
            string probe = Path.Combine(dir, ".hc-portal-write-" + Guid.NewGuid().ToString("N").Substring(0, 8));
            using (new FileStream(probe, FileMode.CreateNew, FileAccess.Write, FileShare.None, 1,
                       FileOptions.DeleteOnClose))
            {
                // 只是探测，关闭时自动删除
            }
            return true;
        }
        catch
        {
            return false;
        }
    }

    // ---------------------------------------------------------------- 解析

    private static Root ReadRoot()
    {
        try
        {
            if (!File.Exists(ConfigPath)) return null;
            string text = File.ReadAllText(ConfigPath);
            if (string.IsNullOrWhiteSpace(text)) return null;

            object parsed = JsonLite.Parse(text);
            var map = parsed as Dictionary<string, object>;
            if (map == null) return null;

            var root = new Root();
            root.shortcuts = ReadShortcuts(map);
            root.settings = ReadSettings(map);
            return root;
        }
        catch (Exception ex)
        {
            Log("读取 shortcuts.json 失败：" + ex.Message);
            return null;
        }
    }

    private static List<ShortcutItem> ReadShortcuts(Dictionary<string, object> map)
    {
        var list = new List<ShortcutItem>();

        object raw;
        if (!map.TryGetValue("shortcuts", out raw)) return list;

        var array = raw as List<object>;
        if (array == null) return list;

        foreach (object entry in array)
        {
            var row = entry as Dictionary<string, object>;
            if (row == null) continue;

            list.Add(new ShortcutItem
            {
                Name = GetString(row, "name", ""),
                Path = GetString(row, "path", ""),
                Args = GetString(row, "args", ""),
                Category = GetString(row, "category", "")
            });
        }

        return list;
    }

    private static AppSettings ReadSettings(Dictionary<string, object> map)
    {
        var settings = new AppSettings();

        object raw;
        if (!map.TryGetValue("settings", out raw)) return settings;

        var row = raw as Dictionary<string, object>;
        if (row == null) return settings;

        settings.EverythingPath = GetString(row, "everythingPath", "");
        settings.SearchResultLimit = GetInt(row, "searchResultLimit", 30);
        settings.SearchSortKey = GetInt(row, "searchSortKey", 0);
        settings.SearchSortAscending = GetInt(row, "searchSortAscending", 1);
        settings.SearchMatchFullPath = GetInt(row, "searchMatchFullPath", 0);
        return settings;
    }

    private static string GetString(Dictionary<string, object> row, string key, string fallback)
    {
        object value;
        if (!row.TryGetValue(key, out value) || value == null) return fallback;
        return value as string ?? value.ToString();
    }

    private static int GetInt(Dictionary<string, object> row, string key, int fallback)
    {
        object value;
        if (!row.TryGetValue(key, out value) || value == null) return fallback;
        if (value is double) return (int)(double)value;
        int parsed;
        return int.TryParse(Convert.ToString(value, CultureInfo.InvariantCulture), out parsed) ? parsed : fallback;
    }

    // ---------------------------------------------------------------- 写出

    /// <summary>
    /// 先写同目录的临时文件再替换，避免写到一半断电/被杀进程留下半截 json。
    /// </summary>
    private static bool WriteRoot(Root root)
    {
        string json = BuildJson(root);
        string tmp = ConfigPath + ".tmp";

        lock (Sync)
        {
            try
            {
                using (var stream = new FileStream(tmp, FileMode.Create, FileAccess.Write, FileShare.None))
                using (var writer = new StreamWriter(stream, new UTF8Encoding(false)))
                {
                    writer.Write(json);
                    writer.Flush();
                    stream.Flush(true);
                }

                if (Replace(tmp, ConfigPath)) return true;

                TryDelete(tmp);
                Log("写入 shortcuts.json 失败：无法替换目标文件");
                return false;
            }
            catch (Exception ex)
            {
                TryDelete(tmp);
                Log("写入 shortcuts.json 失败：" + ex.Message);
                return false;
            }
        }
    }

    private static bool Replace(string tmp, string target)
    {
        try
        {
            File.Replace(tmp, target, null);
            return true;
        }
        catch (Exception ex)
        {
            // FAT32、部分网络盘不支持 Replace，退回删除后改名
            if (!(ex is IOException || ex is PlatformNotSupportedException || ex is UnauthorizedAccessException))
            {
                return false;
            }
        }

        try
        {
            if (File.Exists(target)) File.Delete(target);
            File.Move(tmp, target);
            return true;
        }
        catch
        {
            return false;
        }
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path)) File.Delete(path);
        }
        catch
        {
            // 清理失败不影响结果
        }
    }

    private static string BuildJson(Root root)
    {
        var sb = new StringBuilder();
        sb.Append("{\n");
        sb.Append("  \"shortcuts\": ");

        List<ShortcutItem> items = root.shortcuts ?? new List<ShortcutItem>();
        if (items.Count == 0)
        {
            sb.Append("[],\n");
        }
        else
        {
            sb.Append("[\n");
            for (int i = 0; i < items.Count; i++)
            {
                ShortcutItem it = items[i];
                sb.Append("    {\n");
                sb.Append("      \"name\": ").Append(JsonLite.Quote(it.Name ?? "")).Append(",\n");
                sb.Append("      \"path\": ").Append(JsonLite.Quote(it.Path ?? "")).Append(",\n");
                sb.Append("      \"args\": ").Append(JsonLite.Quote(it.Args ?? "")).Append(",\n");
                sb.Append("      \"category\": ").Append(JsonLite.Quote(it.Category ?? "")).Append('\n');
                sb.Append("    }");
                sb.Append(i < items.Count - 1 ? ",\n" : "\n");
            }
            sb.Append("  ],\n");
        }

        AppSettings s = root.settings ?? new AppSettings();
        sb.Append("  \"settings\": {\n");
        sb.Append("    \"everythingPath\": ").Append(JsonLite.Quote(s.EverythingPath ?? "")).Append(",\n");
        sb.Append("    \"searchResultLimit\": ")
          .Append(s.SearchResultLimit.ToString(CultureInfo.InvariantCulture)).Append(",\n");
        sb.Append("    \"searchSortKey\": ")
          .Append(s.SearchSortKey.ToString(CultureInfo.InvariantCulture)).Append(",\n");
        sb.Append("    \"searchSortAscending\": ")
          .Append(s.SearchSortAscending.ToString(CultureInfo.InvariantCulture)).Append(",\n");
        sb.Append("    \"searchMatchFullPath\": ")
          .Append(s.SearchMatchFullPath.ToString(CultureInfo.InvariantCulture)).Append('\n');
        sb.Append("  }\n");
        sb.Append("}\n");

        return sb.ToString();
    }

    private static List<ShortcutItem> SeedDefaults()
    {
        var items = DefaultConfig.Create();

        // 首次运行直接落盘，不经过 Save，免得多读一次文件。
        var root = new Root { shortcuts = items, settings = Normalize(_settings) };
        if (WriteRoot(root))
        {
            Log("未找到 shortcuts.json，已生成 " + items.Count + " 条默认快捷方式。");
        }
        return items;
    }

    // ---------------------------------------------------------------- 日志

    /// <summary>超过 1 MB 时滚动为 portal.old.log，长期驻留也不会无限增长。</summary>
    public static void Log(string message)
    {
        LogTo(StorageDir, message);
    }

    private static void LogTo(string dir, string message)
    {
        try
        {
            string path = Path.Combine(dir, "ttjiang.log");

            lock (LogSync)
            {
                var info = new FileInfo(path);
                if (info.Exists && info.Length > MaxLogBytes)
                {
                    string old = Path.Combine(dir, "portal.old.log");
                    try
                    {
                        if (File.Exists(old)) File.Delete(old);
                    }
                    catch
                    {
                        // 旧日志删不掉就继续往当前文件追加
                    }

                    try
                    {
                        File.Move(path, old);
                    }
                    catch
                    {
                        // 同上
                    }
                }

                string line = "[" + DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss") + "] " + message
                              + Environment.NewLine;
                File.AppendAllText(path, line);
            }
        }
        catch
        {
            // 日志失败不影响主流程
        }
    }
}
