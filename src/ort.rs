// Minimal in-process binding to the ONNX Runtime C API.
//
// 逐行移植自 github-upload/src/Capture/Ort.cs，函数指针下标来自
// github-upload/src/Capture/OrtApi.g.cs（由 onnxruntime_c_api.h v1.20.1 生成，
// OrtApi 是有序函数指针表，Count = 285）。
//
// 与 C# 完全一致的做法：
//   * 先 SetDllDirectoryW(runtimeDir)，再 LoadLibraryExW(dll, 0, 0x8)，
//     从 ocr/ 目录动态加载 onnxruntime.dll（绝不静态链接、绝不使用 ort crate）。
//   * 取导出函数 OrtGetApiBase（注意它返回的是 OrtApiBase*，而非结构体本身）。
//   * getApi 允许版本 [20, 19, 18, 17, 16]，逐版本回退。
//   * 从函数指针表按 OrtApi.g.cs 的索引绑定所需条目。
//   * Session 参数：SetIntraOpNumThreads、SetSessionGraphOptimizationLevel=99、
//     CreateEnv(LogLevelWarning=2, "ttsprint")、GetAllocatorWithDefaultOptions，
//     Run 后取输出 shape 与 float 数据。
//   * 4.3 起不再调用 DisableCpuMemArena：每个文字行都会单独 Run 一次，
//     启用 CPU memory arena 可复用中间张量内存；17 张回归图实测约省 10%，
//     识别结果逐字一致。函数指针仍保留绑定，便于需要时回退。
//
// 零 crates.io 依赖，只用 std；Win32 全部用 #[link(name="kernel32")] 手写。
// ORT C API 在 Windows 上是 ORT_API_CALL = __stdcall，故函数指针一律用
// "system" 调用约定（x86_64 下与 C# 使用的 Cdecl 等价，且对 Win32 也正确）。

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

// ---- Ort.cs 常量 ------------------------------------------------------------
const LOG_LEVEL_WARNING: i32 = 2;         // Ort.cs: private const int LogLevelWarning = 2;
const GRAPH_OPTIMIZATION_ALL: i32 = 99;   // Ort.cs: private const int GraphOptimizationAll = 99;
const TYPE_FLOAT: i32 = 1;                // Ort.cs: private const int TypeFloat = 1;
const LOG_ID: &[u8] = b"ttsprint\0";      // Ort.cs: _createEnv(LogLevelWarning, "ttsprint", ...)
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008; // Ort.cs: LoadLibraryExW(dll, 0, 0x8)

// ---- OrtApi.g.cs 下标（OrtApi 有序函数指针表，v1.20.1，Count = 285）--------
const API_COUNT: usize = 285;                              // OrtApiIndex.Count
const API_VERSION: u32 = 20;                               // OrtApiIndex.ApiVersion
const IDX_GET_ERROR_MESSAGE: usize = 2;                    // OrtApiIndex.GetErrorMessage
const IDX_CREATE_ENV: usize = 3;                           // OrtApiIndex.CreateEnv
const IDX_CREATE_SESSION: usize = 7;                       // OrtApiIndex.CreateSession
const IDX_RUN: usize = 9;                                  // OrtApiIndex.Run
const IDX_CREATE_SESSION_OPTIONS: usize = 10;              // OrtApiIndex.CreateSessionOptions
#[allow(dead_code)] // 4.3 起不再调用，保留绑定以便回退
const IDX_DISABLE_CPU_MEM_ARENA: usize = 19;               // OrtApiIndex.DisableCpuMemArena
const IDX_SET_SESSION_GRAPH_OPTIMIZATION_LEVEL: usize = 23;// OrtApiIndex.SetSessionGraphOptimizationLevel
const IDX_SET_INTRA_OP_NUM_THREADS: usize = 24;            // OrtApiIndex.SetIntraOpNumThreads
const IDX_SESSION_GET_INPUT_NAME: usize = 36;              // OrtApiIndex.SessionGetInputName
const IDX_SESSION_GET_OUTPUT_NAME: usize = 37;             // OrtApiIndex.SessionGetOutputName
const IDX_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE: usize = 49;// OrtApiIndex.CreateTensorWithDataAsOrtValue
const IDX_GET_TENSOR_MUTABLE_DATA: usize = 51;             // OrtApiIndex.GetTensorMutableData
const IDX_GET_DIMENSIONS_COUNT: usize = 61;                // OrtApiIndex.GetDimensionsCount
const IDX_GET_DIMENSIONS: usize = 62;                      // OrtApiIndex.GetDimensions
const IDX_GET_TENSOR_SHAPE_ELEMENT_COUNT: usize = 64;      // OrtApiIndex.GetTensorShapeElementCount
const IDX_GET_TENSOR_TYPE_AND_SHAPE: usize = 65;           // OrtApiIndex.GetTensorTypeAndShape
const IDX_GET_ALLOCATOR_WITH_DEFAULT_OPTIONS: usize = 78;  // OrtApiIndex.GetAllocatorWithDefaultOptions
const IDX_RELEASE_ENV: usize = 92;                         // OrtApiIndex.ReleaseEnv（Ort.cs 仅绑定）
const IDX_RELEASE_STATUS: usize = 93;                      // OrtApiIndex.ReleaseStatus
const IDX_RELEASE_SESSION: usize = 95;                     // OrtApiIndex.ReleaseSession
const IDX_RELEASE_VALUE: usize = 96;                       // OrtApiIndex.ReleaseValue
const IDX_RELEASE_TENSOR_TYPE_AND_SHAPE_INFO: usize = 99;  // OrtApiIndex.ReleaseTensorTypeAndShapeInfo
const IDX_RELEASE_SESSION_OPTIONS: usize = 100;            // OrtApiIndex.ReleaseSessionOptions

// ---- Win32（全部手写，#[link(name="kernel32")]）-----------------------------
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(lp_lib_file_name: *const u16, h_file: *mut c_void, dw_flags: u32) -> *mut c_void;
    // GetProcAddress 的名称参数是 ANSI（LPCSTR），不能传 UTF-16。
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const c_char) -> *mut c_void;
    fn SetDllDirectoryW(lp_path_name: *const u16) -> i32; // BOOL
    fn GetLastError() -> u32;                             // DWORD
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---- 函数指针类型（对齐 Ort.cs 的 delegates，均为 __stdcall）----------------
type OrtStatus = *mut c_void;

type GetApiBaseFn = unsafe extern "system" fn() -> *const c_void;
type GetApiFn = unsafe extern "system" fn(u32) -> *const c_void;          // OrtApiBase.GetApi
type GetVersionStringFn = unsafe extern "system" fn() -> *const c_char;   // OrtApiBase.GetVersionString

type CreateEnvFn = unsafe extern "system" fn(i32, *const c_char, *mut *mut c_void) -> OrtStatus;
type CreateSessionOptionsFn = unsafe extern "system" fn(*mut *mut c_void) -> OrtStatus;
type SetIntraOpNumThreadsFn = unsafe extern "system" fn(*mut c_void, i32) -> OrtStatus;
type SetOptimizationFn = unsafe extern "system" fn(*mut c_void, i32) -> OrtStatus;
type ToggleFn = unsafe extern "system" fn(*mut c_void) -> OrtStatus;
type CreateSessionFn = unsafe extern "system" fn(*mut c_void, *const u16, *mut c_void, *mut *mut c_void) -> OrtStatus;
type RunFn = unsafe extern "system" fn(
    *mut c_void, *const c_void,
    *const *const c_char, *const *const c_void, usize,
    *const *const c_char, usize, *mut *mut c_void,
) -> OrtStatus;
type GetAllocatorFn = unsafe extern "system" fn(*mut *mut c_void) -> OrtStatus;
type CreateTensorFn = unsafe extern "system" fn(
    *const c_void, *mut c_void, usize, *const i64, usize, i32, *mut *mut c_void,
) -> OrtStatus;
type GetTensorDataFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> OrtStatus;
type GetTensorInfoFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> OrtStatus;
type GetCountFn = unsafe extern "system" fn(*mut c_void, *mut usize) -> OrtStatus; // size_t*（C# out long）
type GetDimensionsFn = unsafe extern "system" fn(*mut c_void, *mut i64, usize) -> OrtStatus;
type GetNameFn = unsafe extern "system" fn(*mut c_void, usize, *mut c_void, *mut *mut c_char) -> OrtStatus;
type GetErrorMessageFn = unsafe extern "system" fn(*mut c_void) -> *const c_char;
type ReleaseFn = unsafe extern "system" fn(*mut c_void); // 返回 void

// ---- 全局状态：对应 Ort.cs 的 static 字段 + Gate（OnceLock 保证仅加载一次）----
struct OrtState {
    env: *mut c_void,
    allocator: *mut c_void,
    version: String,

    create_env: CreateEnvFn,
    create_options: CreateSessionOptionsFn,
    set_threads: SetIntraOpNumThreadsFn,
    set_optimization: SetOptimizationFn,
    #[allow(dead_code)] // 4.3 起不再调用，保留绑定以便回退
    disable_arena: ToggleFn,
    create_session: CreateSessionFn,
    run: RunFn,
    get_allocator: GetAllocatorFn,
    create_tensor: CreateTensorFn,
    get_tensor_data: GetTensorDataFn,
    get_tensor_info: GetTensorInfoFn,
    get_dim_count: GetCountFn,
    get_element_count: GetCountFn,
    get_dims: GetDimensionsFn,
    get_input_name: GetNameFn,
    get_output_name: GetNameFn,
    get_error_message: GetErrorMessageFn,
    release_status: ReleaseFn,
    release_value: ReleaseFn,
    release_session: ReleaseFn,
    release_options: ReleaseFn,
    #[allow(dead_code)]
    release_env: ReleaseFn,
    release_tensor_info: ReleaseFn,
}

// onnxruntime 的 env / allocator / session 可跨线程共享，Run 本身线程安全
// （C# 注释：sessions accept concurrent Run calls）。函数指针与 String 本身是 Send+Sync。
unsafe impl Send for OrtState {}
unsafe impl Sync for OrtState {}

static ORT: OnceLock<Result<OrtState, String>> = OnceLock::new();

fn ort_state() -> Result<&'static OrtState, String> {
    match ORT.get() {
        Some(Ok(s)) => Ok(s),
        Some(Err(e)) => Err(e.clone()), // Ort.cs: 缓存 _failure，失败后直接返回
        None => Err("onnxruntime 尚未初始化（请先调用 ensure_loaded）".to_string()),
    }
}

/// 对应 Ort.EnsureLoaded(runtimeDir)：从 ocr/ 目录加载 DLL、绑定 API、创建 env 与 allocator。
/// 只初始化一次（OnceLock），失败信息被缓存，与 C# 的 _initialised / _failure 语义一致。
pub fn ensure_loaded(dir: &str) -> Result<(), String> {
    match ORT.get_or_init(|| unsafe { load_ort(dir) }) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.clone()),
    }
}

/// 对应 Ort.RuntimeVersion。
pub fn runtime_version() -> String {
    match ORT.get() {
        Some(Ok(s)) => s.version.clone(),
        _ => "unknown".to_string(),
    }
}

// 从函数指针表按索引取函数：对应 Ort.Bind(int index, Type type)。
// 使用 std::mem::transmute_copy 把 usize 地址转成函数指针。
unsafe fn bind_fn<T: Copy>(table: &[usize], index: usize) -> Result<T, String> {
    let ptr = table.get(index).copied().unwrap_or(0);
    if ptr == 0 {
        return Err(format!("onnxruntime API 缺少所需函数（索引 {}）", index));
    }
    Ok(std::mem::transmute_copy::<usize, T>(&ptr))
}

// 对应 Ort.Check(IntPtr status)：把非空 status 转成错误消息并释放。
unsafe fn check_status(s: &OrtState, status: OrtStatus) -> Result<(), String> {
    if status.is_null() {
        return Ok(());
    }
    let message = {
        let p = (s.get_error_message)(status);
        if p.is_null() {
            "onnxruntime 调用失败".to_string()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    (s.release_status)(status);
    Err(message)
}

// Ort.EnsureLoaded 主体。
unsafe fn load_ort(dir: &str) -> Result<OrtState, String> {
    // Ort.EnsureLoaded: dll = Path.Combine(runtimeDir, "onnxruntime.dll")
    let dll = std::path::Path::new(dir).join("onnxruntime.dll");
    if !dll.exists() {
        return Err(format!("缺少 onnxruntime.dll：{}", dll.to_string_lossy()));
    }
    // Ort.EnsureLoaded: SetDllDirectoryW(runtimeDir)（让 VC++ 运行库从 DLL 同目录解析）
    let dir_w = to_wide(dir);
    SetDllDirectoryW(dir_w.as_ptr());
    // Ort.EnsureLoaded: LoadLibraryExW(dll, IntPtr.Zero, 0x00000008)
    let dll_w = to_wide(&dll.to_string_lossy());
    let module = LoadLibraryExW(dll_w.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
    if module.is_null() {
        let code = GetLastError();
        return Err(format!("无法加载 onnxruntime.dll（错误码 {}）", code));
    }

    // Ort.EnsureLoaded: GetProcAddress(module, "OrtGetApiBase")
    let base_ptr = GetProcAddress(module, b"OrtGetApiBase\0".as_ptr() as *const c_char);
    if base_ptr.is_null() {
        return Err("onnxruntime.dll 缺少 OrtGetApiBase".to_string());
    }
    // 导出地址是一个函数（返回 OrtApiBase*），不是结构体本身。
    let get_api_base: GetApiBaseFn = std::mem::transmute_copy(&base_ptr);
    let api_base = get_api_base();
    if api_base.is_null() {
        return Err("OrtGetApiBase 返回空指针".to_string());
    }

    // OrtApiBase 头两个函数指针：offset 0 = GetApi，offset IntPtr.Size = GetVersionString。
    #[repr(C)]
    struct OrtApiBase {
        get_api: GetApiFn,
        get_version_string: GetVersionStringFn,
    }
    let base = &*(api_base as *const OrtApiBase);
    let version_text = (base.get_version_string)();
    let version = if version_text.is_null() {
        "unknown".to_string()
    } else {
        CStr::from_ptr(version_text).to_string_lossy().into_owned()
    };

    // Ort.EnsureLoaded: getApi 允许版本 [ApiVersion, 19, 18, 17, 16]，新版本只追加表项。
    let allowed = [API_VERSION, 19u32, 18, 17, 16];
    let mut api: *const c_void = std::ptr::null();
    for v in allowed {
        let p = (base.get_api)(v);
        if !p.is_null() {
            api = p;
            break;
        }
    }
    if api.is_null() {
        return Err("onnxruntime API 版本不兼容".to_string());
    }

    // Ort.EnsureLoaded: Marshal.Copy(api, _table, 0, OrtApiIndex.Count) —— 285 个指针。
    let table: &[usize] = std::slice::from_raw_parts(api as *const usize, API_COUNT);

    // Ort.Bind()：按 OrtApi.g.cs 索引绑定全部所需条目。
    let mut state = OrtState {
        env: std::ptr::null_mut(),
        allocator: std::ptr::null_mut(),
        version,
        create_env: bind_fn(table, IDX_CREATE_ENV)?,
        create_options: bind_fn(table, IDX_CREATE_SESSION_OPTIONS)?,
        set_threads: bind_fn(table, IDX_SET_INTRA_OP_NUM_THREADS)?,
        set_optimization: bind_fn(table, IDX_SET_SESSION_GRAPH_OPTIMIZATION_LEVEL)?,
        disable_arena: bind_fn(table, IDX_DISABLE_CPU_MEM_ARENA)?,
        create_session: bind_fn(table, IDX_CREATE_SESSION)?,
        run: bind_fn(table, IDX_RUN)?,
        get_allocator: bind_fn(table, IDX_GET_ALLOCATOR_WITH_DEFAULT_OPTIONS)?,
        create_tensor: bind_fn(table, IDX_CREATE_TENSOR_WITH_DATA_AS_ORT_VALUE)?,
        get_tensor_data: bind_fn(table, IDX_GET_TENSOR_MUTABLE_DATA)?,
        get_tensor_info: bind_fn(table, IDX_GET_TENSOR_TYPE_AND_SHAPE)?,
        get_dim_count: bind_fn(table, IDX_GET_DIMENSIONS_COUNT)?,
        get_element_count: bind_fn(table, IDX_GET_TENSOR_SHAPE_ELEMENT_COUNT)?,
        get_dims: bind_fn(table, IDX_GET_DIMENSIONS)?,
        get_input_name: bind_fn(table, IDX_SESSION_GET_INPUT_NAME)?,
        get_output_name: bind_fn(table, IDX_SESSION_GET_OUTPUT_NAME)?,
        get_error_message: bind_fn(table, IDX_GET_ERROR_MESSAGE)?,
        release_status: bind_fn(table, IDX_RELEASE_STATUS)?,
        release_value: bind_fn(table, IDX_RELEASE_VALUE)?,
        release_session: bind_fn(table, IDX_RELEASE_SESSION)?,
        release_options: bind_fn(table, IDX_RELEASE_SESSION_OPTIONS)?,
        release_env: bind_fn(table, IDX_RELEASE_ENV)?,
        release_tensor_info: bind_fn(table, IDX_RELEASE_TENSOR_TYPE_AND_SHAPE_INFO)?,
    };

    // Ort.EnsureLoaded: Check(_createEnv(LogLevelWarning, "ttsprint", out _env))
    let mut env: *mut c_void = std::ptr::null_mut();
    let st = (state.create_env)(LOG_LEVEL_WARNING, LOG_ID.as_ptr() as *const c_char, &mut env);
    check_status(&state, st)?;
    // Ort.EnsureLoaded: Check(_getAllocator(out _allocator))
    let mut allocator: *mut c_void = std::ptr::null_mut();
    let st = (state.get_allocator)(&mut allocator);
    check_status(&state, st)?;
    state.env = env;
    state.allocator = allocator;
    Ok(state)
}

// ---- session -----------------------------------------------------------------

/// 对应 Ort.Session（每个实例持有自己的 handle 与 ANSI 输入/输出名）。
pub struct Session {
    handle: *mut c_void,
    input_name: CString,
    output_name: CString,
}

// 见 OrtState 的说明：ORT handle 可跨线程共享。
unsafe impl Send for Session {}
unsafe impl Sync for Session {}

// Ort.Session..ctor 中 try/finally 保证 options 一定被释放。
struct OptionsGuard<'a> {
    p: *mut c_void,
    s: &'a OrtState,
}
impl Drop for OptionsGuard<'_> {
    fn drop(&mut self) {
        if !self.p.is_null() {
            unsafe { (self.s.release_options)(self.p); }
        }
    }
}

impl Session {
    /// 对应 Ort.Session(string modelPath, int threads)。
    pub fn new(path: &str, threads: i32) -> Result<Session, String> {
        let s = ort_state()?;
        unsafe {
            // Ort.Session..ctor: CreateSessionOptions
            let mut options: *mut c_void = std::ptr::null_mut();
            let st = (s.create_options)(&mut options);
            check_status(s, st)?;
            let guard = OptionsGuard { p: options, s };

            // Ort.Session..ctor: SetIntraOpNumThreads(options, threads)
            let st = (s.set_threads)(options, threads);
            check_status(s, st)?;
            // Ort.Session..ctor: SetSessionGraphOptimizationLevel(options, GraphOptimizationAll=99)
            let st = (s.set_optimization)(options, GRAPH_OPTIMIZATION_ALL);
            check_status(s, st)?;
            // 4.3：不再调用 DisableCpuMemArena，保留默认 CPU memory arena。
            // 每个文字行单独 Run 一次，复用中间张量内存约省 10%，结果逐字一致。
            // Ort.Session..ctor: CreateSession(env, modelPath(LPWStr), options, out handle)
            let mut handle: *mut c_void = std::ptr::null_mut();
            let path_w = to_wide(path);
            let st = (s.create_session)(s.env, path_w.as_ptr(), options, &mut handle);
            check_status(s, st)?;
            drop(guard); // 对应 Ort.Session..ctor 的 finally { _releaseOptions(options); }

            // Ort.Session..ctor: SessionGetInputName / SessionGetOutputName(index 0, allocator)
            let mut in_name: *mut c_char = std::ptr::null_mut();
            let st = (s.get_input_name)(handle, 0, s.allocator, &mut in_name);
            check_status(s, st)?;
            let mut out_name: *mut c_char = std::ptr::null_mut();
            let st = (s.get_output_name)(handle, 0, s.allocator, &mut out_name);
            check_status(s, st)?;
            // Ort.Session..ctor: PtrToStringAnsi -> StringToHGlobalAnsi（保存一份 ANSI 名供 Run 用）
            let input_cstr = CStr::from_ptr(in_name).to_owned();
            let output_cstr = CStr::from_ptr(out_name).to_owned();
            Ok(Session { handle, input_name: input_cstr, output_name: output_cstr })
        }
    }

    /// 对应 Ort.Session.Run(float[] data, long[] shape, out long[] outputShape)：
    /// 单个 float 输入，返回第一份输出张量的数据拷贝与 shape。
    pub fn run(&self, data: Vec<f32>, shape: &[i64]) -> Result<(Vec<f32>, Vec<i64>), String> {
        let s = ort_state()?;
        unsafe {
            // Ort.Session.Run 的 finally 等价物：无论成功失败都释放 info/outputValue/inputValue。
            let mut guard = RunGuard {
                s,
                input_value: std::ptr::null_mut(),
                output_value: std::ptr::null_mut(),
                info: std::ptr::null_mut(),
            };

            // Ort.Session.Run: CreateTensorWithDataAsOrtValue(_allocator, pinned data,
            //   data.Length*4, shape, shape.Length, TypeFloat=1, out inputValue)
            // 注：此处逐行照搬 Ort.cs —— 把 GetAllocatorWithDefaultOptions 得到的指针
            // 作为首参（C API 声明为 OrtMemoryInfo*）传入，未改用 CreateCpuMemoryInfo。
            let st = (s.create_tensor)(
                s.allocator,
                data.as_ptr() as *mut c_void,
                data.len() * 4,
                shape.as_ptr(),
                shape.len(),
                TYPE_FLOAT,
                &mut guard.input_value,
            );
            check_status(s, st)?;

            // Ort.Session.Run: Run(handle, null, inputNames, inputs, 1, outputNames, 1, outputs)
            let in_names: [*const c_char; 1] = [self.input_name.as_ptr()];
            let in_values: [*const c_void; 1] = [guard.input_value as *const c_void];
            let out_names: [*const c_char; 1] = [self.output_name.as_ptr()];
            let mut out_values: [*mut c_void; 1] = [std::ptr::null_mut()];
            let st = (s.run)(
                self.handle,
                std::ptr::null(),
                in_names.as_ptr(),
                in_values.as_ptr(),
                1,
                out_names.as_ptr(),
                1,
                out_values.as_mut_ptr(),
            );
            check_status(s, st)?;
            guard.output_value = out_values[0];

            // Ort.Session.Run: GetTensorTypeAndShape -> GetDimensionsCount
            let st = (s.get_tensor_info)(guard.output_value, &mut guard.info);
            check_status(s, st)?;
            let mut dim_count: usize = 0;
            let st = (s.get_dim_count)(guard.info, &mut dim_count);
            check_status(s, st)?;
            if dim_count < 1 {
                dim_count = 1;
            }
            // Ort.Session.Run: GetDimensions(info, dims, dimCount)
            let mut dims: Vec<i64> = vec![0i64; dim_count];
            let st = (s.get_dims)(guard.info, dims.as_mut_ptr(), dim_count);
            check_status(s, st)?;
            // Ort.Session.Run: GetTensorShapeElementCount(info, out total)
            let mut total: usize = 0;
            let st = (s.get_element_count)(guard.info, &mut total);
            check_status(s, st)?;

            // Ort.Session.Run: GetTensorMutableData -> Marshal.Copy(raw, result, 0, total)
            let mut raw: *mut c_void = std::ptr::null_mut();
            let st = (s.get_tensor_data)(guard.output_value, &mut raw);
            check_status(s, st)?;
            let out = if raw.is_null() || total == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(raw as *const f32, total).to_vec()
            };
            Ok((out, dims)) // guard 在此 drop，释放 info/outputValue/inputValue
        }
    }
}

// Ort.Session.Run 的 finally 等价物。
struct RunGuard<'a> {
    s: &'a OrtState,
    input_value: *mut c_void,
    output_value: *mut c_void,
    info: *mut c_void,
}
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            if !self.info.is_null() {
                (self.s.release_tensor_info)(self.info);
            }
            if !self.output_value.is_null() {
                (self.s.release_value)(self.output_value);
            }
            if !self.input_value.is_null() {
                (self.s.release_value)(self.input_value);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Ort.Session.Dispose: ReleaseSession(handle)；CString 自动释放 HGlobal 名。
        if !self.handle.is_null() {
            if let Some(Ok(s)) = ORT.get() {
                unsafe { (s.release_session)(self.handle); }
            }
            self.handle = std::ptr::null_mut();
        }
    }
}
