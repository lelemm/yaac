using System.Text.Json;

namespace Yaac.WpfPreview.Views;

// Decoded on the pipe reader, before the UI dispatcher sees the frame.
internal readonly record struct TerminalRun(int Column, int Width, int Ink, int Paper, int Flags, string Text);
internal readonly record struct TerminalRow(int Index, TerminalRun[] Runs);
internal sealed record TerminalFrame(ulong Tab, ulong Serial, int Height, TerminalRow[] Rows,
    int CursorColumn, int CursorRow, string Shape, int CursorColor, bool Wide, bool Blinking, bool Mouse, bool Selection, string? Search, bool Panel, TerminalRun? CursorGlyph, string SearchStatus)
{
    internal static TerminalFrame Read(JsonElement frame)
    {
        var rows = frame.GetProperty("rows");
        var decoded = new TerminalRow[rows.GetArrayLength()];
        int i = 0;
        foreach (var row in rows.EnumerateArray())
        {
            var spans = row[1];
            var runs = new TerminalRun[spans.GetArrayLength()];
            int j = 0;
            foreach (var run in spans.EnumerateArray())
                runs[j++] = new(run[0].GetInt32(), run[1].GetInt32(), run[2].GetInt32(),
                    run[3].GetInt32(), run[4].GetInt32(), run[5].GetString()!);
            decoded[i++] = new(row[0].GetInt32(), runs);
        }
        var caret = frame.GetProperty("cursor");
        var glyph = frame.Value("glyph");
        TerminalRun? cursorGlyph = glyph.ValueKind == JsonValueKind.Array ?
            new(caret[0].GetInt32(), caret[4].GetBoolean() ? 2 : 1, glyph[0].GetInt32(),
                -1, glyph[1].GetInt32(), glyph[2].GetString()!) : null;
        string? query = frame.Value("search").OptionalText();
        string status = string.IsNullOrEmpty(query) ? "" : !frame.Flag("found") ? "No results" :
            frame.Value("matches").TryGetInt32(out int count) && count > 0 ? $"{count} here" : "";
        return new(frame.GetProperty("tab").GetUInt64(), frame.GetProperty("frame").GetUInt64(),
            frame.GetProperty("height").GetInt32(), decoded, caret[0].GetInt32(), caret[1].GetInt32(),
            caret[2].GetString()!, caret[3].GetInt32(), caret[4].GetBoolean(), caret[5].GetBoolean(), frame.Flag("mouse"), frame.Flag("selection"), query, frame.Flag("panel"), cursorGlyph, status);
    }
}
