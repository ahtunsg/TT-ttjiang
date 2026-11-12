// Locates the OCR payload that ships in the "ocr" folder next to the exe.
//
// The payload used to be embedded in the assembly and unpacked to
// %LOCALAPPDATA% on first use. Keeping the model weights out of the exe
// instead: the exe stays small, the model files are read straight from disk
// with no first-run extraction (no copy of the payload, none duplicated on disk),
// and swapping a model ships as a plain folder replace rather than a rebuild.
//
// Search order: TTJIANG_RUNTIME environment variable, <exe>\ocr (shipped),
// <exe>\runtime (layout used while developing), then the per-user extraction
// that 3.1.0 and earlier created, so replacing only the exe still works.
using System;
using System.IO;
using System.Reflection;
using System.Threading;

namespace TTJiang.Capture
{
    internal static class Runtime
    {
        private const string FolderName = "ocr";

        // Which files must exist before the engine is allowed to start.
        private static readonly string[] Required =
        {
            "onnxruntime.dll",
            "det.onnx",
            "rec.onnx",
            "keys.txt"
        };

        private static readonly object Gate = new object();
        private static bool _resolved;
        private static string _dir;
        private static string _error;

        internal static string LastError { get { return _error; } }

        internal static string Dir { get { return _dir; } }

        internal static string ExeDir()
        {
            try
            {
                string dir = Path.GetDirectoryName(Assembly.GetEntryAssembly().Location);
                if (!string.IsNullOrEmpty(dir)) return dir;
            }
            catch (Exception) { }
            return AppDomain.CurrentDomain.BaseDirectory;
        }

        // %LOCALAPPDATA%\TT酱\runtime - only used by builds that carried the
        // payload inside the assembly.
        private static string LegacyRoot()
        {
            string local = Environment.GetEnvironmentVariable("LOCALAPPDATA");
            if (string.IsNullOrEmpty(local))
                local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            if (string.IsNullOrEmpty(local)) return null;
            return Path.Combine(Path.Combine(local, "TT酱"), "runtime");
        }

        internal static bool EnsureFiles(out string dir)
        {
            lock (Gate)
            {
                if (_resolved)
                {
                    dir = _dir;
                    return _error == null;
                }
                _resolved = true;
                try
                {
                    string overrideDir = Environment.GetEnvironmentVariable("TTJIANG_RUNTIME");
                    if (!string.IsNullOrEmpty(overrideDir) && HasModels(overrideDir))
                        return Accept(overrideDir, out dir);

                    string shipped = Path.Combine(ExeDir(), FolderName);
                    if (HasModels(shipped)) return Accept(shipped, out dir);

                    string dev = Path.Combine(ExeDir(), "runtime");
                    if (HasModels(dev)) return Accept(dev, out dir);

                    string legacy = FindLegacy();
                    if (legacy != null) return Accept(legacy, out dir);

                    _error = "缺少 ocr 文件夹：det.onnx、rec.onnx、keys.txt 和 onnxruntime.dll 需要与 "
                        + "TT酱.exe 放在同一目录下的 ocr 子目录里。请解压完整压缩包后再运行。";
                    dir = null;
                    return false;
                }
                catch (Exception ex)
                {
                    _error = "定位 OCR 运行文件失败：" + ex.Message;
                    dir = null;
                    return false;
                }
            }
        }

        private static bool Accept(string path, out string dir)
        {
            _dir = path;
            dir = path;
            _error = null;
            return true;
        }

        private static bool HasModels(string path)
        {
            if (string.IsNullOrEmpty(path) || !Directory.Exists(path)) return false;
            foreach (string name in Required)
            {
                if (!File.Exists(Path.Combine(path, name))) return false;
            }
            return true;
        }

        // Looks for an extraction left behind by 3.1.0 or earlier. Newer
        // installs never create one, so this is a compatibility path only.
        private static string FindLegacy()
        {
            try
            {
                string root = LegacyRoot();
                if (root == null || !Directory.Exists(root)) return null;
                foreach (string sub in Directory.GetDirectories(root))
                {
                    if (HasModels(sub)) return sub;
                }
            }
            catch (Exception) { }
            return null;
        }

        // Deletes the payload 3.1.0 unpacked into %LOCALAPPDATA% (about 22 MB
        // per version) once the engine runs from the ocr folder. Called on a
        // background thread at startup. Resolution runs first so a build that
        // still depends on the old extraction never deletes its own files.
        internal static void CleanLegacy()
        {
            try
            {
                string root = LegacyRoot();
                if (root == null || !Directory.Exists(root)) return;
                string dir;
                if (!EnsureFiles(out dir)) return;
                if (!string.IsNullOrEmpty(dir) &&
                    dir.StartsWith(root, StringComparison.OrdinalIgnoreCase)) return;
                foreach (string sub in Directory.GetDirectories(root))
                {
                    DeleteTree(sub);
                }
                try { Directory.Delete(root, false); }
                catch (Exception) { }
            }
            catch (Exception) { }
        }

        // Directory.Delete(true) gives up on the first file the antivirus or the
        // indexer happens to hold, leaving a half-deleted folder behind. Retry
        // the whole tree a few times, then settle for what is left.
        private static void DeleteTree(string path)
        {
            for (int attempt = 0; attempt < 4; attempt++)
            {
                try
                {
                    Directory.Delete(path, true);
                    return;
                }
                catch (Exception)
                {
                    try
                    {
                        foreach (string file in Directory.GetFiles(path))
                        {
                            try { File.Delete(file); }
                            catch (Exception) { }
                        }
                    }
                    catch (Exception) { }
                    if (attempt < 3) Thread.Sleep(250);
                }
            }
        }
    }
}
