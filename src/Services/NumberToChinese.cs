using System.Globalization;
using System.Text;

namespace TTJiang.Services;

/// <summary>数字转中文大写：支持负数、小数（小数位按位读），如 12345 -> 壹万贰仟叁佰肆拾伍。</summary>
public static class NumberToChinese
{
    private const string Digits = "零壹贰叁肆伍陆柒捌玖";
    private static readonly string[] Units = { "", "拾", "佰", "仟" };
    private static readonly string[] BigUnits = { "", "万", "亿", "兆", "京", "垓", "秭", "穰" };

    /// <summary>把字符串形式的数字转成中文大写。非法输入抛 ArgumentException。</summary>
    public static string ToChinese(string input)
    {
        if (input == null) throw new ArgumentException("空输入");
        string s = input.Trim();
        if (s.Length == 0) throw new ArgumentException("空输入");

        bool negative = false;
        if (s[0] == '-')
        {
            negative = true;
            s = s.Substring(1);
        }
        else if (s[0] == '+')
        {
            s = s.Substring(1);
        }

        string intPart = s;
        string fracPart = "";

        int dot = s.IndexOf('.');
        if (dot >= 0)
        {
            intPart = s.Substring(0, dot);
            fracPart = s.Substring(dot + 1);
            if (fracPart.IndexOf('.') >= 0) throw new ArgumentException("多个小数点");
        }

        if (intPart.Length == 0) intPart = "0";
        if (!AllDigits(intPart)) throw new ArgumentException("非法数字");
        if (fracPart.Length > 0 && !AllDigits(fracPart)) throw new ArgumentException("非法数字");

        decimal value;
        if (!decimal.TryParse(intPart, NumberStyles.None, CultureInfo.InvariantCulture, out value))
        {
            throw new ArgumentException("数字超出可处理范围");
        }

        var sb = new StringBuilder(IntToChinese(value));
        if (fracPart.Length > 0)
        {
            sb.Append('点');
            foreach (char c in fracPart)
            {
                sb.Append(Digits[c - '0']);
            }
        }

        string result = sb.ToString();
        return negative ? "负" + result : result;
    }

    private static bool AllDigits(string s)
    {
        if (s.Length == 0) return false;
        foreach (char c in s)
        {
            if (c < '0' || c > '9') return false;
        }
        return true;
    }

    /// <summary>0~9999 的一组装中文。</summary>
    private static string GroupToChinese(int group)
    {
        if (group == 0) return "";

        var sb = new StringBuilder();
        bool zeroPending = false;
        int[] divisors = { 1000, 100, 10, 1 };

        for (int i = 0; i < 4; i++)
        {
            int d = (group / divisors[i]) % 10;
            if (d == 0)
            {
                if (sb.Length > 0) zeroPending = true;
            }
            else
            {
                if (zeroPending)
                {
                    sb.Append('零');
                    zeroPending = false;
                }
                sb.Append(Digits[d]);
                sb.Append(Units[3 - i]);
            }
        }

        string text = sb.ToString();
        if (text.StartsWith("壹拾", StringComparison.Ordinal))
        {
            text = "拾" + text.Substring(2);
        }
        return text;
    }

    private static string IntToChinese(decimal n)
    {
        if (n == 0) return "零";

        var groups = new List<int>();
        while (n > 0)
        {
            groups.Add((int)(n % 10000));
            n = decimal.Floor(n / 10000);
        }

        var parts = new List<string>();
        for (int idx = groups.Count - 1; idx >= 0; idx--)
        {
            int g = groups[idx];
            if (g == 0)
            {
                if (parts.Count > 0 && !parts[parts.Count - 1].EndsWith("零", StringComparison.Ordinal))
                {
                    parts.Add("零");
                }
                continue;
            }

            string groupText = GroupToChinese(g);
            string unit = idx < BigUnits.Length ? BigUnits[idx] : "";
            parts.Add(groupText + unit);
        }

        var sb = new StringBuilder();
        foreach (string p in parts) sb.Append(p);

        string result = sb.ToString();
        while (result.EndsWith("零", StringComparison.Ordinal))
        {
            result = result.Substring(0, result.Length - 1);
        }
        return result;
    }
}
