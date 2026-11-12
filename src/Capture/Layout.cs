// Geometry-based layout reconstruction, ported from the Python implementation.
// Windows OCR splits Chinese into single characters and orders table lines by
// text block, not by visual row. Only word bounding boxes are trusted.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace TTJiang.Capture
{
    internal sealed class OcrWord
    {
        internal string Text;
        internal double X;
        internal double Y;
        internal double W;
        internal double H;

        internal double Right { get { return X + W; } }
        internal double Cy { get { return Y + H / 2.0; } }
    }

    internal sealed class OcrResult
    {
        internal readonly List<OcrWord> Words = new List<OcrWord>();
        internal string Text = "";
        internal string Language = "";
        internal string Engine = "";
        internal int MaxImageDimension;
    }

    internal sealed class Cell
    {
        internal double X1;
        internal double X2;
        internal string Text;
    }

    internal static class Layout
    {
        // ':' is deliberately absent: "Name: Ryan" is a normal English pair and
        // the CJK check below already keeps "说明：内容" free of separators.
        private const string AsciiPunct = ".,;!?%)]}/\\|";
        private const string OpenPunct = "([{<";

        private static double Median(List<double> values, double fallback)
        {
            if (values.Count == 0) return fallback;
            values.Sort();
            int n = values.Count;
            if ((n & 1) == 1) return values[n / 2];
            return (values[n / 2 - 1] + values[n / 2]) / 2.0;
        }

        private static bool IsCjk(char c)
        {
            return (c >= 0x2E80 && c <= 0x9FFF)
                || (c >= 0x3000 && c <= 0x303F)
                || (c >= 0xFF00 && c <= 0xFFEF);
        }

        private static string Normalize(string text, bool tableMode)
        {
            // Full-width numeric signs are OCR artefacts, not Chinese punctuation.
            // Normalise them in both plain-text and table mode so pasted numbers
            // are parsed as numbers by Excel. A full-width space becomes a normal
            // one so the CJK cleanup below can see it.
            return text.Replace('\uFF0E', '.').Replace('\uFF05', '%')
                .Replace('\uFF0D', '-').Replace('\u3000', ' ');
        }

        // Between two digits, a decimal point is often read as a comma / middle
        // dot, and a dash as the Chinese character 一. Repair only that exact
        // context so ordinary Chinese punctuation and wording stay untouched.
        internal static string FinalizeText(string text)
        {
            if (string.IsNullOrEmpty(text)) return "";
            StringBuilder sb = new StringBuilder(text);
            for (int i = 1; i + 1 < sb.Length; i++)
            {
                char c = sb[i];
                if (!char.IsDigit(sb[i - 1]) || !char.IsDigit(sb[i + 1])) continue;
                if (c == ',')
                {
                    // "1,234.56" is a thousands separator, not a misread decimal
                    // point: exactly three digits follow before the next stop.
                    // "1,5" keeps the old repair.
                    int k = i + 1;
                    int digits = 0;
                    while (k < sb.Length && char.IsDigit(sb[k])) { k++; digits++; }
                    bool thousands = digits == 3 && (k >= sb.Length || !char.IsDigit(sb[k]));
                    if (thousands) continue;
                    sb[i] = '.';
                    continue;
                }
                if (c == '\uFF0E' || c == '\uFF0C' || c == '\u3001' ||
                    c == '\u00B7')
                    sb[i] = '.';
                else if (c == '\u4E00' || c == '\u2014' || c == '\u2013')
                    sb[i] = '-';
            }
            return RepairDrivePaths(sb.ToString());
        }

        // The recogniser reads "\" as "/" and often doubles it, so a path line
        // comes back as "C://Users//Ryan//Desktop". Only lines that begin with a
        // drive letter are touched, which keeps URLs and prose out of it.
        private static string RepairDrivePaths(string text)
        {
            if (text.IndexOf(':') < 0) return text;
            string[] lines = text.Split('\n');
            bool changed = false;
            for (int i = 0; i < lines.Length; i++)
            {
                string fixedLine = RepairDrivePath(lines[i]);
                if (fixedLine != lines[i]) { lines[i] = fixedLine; changed = true; }
            }
            return changed ? string.Join("\n", lines) : text;
        }

        private static string RepairDrivePath(string line)
        {
            int start = 0;
            while (start < line.Length && (line[start] == ' ' || line[start] == '\t')) start++;
            if (start + 2 >= line.Length) return line;
            char drive = line[start];
            bool letter = (drive >= 'A' && drive <= 'Z') || (drive >= 'a' && drive <= 'z');
            if (!letter || line[start + 1] != ':') return line;
            char sep = line[start + 2];
            if (sep != '/' && sep != '\\') return line;

            StringBuilder sb = new StringBuilder(line.Length);
            sb.Append(line, 0, start + 2);
            bool lastSep = false;
            for (int i = start + 2; i < line.Length; i++)
            {
                char c = line[i];
                if (c == '/' || c == '\\')
                {
                    if (!lastSep) { sb.Append('\\'); lastSep = true; }
                    continue;
                }
                lastSep = false;
                sb.Append(c);
            }
            return sb.ToString();
        }

        internal static string Flatten(string text)
        {
            if (string.IsNullOrEmpty(text)) return "";
            StringBuilder sb = new StringBuilder(text.Length);
            foreach (char c in text)
                if (!char.IsWhiteSpace(c)) sb.Append(c);
            return sb.ToString();
        }

        private static char FirstNormalized(char c)
        {
            if (c == '\uFF0E') return '.';
            if (c == '\uFF05') return '%';
            if (c == '\uFF0D') return '-';
            return c;
        }

        // Width of one glyph in the word. Windows OCR boxes cover the word only,
        // so an English word is 4-8x wider than the space that follows it; sizing
        // the separator threshold by word width would swallow every space.
        private static double CharWidth(OcrWord word)
        {
            int n = 0;
            foreach (char c in word.Text) if (!char.IsWhiteSpace(c)) n++;
            if (n == 0) return 0.0;
            return word.W / n;
        }

        private static bool NeedsSeparator(OcrWord prev, OcrWord cur, double medianW)
        {
            if (prev.Text.Length == 0 || cur.Text.Length == 0) return false;
            char a = FirstNormalized(prev.Text[prev.Text.Length - 1]);
            char b = FirstNormalized(cur.Text[0]);
            if (char.IsWhiteSpace(a) || char.IsWhiteSpace(b)) return false;
            if (AsciiPunct.IndexOf(a) >= 0 || AsciiPunct.IndexOf(b) >= 0 ||
                OpenPunct.IndexOf(a) >= 0)
                return false;
            bool cjkA = IsCjk(a);
            bool cjkB = IsCjk(b);
            if (cjkA && cjkB)
            {
                // Two Han runs side by side are normally one phrase, but a table
                // row puts its columns almost a full line height apart. Only that
                // unmistakable case becomes a separator.
                double h = Math.Max(prev.H, cur.H);
                return h > 0 && (cur.X - prev.Right) > h * 0.9;
            }
            double glyph = Math.Min(CharWidth(prev), CharWidth(cur));
            double cap = Math.Max(prev.H, cur.H) * 0.9;
            if (glyph <= 0.0 || glyph > cap) glyph = cap;
            if (glyph <= 0.0) glyph = medianW * 0.5;
            return (cur.X - prev.Right) > Math.Max(2.0, glyph * 0.35);
        }

        // Collapse runs of spaces so an inserted word separator never doubles the
        // space the recognition stage already produced.
        private static string CollapseSpaces(string text)
        {
            if (text.IndexOf("  ", StringComparison.Ordinal) < 0) return text;
            StringBuilder sb = new StringBuilder(text.Length);
            bool space = false;
            foreach (char c in text)
            {
                if (c == ' ' || c == '\t')
                {
                    if (!space) sb.Append(' ');
                    space = true;
                }
                else
                {
                    sb.Append(c);
                    space = false;
                }
            }
            return sb.ToString();
        }

        // Chinese does not use spaces to separate words, so a blank between two
        // Han characters is loose kerning, a stray full-width space from the
        // recognition model, or a mis-fired geometry test. Drop it; keep any
        // blank that touches Latin, digits or punctuation.
        private static string DropCjkSpaces(string text)
        {
            if (text.IndexOf(' ') < 0) return text;
            StringBuilder sb = new StringBuilder(text.Length);
            for (int i = 0; i < text.Length; i++)
            {
                if (text[i] == ' ' && i > 0 && i + 1 < text.Length &&
                    IsCjk(text[i - 1]) && IsCjk(text[i + 1]))
                    continue;
                sb.Append(text[i]);
            }
            return sb.ToString();
        }

        private static string JoinWords(List<OcrWord> words, double medianW, bool tableMode)
        {
            StringBuilder sb = new StringBuilder();
            bool hasContent = false;
            for (int i = 0; i < words.Count; i++)
            {
                // Clean each word before joining: a full-width space the model
                // dropped between two Han characters is noise, but the separator
                // inserted below is a real column boundary and must survive.
                string text = DropCjkSpaces(Normalize(words[i].Text, tableMode));
                if (text.Length == 0) continue;
                if (hasContent && NeedsSeparator(words[i - 1], words[i], medianW))
                    sb.Append(' ');
                sb.Append(text);
                hasContent = true;
            }
            return CollapseSpaces(sb.ToString()).Trim();
        }

        private static void GroupRows(List<OcrWord> source, out List<List<OcrWord>> rows,
            out double medianH, out double medianW)
        {
            rows = new List<List<OcrWord>>();
            List<double> heights = new List<double>();
            List<double> widths = new List<double>();
            foreach (OcrWord w in source)
            {
                if (w.H > 0) heights.Add(w.H);
                if (w.W > 0) widths.Add(w.W);
            }
            medianH = Median(heights, 12.0);
            medianW = Median(widths, 12.0);
            if (source.Count == 0) return;

            double tol = Math.Max(7.0, medianH * 0.62);
            List<OcrWord> ordered = new List<OcrWord>(source);
            ordered.Sort(delegate(OcrWord a, OcrWord b)
            {
                int c = a.Cy.CompareTo(b.Cy);
                return c != 0 ? c : a.X.CompareTo(b.X);
            });

            List<OcrWord> current = new List<OcrWord>();
            current.Add(ordered[0]);
            for (int i = 1; i < ordered.Count; i++)
            {
                OcrWord word = ordered[i];
                if (word.Cy - current[current.Count - 1].Cy > tol)
                {
                    current.Sort(delegate(OcrWord a, OcrWord b) { return a.X.CompareTo(b.X); });
                    rows.Add(current);
                    current = new List<OcrWord>();
                }
                current.Add(word);
            }
            if (current.Count > 0)
            {
                current.Sort(delegate(OcrWord a, OcrWord b) { return a.X.CompareTo(b.X); });
                rows.Add(current);
            }
        }

        private static Cell MakeCell(List<OcrWord> words, double medianW, bool tableMode)
        {
            double x1 = double.MaxValue;
            double x2 = double.MinValue;
            foreach (OcrWord w in words)
            {
                if (w.X < x1) x1 = w.X;
                if (w.Right > x2) x2 = w.Right;
            }
            Cell cell = new Cell();
            cell.X1 = x1;
            cell.X2 = x2;
            cell.Text = JoinWords(words, medianW, tableMode);
            return cell;
        }

        private static List<Cell> SplitCells(List<OcrWord> row, double medianW, bool tableMode)
        {
            double gap = Math.Max(14.0, medianW * 1.15);
            List<Cell> cells = new List<Cell>();
            if (row.Count == 0) return cells;
            List<OcrWord> current = new List<OcrWord>();
            current.Add(row[0]);
            for (int i = 1; i < row.Count; i++)
            {
                OcrWord word = row[i];
                if (word.X - current[current.Count - 1].Right > gap)
                {
                    cells.Add(MakeCell(current, medianW, tableMode));
                    current = new List<OcrWord>();
                }
                current.Add(word);
            }
            if (current.Count > 0)
                cells.Add(MakeCell(current, medianW, tableMode));
            return cells;
        }

        private static List<double[]> InferColumns(List<List<Cell>> cellRows)
        {
            List<double[]> intervals = new List<double[]>();
            foreach (List<Cell> row in cellRows)
            {
                if (row.Count < 2) continue;
                foreach (Cell cell in row)
                    intervals.Add(new double[] { cell.X1, cell.X2 });
            }
            intervals.Sort(delegate(double[] a, double[] b)
            {
                int c = a[0].CompareTo(b[0]);
                return c != 0 ? c : a[1].CompareTo(b[1]);
            });
            List<double[]> merged = new List<double[]>();
            foreach (double[] item in intervals)
            {
                if (merged.Count > 0 && item[0] <= merged[merged.Count - 1][1])
                {
                    if (item[1] > merged[merged.Count - 1][1])
                        merged[merged.Count - 1][1] = item[1];
                }
                else
                    merged.Add(new double[] { item[0], item[1] });
            }
            return merged;
        }

        private static double Overlap(double a1, double a2, double b1, double b2)
        {
            return Math.Max(0.0, Math.Min(a2, b2) - Math.Max(a1, b1));
        }

        private static string JoinLines(List<string> lines)
        {
            return string.Join("\r\n", lines.ToArray());
        }

        internal static string ToPlainText(OcrResult result)
        {
            List<List<OcrWord>> rows;
            double medianH, medianW;
            GroupRows(new List<OcrWord>(result.Words), out rows, out medianH, out medianW);
            List<string> lines = new List<string>();
            foreach (List<OcrWord> row in rows)
            {
                string line = JoinWords(row, medianW, false);
                if (line.Length > 0) lines.Add(line);
            }
            return FinalizeText(JoinLines(lines));
        }

        internal static string ToTsv(OcrResult result)
        {
            List<List<OcrWord>> rows;
            double medianH, medianW;
            GroupRows(new List<OcrWord>(result.Words), out rows, out medianH, out medianW);
            if (rows.Count == 0) return "";

            List<List<Cell>> cellRows = new List<List<Cell>>();
            foreach (List<OcrWord> row in rows)
                cellRows.Add(SplitCells(row, medianW, true));
            List<double[]> columns = InferColumns(cellRows);
            if (columns.Count < 2)
                return ToPlainText(result);

            List<string[]> matrix = new List<string[]>();
            foreach (List<Cell> row in cellRows)
            {
                string[] cells = new string[columns.Count];
                for (int i = 0; i < cells.Length; i++) cells[i] = "";
                foreach (Cell cell in row)
                {
                    int best = 0;
                    double bestScore = -1.0;
                    for (int i = 0; i < columns.Count; i++)
                    {
                        double score = Overlap(cell.X1, cell.X2, columns[i][0], columns[i][1]);
                        if (score > bestScore) { bestScore = score; best = i; }
                    }
                    if (bestScore <= 0)
                    {
                        best = 0;
                        double minDist = double.MaxValue;
                        for (int i = 0; i < columns.Count; i++)
                        {
                            double dist = Math.Abs(columns[i][0] - cell.X1);
                            if (dist < minDist) { minDist = dist; best = i; }
                        }
                    }
                    cells[best] = cells[best].Length > 0
                        ? (cells[best] + " " + cell.Text).Trim()
                        : cell.Text;
                }
                matrix.Add(cells);
            }

            List<int> keepCols = new List<int>();
            for (int i = 0; i < columns.Count; i++)
            {
                foreach (string[] row in matrix)
                {
                    if (row[i].Length > 0) { keepCols.Add(i); break; }
                }
            }
            List<string> lines = new List<string>();
            foreach (string[] row in matrix)
            {
                List<string> parts = new List<string>();
                foreach (int i in keepCols) parts.Add(row[i]);
                string line = string.Join("\t", parts.ToArray());
                if (line.Trim('\t', ' ').Length > 0) lines.Add(line);
            }
            return FinalizeText(JoinLines(lines));
        }

        internal static int ColumnCount(string tsv)
        {
            if (string.IsNullOrEmpty(tsv)) return 0;
            int max = 0;
            foreach (string line in tsv.Split(new string[] { "\r\n" }, StringSplitOptions.None))
                max = Math.Max(max, line.Split('\t').Length);
            return max;
        }

        internal static int LineCount(string text)
        {
            if (string.IsNullOrEmpty(text)) return 0;
            return text.Split(new string[] { "\r\n" }, StringSplitOptions.None).Length;
        }

        internal static string FormatNumber(double value)
        {
            return value.ToString("F1", CultureInfo.InvariantCulture);
        }
    }
}
