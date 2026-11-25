using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;

namespace Yaac.WpfPreview;

internal sealed class Controller : IDisposable
{
    private readonly Process? process;
    private readonly StreamReader views;
    private readonly StreamWriter output;
    private readonly record struct Command(string? Line, object? Paste);
    private readonly Channel<Command> commands = Channel.CreateUnbounded<Command>(
        new UnboundedChannelOptions { SingleReader = true, SingleWriter = true });
    private readonly CancellationTokenSource stop = new();

    internal Controller(string path, string scenario, string theme)
    {
        var start = new ProcessStartInfo(path)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            StandardInputEncoding = new UTF8Encoding(false),
            StandardOutputEncoding = Encoding.UTF8,
        };
        start.ArgumentList.Add("--wpf-controller");
        start.ArgumentList.Add(scenario);
        start.ArgumentList.Add(theme);
        process = Process.Start(start) ?? throw new InvalidOperationException("The YAAC controller could not start.");
        views = process.StandardOutput;
        output = process.StandardInput;
    }

    private Controller(StreamReader views, StreamWriter output)
    {
        this.views = views;
        this.output = output;
    }

    internal static Controller FromPipe() => new(
        new StreamReader(Console.OpenStandardInput(), Encoding.UTF8),
        new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false)) { NewLine = "\n" });

    internal void Send(string command, object? payload)
    {
        // Clipboard text is immutable and can be large. Encode it on the ordered
        // writer. Other commands may borrow a JsonElement from the current frame.
        commands.Writer.TryWrite(command == "terminal.paste"
            ? new(null, payload)
            : new(JsonSerializer.Serialize(new { command, payload }), null));
    }

    internal async Task RunAsync(Func<JsonElement, Task> receive)
    {
        var token = stop.Token;
        var read = ReadAsync(receive, token);
        var write = WriteAsync(token);
        try
        {
            var first = await Task.WhenAny(read, write).ConfigureAwait(false);
            await first.ConfigureAwait(false);
            if (!token.IsCancellationRequested)
                throw new IOException("The YAAC controller closed the connection.");
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested)
        {
        }
        finally
        {
            stop.Cancel();
            try
            {
                await Task.WhenAll(read, write).ConfigureAwait(false);
            }
            catch (Exception) when (read.IsFaulted || write.IsFaulted || token.IsCancellationRequested)
            {
                // Preserve the exception from the first completed task above.
            }
            output.Dispose();
            views.Dispose();
            process?.Dispose();
        }
    }

    private async Task ReadAsync(Func<JsonElement, Task> receive, CancellationToken token)
    {
        while (await views.ReadLineAsync(token).ConfigureAwait(false) is string line)
        {
            using var frame = JsonDocument.Parse(line);
            await receive(frame.RootElement).ConfigureAwait(false);
        }
    }

    private async Task WriteAsync(CancellationToken token)
    {
        await foreach (var next in commands.Reader.ReadAllAsync(token).ConfigureAwait(false))
        {
            var line = next.Line ?? JsonSerializer.Serialize(new { command = "terminal.paste", payload = next.Paste });
            await output.WriteLineAsync(line.AsMemory(), token).ConfigureAwait(false);
            await output.FlushAsync(token).ConfigureAwait(false);
        }
    }

    public void Dispose()
    {
        stop.Cancel();
        commands.Writer.TryComplete();
    }
}
