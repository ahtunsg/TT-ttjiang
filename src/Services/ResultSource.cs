using System;
using System.Collections;
using System.Collections.Generic;
using System.Threading;
using TTJiang.Models;

namespace TTJiang.Services;

/// <summary>
/// 虚拟列表的数据源。Count 是引擎返回的全库命中数，索引器只按页向 Everything 取数据。
/// WPF 的 VirtualizingStackPanel 只为可见行调用 IList 索引器，所以 40 万条结果
/// 也只传输真正看过的那几页：滚动范围与 Everything 一致，首屏代价却与总数无关。
///
/// 每次搜索、排序或匹配范围变化都整体替换本对象，页缓存随之失效；
/// 旧对象被 MarkStale 后不再发查询，避免连续输入时旧结果串进新列表。
/// </summary>
public sealed class ResultSource : IList
{
    /// <summary>每页条数。200 条时单页 IPC 实测约 0.2 ms，es.exe 输出远低于 4 MB 截断线。</summary>
    public const int PageSize = 200;

    /// <summary>缓存页数上限，长时间滚动也不会无限占内存。</summary>
    private const int MaxCachedPages = 16;

    private readonly string _query;
    private readonly SearchOptions _options;
    private readonly int _total;
    private readonly Dictionary<int, SearchHit[]> _pages = new Dictionary<int, SearchHit[]>();
    private readonly LinkedList<int> _lru = new LinkedList<int>();
    private readonly object _sync = new object();
    private volatile bool _stale;

    /// <summary>实际向引擎取过多少页，验证脚本用它确认没有发生全量枚举。</summary>
    public int LoadedPages { get; private set; }

    public ResultSource(string query, SearchOptions options, int total, SearchOutcome firstPage)
    {
        _query = query;
        _options = options ?? new SearchOptions();
        _total = Math.Max(0, total);

        // 首页已经随首屏查询拿到了，不重复请求
        if (firstPage != null && firstPage.Hits.Count > 0)
        {
            var hits = firstPage.Hits.ToArray();
            if (hits.Length < Math.Min(PageSize, _total))
                Array.Resize(ref hits, Math.Min(PageSize, _total));
            Store(0, hits);
        }
    }

    /// <summary>查询已被新输入取代；此后索引器只返回占位项，不再发起查询。</summary>
    public void MarkStale()
    {
        _stale = true;
    }

    public int Count { get { return _total; } }

    public object this[int index]
    {
        get
        {
            if (index < 0 || index >= _total) return null;
            if (_stale) return Placeholder();

            int page = index / PageSize;
            int within = index - page * PageSize;
            SearchHit[] hits;

            lock (_sync)
            {
                if (!_pages.TryGetValue(page, out hits))
                {
                    hits = LoadPage(page);
                    Store(page, hits);
                }
                else
                {
                    Touch(page);
                }
            }

            return within < hits.Length && hits[within] != null
                ? (object)hits[within]
                : Placeholder();
        }
        set { throw new NotSupportedException(); }
    }

    private SearchHit[] LoadPage(int page)
    {
        LoadedPages++;

        int offset = page * PageSize;
        int count = Math.Min(PageSize, _total - offset);
        if (count <= 0) return Array.Empty<SearchHit>();

        try
        {
            // QueryAsync 内部对回退链路都用了 ConfigureAwait(false)，
            // 这里在 UI 线程同步等待不会死锁；IPC 主路径是毫秒级。
            SearchOutcome outcome = EverythingSearch
                .QueryAsync(_query, offset, count, _options, CancellationToken.None)
                .GetAwaiter().GetResult();

            if (!outcome.Ok || outcome.Hits.Count == 0) return Array.Empty<SearchHit>();

            var hits = outcome.Hits.ToArray();
            if (hits.Length < count) Array.Resize(ref hits, count);
            return hits;
        }
        catch (Exception ex)
        {
            ConfigStore.Log("读取搜索结果第 " + page + " 页失败：" + ex.Message);
            return Array.Empty<SearchHit>();
        }
    }

    private void Store(int page, SearchHit[] hits)
    {
        _pages[page] = hits;
        _lru.AddLast(page);
        while (_lru.Count > MaxCachedPages)
        {
            int oldest = _lru.First.Value;
            _lru.RemoveFirst();
            _pages.Remove(oldest);
        }
    }

    private void Touch(int page)
    {
        LinkedListNode<int> node = _lru.Find(page);
        if (node == null) return;
        _lru.Remove(node);
        _lru.AddLast(page);
    }

    private static SearchHit Placeholder()
    {
        return new SearchHit
        {
            FullPath = "",
            Name = "（这一段暂时读不出来，滚动后可重试）",
            Directory = ""
        };
    }

    // ---------------------------------------------------------------- IList 样板
    // VirtualizingStackPanel 只用 Count 与索引器；下列成员按只读集合实现。
    public bool IsFixedSize { get { return true; } }
    public bool IsReadOnly { get { return true; } }
    public bool IsSynchronized { get { return false; } }
    public object SyncRoot { get { return _sync; } }

    public int Add(object value) { throw new NotSupportedException(); }
    public void Clear() { throw new NotSupportedException(); }
    public void Insert(int index, object value) { throw new NotSupportedException(); }
    public void Remove(object value) { throw new NotSupportedException(); }
    public void RemoveAt(int index) { throw new NotSupportedException(); }

    /// <summary>不支持线性查找，避免任何调用方意外触发全量扫描。</summary>
    public int IndexOf(object value) { return -1; }
    public bool Contains(object value) { return false; }

    public void CopyTo(Array array, int index)
    {
        int budget = LoadedPages + MaxCachedPages * 2;
        for (int i = 0; i < _total && index < array.Length; i++, index++)
        {
            if (LoadedPages > budget) break;
            array.SetValue(this[i], index);
        }
    }

    public IEnumerator GetEnumerator()
    {
        // 正常虚拟化路径不会走到这里。真被全量枚举时按预算停下，
        // 防止 40 万次索引访问把每一页都拉起来。
        int budget = LoadedPages + MaxCachedPages * 2;
        for (int i = 0; i < _total; i++)
        {
            if (LoadedPages > budget) yield break;
            yield return this[i];
        }
    }
}
