using System.Windows.Documents;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal static class TranscriptSyntax
{
    // The same lightweight, language-agnostic token categories used by the standard window.
    private static readonly HashSet<string> Keywords = new(StringComparer.Ordinal)
    {
        "let", "mut", "fn", "if", "else", "return", "while", "for", "in", "match", "struct", "enum", "impl",
        "pub", "use", "as", "const", "async", "await", "def", "class", "import", "from", "function", "var",
        "true", "false", "None", "Some", "Ok", "Err", "self", "Self", "loop", "break", "continue", "type",
        "interface", "export",
    };

    internal static void Append(Paragraph paragraph, string text, object? lineNumberTag)
    {
        var lines = text.Split('\n');
        var digits = lines.Length.ToString(System.Globalization.CultureInfo.InvariantCulture).Length;
        for (int i = 0; i < lines.Length; i++)
        {
            if (i != 0) paragraph.Inlines.Add(new LineBreak());
            if (lineNumberTag is not null)
            {
                var number = (i + 1).ToString(System.Globalization.CultureInfo.InvariantCulture).PadLeft(digits);
                paragraph.Inlines.Add(new Run(number + "  ") { Foreground = TranscriptColors.Muted, Tag = lineNumberTag });
            }
            AppendLine(paragraph, lines[i]);
        }
    }

    private static void AppendLine(Paragraph paragraph, string line)
    {
        var plain = 0;
        for (var start = 0; start < line.Length;)
        {
            var end = start + 1;
            var ch = line[start];
            Brush? color = null;
            if (ch == '#' || ch == '/' && end < line.Length && line[end] == '/')
            {
                end = line.Length;
                color = Palette.Brush("YaacCodeComment");
            }
            else if (ch is '\'' or '"')
            {
                while (end < line.Length)
                {
                    var next = line[end++];
                    if (next == ch) break;
                    if (next == '\\' && end < line.Length) end++;
                }
                color = Palette.Brush("YaacCodeString");
            }
            else if (char.IsAsciiDigit(ch))
            {
                while (end < line.Length && (Word(line[end]) || line[end] == '.')) end++;
                color = Palette.Brush("YaacCodeNumber");
            }
            else if (char.IsAsciiLetter(ch) || ch == '_')
            {
                while (end < line.Length && Word(line[end])) end++;
                var word = line[start..end];
                if (Keywords.Contains(word)) color = Palette.Brush("YaacCodeKeyword");
                else if (char.IsAsciiLetterUpper(ch) || word is "u32" or "usize" or "i32" or "u8" or "str")
                    color = Palette.Brush("YaacCodeType");
            }

            if (color is not null)
            {
                if (plain < start) paragraph.Inlines.Add(new Run(line[plain..start]) { Foreground = TranscriptColors.Text });
                paragraph.Inlines.Add(new Run(line[start..end]) { Foreground = color });
                plain = end;
            }
            start = end;
        }
        if (plain < line.Length) paragraph.Inlines.Add(new Run(line[plain..]) { Foreground = TranscriptColors.Text });
    }

    private static bool Word(char c) => char.IsAsciiLetterOrDigit(c) || c == '_';
}
