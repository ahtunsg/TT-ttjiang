using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using TTJiang.Models;

namespace TTJiang;

public partial class EditShortcutWindow : Window
{
    private readonly List<string> _categories;

    /// <summary>对话框确认后的结果；取消时为 null。</summary>
    public ShortcutItem Result { get; private set; }

    public EditShortcutWindow(ShortcutItem source, List<string> categories)
    {
        InitializeComponent();

        _categories = categories ?? new List<string>();

        if (source != null)
        {
            Title = "编辑快捷方式";
            NameBox.Text = source.Name;
            PathBox.Text = source.Path;
            ArgsBox.Text = source.Args;
            CategoryBox.Text = source.CategoryOrDefault;
        }
        else
        {
            Title = "添加快捷方式";
            CategoryBox.Text = _categories.Count > 0 ? _categories[0] : "工具";
        }

        Loaded += (s, e) =>
        {
            NameBox.Focus();
            NameBox.SelectAll();
        };

        PreviewKeyDown += (s, e) =>
        {
            if (e.Key == Key.Escape)
            {
                e.Handled = true;
                DialogResult = false;
            }
            else if (e.Key == Key.Enter)
            {
                e.Handled = true;
                Save_Click(s, null);
            }
        };
    }

    private void BrowseFile_Click(object sender, RoutedEventArgs e)
    {
        var dlg = new Microsoft.Win32.OpenFileDialog
        {
            Title = "选择程序或文件",
            Filter = "所有文件|*.*",
            CheckFileExists = true
        };

        if (dlg.ShowDialog(this) == true)
        {
            PathBox.Text = dlg.FileName;
            if (string.IsNullOrWhiteSpace(NameBox.Text))
            {
                NameBox.Text = Path.GetFileNameWithoutExtension(dlg.FileName);
            }
        }
    }

    private void BrowseFolder_Click(object sender, RoutedEventArgs e)
    {
        string folder = FolderPicker.Pick(this, "选择文件夹");
        if (string.IsNullOrEmpty(folder)) return;

        PathBox.Text = folder;
        if (string.IsNullOrWhiteSpace(NameBox.Text))
        {
            NameBox.Text = Path.GetFileName(folder.TrimEnd('\\', '/'));
        }
    }

    /// <summary>原生 SHBrowseForFolder，替代 WinForms 的 FolderBrowserDialog。</summary>
    private static class FolderPicker
    {
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct BROWSEINFO
        {
            public IntPtr hwndOwner;
            public IntPtr pidlRoot;
            public IntPtr pszDisplayName;
            public string lpszTitle;
            public uint ulFlags;
            public IntPtr lpfn;
            public IntPtr lParam;
            public int iImage;
        }

        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        private static extern IntPtr SHBrowseForFolderW(ref BROWSEINFO lpbi);

        [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SHGetPathFromIDListW(IntPtr pidl, StringBuilder pszPath);

        [DllImport("ole32.dll")]
        private static extern void CoTaskMemFree(IntPtr pv);

        private const uint BIF_RETURNONLYFSDIRS = 0x0001;
        private const uint BIF_EDITBOX = 0x0010;
        private const uint BIF_NEWDIALOGSTYLE = 0x0040;

        public static string Pick(Window owner, string title)
        {
            IntPtr display = Marshal.AllocHGlobal(260 * 2);
            try
            {
                var info = new BROWSEINFO
                {
                    hwndOwner = new WindowInteropHelper(owner).Handle,
                    pszDisplayName = display,
                    lpszTitle = title,
                    ulFlags = BIF_RETURNONLYFSDIRS | BIF_EDITBOX | BIF_NEWDIALOGSTYLE
                };

                IntPtr pidl = SHBrowseForFolderW(ref info);
                if (pidl == IntPtr.Zero) return null;

                try
                {
                    var buffer = new StringBuilder(260);
                    return SHGetPathFromIDListW(pidl, buffer) ? buffer.ToString() : null;
                }
                finally
                {
                    CoTaskMemFree(pidl);
                }
            }
            finally
            {
                Marshal.FreeHGlobal(display);
            }
        }
    }

    private void CategoryPick_Click(object sender, RoutedEventArgs e)
    {
        var menu = new ContextMenu();
        if (_categories.Count == 0)
        {
            menu.Items.Add(new MenuItem { Header = "（暂无已有分类）", IsEnabled = false });
        }
        else
        {
            foreach (string c in _categories)
            {
                string value = c;
                var mi = new MenuItem { Header = value };
                mi.Click += (s, args) => CategoryBox.Text = value;
                menu.Items.Add(mi);
            }
        }

        menu.PlacementTarget = CategoryPickButton;
        menu.IsOpen = true;
    }

    private void Save_Click(object sender, RoutedEventArgs e)
    {
        string name = NameBox.Text.Trim();
        string path = PathBox.Text.Trim();

        if (name.Length == 0)
        {
            MessageBox.Show("请填写名称。", "添加快捷方式", MessageBoxButton.OK, MessageBoxImage.Information);
            NameBox.Focus();
            return;
        }

        if (path.Length == 0)
        {
            MessageBox.Show("请填写路径，或用「文件 / 目录」按钮选择。", "添加快捷方式",
                MessageBoxButton.OK, MessageBoxImage.Information);
            PathBox.Focus();
            return;
        }

        string category = CategoryBox.Text.Trim();
        if (category.Length == 0) category = "未分类";

        Result = new ShortcutItem
        {
            Name = name,
            Path = path,
            Args = ArgsBox.Text.Trim(),
            Category = category
        };

        DialogResult = true;
    }

    private void Cancel_Click(object sender, RoutedEventArgs e)
    {
        DialogResult = false;
    }
}
