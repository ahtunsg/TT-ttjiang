namespace TTJiang.Models;

/// <summary>一条快捷方式配置。</summary>
public class ShortcutItem
{
    public string Name { get; set; } = "";

    public string Path { get; set; } = "";

    public string Args { get; set; } = "";

    public string Category { get; set; } = "";

    public string DisplayName
    {
        get
        {
            if (!string.IsNullOrWhiteSpace(Name)) return Name;
            if (!string.IsNullOrWhiteSpace(Path)) return System.IO.Path.GetFileName(Path.TrimEnd('\\', '/'));
            return "(未命名)";
        }
    }

    public string CategoryOrDefault
    {
        get { return string.IsNullOrWhiteSpace(Category) ? "未分类" : Category; }
    }

    public ShortcutItem Clone()
    {
        return new ShortcutItem
        {
            Name = Name,
            Path = Path,
            Args = Args,
            Category = Category
        };
    }
}
