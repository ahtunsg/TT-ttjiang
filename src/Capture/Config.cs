// Configuration, autostart, single instance, DPI and display metrics.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Text;

namespace TTJiang.Capture
{
    internal sealed class Config
    {
        internal string Hotkey = "Ctrl+1";
        internal string SaveDir = "";
        internal string OcrLanguage = "";
        internal string ImageFormat = "png";
        internal string OcrEngine = "auto";
        internal bool ShowToast = true;
        internal string RecentLanguage = "";

        // 截图配置跟 TT酱 的其他数据一样放在 exe 同级目录，便于整体迁移与删除。
        private static string ConfigDir()
        {
            return SystemInfo.ExeDir();
        }

        internal static string ConfigPath()
        {
            return Path.Combine(ConfigDir(), "config.json");
        }

        internal string EffectiveSaveDir()
        {
            if (!string.IsNullOrEmpty(SaveDir))
                return SaveDir;
            return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                "Pictures", "TT酱");
        }

        internal static Config Load()
        {
            Config cfg = new Config();
            try
            {
                string path = ConfigPath();
                if (!File.Exists(path))
                    return cfg;
                string raw = File.ReadAllText(path, Encoding.UTF8);
                Dictionary<string, object> values = Json.ParseObject(raw);
                cfg.Hotkey = Json.GetString(values, "hotkey", cfg.Hotkey);
                cfg.SaveDir = Json.GetString(values, "save_dir", cfg.SaveDir);
                cfg.OcrLanguage = Json.GetString(values, "ocr_language", cfg.OcrLanguage);
                cfg.ImageFormat = Json.GetString(values, "image_format", cfg.ImageFormat);
                cfg.OcrEngine = Json.GetString(values, "ocr_engine", cfg.OcrEngine);
                cfg.RecentLanguage = Json.GetString(values, "recent_language", cfg.RecentLanguage);
                cfg.ShowToast = Json.GetBool(values, "show_toast", cfg.ShowToast);
            }
            catch (Exception)
            {
                // A damaged config must never prevent startup.
            }
            return cfg;
        }

        internal void Save()
        {
            try
            {
                Directory.CreateDirectory(ConfigDir());
                StringBuilder sb = new StringBuilder();
                sb.AppendLine("{");
                sb.AppendLine("  \"hotkey\": " + Json.Quote(Hotkey) + ",");
                sb.AppendLine("  \"save_dir\": " + Json.Quote(SaveDir) + ",");
                sb.AppendLine("  \"ocr_language\": " + Json.Quote(OcrLanguage) + ",");
                sb.AppendLine("  \"image_format\": " + Json.Quote(ImageFormat) + ",");
                sb.AppendLine("  \"ocr_engine\": " + Json.Quote(OcrEngine) + ",");
                sb.AppendLine("  \"show_toast\": " + (ShowToast ? "true" : "false") + ",");
                sb.AppendLine("  \"recent_language\": " + Json.Quote(RecentLanguage));
                sb.AppendLine("}");
                File.WriteAllText(ConfigPath(), sb.ToString(), new UTF8Encoding(false));
            }
            catch (Exception)
            {
            }
        }
    }

    // Deliberately tiny JSON codec: the config has only scalar string/bool fields.
    // Avoids pulling in System.Runtime.Serialization / System.Web.Extensions.
    internal static class Json
    {
        internal static Dictionary<string, object> ParseObject(string text)
        {
            Dictionary<string, object> result = new Dictionary<string, object>();
            int i = 0;
            Skip(text, ref i);
            if (i >= text.Length || text[i] != '{')
                return result;
            i++;
            while (i < text.Length)
            {
                Skip(text, ref i);
                if (i >= text.Length || text[i] == '}')
                    break;
                if (text[i] == ',') { i++; continue; }
                if (text[i] != '"')
                    break;
                string key = ReadString(text, ref i);
                Skip(text, ref i);
                if (i >= text.Length || text[i] != ':')
                    break;
                i++;
                Skip(text, ref i);
                object value;
                if (i < text.Length && text[i] == '"')
                    value = ReadString(text, ref i);
                else if (i < text.Length && (text[i] == 't' || text[i] == 'f'))
                {
                    value = text.IndexOf("true", i, StringComparison.Ordinal) == i;
                    while (i < text.Length && char.IsLetter(text[i])) i++;
                }
                else if (i < text.Length && text[i] == 'n')
                {
                    value = null;
                    while (i < text.Length && char.IsLetter(text[i])) i++;
                }
                else
                {
                    int start = i;
                    while (i < text.Length && (char.IsDigit(text[i]) || text[i] == '-' ||
                        text[i] == '+' || text[i] == '.' || text[i] == 'e' || text[i] == 'E')) i++;
                    double num;
                    double.TryParse(text.Substring(start, i - start),
                        NumberStyles.Float, CultureInfo.InvariantCulture, out num);
                    value = num;
                }
                result[key] = value;
            }
            return result;
        }

        private static void Skip(string text, ref int i)
        {
            while (i < text.Length && char.IsWhiteSpace(text[i])) i++;
        }

        private static string ReadString(string text, ref int i)
        {
            StringBuilder sb = new StringBuilder();
            i++; // opening quote
            while (i < text.Length)
            {
                char c = text[i++];
                if (c == '"')
                    break;
                if (c != '\\') { sb.Append(c); continue; }
                if (i >= text.Length) break;
                char e = text[i++];
                switch (e)
                {
                    case '"': sb.Append('"'); break;
                    case '\\': sb.Append('\\'); break;
                    case '/': sb.Append('/'); break;
                    case 'b': sb.Append('\b'); break;
                    case 'f': sb.Append('\f'); break;
                    case 'n': sb.Append('\n'); break;
                    case 'r': sb.Append('\r'); break;
                    case 't': sb.Append('\t'); break;
                    case 'u':
                        if (i + 4 <= text.Length)
                        {
                            int code;
                            if (int.TryParse(text.Substring(i, 4), NumberStyles.HexNumber,
                                CultureInfo.InvariantCulture, out code))
                                sb.Append((char)code);
                            i += 4;
                        }
                        break;
                    default: sb.Append(e); break;
                }
            }
            return sb.ToString();
        }

        internal static string Quote(string s)
        {
            if (s == null) s = "";
            StringBuilder sb = new StringBuilder(s.Length + 2);
            sb.Append('"');
            foreach (char c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < 0x20)
                            sb.Append("\\u" + ((int)c).ToString("x4", CultureInfo.InvariantCulture));
                        else
                            sb.Append(c);
                        break;
                }
            }
            sb.Append('"');
            return sb.ToString();
        }

        internal static string GetString(Dictionary<string, object> values, string key, string fallback)
        {
            object v;
            if (values.TryGetValue(key, out v) && v is string)
                return (string)v;
            return fallback;
        }

        internal static bool GetBool(Dictionary<string, object> values, string key, bool fallback)
        {
            object v;
            if (values.TryGetValue(key, out v) && v is bool)
                return (bool)v;
            return fallback;
        }
    }

    internal static class SystemInfo
    {
        internal static string ExePath()
        {
            try { return Assembly.GetEntryAssembly().Location; }
            catch (Exception) { return Environment.GetCommandLineArgs()[0]; }
        }

        internal static string ExeDir()
        {
            try
            {
                string dir = Path.GetDirectoryName(ExePath());
                if (!string.IsNullOrEmpty(dir)) return dir;
            }
            catch (Exception) { }
            return AppDomain.CurrentDomain.BaseDirectory;
        }

        internal static void OpenFolder(string path)
        {
            try
            {
                Directory.CreateDirectory(path);
                Native.ShellExecute(IntPtr.Zero, "open", path, null, null, Native.SW_SHOW);
            }
            catch (Exception) { }
        }

        internal static void VirtualScreen(out int x, out int y, out int w, out int h)
        {
            x = Native.GetSystemMetrics(Native.SM_XVIRTUALSCREEN);
            y = Native.GetSystemMetrics(Native.SM_YVIRTUALSCREEN);
            w = Native.GetSystemMetrics(Native.SM_CXVIRTUALSCREEN);
            h = Native.GetSystemMetrics(Native.SM_CYVIRTUALSCREEN);
            if (w <= 0 || h <= 0)
            {
                x = y = 0;
                w = Native.GetSystemMetrics(Native.SM_CXSCREEN);
                h = Native.GetSystemMetrics(Native.SM_CYSCREEN);
            }
        }

        internal static void WorkArea(out int left, out int top, out int right, out int bottom)
        {
            Native.RECT r = new Native.RECT();
            if (Native.SystemParametersInfo(Native.SPI_GETWORKAREA, 0, ref r, 0))
            {
                left = r.Left; top = r.Top; right = r.Right; bottom = r.Bottom;
            }
            else
            {
                left = top = 0;
                right = Native.GetSystemMetrics(Native.SM_CXSCREEN);
                bottom = Native.GetSystemMetrics(Native.SM_CYSCREEN);
            }
        }

    }
}
