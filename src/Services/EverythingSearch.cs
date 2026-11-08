using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;
using TTJiang.Models;

namespace TTJiang.Services;

/// <summary>一条文件搜索结果。</summary>
public sealed class SearchHit
{
    public string FullPath { get; set; } = "";
    public string Name { get; set; } = "";
    public string Directory { get; set; } = "";
    public string Extension { get; set; } = "";
    public string Type { get; set; } = "";
    /// <summary>FILETIME；0 表示没有有效值。</summary>
    public long Modified { get; set; }

    public static SearchHit FromPath(string full)
    {
        string name = "";
        string dir = "";
        try
        {
            name = Path.GetFileName(full);
            dir = Path.GetDirectoryName(full) ?? "";
        }
        catch
        {
            // 路径异常时原样展示
        }

        if (string.IsNullOrWhiteSpace(name)) name = full;
        if (string.IsNullOrWhiteSpace(dir)) dir = full;

        return new SearchHit { FullPath = full, Name = name, Directory = dir };
    }

    public static SearchHit FromIpc(IpcHit hit)
    {
        SearchHit result = FromPath(hit.FullPath);
        result.Extension = hit.Extension ?? "";
        result.Type = hit.Type ?? "";
        result.Modified = hit.Modified;
        return result;
    }

    /// <summary>es.exe 只输出路径，回退链路用文件系统补齐展示属性。</summary>
    public static void EnrichFromFile(SearchHit hit)
    {
        try
        {
            var info = new FileInfo(hit.FullPath);
            if (!info.Exists) return;

            hit.Modified = info.LastWriteTimeUtc.ToFileTimeUtc();
            hit.Extension = info.Extension.TrimStart('.');
            hit.Type = string.IsNullOrEmpty(hit.Extension)
                ? "文件"
                : hit.Extension.ToUpperInvariant() + " 文件";
        }
        catch
        {
            // 文件刚被删掉或路径不可访问时保留空值
        }
    }

    /// <summary>结果行右侧的元信息：文件类型描述 + 本地化修改时间。供虚拟列表模板绑定。</summary>
    public string MetaText
    {
        get
        {
            string type = !string.IsNullOrEmpty(Type) ? Type : Extension.ToUpperInvariant();

            string date = "";
            if (Modified > 0)
            {
                try
                {
                    date = DateTime.FromFileTimeUtc(Modified).ToLocalTime().ToString("yyyy-MM-dd HH:mm");
                }
                catch
                {
                    // FILETIME 越界时忽略
                }
            }

            if (type.Length == 0) return date;
            if (date.Length == 0) return type;
            return type + "  ·  " + date;
        }
    }
}

public sealed class SearchOutcome
{
    public bool Ok { get; set; }
    public string Error { get; set; } = "";
    /// <summary>本次查询的全库命中数，不受分页影响；虚拟列表用它决定滚动范围。</summary>
    public int Total { get; set; }
    public List<SearchHit> Hits { get; } = new();
}

/// <summary>
/// 调用内置的 es.exe 查内置 Everything 实例。
/// 不涉及任何"用户填路径"的逻辑：工具位置和实例名都由 EverythingHost 决定。
/// 主路径是 SDK3 命名管道，es.exe 只在组件缺失/实例异常时兜底。
/// </summary>
public static class EverythingSearch
{
    public const int DefaultLimit = 30;

    /// <summary>输出上限。es.exe 被替换或参数异常时，避免把内存吃掉。</summary>
    private const int MaxCaptureBytes = 4 * 1024 * 1024;

    private const int TimeoutMs = 10000;

    /// <summary>一次读取的结果，超出上限时只保留前 4 MB，其余丢弃但继续读，避免管道堵塞。</summary>
    private sealed class Capture
    {
        public byte[] Data = Array.Empty<byte>();
        public bool Truncated;
    }

    /// <summary>es.exe 一次调用的原始结果。</summary>
    private sealed class EsRun
    {
        public int ExitCode = -1;
        public byte[] Stdout = Array.Empty<byte>();
        public byte[] Stderr = Array.Empty<byte>();
        public bool Truncated;
        public bool Canceled;
        public string Failure;
    }

    public static Task<SearchOutcome> QueryAsync(string query, int limit, CancellationToken ct)
    {
        return QueryAsync(query, limit, null, ct);
    }

    /// <summary>取第一页；保留旧调用签名。</summary>
    public static Task<SearchOutcome> QueryAsync(string query, int limit,
                                                 SearchOptions options, CancellationToken ct)
    {
        return QueryAsync(query, 0, limit, options, ct);
    }

    /// <summary>按全库排序后的 [offset, offset+count) 取一页；Total 始终是全部命中数。</summary>
    public static async Task<SearchOutcome> QueryAsync(string query, int offset, int count,
                                                       SearchOptions options, CancellationToken ct)
    {
        var outcome = new SearchOutcome();

        if (string.IsNullOrWhiteSpace(query))
        {
            outcome.Ok = true;
            return outcome;
        }

        if (count <= 0) count = DefaultLimit;
        if (offset < 0) offset = 0;
        if (options == null) options = new SearchOptions();

        // 首选 SDK3 命名管道：进程内长连接，不再为每次输入启动一个 es.exe 进程。
        // 查询本身是同步的，调用方已在后台任务里；实测为毫秒级。
        ct.ThrowIfCancellationRequested();
        IpcResult ipc = EverythingIpc.Query(query, offset, count, options);
        if (ipc.Ok)
        {
            foreach (IpcHit hit in ipc.Hits) outcome.Hits.Add(SearchHit.FromIpc(hit));
            outcome.Total = ipc.Total;
            outcome.Ok = true;
            return outcome;
        }

        // IPC 不可用（组件缺失、实例没起来、查询失败）时回退到原有 es.exe 链路
        string es = EverythingHost.EsPath;
        if (!File.Exists(es))
        {
            outcome.Error = "内置搜索组件缺失，请重新解压完整的 TT酱 压缩包";
            return outcome;
        }

        EsRun run = await RunEsAsync(es, query, offset, count, options, ct).ConfigureAwait(false);
        if (run.Canceled) return outcome;

        if (run.Failure != null)
        {
            outcome.Error = run.Failure;
            return outcome;
        }

        if (run.ExitCode != 0)
        {
            string detail = Decode(run.Stderr).Trim();
            ConfigStore.Log("es.exe 退出码 " + run.ExitCode + "：" + detail);
            outcome.Error = MapError(run.ExitCode, detail);
            return outcome;
        }

        if (run.Truncated)
        {
            ConfigStore.Log("es.exe 输出超过 " + MaxCaptureBytes + " 字节，已截断：" + query);
        }

        foreach (string raw in Decode(run.Stdout).Split('\n'))
        {
            string line = raw.Trim('\r', ' ', '\t');
            if (line.Length == 0) continue;
            SearchHit hit = SearchHit.FromPath(line);
            SearchHit.EnrichFromFile(hit);
            outcome.Hits.Add(hit);
            if (outcome.Hits.Count >= count) break;
        }

        // es.exe 不随结果返回总数，取第一页时补一次 -get-result-count；
        // 后续页沿用调用方已经拿到的 Total，不重复起进程。
        outcome.Total = offset == 0
            ? await RunEsCountAsync(es, query, options, ct).ConfigureAwait(false)
            : 0;
        if (outcome.Total < 0) outcome.Total = outcome.Hits.Count;

        outcome.Ok = true;
        return outcome;
    }

    /// <summary>
    /// es.exe 会把多个命令行参数拼成搜索文本。带空格的查询必须拆成多个参数，
    /// 否则整串被 QuoteArg 包成一个参数，Everything 会当成精确短语（实测返回 0 条）。
    /// 引号内的空格不拆，保留短语语义。
    /// </summary>
    public static string[] SplitQuery(string query)
    {
        var parts = new List<string>();
        var sb = new StringBuilder();
        bool inQuote = false;

        foreach (char c in query)
        {
            if (c == '"')
            {
                inQuote = !inQuote;
                sb.Append(c);
                continue;
            }

            if (char.IsWhiteSpace(c) && !inQuote)
            {
                if (sb.Length > 0) { parts.Add(sb.ToString()); sb.Clear(); }
                continue;
            }

            sb.Append(c);
        }

        if (sb.Length > 0) parts.Add(sb.ToString());
        return parts.Count > 0 ? parts.ToArray() : new[] { "" };
    }

    private static async Task<EsRun> RunEsAsync(string esPath, string query, int offset, int count,
                                                SearchOptions options, CancellationToken ct)
    {
        var run = new EsRun();

        var args = new StringBuilder();
        args.Append("-instance ").Append(EverythingHost.InstanceName)
            // 分页必须用 -viewport-count：-n 会先截断搜索范围，
            // 与 -viewport-offset 一起用会返回 0 条（实测 ES 1.1.0.38）。
            .Append(" -viewport-count ").Append(count.ToString(CultureInfo.InvariantCulture))
            .Append(" -viewport-offset ").Append(offset.ToString(CultureInfo.InvariantCulture))
            .Append(" -sort ").Append(options.EsSortArgument);
        if (options.MatchFullPath) args.Append(" -p");
        foreach (string word in SplitQuery(query)) args.Append(' ').Append(QuoteArg(word));

        var psi = new ProcessStartInfo(esPath)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = EverythingHost.ToolsDir,
            Arguments = args.ToString()
        };

        using (var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct))
        {
            timeout.CancelAfter(TimeoutMs);

            using var proc = new Process { StartInfo = psi };
            try
            {
                if (!proc.Start())
                {
                    run.Failure = "无法启动搜索组件 es.exe";
                    return run;
                }

                Task<Capture> outTask = ReadAllAsync(proc.StandardOutput.BaseStream, timeout.Token);
                Task<Capture> errTask = ReadAllAsync(proc.StandardError.BaseStream, timeout.Token);

                using (timeout.Token.Register(() =>
                       {
                           try
                           {
                               if (!proc.HasExited) proc.Kill();
                           }
                           catch
                           {
                               // 进程可能刚好退出
                           }
                       }))
                {
                    await WaitForExitAsync(proc, timeout.Token).ConfigureAwait(false);
                }

                Capture outCapture = await outTask.ConfigureAwait(false);
                Capture errCapture = await errTask.ConfigureAwait(false);
                run.Stdout = outCapture.Data;
                run.Stderr = errCapture.Data;
                run.Truncated = outCapture.Truncated;
                run.ExitCode = proc.ExitCode;
            }
            catch (OperationCanceledException)
            {
                if (ct.IsCancellationRequested)
                {
                    run.Canceled = true;
                    return run;
                }

                ConfigStore.Log("es.exe 调用超时：" + query);
                run.Failure = "搜索响应超时";
                return run;
            }
            catch (Exception ex)
            {
                ConfigStore.Log("调用 es.exe 失败：" + ex.Message);
                run.Failure = "调用搜索组件失败：" + ex.Message;
                return run;
            }
        }

        return run;
    }

    /// <summary>回退链路单独取全库命中数；失败返回 -1，调用方按本页条数兜底。</summary>
    private static async Task<int> RunEsCountAsync(string esPath, string query,
                                                   SearchOptions options, CancellationToken ct)
    {
        var args = new StringBuilder();
        args.Append("-instance ").Append(EverythingHost.InstanceName)
            .Append(" -get-result-count -sort ").Append(options.EsSortArgument);
        if (options.MatchFullPath) args.Append(" -p");
        foreach (string word in SplitQuery(query)) args.Append(' ').Append(QuoteArg(word));

        var psi = new ProcessStartInfo(esPath)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = EverythingHost.ToolsDir,
            Arguments = args.ToString()
        };

        try
        {
            using var proc = new Process { StartInfo = psi };
            if (!proc.Start()) return -1;

            Task<Capture> outTask = ReadAllAsync(proc.StandardOutput.BaseStream, ct);
            Task<Capture> errTask = ReadAllAsync(proc.StandardError.BaseStream, ct);
            await WaitForExitAsync(proc, ct).ConfigureAwait(false);
            Capture capture = await outTask.ConfigureAwait(false);
            await errTask.ConfigureAwait(false);

            string text = Decode(capture.Data).Trim();
            return int.TryParse(text, out int total) && total >= 0 ? total : -1;
        }
        catch (OperationCanceledException)
        {
            return -1;
        }
        catch (Exception ex)
        {
            ConfigStore.Log("读取 es.exe 结果总数失败：" + ex.Message);
            return -1;
        }
    }

    /// <summary>按 Windows 命令行规则给参数加引号，保证带空格/引号的路径与关键词不被拆开。</summary>
    public static string QuoteArg(string value)
    {
        if (string.IsNullOrEmpty(value)) return "\"\"";
        if (value.IndexOfAny(new[] { ' ', '\t', '\n', '\v', '"' }) < 0) return value;

        var sb = new StringBuilder("\"");
        int backslashes = 0;

        foreach (char c in value)
        {
            if (c == '\\')
            {
                backslashes++;
                continue;
            }

            if (c == '"')
            {
                sb.Append('\\', backslashes * 2 + 1);
                sb.Append('"');
                backslashes = 0;
                continue;
            }

            sb.Append('\\', backslashes);
            sb.Append(c);
            backslashes = 0;
        }

        sb.Append('\\', backslashes * 2);
        sb.Append('"');
        return sb.ToString();
    }

    /// <summary>.NET Framework 没有 WaitForExitAsync，用 Exited 事件包一层。</summary>
    private static async Task WaitForExitAsync(Process proc, CancellationToken ct)
    {
        var tcs = new TaskCompletionSource<bool>();
        EventHandler handler = (s, e) => tcs.TrySetResult(true);

        proc.EnableRaisingEvents = true;
        proc.Exited += handler;

        try
        {
            using (ct.Register(() => tcs.TrySetCanceled()))
            {
                if (proc.HasExited) return;
                await tcs.Task.ConfigureAwait(false);
            }
        }
        finally
        {
            try
            {
                proc.Exited -= handler;
            }
            catch
            {
                // 退出阶段忽略
            }
        }
    }

    private static async Task<Capture> ReadAllAsync(Stream stream, CancellationToken ct)
    {
        var capture = new Capture();
        using var ms = new MemoryStream();
        var buffer = new byte[8192];

        try
        {
            int read;
            while ((read = await stream.ReadAsync(buffer, 0, buffer.Length, ct).ConfigureAwait(false)) > 0)
            {
                if (ms.Length + read <= MaxCaptureBytes)
                {
                    ms.Write(buffer, 0, read);
                }
                else
                {
                    // 超出上限后继续把剩余数据读掉（不保存），否则子进程会卡在写管道上不退出
                    capture.Truncated = true;
                }
            }
        }
        catch (OperationCanceledException)
        {
            // 超时或上层取消，返回已读到的部分
        }
        catch (IOException)
        {
            // 进程被强制结束时读流会抛异常
        }
        catch (ObjectDisposedException)
        {
            // 同上
        }

        capture.Data = ms.ToArray();
        return capture;
    }

    /// <summary>es.exe 1.1 输出 ANSI（中文系统为 GBK），新版可能输出 UTF-8，这里两边都兼容。</summary>
    private static string Decode(byte[] bytes)
    {
        if (bytes == null || bytes.Length == 0) return "";

        // 中文结果几乎必然不是 UTF-8。先做一次无异常的合法性扫描，
        // 避免每条结果都走「抛 DecoderFallbackException 再回退」这条昂贵路径。
        Encoding encoding = IsValidUtf8(bytes) ? new UTF8Encoding(false) : Encoding.GetEncoding(936);

        try
        {
            return encoding.GetString(bytes);
        }
        catch
        {
            return Encoding.Default.GetString(bytes);
        }
    }

    /// <summary>按 UTF-8 编码规则逐字节检查，不分配、不抛异常。</summary>
    private static bool IsValidUtf8(byte[] bytes)
    {
        int i = 0;
        while (i < bytes.Length)
        {
            byte b = bytes[i];
            if (b < 0x80)
            {
                i++;
                continue;
            }

            int need;
            uint cp;
            if (b >= 0xC2 && b <= 0xDF) { need = 1; cp = (uint)(b & 0x1F); }
            else if (b >= 0xE0 && b <= 0xEF) { need = 2; cp = (uint)(b & 0x0F); }
            else if (b >= 0xF0 && b <= 0xF4) { need = 3; cp = (uint)(b & 0x07); }
            else return false;

            if (i + need >= bytes.Length) return false;
            for (int k = 1; k <= need; k++)
            {
                byte c = bytes[i + k];
                if ((c & 0xC0) != 0x80) return false;
                cp = (cp << 6) | (uint)(c & 0x3F);
            }

            if ((need == 1 && cp < 0x80) || (need == 2 && cp < 0x800)
                || (need == 3 && cp < 0x10000) || cp > 0x10FFFF
                || (cp >= 0xD800 && cp <= 0xDFFF))
            {
                return false;
            }

            i += need + 1;
        }

        return true;
    }

    private static string MapError(int code, string detail)
    {
        switch (code)
        {
            case 8:
                return "内置搜索实例没有运行。请在设置里确认「索引服务」已安装，或重启 TT酱。";
            case 6:
                return "内置搜索组件不支持当前参数，请重新解压完整的 TT酱 压缩包";
            case 4:
                return "Everything 拒绝了请求，可能被权限设置限制";
            default:
                if (!string.IsNullOrWhiteSpace(detail)) return detail;
                return "搜索组件返回错误码 " + code;
        }
    }
}
