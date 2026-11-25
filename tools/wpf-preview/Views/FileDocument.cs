using System.Text;
using System.Text.Json;
using ICSharpCode.AvalonEdit.Document;

namespace Yaac.WpfPreview.Views;

internal sealed record CodeSpan(int Start, int End, string Scope);
internal sealed record CodeStyle(CodeSpan[] Spans, int[][]? Words);
internal sealed record CodeSource(string Text, Dictionary<int, CodeStyle> Styles);
internal sealed record FileDocument(CodeSource? File, CodeSource? Old, CodeSource? New, int?[][]? Rows, bool SideBySide)
{
    private static readonly JsonSerializerOptions Wire = new(JsonSerializerDefaults.Web);

    internal static FileDocument Read(string json) => JsonSerializer.Deserialize<FileDocument>(json, Wire)
        ?? throw new JsonException("Missing file document.");

    internal FileLayout Layout(IReadOnlySet<int> expanded, CancellationToken cancel)
    {
        cancel.ThrowIfCancellationRequested();
        if (File is { } file)
            return new FileLayout(Prepare(file.Text, file.Styles, null), null);

        var oldLines = Old!.Text.Split('\n');
        var newLines = New!.Text.Split('\n');
        var first = new EditorBuilder();
        var second = SideBySide ? new EditorBuilder() : null;
        for (int i = 0; i < Rows!.Length; i++)
        {
            cancel.ThrowIfCancellationRequested();
            var old = Rows[i][0] ?? -1;
            var @new = Rows[i][1] ?? -1;
            var count = Rows[i][2] ?? 0;
            if (count > 0 && !expanded.Contains(i))
            {
                var marker = new DiffLine(i, 0, -1, -1, "", count, true);
                first.Add($"⋯ {count} unchanged lines", marker);
                second?.Add($"⋯ {count} unchanged lines", marker);
                continue;
            }
            for (var offset = 0; offset < Math.Max(count, 1); offset++)
            {
                cancel.ThrowIfCancellationRequested();
                var a = old < 0 ? -1 : old + offset;
                var b = @new < 0 ? -1 : @new + offset;
                var fold = count > 0 && offset == 0;
                Old.Styles.TryGetValue(a, out var oldStyle);
                New.Styles.TryGetValue(b, out var newStyle);

                if (second is null)
                {
                    var sign = a < 0 ? "+" : b < 0 ? "−" : "";
                    var line = new DiffLine(i, offset, a, b, sign, 0, fold);
                    first.Add(b < 0 ? oldLines[a] : newLines[b], line, b < 0 ? oldStyle : newStyle);
                }
                else
                {
                    var changed = a < 0 || b < 0 || oldStyle?.Words is not null || newStyle?.Words is not null;
                    first.Add(a < 0 ? "" : oldLines[a],
                        new DiffLine(i, offset, a, -1, a >= 0 && changed ? "−" : "", 0, fold), oldStyle);
                    second.Add(b < 0 ? "" : newLines[b],
                        new DiffLine(i, offset, -1, b, b >= 0 && changed ? "+" : "", 0, fold), newStyle);
                }
            }
        }

        return new FileLayout(first.Finish(), second?.Finish());
    }

    private static EditorDocument Prepare(string text, Dictionary<int, CodeStyle> styles, List<DiffLine>? rows)
    {
        var document = new TextDocument(text);
        // AvalonEdit supports transferring a document built on a worker to the dispatcher.
        document.SetOwnerThread(null);
        return new EditorDocument(document, styles, rows);
    }

    private sealed class EditorBuilder
    {
        private readonly StringBuilder text = new();
        private readonly Dictionary<int, CodeStyle> styles = new();
        private readonly List<DiffLine> rows = new();

        internal void Add(string line, DiffLine row, CodeStyle? style = null)
        {
            if (rows.Count != 0) text.Append('\n');
            text.Append(line);
            if (style is not null) styles.Add(rows.Count, style);
            rows.Add(row);
        }

        internal EditorDocument Finish() => Prepare(text.ToString(), styles, rows);
    }
}

internal readonly record struct DiffLine(int Row, int Offset, int Old, int New, string Sign, int FoldCount, bool FoldButton);
internal sealed record EditorDocument(TextDocument Text, Dictionary<int, CodeStyle> Styles, List<DiffLine>? Rows);
internal sealed record FileLayout(EditorDocument Left, EditorDocument? Right);
internal sealed record IncomingFileDocument(ulong Id, FileDocument Data, FileLayout Layout);

// Runs on the pipe reader. A new transfer replaces any unfinished one.
internal sealed class FileDocumentReader
{
    private ulong? expected;
    private StringBuilder? data;
    private CancellationTokenSource? projection;

    internal (ulong Id, string Text, CancellationToken Cancel)? Accept(JsonElement frame)
    {
        var incoming = frame.GetProperty("id").GetUInt64();
        if (incoming != expected) return null;
        if (frame.Value("start").ValueKind == JsonValueKind.True)
            data = new StringBuilder();
        if (data is null) return null;
        data.Append(frame.GetProperty("chunk").GetString());
        if (frame.Value("end").ValueKind != JsonValueKind.True) return null;
        var complete = data.ToString();
        data = null;
        projection = new CancellationTokenSource();
        return (incoming, complete, projection.Token);
    }

    internal void Retain(ulong? next)
    {
        if (expected == next) return;
        expected = next;
        data = null;
        projection?.Cancel();
        projection = null;
    }

    internal static ulong? Expected(JsonElement state) =>
        state.Value("files").Value("document") is { ValueKind: JsonValueKind.Number } document
            ? document.GetUInt64() : null;
}
