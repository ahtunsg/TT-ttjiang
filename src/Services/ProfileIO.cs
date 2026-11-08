using System.Globalization;
using System.IO;
using System.Text;
using TTJiang.Models;

namespace TTJiang.Services;

/// <summary>
/// 快捷方式配置的导出与导入。
/// 导出文件里只有一个 shortcuts 数组（外加格式与时间标记），结构与 shortcuts.json 的
/// shortcuts 段完全一致，所以导出的文件既可以被本程序导入，也可以整体当作 shortcuts.json 使用。
/// </summary>
public static class ProfileIO
{
    private const string FormatName = "hc-portal-shortcuts";

    public const string Extension = ".json";

    /// <summary>单次导入上限。图标网格没有虚拟化，条目太多会明显拖慢首屏。</summary>
    public const int MaxImportCount = 500;

    public static string DefaultFileName()
    {
        return "快捷方式-" + DateTime.Now.ToString("yyyyMMdd", CultureInfo.InvariantCulture) + Extension;
    }

    /// <summary>把当前快捷方式写成文件。失败时 error 里带原因。</summary>
    public static bool Export(IEnumerable<ShortcutItem> items, string path, out string error)
    {
        error = null;

        try
        {
            var list = new List<ShortcutItem>();
            if (items != null)
            {
                foreach (ShortcutItem it in items)
                {
                    if (it != null) list.Add(it);
                }
            }

            var sb = new StringBuilder();
            sb.Append("{\n");
            sb.Append("  \"format\": ").Append(JsonLite.Quote(FormatName)).Append(",\n");
            sb.Append("  \"version\": 1,\n");
            sb.Append("  \"exportedAt\": ")
              .Append(JsonLite.Quote(DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture)))
              .Append(",\n");
            sb.Append("  \"shortcuts\": ");

            if (list.Count == 0)
            {
                sb.Append("[]\n");
            }
            else
            {
                sb.Append("[\n");
                for (int i = 0; i < list.Count; i++)
                {
                    ShortcutItem it = list[i];
                    sb.Append("    {\n");
                    sb.Append("      \"name\": ").Append(JsonLite.Quote(it.Name ?? "")).Append(",\n");
                    sb.Append("      \"path\": ").Append(JsonLite.Quote(it.Path ?? "")).Append(",\n");
                    sb.Append("      \"args\": ").Append(JsonLite.Quote(it.Args ?? "")).Append(",\n");
                    sb.Append("      \"category\": ").Append(JsonLite.Quote(it.Category ?? "")).Append('\n');
                    sb.Append("    }");
                    sb.Append(i < list.Count - 1 ? ",\n" : "\n");
                }
                sb.Append("  ]\n");
            }

            sb.Append("}\n");

            File.WriteAllText(path, sb.ToString(), new UTF8Encoding(false));
            ConfigStore.Log("导出快捷方式 " + list.Count + " 条到 " + path);
            return true;
        }
        catch (Exception ex)
        {
            error = ex.Message;
            ConfigStore.Log("导出快捷方式失败：" + ex.Message);
            return false;
        }
    }

    /// <summary>
    /// 读取导出文件。成功返回列表（可能为空），失败返回 null 并写 error。
    /// 兼容两种写法：带 shortcuts 字段的对象，或直接是快捷方式数组。
    /// </summary>
    public static List<ShortcutItem> Import(string path, out string error)
    {
        error = null;

        try
        {
            if (!File.Exists(path))
            {
                error = "文件不存在：" + path;
                return null;
            }

            string text = File.ReadAllText(path);
            if (string.IsNullOrWhiteSpace(text))
            {
                error = "文件是空的。";
                return null;
            }

            object parsed = JsonLite.Parse(text);

            List<object> array = null;
            var map = parsed as Dictionary<string, object>;
            if (map != null)
            {
                object raw;
                if (map.TryGetValue("shortcuts", out raw)) array = raw as List<object>;
            }
            else
            {
                array = parsed as List<object>;
            }

            if (array == null)
            {
                error = "文件里没有 shortcuts 列表，不是本程序导出的快捷方式配置。";
                return null;
            }

            var list = new List<ShortcutItem>();
            foreach (object entry in array)
            {
                var row = entry as Dictionary<string, object>;
                if (row == null) continue;

                string itemPath = ReadString(row, "path").Trim();
                if (itemPath.Length == 0) continue;

                list.Add(new ShortcutItem
                {
                    Name = ReadString(row, "name").Trim(),
                    Path = itemPath,
                    Args = ReadString(row, "args").Trim(),
                    Category = ReadString(row, "category").Trim()
                });
            }

            ConfigStore.Log("从 " + path + " 导入快捷方式 " + list.Count + " 条");
            return list;
        }
        catch (Exception ex)
        {
            error = "读取失败：" + ex.Message;
            ConfigStore.Log("导入快捷方式失败：" + ex.Message);
            return null;
        }
    }

    private static string ReadString(Dictionary<string, object> row, string key)
    {
        object value;
        if (!row.TryGetValue(key, out value) || value == null) return "";
        return value as string ?? value.ToString();
    }
}
