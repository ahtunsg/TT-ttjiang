using Microsoft.Win32;

namespace TTJiang.Services;

/// <summary>开机自启：只写当前用户的 Run 项，不装服务、不改系统目录。</summary>
public static class AutoStart
{
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ValueName = "TTJiang";

    public static string ExePath
    {
        get
        {
            try
            {
                var asm = System.Reflection.Assembly.GetEntryAssembly();
                if (asm != null && !string.IsNullOrEmpty(asm.Location)) return asm.Location;
            }
            catch
            {
                // 取不到就退到下一种方式
            }

            try
            {
                using (var proc = System.Diagnostics.Process.GetCurrentProcess())
                {
                    return proc.MainModule.FileName;
                }
            }
            catch
            {
                return "";
            }
        }
    }

    public static bool IsEnabled
    {
        get
        {
            try
            {
                using (RegistryKey key = Registry.CurrentUser.OpenSubKey(RunKeyPath, false))
                {
                    if (key == null) return false;
                    return key.GetValue(ValueName) != null;
                }
            }
            catch
            {
                return false;
            }
        }
    }

    public static bool SetEnabled(bool enabled)
    {
        try
        {
            using (RegistryKey key = Registry.CurrentUser.OpenSubKey(RunKeyPath, true))
            {
                if (key == null) return false;

                if (enabled)
                {
                    string exe = ExePath;
                    if (exe.Length == 0) return false;
                    key.SetValue(ValueName, "\"" + exe + "\"");
                }
                else
                {
                    key.DeleteValue(ValueName, false);
                }
                return true;
            }
        }
        catch (Exception ex)
        {
            ConfigStore.Log("设置开机自启失败：" + ex.Message);
            return false;
        }
    }
}
