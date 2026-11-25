using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Microsoft.Win32;

namespace Yaac.WpfPreview.Views;

internal sealed class MermaidDiagram : StackPanel
{
    private readonly string source;
    private readonly Image image = new() { Stretch = Stretch.Uniform, StretchDirection = StretchDirection.DownOnly,
        HorizontalAlignment = HorizontalAlignment.Left };
    private readonly TextBlock status = new() { TextWrapping = TextWrapping.Wrap, Foreground = TranscriptColors.Text };
    private readonly Button png;
    private readonly Button svg;
    private CancellationTokenSource? request;
    private sealed record Render(BitmapImage Image, byte[] Png, byte[] Svg);
    private Render? rendered;
    private static readonly SemaphoreSlim Gate = new(1);
    private readonly SolidColorBrush canvas = (SolidColorBrush)Palette.Brush("YaacCanvas");

    public MermaidDiagram(string source)
    {
        this.source = source;
        Children.Add(image);
        Children.Add(status);
        var actions = new WrapPanel();
        Children.Add(actions);
        Button Action(string label, string extension, Func<byte[]> bytes)
        {
            var button = new Button { Content = label, Style = (Style)Application.Current.FindResource("YaacQuietButton") };
            button.Click += async (_, _) =>
            {
                var dialog = new SaveFileDialog { FileName = $"diagram.{extension}", Filter = $"{extension}|*.{extension}" };
                if (dialog.ShowDialog(Window.GetWindow(this)) != true) return;
                try { await SaveFile(dialog.FileName, bytes()); }
                catch (Exception error) { status.Text = error.Message; }
            };
            actions.Children.Add(button);
            return button;
        }
        png = Action("Save PNG", "png", () => rendered!.Png);
        svg = Action("Save SVG", "svg", () => rendered!.Svg);
        var copy = new Button { Content = "Copy source", Style = (Style)Application.Current.FindResource("YaacQuietButton") };
        copy.Click += (_, _) => TranscriptText.Copy(source);
        actions.Children.Add(copy);
        Action("Save source", "mmd", () => Encoding.UTF8.GetBytes(source));
        Loaded += (_, _) => { canvas.Changed += Changed; Refresh(); };
        Unloaded += (_, _) => { canvas.Changed -= Changed; request?.Cancel(); };
    }

    private void Changed(object? sender, EventArgs args) => Refresh();

    private async void Refresh()
    {
        request?.Cancel();
        using var work = new CancellationTokenSource();
        request = work;
        rendered = null;
        image.Source = null;
        svg.IsEnabled = png.IsEnabled = false;
        status.Text = "Rendering diagram…";
        var dark = canvas.Color.R < 128;
        try
        {
            if (Encoding.UTF8.GetByteCount(source) > 65536) throw new InvalidDataException("Mermaid source exceeds 64 KiB.");
            await Task.Delay(350, work.Token);
            await Gate.WaitAsync(work.Token);
            try
            {
                var result = await Task.Run(() => Run(source, dark), work.Token);
                work.Token.ThrowIfCancellationRequested();
                rendered = result;
                image.Source = result.Image;
                svg.IsEnabled = png.IsEnabled = true;
                status.Text = "";
            }
            finally { Gate.Release(); }
        }
        catch (OperationCanceledException) { }
        catch (Exception e) { if (!work.IsCancellationRequested) status.Text = $"Mermaid preview unavailable · {e.Message}\n{source}"; }
        finally { if (ReferenceEquals(request, work)) request = null; }
    }

    private static async Task SaveFile(string destination, byte[] bytes)
    {
        var temporary = Path.Combine(Path.GetDirectoryName(destination)!, Path.GetRandomFileName());
        var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None, 65536, FileOptions.Asynchronous);
        try
        {
            await using (stream) await stream.WriteAsync(bytes);
            File.Move(temporary, destination, overwrite: true);
        }
        finally { File.Delete(temporary); }
    }

    private static async Task<Render> Run(string source, bool dark)
    {
        var runtime = new[] { Path.Combine(AppContext.BaseDirectory,"..","runtime"), Path.Combine(AppContext.BaseDirectory,"runtime") }
            .FirstOrDefault(dir => File.Exists(Path.Combine(dir,"mermaid","render.cjs")))
            ?? throw new FileNotFoundException("The Mermaid runtime is missing.");
        var start = new ProcessStartInfo(Path.Combine(runtime,"node.exe"))
        {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true,
            RedirectStandardOutput = true, RedirectStandardError = true, StandardInputEncoding = new UTF8Encoding(false),
            StandardOutputEncoding = Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8,
        };
        start.ArgumentList.Add(Path.Combine(runtime,"mermaid","render.cjs"));
        start.ArgumentList.Add(dark ? "dark" : "light");
        using var process = Process.Start(start) ?? throw new IOException("Mermaid could not start.");
        var output = process.StandardOutput.ReadToEndAsync();
        var error = process.StandardError.ReadToEndAsync();
        await process.StandardInput.WriteAsync(source);
        process.StandardInput.Close();
        await process.WaitForExitAsync();
        if (process.ExitCode != 0) throw new InvalidDataException(await error);
        using var json = JsonDocument.Parse(await output);
        var bytes = json.RootElement.GetProperty("png").GetBytesFromBase64();
        using var input = new MemoryStream(bytes);
        var bitmap = new BitmapImage();
        bitmap.BeginInit();
        bitmap.CacheOption = BitmapCacheOption.OnLoad;
        bitmap.StreamSource = input;
        bitmap.EndInit();
        bitmap.Freeze();
        return new Render(bitmap, bytes, json.RootElement.GetProperty("svg").GetBytesFromBase64());
    }
}
