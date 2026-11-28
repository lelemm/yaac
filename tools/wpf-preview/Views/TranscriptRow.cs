using System.Text.Json;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal static class TranscriptJson
{
    public static JsonElement Value(this JsonElement data, string name) =>
        data.ValueKind == JsonValueKind.Object && data.TryGetProperty(name, out var value) ? value : default;

    public static string? OptionalText(this JsonElement data) =>
        data.ValueKind == JsonValueKind.String && data.GetString() is { Length: > 0 } text ? text : null;

    public static string Text(this JsonElement data) => data.OptionalText() ?? "";

    public static string? Scalar(this JsonElement data) =>
        data.ValueKind == JsonValueKind.Number ? data.GetRawText() : data.OptionalText();

    public static bool Flag(this JsonElement data, bool fallback = false) => data.ValueKind switch
    {
        JsonValueKind.True => true,
        JsonValueKind.False => false,
        _ => fallback,
    };

    public static IEnumerable<JsonElement> Elements(this JsonElement data)
    {
        if (data.ValueKind == JsonValueKind.Array)
        {
            foreach (var element in data.EnumerateArray())
                yield return element;
        }
    }

    public static JsonElement Snapshot(this JsonElement data) =>
        data.ValueKind == JsonValueKind.Undefined ? default : data.Clone();

    public static bool Same(JsonElement left, JsonElement right) =>
        left.ValueKind == right.ValueKind &&
        (left.ValueKind == JsonValueKind.Undefined || JsonElement.DeepEquals(left, right));
}

internal static class TranscriptColors
{
    public static Brush Text => Palette.Brush("YaacText");
    public static Brush Muted => Palette.Brush("YaacMuted");
    public static Brush Rule => Palette.Brush("YaacRule");
    public static Brush Panel => Palette.Brush("YaacCard");
    public static Brush Raised => Palette.Brush("YaacRaised");
    public static Brush User => Palette.Brush("YaacUser");
    public static Brush Code => Palette.Brush("YaacInput");
    public static Brush Warning => Palette.Brush("YaacWarning");
    public static Brush Error => Palette.Brush("YaacFailure");
    public static Brush Success => Palette.Brush("YaacSuccess");
    public static Brush Link => Palette.Brush("YaacLink");

    public static Brush Tone(string name, Brush fallback) => name switch
    {
        "warning" => Warning,
        "error" => Error,
        "success" => Success,
        "muted" => Muted,
        _ => fallback,
    };
}

internal sealed class TranscriptRowState
{
    public Dictionary<string, bool> Expanded { get; } = new();
    public Dictionary<string, TranscriptAnswerDraft> Answers { get; } = new();
    public Dictionary<string, string> Fields { get; } = new();
    /// <summary>An external dialog's entered values, by input id (P418); in memory only.</summary>
    public Dictionary<string, object?> Dialog { get; } = new();
    public Dictionary<string, (int Start, int Length)> Selection { get; } = new();
}

internal sealed class TranscriptAnswerDraft
{
    public HashSet<string> Selected { get; } = new();
    public string Other { get; set; } = "";
    public bool Secret { get; set; }
}

internal sealed class TranscriptRow
{
    private readonly Func<TranscriptRowState> state;
    private readonly Action<TranscriptRow, object> emit;

    public TranscriptRow(string id, string threadId, JsonElement data, Func<TranscriptRowState> state, Action<TranscriptRow, object> emit)
    {
        Id = id;
        ThreadId = threadId;
        this.state = state;
        this.emit = emit;
        Data = data.Snapshot();
        RefreshLabel();
    }

    public string Id { get; }
    public string ThreadId { get; }
    public string ActionId => Data.Value("actionId").OptionalText() ?? Id;
    public int Index { get; set; }
    internal int Revision { get; private set; }
    public JsonElement Data { get; private set; }
    public string Kind { get; private set; } = "";
    public string Prompt { get; private set; } = "";
    public TranscriptRowState State => state();
    public event Action? Changed;

    public bool Update(JsonElement data)
    {
        if (TranscriptJson.Same(Data, data))
            return false;
        Data = data.Snapshot();
        Revision++;
        RefreshLabel();
        Changed?.Invoke();
        return true;
    }

    private void RefreshLabel()
    {
        Kind = Data.Value("kind").Text();
        if (Kind is not ("user" or "task"))
        {
            Prompt = "";
            return;
        }
        var text = Data.Value("text").OptionalText() ?? Data.Value("copyText").OptionalText() ??
            Data.Value("parts").Elements()
                .Where(part => part.Value("kind").Text() is "text" or "markdown" or "code" or "pasted" or "command")
                .Select(part => part.Value("text").OptionalText()).FirstOrDefault(value => value is not null) ?? "";
        var end = text.AsSpan().IndexOfAny('\r', '\n');
        Prompt = text[..Math.Min(end >= 0 ? end : text.Length, 110)].Trim();
        if (Prompt.Length == 0)
            Prompt = Kind == "task" ? "Agent message" : "User message";
    }

    public void Send(object payload) => emit(this, payload);
}
