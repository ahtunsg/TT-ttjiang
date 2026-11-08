using System.Globalization;
using System.Text;

namespace TTJiang.Services;

/// <summary>
/// 极简 JSON 读写。配置文件结构固定（一个数组 + 一个对象），
/// 自己解析可以做到零第三方依赖、产物不带额外 dll，同时保持中文不转义、可手改。
/// 解析结果用 Dictionary&lt;string,object&gt; / List&lt;object&gt; / string / double / bool / null 表示。
/// </summary>
internal static class JsonLite
{
    public static object Parse(string text)
    {
        if (text == null) throw new FormatException("空文本");
        var parser = new Parser(text);
        return parser.ParseDocument();
    }

    /// <summary>转成 JSON 字符串字面量（含两侧引号），中文原样保留。</summary>
    public static string Quote(string value)
    {
        var sb = new StringBuilder((value ?? "").Length + 2);
        sb.Append('"');
        foreach (char c in value ?? "")
        {
            switch (c)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\b': sb.Append("\\b"); break;
                case '\f': sb.Append("\\f"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                default:
                    if (c < 0x20)
                    {
                        sb.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                    }
                    else
                    {
                        sb.Append(c);
                    }
                    break;
            }
        }
        sb.Append('"');
        return sb.ToString();
    }

    private sealed class Parser
    {
        private readonly string _s;
        private int _i;

        public Parser(string s)
        {
            _s = s;
            _i = 0;
        }

        public object ParseDocument()
        {
            SkipWs();
            object value = ParseValue();
            SkipWs();
            return value;
        }

        private object ParseValue()
        {
            SkipWs();
            if (_i >= _s.Length) throw new FormatException("JSON 内容不完整");

            char c = _s[_i];
            switch (c)
            {
                case '{': return ParseObject();
                case '[': return ParseArray();
                case '"': return ParseString();
                case 't': Expect("true"); return true;
                case 'f': Expect("false"); return false;
                case 'n': Expect("null"); return null;
                default: return ParseNumber();
            }
        }

        private Dictionary<string, object> ParseObject()
        {
            var map = new Dictionary<string, object>(StringComparer.OrdinalIgnoreCase);
            _i++; // {

            while (true)
            {
                SkipWs();
                if (_i >= _s.Length) throw new FormatException("对象未闭合");
                if (_s[_i] == '}') { _i++; return map; }
                if (_s[_i] == ',') { _i++; continue; }

                string key = ParseString();
                SkipWs();
                if (_i >= _s.Length || _s[_i] != ':') throw new FormatException("缺少冒号");
                _i++;
                map[key] = ParseValue();
            }
        }

        private List<object> ParseArray()
        {
            var list = new List<object>();
            _i++; // [

            while (true)
            {
                SkipWs();
                if (_i >= _s.Length) throw new FormatException("数组未闭合");
                if (_s[_i] == ']') { _i++; return list; }
                if (_s[_i] == ',') { _i++; continue; }
                list.Add(ParseValue());
            }
        }

        private string ParseString()
        {
            SkipWs();
            if (_i >= _s.Length || _s[_i] != '"') throw new FormatException("缺少字符串");

            _i++;
            var sb = new StringBuilder();

            while (_i < _s.Length)
            {
                char c = _s[_i++];
                if (c == '"') return sb.ToString();

                if (c != '\\')
                {
                    sb.Append(c);
                    continue;
                }

                if (_i >= _s.Length) break;
                char esc = _s[_i++];
                switch (esc)
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
                        if (_i + 4 > _s.Length) throw new FormatException("\\u 转义不完整");
                        string hex = _s.Substring(_i, 4);
                        _i += 4;
                        sb.Append((char)int.Parse(hex, NumberStyles.HexNumber, CultureInfo.InvariantCulture));
                        break;
                    default:
                        sb.Append(esc);
                        break;
                }
            }

            throw new FormatException("字符串未闭合");
        }

        private object ParseNumber()
        {
            int start = _i;
            while (_i < _s.Length)
            {
                char c = _s[_i];
                if (char.IsDigit(c) || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E') _i++;
                else break;
            }

            string token = _s.Substring(start, _i - start);
            double d;
            if (!double.TryParse(token, NumberStyles.Float, CultureInfo.InvariantCulture, out d))
            {
                throw new FormatException("非法数字：" + token);
            }
            return d;
        }

        private void Expect(string literal)
        {
            if (_i + literal.Length > _s.Length || string.CompareOrdinal(_s, _i, literal, 0, literal.Length) != 0)
            {
                throw new FormatException("非法字面量");
            }
            _i += literal.Length;
        }

        private void SkipWs()
        {
            while (_i < _s.Length)
            {
                char c = _s[_i];
                if (c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '\uFEFF')
                {
                    _i++;
                    continue;
                }

                if (c == '/' && _i + 1 < _s.Length)
                {
                    if (_s[_i + 1] == '/')
                    {
                        _i += 2;
                        while (_i < _s.Length && _s[_i] != '\n') _i++;
                        continue;
                    }

                    if (_s[_i + 1] == '*')
                    {
                        _i += 2;
                        while (_i + 1 < _s.Length && !(_s[_i] == '*' && _s[_i + 1] == '/')) _i++;
                        _i = Math.Min(_i + 2, _s.Length);
                        continue;
                    }
                }

                break;
            }
        }
    }
}
