using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;
using TTJiang.Models;
using TTJiang.Services;

namespace TTJiang;

public partial class MainWindow
{
    // ---------------------------------------------------------------- 图标网格
    private void RenderTiles()
    {
        int gen = ++_tileGeneration;
        TilePanel.Children.Clear();

        foreach (var it in _items)
        {
            TilePanel.Children.Add(CreateTile(it, gen));
        }
    }

    private Border CreateTile(ShortcutItem item, int generation)
    {
        // 先放占位，图标在后台抽取完再回填，启动不再卡在 UI 线程上
        var iconHost = new Border
        {
            Width = 48,
            Height = 48,
            CornerRadius = new CornerRadius(8),
            Background = HoverBg,
            HorizontalAlignment = HorizontalAlignment.Center,
            Child = CreatePlaceholder(item.DisplayName)
        };

        var stack = new StackPanel
        {
            VerticalAlignment = VerticalAlignment.Center,
            HorizontalAlignment = HorizontalAlignment.Center
        };
        stack.Children.Add(iconHost);

        stack.Children.Add(new TextBlock
        {
            Text = item.DisplayName,
            Foreground = TextFg,
            FontSize = 12,
            TextWrapping = TextWrapping.NoWrap,
            TextTrimming = TextTrimming.CharacterEllipsis,
            TextAlignment = TextAlignment.Center,
            HorizontalAlignment = HorizontalAlignment.Center,
            Margin = new Thickness(4, 8, 4, 0),
            MaxWidth = 100
        });

        string tip = item.Path;
        if (!string.IsNullOrWhiteSpace(item.Args)) tip = tip + " " + item.Args;

        var card = new Border
        {
            Width = 110,
            Height = 108,
            Margin = new Thickness(0, 0, 2, 2),
            CornerRadius = new CornerRadius(4),
            Background = Brushes.Transparent,
            Child = stack,
            Cursor = Cursors.Hand,
            ToolTip = tip
        };

        card.MouseLeftButtonUp += (s, e) =>
        {
            e.Handled = true;
            OpenItem(item);
            Hide();
        };
        card.MouseEnter += (s, e) => card.Background = CardBg;
        card.MouseLeave += (s, e) => card.Background = Brushes.Transparent;
        card.MouseRightButtonUp += (s, e) =>
        {
            e.Handled = true;
            ShowTileMenu(card, item);
        };

        QueueIcon(item.Path, iconHost, generation);
        return card;
    }

    /// <summary>
    /// 后台抽取图标。IconLoader 返回的 BitmapSource 已 Freeze，可以跨线程使用；
    /// 并发用信号量压到 2，避免一次性重绘几百个条目把 CPU 占满。
    /// </summary>
    private void QueueIcon(string path, Border host, int generation)
    {
        if (string.IsNullOrWhiteSpace(path)) return;

        Dispatcher dispatcher = Dispatcher;

        _ = Task.Run(async () =>
        {
            ImageSource icon = null;

            try
            {
                await _iconSlots.WaitAsync().ConfigureAwait(false);
                try
                {
                    icon = IconLoader.Load(path);
                }
                finally
                {
                    _iconSlots.Release();
                }
            }
            catch (Exception ex)
            {
                ConfigStore.Log("加载图标失败：" + path + " -> " + ex.Message);
            }

            if (icon == null) return;

            _ = dispatcher.BeginInvoke(DispatcherPriority.Background, new Action(() =>
            {
                // 期间列表重绘过，这个结果已经过期
                if (generation != _tileGeneration) return;

                var image = new Image
                {
                    Source = icon,
                    Width = 48,
                    Height = 48,
                    Stretch = Stretch.Uniform,
                    HorizontalAlignment = HorizontalAlignment.Center
                };
                RenderOptions.SetBitmapScalingMode(image, BitmapScalingMode.HighQuality);
                host.Child = image;
            }));
        });
    }

    /// <summary>取不到图标时用首字占位。</summary>
    private static TextBlock CreatePlaceholder(string name)
    {
        string ch = string.IsNullOrEmpty(name) ? "?" : name.Substring(0, 1);

        return new TextBlock
        {
            Text = ch,
            FontSize = 20,
            Foreground = AccentFg,
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center
        };
    }

    private void AddTile_Click(object sender, MouseButtonEventArgs e)
    {
        e.Handled = true;
        AddShortcut();
    }

    private void ShowTileMenu(Border card, ShortcutItem item)
    {
        var menu = new ContextMenu();
        menu.Items.Add(CreateMenuItem("打开", () => OpenItem(item)));
        menu.Items.Add(CreateMenuItem("编辑", () => EditShortcut(item)));
        menu.Items.Add(CreateMenuItem("复制路径", () => CopyText(item.Path)));
        menu.Items.Add(new Separator());
        menu.Items.Add(CreateMenuItem("打开所在位置", () => OpenLocation(item)));
        menu.Items.Add(CreateMenuItem("删除", () => DeleteItem(item)));

        menu.PlacementTarget = card;
        menu.IsOpen = true;
    }

    private static MenuItem CreateMenuItem(string header, Action action)
    {
        var mi = new MenuItem { Header = header, Foreground = TextFg };
        mi.Click += (s, e) => action();
        return mi;
    }
}
