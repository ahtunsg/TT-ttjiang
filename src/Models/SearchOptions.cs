namespace TTJiang.Models;

/// <summary>结果排序字段。</summary>
public enum SearchSortKey
{
    Name = 0,
    Type = 1,
    DateModified = 2
}

/// <summary>
/// 一次查询的排序与匹配范围。排序在 Everything 引擎侧完成（全库排序后再取前 N 条），
/// 不是只对已经取回的结果做本地重排。
/// </summary>
public sealed class SearchOptions
{
    public SearchSortKey SortKey { get; set; } = SearchSortKey.Name;
    public bool Ascending { get; set; } = true;

    /// <summary>默认 false：空格分隔的多个词都只匹配文件名。打开后匹配完整路径。</summary>
    public bool MatchFullPath { get; set; }

    /// <summary>SDK3 的 EVERYTHING3_PROPERTY_ID_*；类型按扩展名排，与 es.exe 的 extension 同义。</summary>
    public int SortPropertyId
    {
        get
        {
            switch (SortKey)
            {
                case SearchSortKey.Type: return 3;           // EXTENSION
                case SearchSortKey.DateModified: return 5;   // DATE_MODIFIED
                default: return 0;                          // NAME
            }
        }
    }

    /// <summary>es.exe -sort 的参数值。</summary>
    public string EsSortArgument
    {
        get
        {
            string name = SortKey == SearchSortKey.Type ? "extension"
                        : SortKey == SearchSortKey.DateModified ? "date-modified"
                        : "name";
            return name + (Ascending ? "-ascending" : "-descending");
        }
    }

    public static SearchOptions FromSettings(AppSettings s)
    {
        int key = s == null ? 0 : s.SearchSortKey;
        if (key < 0 || key > 2) key = 0;

        return new SearchOptions
        {
            SortKey = (SearchSortKey)key,
            Ascending = s == null || s.SearchSortAscending != 0,
            MatchFullPath = s != null && s.SearchMatchFullPath == 1
        };
    }
}
