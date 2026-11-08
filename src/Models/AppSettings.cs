namespace TTJiang.Models;

/// <summary>程序设置，随 shortcuts.json 一起保存。</summary>
public class AppSettings
{
    /// <summary>Everything 的 es.exe（或 Everything.exe，会取同目录的 es.exe）路径。</summary>
    public string EverythingPath { get; set; } = "";

    /// <summary>
    /// 旧版的结果条数上限，结果区改为虚拟列表后不再参与查询。
    /// 保留字段只为能读回历史 shortcuts.json，写入时原样带回。
    /// </summary>
    public int SearchResultLimit { get; set; } = 30;

    /// <summary>结果排序字段：0 名称，1 类型（按扩展名），2 修改时间。</summary>
    public int SearchSortKey { get; set; } = 0;

    /// <summary>排序是否升序：1 是，0 否。</summary>
    public int SearchSortAscending { get; set; } = 1;

    /// <summary>是否匹配完整路径：0 只匹配文件名（默认），1 匹配完整路径。</summary>
    public int SearchMatchFullPath { get; set; } = 0;

    public AppSettings Clone()
    {
        return new AppSettings
        {
            EverythingPath = EverythingPath,
            SearchResultLimit = SearchResultLimit,
            SearchSortKey = SearchSortKey,
            SearchSortAscending = SearchSortAscending,
            SearchMatchFullPath = SearchMatchFullPath
        };
    }
}
