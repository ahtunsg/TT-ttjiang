using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using TTJiang.Models;
using TTJiang.Services;

namespace TTJiang;

/// <summary>
/// 主窗口。为了便于维护，按职责拆成几个 partial 文件：
/// MainWindow.Search.cs  搜索与数字大写
/// MainWindow.Tiles.cs   图标网格
/// MainWindow.Profile.cs 快捷方式的增删改与导入导出
/// MainWindow.Actions.cs 打开与复制
/// </summary>
public partial class MainWindow : Window
{
    private enum BodyMode
    {
        Shortcuts,
        Files,
        Dx
    }

    private static readonly Brush CardBg = Frozen(0xE3, 0xEE, 0xFB);   // 悬停底
    private static readonly Brush HoverBg = Frozen(0xEA, 0xF2, 0xFD);  // 强调底
    private static readonly Brush TextFg = Frozen(0x1F, 0x1F, 0x1F);
    private static readonly Brush MutedFg = Frozen(0x6B, 0x6B, 0x6B);
    private static readonly Brush SubFg = Frozen(0x9A, 0x9A, 0x9A);
    private static readonly Brush AccentFg = Frozen(0x0F, 0x6C, 0xBD);

    private readonly List<ShortcutItem> _items = new();
    private readonly List<SearchHit> _hits = new();

    /// <summary>图标后台抽取的并发上限，避免一次重绘把 CPU 拉满。</summary>
    private readonly SemaphoreSlim _iconSlots = new(2);

    private AppSettings _settings = new();
    private SearchOptions _searchOptions = new();
    private KeyboardHook _hook;
    private TrayIcon _tray;
    private CancellationTokenSource _searchCts;
    private int _searchSeq;
    private bool _searching;
    private string _statusMessage;

    /// <summary>配置写盘失败的提示。与搜索提示分开，避免被界面刷新清掉。</summary>
    private string _storageMessage;
    private bool _exiting;
    private string _lastDxResult;

    /// <summary>每次重绘 +1，后台加载完的图标发现代次变了就丢弃。</summary>
    private int _tileGeneration;

    private bool _writeFailedReported;

    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool BringWindowToTop(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("user32.dll")]
    private static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool attach);

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [DllImport("user32.dll")]
    private static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);

    public MainWindow()
    {
        InitializeComponent();

        SourceInitialized += OnSourceInitialized;
        Loaded += OnLoaded;
        Closing += OnClosing;
        Closed += OnClosed;
        PreviewKeyDown += OnPreviewKeyDown;

        Reload();
    }

    private static Brush Frozen(byte r, byte g, byte b)
    {
        var brush = new SolidColorBrush(Color.FromRgb(r, g, b));
        brush.Freeze();
        return brush;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        SearchBox.Focus();
    }

    private void OnPreviewKeyDown(object sender, KeyEventArgs e)
    {
        bool ctrl = (Keyboard.Modifiers & ModifierKeys.Control) == ModifierKeys.Control;

        if (e.Key == Key.N && ctrl)
        {
            e.Handled = true;
            AddShortcut();
            return;
        }

        if (e.Key == Key.Q && ctrl)
        {
            e.Handled = true;
            ExitApp();
            return;
        }

        if (e.Key == Key.OemComma && ctrl)
        {
            e.Handled = true;
            OpenSettings();
            return;
        }

        if (e.Key == Key.F && ctrl)
        {
            e.Handled = true;
            SearchBox.Focus();
            SearchBox.SelectAll();
            return;
        }

        if (e.Key == Key.Escape)
        {
            e.Handled = true;
            if (SearchBox.Text.Length > 0) SearchBox.Text = "";
            else Hide();
        }
    }

    private void OnSourceInitialized(object sender, EventArgs e)
    {
        try
        {
            _hook = new KeyboardHook();
            _hook.DoubleCtrl += OnDoubleCtrl;

            if (!_hook.IsInstalled)
            {
                MessageBox.Show("全局热键（双击 Ctrl）注册失败，窗口本身仍可正常使用。" + Environment.NewLine
                                + "可以稍后右键托盘图标选「重新注册全局热键」再试一次。", "TT酱",
                    MessageBoxButton.OK, MessageBoxImage.Warning);
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("安装全局热键失败：" + ex.Message);
        }

        SetupTray();
    }

    private void SetupTray()
    {
        try
        {
            _tray = new TrayIcon();
            _tray.ShowRequested = () => Dispatcher.BeginInvoke(new Action(ShowFromTray));
            _tray.AddRequested = () => Dispatcher.BeginInvoke(new Action(() =>
            {
                ShowFromTray();
                AddShortcut();
            }));
            _tray.SettingsRequested = () => Dispatcher.BeginInvoke(new Action(() =>
            {
                ShowFromTray();
                OpenSettings();
            }));
            _tray.ExportRequested = () => Dispatcher.BeginInvoke(new Action(() =>
            {
                ShowFromTray();
                ExportProfile();
            }));
            _tray.ImportRequested = () => Dispatcher.BeginInvoke(new Action(() =>
            {
                ShowFromTray();
                ImportProfile();
            }));
            _tray.HookRepairRequested = () => Dispatcher.BeginInvoke(new Action(RepairHook));
            _tray.ExitRequested = () => Dispatcher.BeginInvoke(new Action(ExitApp));
        }
        catch (Exception ex)
        {
            ConfigStore.Log("创建托盘图标失败：" + ex.Message);
        }
    }

    private void OnClosing(object sender, CancelEventArgs e)
    {
        if (_exiting) return;

        // 关闭按钮只是收进托盘，程序继续驻留，双击 Ctrl 仍能唤起。
        e.Cancel = true;
        Hide();
        if (_tray != null) _tray.ShowHideHint();
    }

    private void ExitApp()
    {
        if (_exiting) return;
        _exiting = true;

        // 静默启动时窗口可能从未显示过，Close 不一定触发 Closed；
        // 显式收尾后直接退出应用，托盘菜单的「退出」在任何启动方式下都可靠。
        try { Close(); }
        catch { }
        Application.Current.Shutdown();
    }

    /// <summary>从托盘或第二个实例唤回窗口。</summary>
    public void ShowFromTray()
    {
        if (_exiting) return;

        Show();
        if (WindowState == WindowState.Minimized) WindowState = WindowState.Normal;
        BringToFront();
    }

    private void OnClosed(object sender, EventArgs e)
    {
        _exiting = true;
        _tileGeneration++;
        CancelSearch();

        try
        {
            if (_hook != null) _hook.Dispose();
        }
        catch
        {
            // 退出阶段忽略
        }

        try
        {
            if (_tray != null) _tray.Dispose();
        }
        catch
        {
            // 退出阶段忽略
        }
    }

    // ---------------------------------------------------------------- 数据
    private void Reload()
    {
        // 只读一次配置文件，快捷方式与设置一起拿到
        ConfigStore.Snapshot snapshot = ConfigStore.LoadAll();

        _items.Clear();
        _items.AddRange(snapshot.Items);
        _settings = snapshot.Settings;
        _searchOptions = SearchOptions.FromSettings(_settings);
        RefreshSortBar();

        RenderTiles();
        UpdateHint();
        UpdateStatus();
    }

    /// <summary>已有分类，用于新增/编辑对话框的分类下拉。</summary>
    private List<string> CurrentCategories()
    {
        var list = new List<string>();
        foreach (var it in _items)
        {
            string c = it.CategoryOrDefault;
            if (!list.Contains(c)) list.Add(c);
        }
        list.Sort(StringComparer.CurrentCulture);
        return list;
    }

    private void SetMode(BodyMode mode)
    {
        TileScroll.Visibility = mode == BodyMode.Shortcuts ? Visibility.Visible : Visibility.Collapsed;
        ResultArea.Visibility = mode == BodyMode.Files ? Visibility.Visible : Visibility.Collapsed;
        DxPanel.Visibility = mode == BodyMode.Dx ? Visibility.Visible : Visibility.Collapsed;
        SortBar.Visibility = mode == BodyMode.Files ? Visibility.Visible : Visibility.Collapsed;
    }

    // ---------------------------------------------------------------- 结果排序
    private void RefreshSortBar()
    {
        SearchOptions o = _searchOptions;
        string arrow = o.Ascending ? " ↑" : " ↓";

        SortName.Text = "名称" + (o.SortKey == SearchSortKey.Name ? arrow : "");
        SortType.Text = "类型" + (o.SortKey == SearchSortKey.Type ? arrow : "");
        SortModified.Text = "修改时间" + (o.SortKey == SearchSortKey.DateModified ? arrow : "");

        SortName.Foreground = o.SortKey == SearchSortKey.Name ? AccentFg : MutedFg;
        SortType.Foreground = o.SortKey == SearchSortKey.Type ? AccentFg : MutedFg;
        SortModified.Foreground = o.SortKey == SearchSortKey.DateModified ? AccentFg : MutedFg;

        SortName.FontWeight = o.SortKey == SearchSortKey.Name ? FontWeights.SemiBold : FontWeights.Normal;
        SortType.FontWeight = o.SortKey == SearchSortKey.Type ? FontWeights.SemiBold : FontWeights.Normal;
        SortModified.FontWeight = o.SortKey == SearchSortKey.DateModified ? FontWeights.SemiBold : FontWeights.Normal;

        MatchPathToggle.Text = o.MatchFullPath ? "含路径 ✓" : "含路径";
        MatchPathToggle.Foreground = o.MatchFullPath ? AccentFg : MutedFg;
    }

    private void SortName_Click(object sender, MouseButtonEventArgs e)
    {
        ChangeSort(SearchSortKey.Name);
    }

    private void SortType_Click(object sender, MouseButtonEventArgs e)
    {
        ChangeSort(SearchSortKey.Type);
    }

    private void SortModified_Click(object sender, MouseButtonEventArgs e)
    {
        ChangeSort(SearchSortKey.DateModified);
    }

    private void MatchPathToggle_Click(object sender, MouseButtonEventArgs e)
    {
        _searchOptions.MatchFullPath = !_searchOptions.MatchFullPath;
        PersistSearchOptions();
        RefreshSortBar();
        RefreshBody();
    }

    /// <summary>点同一个字段切换升降序，换字段时回到升序。</summary>
    private void ChangeSort(SearchSortKey key)
    {
        if (_searchOptions.SortKey == key)
        {
            _searchOptions.Ascending = !_searchOptions.Ascending;
        }
        else
        {
            _searchOptions.SortKey = key;
            _searchOptions.Ascending = true;
        }

        PersistSearchOptions();
        RefreshSortBar();
        RefreshBody();
    }

    private void PersistSearchOptions()
    {
        _settings.SearchSortKey = (int)_searchOptions.SortKey;
        _settings.SearchSortAscending = _searchOptions.Ascending ? 1 : 0;
        _settings.SearchMatchFullPath = _searchOptions.MatchFullPath ? 1 : 0;
        ReportSaveResult(ConfigStore.SaveSettings(_settings));
    }

    private void UpdateStatus()
    {
        // 提示类消息（保存失败、搜索错误）优先于常规说明
        if (!string.IsNullOrEmpty(_statusMessage))
        {
            StatusText.Text = _statusMessage;
            return;
        }

        if (!string.IsNullOrEmpty(_storageMessage))
        {
            StatusText.Text = _storageMessage;
            return;
        }

        if (SearchBox.Text.Trim().Length == 0)
        {
            StatusText.Text = _items.Count + " 个快捷方式 · 双击 Ctrl 唤起 · Ctrl+N 添加 · Ctrl+Q 退出";
            return;
        }

        if (DxPanel.Visibility == Visibility.Visible)
        {
            StatusText.Text = "数字转中文大写 · 点击结果复制 · Esc 清空输入";
            return;
        }

        if (_searching)
        {
            StatusText.Text = "正在搜索…";
            return;
        }

        StatusText.Text = _resultTotal > 0
            ? "共 " + _resultTotal.ToString("N0") + " 个结果 · 单击打开 · 右键更多 · Esc 清空"
            : "没有找到匹配的文件";
    }

    // ---------------------------------------------------------------- 设置
    private void Settings_Click(object sender, RoutedEventArgs e)
    {
        OpenSettings();
    }

    private void OpenSettings()
    {
        CancelSearch();

        SettingsWindow dlg = null;
        dlg = new SettingsWindow(_settings,
            () => ExportProfile(dlg),
            () => ImportProfile(dlg))
        {
            Owner = this
        };

        if (dlg.ShowDialog() != true || dlg.Result == null)
        {
            RefreshBody();
            return;
        }

        _settings = dlg.Result;
        _searchOptions = SearchOptions.FromSettings(_settings);
        RefreshSortBar();
        ReportSaveResult(ConfigStore.SaveSettings(_settings));
        RefreshBody();
    }

    /// <summary>保存失败时状态栏提示一次，并弹一次说明，避免改动悄悄丢失。</summary>
    private void ReportSaveResult(bool ok)
    {
        if (ok)
        {
            _writeFailedReported = false;
            _storageMessage = null;
            return;
        }

        _storageMessage = "配置没能写入磁盘：" + ConfigStore.BaseDir;
        UpdateStatus();
        ConfigStore.Log("配置保存失败，目标目录：" + ConfigStore.BaseDir);

        if (_writeFailedReported) return;
        _writeFailedReported = true;

        MessageBox.Show(
            "改动没能写入磁盘，重启后会回到上一次保存的状态。" + Environment.NewLine + Environment.NewLine
            + "配置目录：" + ConfigStore.BaseDir + Environment.NewLine
            + "常见原因：程序放在只读目录、磁盘已满、或被安全软件拦住了写入。",
            "保存失败", MessageBoxButton.OK, MessageBoxImage.Warning);
    }

    // ---------------------------------------------------------------- 热键
    private void OnDoubleCtrl()
    {
        Dispatcher.BeginInvoke(new Action(ToggleWindow));
    }

    private void ToggleWindow()
    {
        if (WindowState == WindowState.Minimized)
        {
            WindowState = WindowState.Normal;
            BringToFront();
            return;
        }

        if (IsForeground())
        {
            WindowState = WindowState.Minimized;
            return;
        }

        BringToFront();
    }

    private bool IsForeground()
    {
        IntPtr hwnd = new WindowInteropHelper(this).Handle;
        return hwnd != IntPtr.Zero && GetForegroundWindow() == hwnd;
    }

    private void BringToFront()
    {
        Show();
        if (WindowState == WindowState.Minimized) WindowState = WindowState.Normal;

        IntPtr hwnd = new WindowInteropHelper(this).Handle;
        ForceForeground(hwnd);

        Topmost = true;
        Topmost = false;
        Activate();
        Focus();

        // 每次唤起都回到干净状态：清空搜索框，回到图标网格
        if (SearchBox.Text.Length > 0) SearchBox.Text = "";
        SearchBox.Focus();
    }

    /// <summary>
    /// 后台进程直接调 SetForegroundWindow 常被前台锁拦住（窗口弹出来了但键盘还在别的程序里）。
    /// 先接上前台线程的输入队列再抢，抢不到就补一次 Alt 轻敲，让系统认为本进程有输入焦点。
    /// </summary>
    private static void ForceForeground(IntPtr hwnd)
    {
        if (hwnd == IntPtr.Zero) return;

        try
        {
            if (GetForegroundWindow() == hwnd) return;

            uint owner;
            uint fgThread = GetWindowThreadProcessId(GetForegroundWindow(), out owner);
            uint me = GetCurrentThreadId();
            bool attached = false;

            try
            {
                if (fgThread != 0 && fgThread != me) attached = AttachThreadInput(me, fgThread, true);

                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
            }
            finally
            {
                if (attached) AttachThreadInput(me, fgThread, false);
            }

            if (GetForegroundWindow() == hwnd) return;

            // 仍然没抢到：模拟一次 Alt 按键，前台锁会放行紧随其后的 SetForegroundWindow
            keybd_event(0x12, 0, 0, UIntPtr.Zero);
            keybd_event(0x12, 0, 2, UIntPtr.Zero);
            SetForegroundWindow(hwnd);
        }
        catch
        {
            // 抢前台失败不影响窗口已经显示出来
        }
    }

    /// <summary>托盘菜单入口：钩子被安全软件摘掉后重新注册，不用重启程序。</summary>
    private void RepairHook()
    {
        ShowFromTray();

        bool ok;
        try
        {
            if (_hook == null)
            {
                _hook = new KeyboardHook();
                _hook.DoubleCtrl += OnDoubleCtrl;
                ok = _hook.IsInstalled;
            }
            else
            {
                ok = _hook.Reinstall();
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("重新注册全局热键失败：" + ex.Message);
            ok = false;
        }

        MessageBox.Show(
            ok
                ? "全局热键已重新注册，双击 Ctrl 可以唤起窗口。"
                : "重新注册失败，可能被安全软件拦截。可以退出程序后重新打开再试。",
            "全局热键", MessageBoxButton.OK,
            ok ? MessageBoxImage.Information : MessageBoxImage.Warning);
    }
}
