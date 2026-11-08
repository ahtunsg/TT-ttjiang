using System.Threading;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using TTJiang.Models;
using TTJiang.Services;

namespace TTJiang;

public partial class SettingsWindow : Window
{
    private static readonly Brush MutedFg = Frozen(0x88, 0x88, 0x88);
    private static readonly Brush ErrorFg = Frozen(0xE9, 0x45, 0x60);
    private static readonly Brush OkFg = Frozen(0x9F, 0xB4, 0xD8);

    private readonly Action _exportAction;
    private readonly Action _importAction;

    public AppSettings Result { get; private set; }

    public SettingsWindow(AppSettings current, Action exportAction, Action importAction)
    {
        InitializeComponent();

        _exportAction = exportAction;
        _importAction = importAction;

        // 保留调用方传进来的全部设置，保存时只覆盖本窗口负责的字段
        Result = (current ?? new AppSettings()).Clone();

        Loaded += (sender, e) => RefreshServiceState();

        PreviewKeyDown += OnPreviewKeyDown;
    }

    private static Brush Frozen(byte r, byte g, byte b)
    {
        var brush = new SolidColorBrush(Color.FromRgb(r, g, b));
        brush.Freeze();
        return brush;
    }

    private void OnPreviewKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape)
        {
            e.Handled = true;
            DialogResult = false;
        }
        else if (e.Key == Key.Enter)
        {
            e.Handled = true;
            Save_Click(sender, e);
        }
    }

    private void RefreshServiceState()
    {
        bool ready = EverythingHost.IsSearchReady();
        bool ours = EverythingHost.FindServiceName() != null;
        bool clientRunning = EverythingHost.IsClientRunning();

        ServiceText.Text = "索引服务：" + (ready ? (ours ? "已安装" : "共用系统已有") : "未安装");
        ServiceText.Foreground = ready ? OkFg : ErrorFg;

        InstallButton.IsEnabled = !ready;
        UninstallButton.IsEnabled = ours;

        if (!ready)
        {
            SetStatus("装好索引服务后才能全盘搜索；安装过程会弹一次管理员授权。", true);
        }
        else if (!ours)
        {
            SetStatus("检测到系统里已有 Everything 索引服务，TT酱 直接共用，不会改动它。", false);
        }
        else if (!clientRunning)
        {
            SetStatus("索引服务已就绪，搜索实例会在第一次搜索时自动启动。", false);
        }
        else
        {
            SetStatus("索引服务已就绪，搜索实例正在运行。", false);
        }
    }

    private async void InstallService_Click(object sender, RoutedEventArgs e)
    {
        await RunServiceChange(true);
    }

    private async void UninstallService_Click(object sender, RoutedEventArgs e)
    {
        MessageBoxResult confirm = MessageBox.Show(
            "卸载索引服务后，文件搜索会立即不可用，全盘索引也会重建。" + Environment.NewLine +
            "如果你只是想删掉整个 TT酱 文件夹，卸载服务就是清理的最后一步。" + Environment.NewLine + Environment.NewLine +
            "继续卸载？",
            "卸载索引服务", MessageBoxButton.OKCancel, MessageBoxImage.Warning);

        if (confirm != MessageBoxResult.OK) return;

        await RunServiceChange(false);
    }

    private async Task RunServiceChange(bool install)
    {
        if (!EverythingHost.EnsureTools())
        {
            SetStatus(EverythingHost.LastError, true);
            return;
        }

        InstallButton.IsEnabled = false;
        UninstallButton.IsEnabled = false;
        TestButton.IsEnabled = false;
        SetStatus(install ? "等待管理员授权…" : "正在卸载…", false);

        try
        {
            string failure = await Task.Run(() =>
            {
                if (install)
                {
                    string error;
                    bool done = EverythingHost.InstallService(out error);
                    if (done) EverythingHost.StartClient(5000);
                    return done ? null : error;
                }
                else
                {
                    EverythingHost.StopClient();
                    string error;
                    bool done = EverythingHost.UninstallService(out error);
                    return done ? null : error;
                }
            });

            SetStatus(failure ?? (install ? "索引服务已安装" : "索引服务已卸载"), failure != null);
        }
        catch (Exception ex)
        {
            ConfigStore.Log("服务操作异常：" + ex.Message);
            SetStatus("操作失败：" + ex.Message, true);
        }
        finally
        {
            TestButton.IsEnabled = true;
            RefreshServiceState();
        }
    }

    private async void Test_Click(object sender, RoutedEventArgs e)
    {
        TestButton.IsEnabled = false;
        SetStatus("正在测试…", false);

        try
        {
            string error = await Task.Run(() =>
            {
                if (!EverythingHost.EnsureTools()) return EverythingHost.LastError;
                if (!EverythingHost.StartClient()) return "内置搜索实例没有起来：" + EverythingHost.LastError;
                return null;
            });

            if (error != null)
            {
                SetStatus(error, true);
                return;
            }

            SearchOutcome outcome = await EverythingSearch.QueryAsync("exe", 5, CancellationToken.None);
            if (!string.IsNullOrEmpty(outcome.Error))
            {
                SetStatus(outcome.Error, true);
            }
            else
            {
                SetStatus("连接正常，返回 " + outcome.Hits.Count + " 条结果", false);
            }
        }
        catch (Exception ex)
        {
            // 测试按钮不能把整个程序带崩
            ConfigStore.Log("测试搜索异常：" + ex.Message);
            SetStatus("测试失败：" + ex.Message, true);
        }
        finally
        {
            TestButton.IsEnabled = true;
        }
    }

    private void Save_Click(object sender, RoutedEventArgs e)
    {
        // 结果条数不再设上限，本窗口只需原样带回既有设置
        DialogResult = true;
    }

    private void Cancel_Click(object sender, RoutedEventArgs e)
    {
        DialogResult = false;
    }

    private void Export_Click(object sender, RoutedEventArgs e)
    {
        if (_exportAction != null) _exportAction();
    }

    private void Import_Click(object sender, RoutedEventArgs e)
    {
        if (_importAction != null) _importAction();
    }

    private void SetStatus(string text, bool error)
    {
        StatusText.Text = text;
        StatusText.Foreground = error ? ErrorFg : OkFg;
        if (!error && text.Length == 0) StatusText.Foreground = MutedFg;
    }
}
