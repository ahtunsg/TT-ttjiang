using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Threading;
using TTJiang.Services;

namespace TTJiang;

public partial class App : Application
{
    private const string MutexName = @"Local\TTJiang_SingleInstance_v1";
    private const string ActivateEventName = @"Local\TTJiang_Activate_v1";

    private Mutex _mutex;
    private EventWaitHandle _activateEvent;
    private Thread _watcher;
    private bool _fatalReported;

    protected override void OnStartup(StartupEventArgs e)
    {
        InstallCrashHandlers();

        if (!IsSupportedWindows())
        {
            MessageBox.Show(
                "TT酱仅支持 Windows 10 与 Windows 11。" + Environment.NewLine +
                "当前系统版本：" + Environment.OSVersion.Version,
                "不支持的系统", MessageBoxButton.OK, MessageBoxImage.Error);
            Shutdown();
            return;
        }

        bool createdNew;
        _mutex = new Mutex(true, MutexName, out createdNew);

        if (!createdNew)
        {
            // 已有实例在跑：唤醒它，不再开第二个窗口，也不重复装钩子。
            bool signaled = SignalExistingInstance();
            if (!signaled)
            {
                MessageBox.Show("TT酱已经在运行了，双击 Ctrl 即可唤起窗口。", "TT酱",
                    MessageBoxButton.OK, MessageBoxImage.Information);
            }

            Shutdown();
            return;
        }

        StartActivationWatcher();

        base.OnStartup(e);

        var win = new MainWindow();
        MainWindow = win;
        win.Show();

        StartSearchEngine();
    }

    /// <summary>
    /// 释放内置 Everything、按需安装索引服务、拉起搜索实例。
    /// 放到窗口显示之后做，首次解压和 UAC 都不会卡住界面。
    /// </summary>
    private void StartSearchEngine()
    {
        Task.Run(() =>
        {
            if (!EverythingHost.EnsureTools())
            {
                Dispatcher.BeginInvoke(new Action(() =>
                    MessageBox.Show(EverythingHost.LastError, "TT酱",
                    MessageBoxButton.OK, MessageBoxImage.Warning)));
                return;
            }

            foreach (string dir in new[] { ConfigStore.BaseDir, EverythingHost.ToolsDir })
            {
                if (!ConfigStore.CanWrite(dir))
                {
                    Dispatcher.BeginInvoke(new Action(() =>
                        MessageBox.Show("当前目录没有写入权限，TT酱 需要把配置和索引写在 exe 同级目录。" + Environment.NewLine +
                                        "请把整个文件夹移到有写入权限的位置（例如桌面或 D 盘）后重新打开。",
                            "TT酱", MessageBoxButton.OK, MessageBoxImage.Error)));
                    return;
                }
            }

            bool installed = EverythingHost.IsSearchReady();

            Dispatcher.BeginInvoke(new Action(() =>
            {
                if (installed)
                {
                    Task.Run(() => EverythingHost.StartClient(15000));
                    return;
                }

                MessageBoxResult answer = MessageBox.Show(
                    "首次使用需要安装一次索引服务，用来读取全盘文件索引。" + Environment.NewLine +
                    "系统会弹出管理员授权窗口，同意即可；之后每次启动都不再询问。" + Environment.NewLine + Environment.NewLine +
                    "现在安装？以后也可以在设置里安装。",
                    "TT酱 · 首次使用", MessageBoxButton.OKCancel, MessageBoxImage.Information);

                if (answer != MessageBoxResult.OK) return;

                Task.Run(() =>
                {
                    string error;
                    bool ok = EverythingHost.InstallService(out error);
                    ConfigStore.Log(ok ? "索引服务安装成功" : "索引服务安装失败：" + error);
                    if (ok) EverythingHost.StartClient(15000);
                });
            }));
        });
    }

    // ---------------------------------------------------------------- 异常兜底

    /// <summary>
    /// 这是常驻托盘的程序：任何一个漏网的异常让进程静默退出，用户只会看到
    /// 托盘图标消失、双击 Ctrl 失灵，日志里还什么都没有。所以三条通道都要接住。
    /// </summary>
    private void InstallCrashHandlers()
    {
        DispatcherUnhandledException += OnDispatcherUnhandledException;
        AppDomain.CurrentDomain.UnhandledException += OnDomainUnhandledException;
        TaskScheduler.UnobservedTaskException += OnUnobservedTaskException;
    }

    private void OnDispatcherUnhandledException(object sender, DispatcherUnhandledExceptionEventArgs e)
    {
        ConfigStore.Log("界面线程未处理异常：" + Describe(e.Exception));
        e.Handled = true;

        if (_fatalReported) return;
        _fatalReported = true;

        try
        {
            MessageBox.Show(
                "程序遇到了一个内部错误，已记录到日志，可以继续使用。" + Environment.NewLine +
                "如果界面变得不正常，请右键托盘图标退出后重新打开。" + Environment.NewLine + Environment.NewLine +
                e.Exception.Message,
                "TT酱", MessageBoxButton.OK, MessageBoxImage.Warning);
        }
        catch
        {
            // 提示失败也不能让它把进程带走
        }
    }

    private static void OnDomainUnhandledException(object sender, UnhandledExceptionEventArgs e)
    {
        var ex = e.ExceptionObject as Exception;
        ConfigStore.Log("未处理异常（"
                        + (e.IsTerminating ? "将退出" : "继续运行") + "）：" + Describe(ex));
    }

    private static void OnUnobservedTaskException(object sender, UnobservedTaskExceptionEventArgs e)
    {
        ConfigStore.Log("后台任务异常：" + Describe(e.Exception));
        e.SetObserved();
    }

    private static string Describe(Exception ex)
    {
        if (ex == null) return "(null)";
        return ex.GetType().Name + ": " + ex.Message + Environment.NewLine + ex.StackTrace;
    }

    // ---------------------------------------------------------------- 单实例

    /// <summary>只面向 Windows 10 / 11：两者主版本号均为 10。</summary>
    private static bool IsSupportedWindows()
    {
        Version v = Environment.OSVersion.Version;
        return v.Major >= 10;
    }

    private void StartActivationWatcher()
    {
        try
        {
            _activateEvent = new EventWaitHandle(false, EventResetMode.AutoReset, ActivateEventName);
            _watcher = new Thread(WatchActivation) { IsBackground = true };
            _watcher.Start();
        }
        catch (Exception ex)
        {
            ConfigStore.Log("创建单实例激活通道失败：" + ex.Message);
        }
    }

    private void WatchActivation()
    {
        while (true)
        {
            bool signaled;
            try
            {
                signaled = _activateEvent.WaitOne();
            }
            catch
            {
                return;
            }

            if (!signaled) return;

            Dispatcher.BeginInvoke(new Action(() =>
            {
                var win = MainWindow as TTJiang.MainWindow;
                if (win != null) win.ShowFromTray();
            }));
        }
    }

    /// <summary>
    /// 连续双击图标时，第一个实例可能还没建好激活通道，这里等一小会儿再判定，
    /// 避免弹出「已经在运行」这种多余提示。
    /// </summary>
    private static bool SignalExistingInstance()
    {
        for (int attempt = 0; attempt < 24; attempt++)
        {
            try
            {
                using (EventWaitHandle handle = EventWaitHandle.OpenExisting(ActivateEventName))
                {
                    handle.Set();
                    return true;
                }
            }
            catch (WaitHandleCannotBeOpenedException)
            {
                // 对方还没建好，稍后重试
            }
            catch
            {
                // 权限等问题，直接放弃等待
                return false;
            }

            Thread.Sleep(60);
        }

        return false;
    }

    protected override void OnExit(ExitEventArgs e)
    {
        // 先断开 SDK3 长连接并释放 DLL，否则删目录时 DLL 仍被进程占用
        try
        {
            EverythingIpc.Close();
        }
        catch
        {
            // 退出阶段忽略
        }

        // 再收掉自己拉起来的 Everything 实例，避免退出后还有进程占着程序目录
        try
        {
            EverythingHost.StopClient();
        }
        catch
        {
            // 退出阶段忽略
        }

        try
        {
            if (_activateEvent != null)
            {
                _activateEvent.Set();
                _activateEvent.Dispose();
                _activateEvent = null;
            }
        }
        catch
        {
            // 退出阶段忽略
        }

        try
        {
            if (_mutex != null)
            {
                _mutex.ReleaseMutex();
                _mutex.Dispose();
            }
        }
        catch
        {
            // 退出阶段忽略
        }

        base.OnExit(e);
    }
}
