// Minimal in-process binding to the ONNX Runtime C API.
//
// Why P/Invoke instead of the Microsoft.ML.OnnxRuntime managed package:
// the C# compiler shipped with Windows here is C# 5 and there is no NuGet
// toolchain, so a managed wrapper would drag in netstandard facades plus
// System.Memory. The C API needs one native DLL and nothing else, which also
// keeps the delivered exe self-contained.
using System;
using System.IO;
using System.Runtime.InteropServices;

namespace TTJiang.Capture
{
    internal sealed class OrtException : Exception
    {
        internal OrtException(string message) : base(message) { }
    }

    internal static class Ort
    {
        private const int LogLevelWarning = 2;
        private const int GraphOptimizationAll = 99;
        private const int TypeFloat = 1;

        // ---- Win32 ---------------------------------------------------------------

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);

        // GetProcAddress takes an ANSI name; the Unicode CharSet used above would
        // marshal a UTF-16 string and the lookup would silently fail.
        [DllImport("kernel32.dll", CharSet = CharSet.Ansi, SetLastError = true,
            ExactSpelling = true)]
        private static extern IntPtr GetProcAddress(IntPtr module, string name);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
        private static extern bool SetDllDirectoryW(string path);

        // ---- delegates -----------------------------------------------------------

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetApiBaseDelegate();

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetApiDelegate(uint version);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetVersionStringDelegate();

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr CreateEnvDelegate(int logLevel,
            [MarshalAs(UnmanagedType.LPStr)] string logId, out IntPtr env);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr CreateSessionOptionsDelegate(out IntPtr options);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr SetIntraOpNumThreadsDelegate(IntPtr options, int threads);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr SetOptimizationDelegate(IntPtr options, int level);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr ToggleDelegate(IntPtr options);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr CreateSessionDelegate(IntPtr env,
            [MarshalAs(UnmanagedType.LPWStr)] string modelPath, IntPtr options,
            out IntPtr session);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr RunDelegate(IntPtr session, IntPtr runOptions,
            IntPtr[] inputNames, IntPtr[] inputs, UIntPtr inputCount,
            IntPtr[] outputNames, UIntPtr outputCount, IntPtr[] outputs);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetAllocatorDelegate(out IntPtr allocator);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr CreateTensorDelegate(IntPtr allocator, IntPtr data,
            UIntPtr dataLength, long[] shape, UIntPtr shapeLength, int type,
            out IntPtr value);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetTensorDataDelegate(IntPtr value, out IntPtr data);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetTensorInfoDelegate(IntPtr value, out IntPtr info);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetCountDelegate(IntPtr info, out long count);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetDimensionsDelegate(IntPtr info, long[] dims,
            UIntPtr capacity);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetNameDelegate(IntPtr session, int index, IntPtr allocator,
            out IntPtr name);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate IntPtr GetErrorMessageDelegate(IntPtr status);

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate void ReleaseDelegate(IntPtr value);

        // ---- state ---------------------------------------------------------------

        private static readonly object Gate = new object();
        private static bool _initialised;
        private static string _failure;

        private static IntPtr[] _table;
        private static IntPtr _env;
        private static IntPtr _allocator;

        private static CreateEnvDelegate _createEnv;
        private static CreateSessionOptionsDelegate _createOptions;
        private static SetIntraOpNumThreadsDelegate _setThreads;
        private static SetOptimizationDelegate _setOptimization;
        private static ToggleDelegate _disableArena;
        private static CreateSessionDelegate _createSession;
        private static RunDelegate _run;
        private static GetAllocatorDelegate _getAllocator;
        private static CreateTensorDelegate _createTensor;
        private static GetTensorDataDelegate _getTensorData;
        private static GetTensorInfoDelegate _getTensorInfo;
        private static GetCountDelegate _getDimCount;
        private static GetCountDelegate _getElementCount;
        private static GetDimensionsDelegate _getDims;
        private static GetNameDelegate _getInputName;
        private static GetNameDelegate _getOutputName;
        private static GetErrorMessageDelegate _getErrorMessage;
        private static ReleaseDelegate _releaseStatus;
        private static ReleaseDelegate _releaseValue;
        private static ReleaseDelegate _releaseSession;
        private static ReleaseDelegate _releaseOptions;
        private static ReleaseDelegate _releaseEnv;
        private static ReleaseDelegate _releaseTensorInfo;

        internal static string RuntimeVersion { get; private set; }

        // Optional diagnostics hook; the console probe (tests/OrtProbe.cs) uses
        // it to bisect load and bind failures. Null in the shipped product.
        internal static Action<string> Trace = null;

        private static void Step(string message)
        {
            Action<string> trace = Trace;
            if (trace != null) trace(message);
        }

        internal static string LastFailure { get { return _failure; } }

        // Loads onnxruntime.dll from the extracted runtime directory and resolves the
        // handful of C API entries we need. Returns false with LastFailure set when
        // the runtime is unavailable so callers can fall back to Windows OCR.
        internal static bool EnsureLoaded(string runtimeDir)
        {
            lock (Gate)
            {
                if (_initialised) return _failure == null;
                _initialised = true;
                try
                {
                    string dll = Path.Combine(runtimeDir, "onnxruntime.dll");
                    if (!File.Exists(dll))
                        throw new OrtException("缺少 onnxruntime.dll：" + dll);
                    // VC++ runtime ships next to the DLL; let the loader find it there.
                    SetDllDirectoryW(runtimeDir);
                    // LOAD_WITH_ALTERED_SEARCH_PATH makes the VC++ runtime resolve
                    // from the DLL's own folder instead of the process directory.
                    IntPtr module = LoadLibraryExW(dll, IntPtr.Zero, 0x00000008);
                    if (module == IntPtr.Zero)
                        throw new OrtException("无法加载 onnxruntime.dll（错误码 " +
                            Marshal.GetLastWin32Error() + "）");

                    Step("module loaded");

                    IntPtr basePtr = GetProcAddress(module, "OrtGetApiBase");
                    if (basePtr == IntPtr.Zero)
                        throw new OrtException("onnxruntime.dll 缺少 OrtGetApiBase");

                    // OrtGetApiBase is a function returning a pointer to the OrtApiBase
                    // struct, which itself holds two function pointers. Treating the
                    // exported address as the struct is a classic (crashing) mistake.
                    GetApiBaseDelegate getApiBase = (GetApiBaseDelegate)
                        Marshal.GetDelegateForFunctionPointer(basePtr, typeof(GetApiBaseDelegate));
                    IntPtr apiBase = getApiBase();
                    if (apiBase == IntPtr.Zero)
                        throw new OrtException("OrtGetApiBase 返回空指针");

                    GetApiDelegate getApi = (GetApiDelegate)Marshal.GetDelegateForFunctionPointer(
                        Marshal.ReadIntPtr(apiBase, 0), typeof(GetApiDelegate));
                    GetVersionStringDelegate getVersion =
                        (GetVersionStringDelegate)Marshal.GetDelegateForFunctionPointer(
                            Marshal.ReadIntPtr(apiBase, IntPtr.Size),
                            typeof(GetVersionStringDelegate));
                    Step("OrtApiBase resolved");
                    IntPtr versionText = getVersion();
                    RuntimeVersion = versionText == IntPtr.Zero
                        ? "unknown" : Marshal.PtrToStringAnsi(versionText);

                    // New versions only append entries, so a lower version is safe to
                    // index with the constants generated from the 1.20 header.
                    IntPtr api = IntPtr.Zero;
                    uint[] allowed = new uint[] { OrtApiIndex.ApiVersion, 19, 18, 17, 16 };
                    for (int i = 0; i < allowed.Length && api == IntPtr.Zero; i++)
                        api = getApi(allowed[i]);
                    if (api == IntPtr.Zero)
                        throw new OrtException("onnxruntime API 版本不兼容");

                    Step("api table at 0x" + api.ToInt64().ToString("x"));
                    _table = new IntPtr[OrtApiIndex.Count];
                    Marshal.Copy(api, _table, 0, _table.Length);
                    Step("api table copied");
                    Bind();
                    Step("delegates bound");

                    Check(_createEnv(LogLevelWarning, "ttsprint", out _env));
                    Step("env created");
                    Check(_getAllocator(out _allocator));
                    Step("allocator acquired");
                    _failure = null;
                    return true;
                }
                catch (Exception ex)
                {
                    _failure = ex.Message;
                    return false;
                }
            }
        }

        private static void Bind()
        {
            _createEnv = (CreateEnvDelegate)Bind(OrtApiIndex.CreateEnv, typeof(CreateEnvDelegate));
            _createOptions = (CreateSessionOptionsDelegate)Bind(OrtApiIndex.CreateSessionOptions,
                typeof(CreateSessionOptionsDelegate));
            _setThreads = (SetIntraOpNumThreadsDelegate)Bind(OrtApiIndex.SetIntraOpNumThreads,
                typeof(SetIntraOpNumThreadsDelegate));
            _setOptimization = (SetOptimizationDelegate)Bind(
                OrtApiIndex.SetSessionGraphOptimizationLevel, typeof(SetOptimizationDelegate));
            _disableArena = (ToggleDelegate)Bind(OrtApiIndex.DisableCpuMemArena,
                typeof(ToggleDelegate));
            _createSession = (CreateSessionDelegate)Bind(OrtApiIndex.CreateSession,
                typeof(CreateSessionDelegate));
            _run = (RunDelegate)Bind(OrtApiIndex.Run, typeof(RunDelegate));
            _getAllocator = (GetAllocatorDelegate)Bind(OrtApiIndex.GetAllocatorWithDefaultOptions,
                typeof(GetAllocatorDelegate));
            _createTensor = (CreateTensorDelegate)Bind(
                OrtApiIndex.CreateTensorWithDataAsOrtValue, typeof(CreateTensorDelegate));
            _getTensorData = (GetTensorDataDelegate)Bind(OrtApiIndex.GetTensorMutableData,
                typeof(GetTensorDataDelegate));
            _getTensorInfo = (GetTensorInfoDelegate)Bind(OrtApiIndex.GetTensorTypeAndShape,
                typeof(GetTensorInfoDelegate));
            _getDimCount = (GetCountDelegate)Bind(OrtApiIndex.GetDimensionsCount,
                typeof(GetCountDelegate));
            _getElementCount = (GetCountDelegate)Bind(OrtApiIndex.GetTensorShapeElementCount,
                typeof(GetCountDelegate));
            _getDims = (GetDimensionsDelegate)Bind(OrtApiIndex.GetDimensions,
                typeof(GetDimensionsDelegate));
            _getInputName = (GetNameDelegate)Bind(OrtApiIndex.SessionGetInputName,
                typeof(GetNameDelegate));
            _getOutputName = (GetNameDelegate)Bind(OrtApiIndex.SessionGetOutputName,
                typeof(GetNameDelegate));
            _getErrorMessage = (GetErrorMessageDelegate)Bind(OrtApiIndex.GetErrorMessage,
                typeof(GetErrorMessageDelegate));
            _releaseStatus = (ReleaseDelegate)Bind(OrtApiIndex.ReleaseStatus, typeof(ReleaseDelegate));
            _releaseValue = (ReleaseDelegate)Bind(OrtApiIndex.ReleaseValue, typeof(ReleaseDelegate));
            _releaseSession = (ReleaseDelegate)Bind(OrtApiIndex.ReleaseSession,
                typeof(ReleaseDelegate));
            _releaseOptions = (ReleaseDelegate)Bind(OrtApiIndex.ReleaseSessionOptions,
                typeof(ReleaseDelegate));
            _releaseEnv = (ReleaseDelegate)Bind(OrtApiIndex.ReleaseEnv, typeof(ReleaseDelegate));
            _releaseTensorInfo = (ReleaseDelegate)Bind(OrtApiIndex.ReleaseTensorTypeAndShapeInfo,
                typeof(ReleaseDelegate));
        }

        private static Delegate Bind(int index, Type type)
        {
            IntPtr ptr = index < _table.Length ? _table[index] : IntPtr.Zero;
            if (ptr == IntPtr.Zero)
                throw new OrtException("onnxruntime API 缺少所需函数（索引 " + index + "）");
            return Marshal.GetDelegateForFunctionPointer(ptr, type);
        }

        private static void Check(IntPtr status)
        {
            if (status == IntPtr.Zero) return;
            string message = "onnxruntime 调用失败";
            try
            {
                IntPtr ptr = _getErrorMessage(status);
                if (ptr != IntPtr.Zero) message = Marshal.PtrToStringAnsi(ptr);
            }
            finally { _releaseStatus(status); }
            throw new OrtException(message);
        }

        // ---- session -------------------------------------------------------------

        internal sealed class Session : IDisposable
        {
            private IntPtr _handle;
            private IntPtr _inputName;
            private IntPtr _outputName;

            internal string InputName { get; private set; }
            internal string OutputName { get; private set; }

            internal Session(string modelPath, int threads)
            {
                IntPtr options;
                Check(_createOptions(out options));
                try
                {
                    Check(_setThreads(options, threads));
                    Check(_setOptimization(options, GraphOptimizationAll));
                    // A screenshot session runs a handful of inferences and then sits
                    // idle; the arena would hold tens of megabytes between them.
                    Check(_disableArena(options));
                    Check(_createSession(_env, modelPath, options, out _handle));
                }
                finally { _releaseOptions(options); }

                IntPtr inName, outName;
                Check(_getInputName(_handle, 0, _allocator, out inName));
                Check(_getOutputName(_handle, 0, _allocator, out outName));
                InputName = Marshal.PtrToStringAnsi(inName);
                OutputName = Marshal.PtrToStringAnsi(outName);
                _inputName = Marshal.StringToHGlobalAnsi(InputName);
                _outputName = Marshal.StringToHGlobalAnsi(OutputName);
            }

            // Runs a single float input and returns a copy of the first output tensor.
            internal float[] Run(float[] data, long[] shape, out long[] outputShape)
            {
                GCHandle pin = GCHandle.Alloc(data, GCHandleType.Pinned);
                IntPtr inputValue = IntPtr.Zero;
                IntPtr outputValue = IntPtr.Zero;
                IntPtr info = IntPtr.Zero;
                try
                {
                    Check(_createTensor(_allocator, pin.AddrOfPinnedObject(),
                        (UIntPtr)((ulong)data.Length * 4UL), shape,
                        (UIntPtr)(ulong)shape.Length, TypeFloat, out inputValue));

                    IntPtr[] inputNames = new IntPtr[] { _inputName };
                    IntPtr[] inputs = new IntPtr[] { inputValue };
                    IntPtr[] outputNames = new IntPtr[] { _outputName };
                    IntPtr[] outputs = new IntPtr[1];
                    Check(_run(_handle, IntPtr.Zero, inputNames, inputs, (UIntPtr)1,
                        outputNames, (UIntPtr)1, outputs));
                    outputValue = outputs[0];

                    Check(_getTensorInfo(outputValue, out info));
                    long dimCount;
                    Check(_getDimCount(info, out dimCount));
                    if (dimCount < 1) dimCount = 1;
                    long[] dims = new long[dimCount];
                    Check(_getDims(info, dims, (UIntPtr)(ulong)dimCount));
                    long total;
                    Check(_getElementCount(info, out total));
                    outputShape = dims;

                    IntPtr raw;
                    Check(_getTensorData(outputValue, out raw));
                    float[] result = new float[total];
                    Marshal.Copy(raw, result, 0, (int)total);
                    return result;
                }
                finally
                {
                    if (info != IntPtr.Zero) _releaseTensorInfo(info);
                    if (outputValue != IntPtr.Zero) _releaseValue(outputValue);
                    if (inputValue != IntPtr.Zero) _releaseValue(inputValue);
                    pin.Free();
                }
            }

            public void Dispose()
            {
                if (_handle != IntPtr.Zero) { _releaseSession(_handle); _handle = IntPtr.Zero; }
                if (_inputName != IntPtr.Zero) { Marshal.FreeHGlobal(_inputName); _inputName = IntPtr.Zero; }
                if (_outputName != IntPtr.Zero) { Marshal.FreeHGlobal(_outputName); _outputName = IntPtr.Zero; }
            }
        }
    }
}
