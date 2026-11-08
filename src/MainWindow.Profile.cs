using System.IO;
using System.Windows;
using TTJiang.Models;
using TTJiang.Services;

namespace TTJiang;

public partial class MainWindow
{
    private bool SaveItems()
    {
        bool ok = ConfigStore.Save(_items);
        ReportSaveResult(ok);
        return ok;
    }

    // ---------------------------------------------------------------- 增删改
    private void AddShortcut()
    {
        var dlg = new EditShortcutWindow(null, CurrentCategories()) { Owner = this };
        if (dlg.ShowDialog() != true || dlg.Result == null) return;

        _items.Add(dlg.Result);
        SaveItems();

        SearchBox.Text = "";
        RenderTiles();
        UpdateStatus();
    }

    private void EditShortcut(ShortcutItem item)
    {
        var dlg = new EditShortcutWindow(item, CurrentCategories()) { Owner = this };
        if (dlg.ShowDialog() != true || dlg.Result == null) return;

        item.Name = dlg.Result.Name;
        item.Path = dlg.Result.Path;
        item.Args = dlg.Result.Args;
        item.Category = dlg.Result.Category;

        SaveItems();
        RenderTiles();
    }

    private void DeleteItem(ShortcutItem item)
    {
        var result = MessageBox.Show("确定删除「" + item.DisplayName + "」吗？", "确认删除",
            MessageBoxButton.YesNo, MessageBoxImage.Question);
        if (result != MessageBoxResult.Yes) return;

        _items.Remove(item);
        SaveItems();
        RenderTiles();
        UpdateStatus();
    }

    // ---------------------------------------------------------------- 导入导出
    /// <summary>把当前快捷方式导出成一个 json 文件，可在另一台机器导入。</summary>
    public void ExportProfile(Window owner = null)
    {
        var dlg = new Microsoft.Win32.SaveFileDialog
        {
            Title = "导出快捷方式",
            Filter = "快捷方式配置 (*.json)|*.json|所有文件 (*.*)|*.*",
            DefaultExt = "json",
            AddExtension = true,
            FileName = ProfileIO.DefaultFileName(),
            OverwritePrompt = true
        };

        if (dlg.ShowDialog(owner ?? this) != true) return;

        string error;
        if (!ProfileIO.Export(_items, dlg.FileName, out error))
        {
            MessageBox.Show("导出失败：" + Environment.NewLine + error, "导出快捷方式",
                MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        MessageBox.Show("已导出 " + _items.Count + " 个快捷方式：" + Environment.NewLine + dlg.FileName,
            "导出快捷方式", MessageBoxButton.OK, MessageBoxImage.Information);
    }

    /// <summary>从导出的 json 文件读取快捷方式，替换当前列表。</summary>
    public void ImportProfile(Window owner = null)
    {
        var dlg = new Microsoft.Win32.OpenFileDialog
        {
            Title = "导入快捷方式",
            Filter = "快捷方式配置 (*.json)|*.json|所有文件 (*.*)|*.*",
            DefaultExt = "json",
            CheckFileExists = true
        };

        if (dlg.ShowDialog(owner ?? this) != true) return;

        string error;
        List<ShortcutItem> imported = ProfileIO.Import(dlg.FileName, out error);

        if (imported == null)
        {
            MessageBox.Show(error, "导入快捷方式", MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        if (imported.Count == 0)
        {
            MessageBox.Show("这个文件里没有可用的快捷方式（每条都需要 path）。", "导入快捷方式",
                MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        if (imported.Count > ProfileIO.MaxImportCount)
        {
            MessageBox.Show(
                "一次最多导入 " + ProfileIO.MaxImportCount + " 个快捷方式，这个文件里有 "
                + imported.Count + " 个。" + Environment.NewLine
                + "条目过多时首屏会明显变慢，建议拆成几份分别导入。",
                "导入快捷方式", MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        if (_items.Count > 0)
        {
            var confirm = MessageBox.Show(
                "导入会替换当前的 " + _items.Count + " 个快捷方式，换成文件里的 " + imported.Count + " 个。"
                + Environment.NewLine + Environment.NewLine + "继续吗？",
                "导入快捷方式", MessageBoxButton.YesNo, MessageBoxImage.Question);

            if (confirm != MessageBoxResult.Yes) return;
        }

        // 导入是覆盖式操作，先留一份备份，走错了还能捞回来
        string backup = ConfigStore.BackupConfig("导入前");

        _items.Clear();
        _items.AddRange(imported);
        SaveItems();

        SearchBox.Text = "";
        RenderTiles();
        UpdateStatus();

        string message = "已导入 " + imported.Count + " 个快捷方式。";
        if (!string.IsNullOrEmpty(backup))
        {
            message += Environment.NewLine + Environment.NewLine
                       + "原配置已备份为：" + Environment.NewLine + Path.GetFileName(backup);
        }

        MessageBox.Show(message, "导入快捷方式", MessageBoxButton.OK, MessageBoxImage.Information);
    }
}
