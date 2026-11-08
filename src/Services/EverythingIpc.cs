using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using TTJiang.Models;

namespace TTJiang.Services;

/// <summary>一次 IPC 查询得到的单条结果，含从索引里读出的展示属性。</summary>
public sealed class IpcHit
{
    public string FullPath { get; set; } = "";
    public string Extension { get; set; } = "";
    public string Type { get; set; } = "";
    /// <summary>FILETIME；0 表示索引里没有有效值。</summary>
    public long Modified { get; set; }
}

/// <summary>一次 IPC 查询的原始结果。</summary>
public sealed class IpcResult
{
    public bool Ok { get; set; }
    public string Reason { get; set; } = "";
    public int Total { get; set; }
    /// <summary>引擎返回的 viewport 条数，用于诊断结果读取。</summary>
    public int Viewport { get; set; }
    public List<IpcHit> Hits { get; } = new();
    /// <summary>只含路径的视图，供早期验证脚本使用。</summary>
    public List<string> Paths { get; } = new();
}

/// <summary>
/// 通过 Everything 1.5 的 SDK3 命名管道直接查询，不再为每次输入启动 es.exe。
/// 官方 SDK3 是 MIT 许可，DLL 随内置组件一起释放；连接与搜索状态在进程内复用。
/// IPC 不可用时返回 Ok=false，由 EverythingSearch 回退到 es.exe。
/// </summary>
public static class EverythingIpc
{
    /// <summary>单次查询的路径上限，同时也是传给 Everything 的 viewport 大小。</summary>
    private const int MaxPathChars = 2048;

    /// <summary>属性文本缓冲区。</summary>
    private const int MaxPropChars = 256;

    private static readonly object Sync = new object();
    private static IntPtr _module;
    private static IntPtr _client;
    private static IntPtr _state;
    private static Api _api;
    private static string _lastReason = "";

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadLibraryW(string path);

    [DllImport("kernel32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
    private static extern IntPtr GetProcAddress(IntPtr module, string name);

    [DllImport("kernel32.dll")]
    private static extern bool FreeLibrary(IntPtr module);

    [UnmanagedFunctionPointer(CallingConvention.Winapi, CharSet = CharSet.Unicode)]
    private delegate IntPtr FnConnect(string instance);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate IntPtr FnCreateState();

    [UnmanagedFunctionPointer(CallingConvention.Winapi, CharSet = CharSet.Unicode)]
    private delegate int FnSetText(IntPtr state, string text);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int FnSetFlag(IntPtr state, int value);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int FnSetSize(IntPtr state, UIntPtr value);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int FnAddProperty(IntPtr state, uint propertyId);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int FnSetSort(IntPtr state, uint propertyId, int ascending);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate IntPtr FnSearch(IntPtr client, IntPtr state);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate UIntPtr FnGetCount(IntPtr result);

    [UnmanagedFunctionPointer(CallingConvention.Winapi, CharSet = CharSet.Unicode)]
    private delegate UIntPtr FnGetPath(IntPtr result, UIntPtr index, StringBuilder buffer, UIntPtr size);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate ulong FnGetModified(IntPtr result, UIntPtr index);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int FnDestroy(IntPtr obj);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate uint FnLastError();

    private sealed class Api
    {
        public FnConnect Connect;
        public FnCreateState CreateState;
        public FnSetText SetText;
        public FnSetFlag SetMatchPath;
        public FnSetFlag SetMatchCase;
        public FnSetSize SetViewportOffset;
        public FnSetSize SetViewportCount;
        public FnDestroy ClearProperties;
        public FnAddProperty AddProperty;
        public FnDestroy ClearSorts;
        public FnSetSort SetSort;
        public FnSearch Search;
        public FnGetCount GetResultCount;
        public FnGetCount GetViewportCount;
        public FnGetPath GetDirectory;
        public FnGetPath GetName;
        public FnGetPath GetExtension;
        public FnGetPath GetTypeText;
        public FnGetModified GetModified;
        public FnDestroy DestroyResult;
        public FnDestroy DestroyState;
        public FnDestroy DestroyClient;
        public FnLastError LastError;
    }

    private static T Bind<T>(IntPtr module, string name) where T : Delegate
    {
        IntPtr ptr = GetProcAddress(module, name);
        if (ptr == IntPtr.Zero) throw new EntryPointNotFoundException(name);
        return (T)Marshal.GetDelegateForFunctionPointer(ptr, typeof(T));
    }

    private static bool EnsureModule(string dllPath, IpcResult result)
    {
        if (_module != IntPtr.Zero && _api != null) return true;

        if (!System.IO.File.Exists(dllPath))
        {
            result.Reason = "SDK3 组件缺失";
            return false;
        }

        IntPtr module = LoadLibraryW(dllPath);
        if (module == IntPtr.Zero)
        {
            result.Reason = "无法加载 SDK3 组件，Win32 错误 " + Marshal.GetLastWin32Error();
            return false;
        }

        try
        {
            var api = new Api
            {
                Connect = Bind<FnConnect>(module, "Everything3_ConnectW"),
                CreateState = Bind<FnCreateState>(module, "Everything3_CreateSearchState"),
                SetText = Bind<FnSetText>(module, "Everything3_SetSearchTextW"),
                SetMatchPath = Bind<FnSetFlag>(module, "Everything3_SetSearchMatchPath"),
                SetMatchCase = Bind<FnSetFlag>(module, "Everything3_SetSearchMatchCase"),
                SetViewportOffset = Bind<FnSetSize>(module, "Everything3_SetSearchViewportOffset"),
                SetViewportCount = Bind<FnSetSize>(module, "Everything3_SetSearchViewportCount"),
                ClearProperties = Bind<FnDestroy>(module, "Everything3_ClearSearchPropertyRequests"),
                AddProperty = Bind<FnAddProperty>(module, "Everything3_AddSearchPropertyRequest"),
                ClearSorts = Bind<FnDestroy>(module, "Everything3_ClearSearchSorts"),
                SetSort = Bind<FnSetSort>(module, "Everything3_SetSearchSort"),
                Search = Bind<FnSearch>(module, "Everything3_Search"),
                GetResultCount = Bind<FnGetCount>(module, "Everything3_GetResultListCount"),
                GetViewportCount = Bind<FnGetCount>(module, "Everything3_GetResultListViewportCount"),
                GetDirectory = Bind<FnGetPath>(module, "Everything3_GetResultPathW"),
                GetName = Bind<FnGetPath>(module, "Everything3_GetResultNameW"),
                GetExtension = Bind<FnGetPath>(module, "Everything3_GetResultExtensionW"),
                GetTypeText = Bind<FnGetPath>(module, "Everything3_GetResultTypeW"),
                GetModified = Bind<FnGetModified>(module, "Everything3_GetResultDateModified"),
                DestroyResult = Bind<FnDestroy>(module, "Everything3_DestroyResultList"),
                DestroyState = Bind<FnDestroy>(module, "Everything3_DestroySearchState"),
                DestroyClient = Bind<FnDestroy>(module, "Everything3_DestroyClient"),
                LastError = Bind<FnLastError>(module, "Everything3_GetLastError")
            };

            _module = module;
            _api = api;
            return true;
        }
        catch (Exception ex)
        {
            FreeLibrary(module);
            result.Reason = "SDK3 组件不完整：" + ex.Message;
            return false;
        }
    }

    private static bool EnsureSession(string dllPath, IpcResult result)
    {
        if (!EnsureModule(dllPath, result)) return false;

        if (_client == IntPtr.Zero)
        {
            _client = _api.Connect(EverythingHost.InstanceName);
            if (_client == IntPtr.Zero)
            {
                result.Reason = "SDK3 无法连接内置搜索实例 0x"
                                + _api.LastError().ToString("X8");
                return false;
            }
        }

        if (_state == IntPtr.Zero)
        {
            _state = _api.CreateState();
            if (_state == IntPtr.Zero)
            {
                result.Reason = "SDK3 无法创建搜索状态 0x" + _api.LastError().ToString("X8");
                return false;
            }
        }

        return true;
    }

    private static bool TrySearch(string query, int offset, int count,
                                  SearchOptions options, IpcResult result)
    {
        IntPtr list = IntPtr.Zero;
        try
        {
            // 属性必须先请求，否则 GetResultExtension/Type/DateModified 全返回空或无效值
            _api.ClearProperties(_state);
            _api.AddProperty(_state, 0);   // NAME
            _api.AddProperty(_state, 1);   // PATH
            _api.AddProperty(_state, 3);   // EXTENSION
            _api.AddProperty(_state, 4);   // TYPE
            _api.AddProperty(_state, 5);   // DATE_MODIFIED

            // 排序交给引擎：全库排序后再取 viewport，不是只重排已取回的 N 条
            _api.ClearSorts(_state);
            _api.SetSort(_state, (uint)options.SortPropertyId, options.Ascending ? 1 : 0);

            _api.SetText(_state, query);
            // 默认只匹配文件名；用户在结果区打开「含路径」时才匹配完整路径
            _api.SetMatchPath(_state, options.MatchFullPath ? 1 : 0);
            _api.SetMatchCase(_state, 0);
            // 虚拟列表按需取页：offset 是全库排序后的起始位置，count 是本页条数。
            // 引擎侧仍然对全库排序，只对这一段做数据传输。
            _api.SetViewportOffset(_state, new UIntPtr((uint)offset));
            _api.SetViewportCount(_state, new UIntPtr((uint)count));

            list = _api.Search(_client, _state);
            if (list == IntPtr.Zero)
            {
                result.Reason = "SDK3 查询失败 0x" + _api.LastError().ToString("X8");
                return false;
            }

            ulong total = _api.GetResultCount(list).ToUInt64();
            result.Total = total > int.MaxValue ? int.MaxValue : (int)total;

            ulong viewport = _api.GetViewportCount(list).ToUInt64();
            result.Viewport = (int)Math.Min(viewport, (ulong)int.MaxValue);
            int take = (int)Math.Min(viewport, (ulong)count);
            var buffer = new StringBuilder(MaxPathChars);
            var prop = new StringBuilder(MaxPropChars);
            var pathSize = new UIntPtr((uint)MaxPathChars);
            var propSize = new UIntPtr((uint)MaxPropChars);

            for (int i = 0; i < take; i++)
            {
                UIntPtr index = new UIntPtr((uint)i);

                // SDK3 一旦发起属性请求，GetResultFullPathNameW 就返回 0；
                // 改为请求 PATH + NAME 两个属性后自行拼出完整路径。
                buffer.Clear();
                _api.GetDirectory(list, index, buffer, pathSize);
                string dir = buffer.ToString();

                prop.Clear();
                _api.GetName(list, index, prop, propSize);
                string name = prop.ToString();
                if (name.Length == 0) continue;

                string path = string.IsNullOrEmpty(dir)
                    ? name
                    : (dir.EndsWith("\\", StringComparison.Ordinal)
                        ? dir + name
                        : dir + "\\" + name);

                prop.Clear();
                _api.GetExtension(list, index, prop, propSize);
                string extension = prop.ToString();

                prop.Clear();
                _api.GetTypeText(list, index, prop, propSize);
                string type = prop.ToString();

                ulong modified = _api.GetModified(list, index);

                result.Paths.Add(path);
                result.Hits.Add(new IpcHit
                {
                    FullPath = path,
                    Extension = extension,
                    Type = type,
                    Modified = modified == ulong.MaxValue ? 0 : (long)modified
                });
            }

            result.Ok = true;
            return true;
        }
        catch (Exception ex)
        {
            result.Reason = "SDK3 调用异常：" + ex.Message;
            return false;
        }
        finally
        {
            if (list != IntPtr.Zero) _api.DestroyResult(list);
        }
    }

    /// <summary>取第一页；保留旧调用签名给既有验证工程使用。</summary>
    public static IpcResult Query(string query, int limit, SearchOptions options = null,
                                  string dllPath = null)
    {
        return Query(query, 0, limit, options, dllPath);
    }

    /// <summary>
    /// 同步分页查询。dllPath 为空时取 EverythingHost.Sdk3Path；
    /// 传入绝对路径便于在进程外做验证。Total 始终是本次查询的全库命中数，不是本页条数。
    /// </summary>
    public static IpcResult Query(string query, int offset, int count,
                                  SearchOptions options = null, string dllPath = null)
    {
        var result = new IpcResult();
        if (string.IsNullOrWhiteSpace(query)) return result;
        if (count <= 0) count = EverythingSearch.DefaultLimit;
        if (offset < 0) offset = 0;
        if (options == null) options = new SearchOptions();
        if (string.IsNullOrEmpty(dllPath)) dllPath = EverythingHost.Sdk3Path;

        lock (Sync)
        {
            // 实例重启会让旧连接失效：重建一次连接再试
            for (int attempt = 0; attempt < 2; attempt++)
            {
                if (!EnsureSession(dllPath, result)) break;
                if (TrySearch(query, offset, count, options, result)) return result;
                CloseSession();
            }
        }

        if (result.Reason != _lastReason)
        {
            _lastReason = result.Reason;
            ConfigStore.Log("SDK3 IPC 不可用，回退 es.exe：" + result.Reason);
        }
        return result;
    }

    /// <summary>退出程序时释放长连接与 DLL 句柄。</summary>
    public static void Close()
    {
        lock (Sync)
        {
            CloseSession();
            if (_module != IntPtr.Zero)
            {
                FreeLibrary(_module);
                _module = IntPtr.Zero;
            }
            _api = null;
        }
    }

    /// <summary>只断开连接，DLL 仍留在进程里，下次查询会重新连。</summary>
    private static void CloseSession()
    {
        if (_api != null)
        {
            try
            {
                if (_state != IntPtr.Zero) _api.DestroyState(_state);
                if (_client != IntPtr.Zero) _api.DestroyClient(_client);
            }
            catch
            {
                // 连接已失效时忽略
            }
        }

        _state = IntPtr.Zero;
        _client = IntPtr.Zero;
    }
}
