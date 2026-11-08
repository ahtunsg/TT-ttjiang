using TTJiang.Models;

namespace TTJiang.Services;

/// <summary>首次运行（没有 shortcuts.json）时写入的默认快捷方式。</summary>
public static class DefaultConfig
{
    private static ShortcutItem Make(string name, string path, string category, string args = "")
    {
        return new ShortcutItem { Name = name, Path = path, Category = category, Args = args };
    }

    public static List<ShortcutItem> Create()
    {
        return new List<ShortcutItem>
        {
            Make("计算器", "calc.exe", "工具"),
            Make("画图", "mspaint.exe", "工具")
        };
    }
}
