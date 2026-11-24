using System.IO;
using System.IO.MemoryMappedFiles;
using System.Text.Json;
using System.Windows;

namespace Yaac.WpfPreview.Views;

internal sealed record BrowserFrame(ulong Id, ulong Tab, bool Popup, int Width, int Height, bool ReadFailed, IReadOnlyList<BrowserFrame.Patch> Patches)
{
    internal sealed record Patch(Int32Rect Rect, byte[] Bytes);

    // Called by the pipe reader, outside the dispatcher. Disposing the view before acknowledging
    // lets CEF resize or reuse it safely even if the current tab changes during a paint.
    internal static BrowserFrame? Read(JsonElement data)
    {
        if (data.ValueKind != JsonValueKind.Object) return null;
        var id = data.GetProperty("id").GetUInt64();
        var tab = data.GetProperty("tab").GetUInt64();
        var popup = data.GetProperty("popup").GetBoolean();
        var size = data.GetProperty("size");
        var width = size[0].GetInt32();
        var height = size[1].GetInt32();
        var patches = new List<Patch>();

        try
        {
            if (width is < 1 or > 16384 || height is < 1 or > 16384)
                throw new InvalidDataException("Invalid browser surface size.");

            using var file = new FileStream(data.GetProperty("path").GetString()!, FileMode.Open, FileAccess.Read,
                FileShare.ReadWrite | FileShare.Delete);
            using var mapping = MemoryMappedFile.CreateFromFile(file, null, 0, MemoryMappedFileAccess.Read,
                HandleInheritability.None, leaveOpen: true);
            using var view = mapping.CreateViewAccessor(0, (long)width * height * 4, MemoryMappedFileAccess.Read);
            foreach (var rect in data.GetProperty("dirty").EnumerateArray())
            {
                int x = rect[0].GetInt32(), y = rect[1].GetInt32();
                int w = rect[2].GetInt32(), h = rect[3].GetInt32();
                if (x < 0 || y < 0 || w <= 0 || h <= 0 || x > width - w || y > height - h)
                    throw new InvalidDataException("Invalid browser surface rectangle.");
                var bytes = new byte[checked(w * h * 4)];
                if (w == width)
                    view.ReadArray((long)y * width * 4, bytes, 0, bytes.Length);
                else
                    for (int row = 0; row < h; row++)
                        view.ReadArray(((long)(y + row) * width + x) * 4, bytes, row * w * 4, w * 4);
                patches.Add(new Patch(new Int32Rect(x, y, w, h), bytes));
            }
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException)
        {
            return new BrowserFrame(id, tab, popup, width, height, true, []);
        }

        return new BrowserFrame(id, tab, popup, width, height, false, patches);
    }
}
