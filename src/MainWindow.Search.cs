using System;
using System.Text.RegularExpressions;
using System.Threading;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using TTJiang.Models;
using TTJiang.Services;

namespace TTJiang;

public partial class MainWindow
{
    private const int SearchDebounceMs = 300;

    /// <summary>当前查询的虚拟列表数据源；换查询或换排序时整体替换。</summary>
    private ResultSource _resultSource;
    /// <summary>全库命中数，用于状态栏与虚拟列表的滚动范围。</summary>
    private int _resultTotal;

    private static readonly Regex DxPattern = new(@"^dx\s+(.*)$", RegexOptions.IgnoreCase | RegexOptions.Singleline);
    private static readonly Regex DxOnly = new(@"^dx$", RegexOptions.IgnoreCase);

    // ---------------------------------------------------------------- 输入

    /// <summary>清空并作废当前结果源，避免旧查询继续取页后串进新结果。</summary>
    private void ClearResults()
    {
        if (_resultSource != null) _resultSource.MarkStale();
        _resultSource = null;
        ResultList.ItemsSource = null;
        _hits.Clear();
        _resultTotal = 0;
    }

    private void UpdateHint()
    {
        SearchHint.Visibility = SearchBox.Text.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    private void SearchBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        RefreshBody();
    }

    private void RefreshBody()
    {
        UpdateHint();

        string payload;
        if (TryGetDxPayload(out payload))
        {
            CancelSearch();
            ClearResults();
            _statusMessage = null;
            SetMode(BodyMode.Dx);
            RenderDx(payload);
            UpdateStatus();
            return;
        }

        _lastDxResult = null;
        string query = SearchBox.Text.Trim();

        if (query.Length == 0)
        {
            CancelSearch();
            ClearResults();
            _statusMessage = null;
            SetMode(BodyMode.Shortcuts);
            RenderTiles();
            UpdateStatus();
            return;
        }

        SetMode(BodyMode.Files);
        StartSearch(query);
    }

    private bool TryGetDxPayload(out string payload)
    {
        payload = "";
        string raw = SearchBox.Text.Trim();
        if (raw.Length == 0) return false;

        Match m = DxPattern.Match(raw);
        if (m.Success)
        {
            payload = m.Groups[1].Value.Trim();
            return true;
        }

        return DxOnly.IsMatch(raw);
    }

    // ---------------------------------------------------------------- 数字大写
    private void RenderDx(string payload)
    {
        if (payload.Length == 0)
        {
            DxResult.Text = "继续输入数字，如 dx 12345.67";
            DxResult.Foreground = MutedFg;
            DxTip.Text = "";
            _lastDxResult = null;
            return;
        }

        try
        {
            string text = NumberToChinese.ToChinese(payload);
            DxResult.Text = text;
            DxResult.Foreground = AccentFg;
            DxTip.Text = "点击结果复制 · 按 Esc 退出大写模式";
            _lastDxResult = text;
        }
        catch
        {
            DxResult.Text = "无法识别为数字";
            DxResult.Foreground = MutedFg;
            DxTip.Text = "";
            _lastDxResult = null;
        }
    }

    private async void DxResult_Click(object sender, MouseButtonEventArgs e)
    {
        if (string.IsNullOrEmpty(_lastDxResult)) return;

        DxTip.Text = "正在复制…";
        bool ok = await CopyTextAsync(_lastDxResult);
        DxTip.Text = ok ? "已复制到剪贴板" : "剪贴板被其他程序占用，请再试一次";
    }

    // ---------------------------------------------------------------- 搜索
    private void StartSearch(string query)
    {
        CancelSearch();
        _searchSeq++;
        int seq = _searchSeq;

        var cts = new CancellationTokenSource();
        _searchCts = cts;
        _searching = true;
        _statusMessage = null;

        // 连续输入时保留上一次结果，只在没有结果可显示时才提示搜索中，避免列表闪烁
        if (_hits.Count == 0) ShowSearchMessage("正在搜索…", "「" + query + "」", false);
        UpdateStatus();

        int pageSize = ResultSource.PageSize;
        string capturedQuery = query;

        // 排序可能在查询期间被用户切换，这里取快照，避免后台任务读到中途变化的状态
        var options = new SearchOptions
        {
            SortKey = _searchOptions.SortKey,
            Ascending = _searchOptions.Ascending,
            MatchFullPath = _searchOptions.MatchFullPath
        };

        _ = Task.Run(async () =>
        {
            SearchOutcome outcome;
            try
            {
                await Task.Delay(SearchDebounceMs, cts.Token).ConfigureAwait(false);

                if (!EverythingHost.EnsureTools())
                {
                    outcome = new SearchOutcome { Error = EverythingHost.LastError };
                }
                else if (!EverythingHost.StartClient())
                {
                    outcome = new SearchOutcome { Error = BuildNoInstanceHint() };
                }
                else
                {
                    outcome = await EverythingSearch
                        .QueryAsync(capturedQuery, 0, pageSize, options, cts.Token)
                        .ConfigureAwait(false);
                }
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (Exception ex)
            {
                // 兜底：任何漏网异常都要落到界面，否则会永远停在「正在搜索…」
                ConfigStore.Log("搜索任务异常：" + ex.Message + Environment.NewLine + ex.StackTrace);
                outcome = new SearchOutcome { Error = "搜索出错：" + ex.Message };
            }

            if (cts.Token.IsCancellationRequested) return;

            _ = Dispatcher.BeginInvoke(new Action(() =>
            {
                if (seq != _searchSeq) return;

                _searching = false;

                if (!string.IsNullOrEmpty(outcome.Error))
                {
                    ClearResults();
                    _statusMessage = outcome.Error;
                    ShowSearchMessage("搜索失败", outcome.Error, true);
                    UpdateStatus();
                    return;
                }

                if (_resultSource != null) _resultSource.MarkStale();
                _hits.Clear();
                _hits.AddRange(outcome.Hits);
                _resultTotal = outcome.Total > 0 ? outcome.Total : outcome.Hits.Count;
                _resultSource = new ResultSource(capturedQuery, options, _resultTotal, outcome);
                _statusMessage = null;
                RenderHits(capturedQuery);
                UpdateStatus();
            }));
        });
    }

    private void CancelSearch()
    {
        _searching = false;

        var cts = _searchCts;
        _searchCts = null;
        if (cts == null) return;

        try
        {
            cts.Cancel();
        }
        catch
        {
            // 已释放或已取消
        }
    }

    private string BuildNoInstanceHint()
    {
        if (!EverythingHost.IsSearchReady())
        {
            return "文件搜索依赖内置的 Everything 索引服务，目前还没有安装。"
                   + "打开设置（右下角齿轮）→「安装索引服务」，首次会请求一次管理员授权；"
                   + "装好之后每次启动都不再需要授权。";
        }

        return "内置搜索实例没有启动。稍等几秒会自动重试；一直不行就在托盘菜单里退出后重新打开 TT酱。";
    }

    private void ShowSearchMessage(string title, string detail, bool withSettingsButton)
    {
        MessageStack.Children.Clear();
        ResultList.Visibility = Visibility.Collapsed;
        MessagePanel.Visibility = Visibility.Visible;

        var stack = new StackPanel
        {
            Margin = new Thickness(6, 16, 6, 0),
            MaxWidth = 560,
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top
        };

        stack.Children.Add(new TextBlock
        {
            Text = title,
            FontSize = 14,
            Foreground = TextFg
        });

        if (!string.IsNullOrWhiteSpace(detail))
        {
            stack.Children.Add(new TextBlock
            {
                Text = detail,
                FontSize = 12,
                Foreground = MutedFg,
                TextWrapping = TextWrapping.Wrap,
                LineHeight = 20,
                Margin = new Thickness(0, 8, 0, 0)
            });
        }

        if (withSettingsButton)
        {
            var button = new Button
            {
                Content = "打开设置",
                Style = (Style)FindResource("FlatButton"),
                HorizontalAlignment = HorizontalAlignment.Left,
                Margin = new Thickness(0, 14, 0, 0),
                Padding = new Thickness(16, 6, 16, 6)
            };
            button.Click += (s, e) => OpenSettings();
            stack.Children.Add(button);
        }

        MessageStack.Children.Add(stack);
    }

    private void RenderHits(string query)
    {
        if (_hits.Count == 0)
        {
            ShowSearchMessage("没有找到匹配的文件", "换一个关键字试试；Everything 只搜索它已索引的磁盘。", false);
            _statusMessage = "没有找到匹配「" + query + "」的文件";
            return;
        }

        // 只把数据源交给虚拟列表：WPF 仅为可见行建控件，Count 用全库命中数，
        // 所以滚动范围与 Everything 一致，内存与结果总数无关。
        MessagePanel.Visibility = Visibility.Collapsed;
        MessageStack.Children.Clear();
        ResultList.Visibility = Visibility.Visible;
        ResultList.ItemsSource = _resultSource;
        _statusMessage = null;
    }

    /// <summary>从模板里的 Border.Tag 取当前行结果；容器回收后 Tag 会随 DataContext 更新。</summary>
    private static SearchHit HitFromEvent(DependencyObject source, out FrameworkElement target)
    {
        target = null;
        DependencyObject node = source;
        while (node != null)
        {
            var fe = node as FrameworkElement;
            if (fe != null && fe.Tag is SearchHit)
            {
                target = fe;
                return (SearchHit)fe.Tag;
            }
            node = VisualTreeHelper.GetParent(node);
        }
        return null;
    }

    private void ResultList_PreviewMouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        SearchHit hit = HitFromEvent(e.OriginalSource as DependencyObject, out _);
        if (hit == null) return;
        e.Handled = true;
        if (string.IsNullOrEmpty(hit.FullPath)) return;   // 取页失败的占位项不可打开
        OpenPath(hit.FullPath);
        Hide();
    }

    private void ResultList_PreviewMouseRightButtonUp(object sender, MouseButtonEventArgs e)
    {
        SearchHit hit = HitFromEvent(e.OriginalSource as DependencyObject, out FrameworkElement target);
        if (hit == null || string.IsNullOrEmpty(hit.FullPath)) return;
        e.Handled = true;
        ShowHitMenu(target, hit);
    }

    private void ShowHitMenu(FrameworkElement row, SearchHit hit)
    {
        var menu = new ContextMenu();
        menu.Items.Add(CreateMenuItem("打开", () =>
        {
            OpenPath(hit.FullPath);
            Hide();
        }));
        menu.Items.Add(CreateMenuItem("打开所在位置", () => OpenPathLocation(hit.FullPath)));
        menu.Items.Add(CreateMenuItem("复制完整路径", () => CopyText(hit.FullPath)));

        menu.PlacementTarget = row;
        menu.IsOpen = true;
    }

    private async void SearchBox_KeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Enter) return;

        if (!string.IsNullOrEmpty(_lastDxResult))
        {
            e.Handled = true;
            DxTip.Text = "正在复制…";
            bool ok = await CopyTextAsync(_lastDxResult);
            DxTip.Text = ok ? "已复制到剪贴板" : "剪贴板被其他程序占用，请再试一次";
            return;
        }

        if (ResultArea.Visibility == Visibility.Visible && _hits.Count > 0
            && !string.IsNullOrEmpty(_hits[0].FullPath))
        {
            e.Handled = true;
            OpenPath(_hits[0].FullPath);
            Hide();
        }
    }
}
