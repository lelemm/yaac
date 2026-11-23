using System.ComponentModel;
using System.Globalization;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public sealed class AgentsTemplates : DataTemplateSelector
{
    public DataTemplate? Row { get; set; }
    public DataTemplate? PillRow { get; set; }
    public DataTemplate? PillLabel { get; set; }
    public DataTemplate? Rule { get; set; }
    public DataTemplate? More { get; set; }
    public DataTemplate? Heading { get; set; }
    public DataTemplate? Info { get; set; }
    public DataTemplate? Root { get; set; }
    public DataTemplate? Fold { get; set; }

    public override DataTemplate? SelectTemplate(object item, DependencyObject container) => (item as AgentsEntry)?.Kind switch
    {
        "cardRow" or "dockRow" => Row,
        "pillRow" => PillRow,
        "pillLabel" => PillLabel,
        "rule" => Rule,
        "more" => More,
        "heading" => Heading,
        "info" => Info,
        "root" => Root,
        "fold" => Fold,
        _ => base.SelectTemplate(item, container)
    };
}

internal sealed class AgentsEntry(string kind) : INotifyPropertyChanged
{
    private AgentsData _data = new();
    private bool _expanded;
    private bool _menuOpen;

    public string Kind { get; } = kind;
    public string CommandId { get; set; } = "";
    public string RowId { get; set; } = "";
    public string GroupId { get; set; } = "";
    public IReadOnlyList<AgentsEntry> FoldRows { get; set; } = [];
    public IReadOnlyList<AgentsAction> Actions { get; set; } = [];
    public AgentsAction? BadgeAction { get; set; }
    public event PropertyChangedEventHandler? PropertyChanged;

    public AgentsData Data
    {
        get => _data;
        set
        {
            if (_data == value)
                return;
            _data = value;
            PropertyChanged?.Invoke(this, new(nameof(Data)));
        }
    }

    public bool Expanded
    {
        get => _expanded;
        set
        {
            if (_expanded == value)
                return;
            _expanded = value;
            PropertyChanged?.Invoke(this, new(nameof(Expanded)));
        }
    }

    public bool MenuOpen
    {
        get => _menuOpen;
        set
        {
            if (_menuOpen == value)
                return;
            _menuOpen = value;
            PropertyChanged?.Invoke(this, new(nameof(MenuOpen)));
        }
    }
}

internal sealed record AgentsData
{
    public string Title { get; init; } = "";
    public string Tooltip { get; init; } = "";
    public string LastAction { get; init; } = "";
    public string Metadata { get; init; } = "";
    public string Activity { get; init; } = "";
    public string Flow { get; init; } = "";
    public string Initial { get; init; } = "?";
    public Geometry? Logo { get; init; }
    public Brush LogoColor { get; init; } = Palette.Brush("YaacText");
    public string Mark { get; init; } = "";
    public string MarkLabel { get; init; } = "";
    public Brush MarkColor { get; init; } = Palette.Brush("YaacMuted");
    public string Traits { get; init; } = "";
    public string Branch { get; init; } = "";
    public string Changes { get; init; } = "";
    public string Ahead { get; init; } = "";
    public string Behind { get; init; } = "";
    public string Badge { get; init; } = "";
    public string BadgeDetail { get; init; } = "";
    public bool Attention { get; init; }
    public bool Unseen { get; init; }
    public bool Running { get; init; }
    public bool Finished { get; init; }
    public bool HasActions { get; init; }
    public bool CanOpen { get; init; }
    public bool CanUseBadge { get; init; }
    public Thickness Indent { get; init; }
    public bool HasLogo => Logo is not null;
    public bool HasLastAction => LastAction.Length > 0;
    public bool HasMetadata => Metadata.Length > 0;
    public bool HasActivity => Activity.Length > 0;
    public bool HasGit => Branch.Length + Changes.Length + Ahead.Length + Behind.Length > 0;
    public bool HasBadge => Badge.Length > 0;
}

internal sealed record AgentsGroup(string Id, string Label, string Glyph, int? Count,
    IReadOnlyList<AgentsRow> Rows, JsonElement Docked);

internal sealed record AgentsRow(string Id, string CommandId, string Raw, int Depth, AgentsData Data,
    IReadOnlyList<AgentsAction> Actions, AgentsAction? BadgeAction)
{
    private static Brush Muted => Palette.Brush("YaacMuted");
    private static Brush Red => Palette.Brush("YaacFailure");
    private static Brush Green => Palette.Brush("YaacSuccess");
    private static Brush Yellow => Palette.Brush("YaacWarning");

    public static AgentsRow Read(string identity, string id, string raw, JsonElement row)
    {
        string title = row.StringField("title", id != "" ? id : "Agent");
        string provider = row.StringField("provider");
        string origin = row.StringField("origin");
        bool subagent = row.BoolField("subagent");
        string metadata = Joined(" · ", origin, subagent ? "" : provider, row.StringField("model"), row.StringField("target"));
        string last = row.StringField("lastAction", row.StringField("activity"));
        if (subagent)
            last = Joined(" · ", origin, last);

        var (symbol, label, brush) = row.StringField("mark") switch
        {
            "needsYou" => ("!", "needs you", Red),
            "running" => ("", "running", row.BoolField("fast") ? Palette.Brush("YaacFast") : Palette.Brush("YaacText")),
            "paused" => ("Ⅱ", "paused", Yellow),
            "waiting" => ("⌛", "waiting", Muted),
            "done" => ("✓", "done", Green),
            "failed" => ("×", "failed", Red),
            "offline" => ("", "offline", Muted),
            "off" => ("", "off", Muted),
            _ => ("", "", Muted)
        };
        label = row.StringField("markLabel", label);
        var (flow, transfer) = row.StringField("flow") switch
        {
            "sending" => ("↑", "sending"),
            "receiving" => ("↓", "receiving"),
            _ => ("", "")
        };

        var actions = row.Array("actions").Select(AgentsAction.Read).OfType<AgentsAction>().ToArray();
        var badge = row.Field("badge");
        var badgeAction = AgentsAction.Read(badge.Field("action"));
        var git = row.Field("git");
        var traits = Joined(" ", row.BoolField("readOnly") ? "🔒" : "", row.BoolField("fast") ? "ϟ" : "");
        bool unseen = row.BoolField("unseen");
        string attention = row.StringField("attention");
        string info = Joined(" · ", label, transfer, attention, unseen ? "unread" : "",
            row.BoolField("readOnly") ? "read-only" : "", row.BoolField("fast") ? "fast" : "");
        var data = new AgentsData
        {
            CanOpen = id != "" && row.BoolField("canOpen"),
            Title = title,
            Tooltip = Joined("\n", title, info, metadata, row.StringField("activity", last)),
            LastAction = last,
            Metadata = metadata,
            Activity = row.StringField("activity"),
            Running = row.StringField("mark") == "running",
            Finished = row.BoolField("finished"),
            Attention = row.BoolField("attention") || attention != "" || row.StringField("mark") == "needsYou",
            Unseen = unseen,
            HasActions = actions.Length > 0,
            Initial = provider.Length > 0 ? StringInfo.GetNextTextElement(provider).ToUpperInvariant() : "?",
            Logo = ReadLogo(row.StringField("logo")),
            LogoColor = ReadColor(row.StringField("logoColor")),
            Mark = symbol,
            MarkLabel = label,
            MarkColor = brush,
            Traits = traits,
            Flow = flow,
            Branch = git.StringField("branch"),
            Changes = git.CountText("changes"),
            Ahead = WithArrow(git.CountText("ahead"), '↑'),
            Behind = WithArrow(git.CountText("behind"), '↓'),
            Badge = badge.StringField("label"),
            BadgeDetail = badge.StringField("detail"),
            CanUseBadge = badgeAction is { Enabled: true }
        };
        return new(identity, id, raw, Math.Max(0, row.Number("depth")), data, actions, badgeAction);
    }

    private static string Joined(string delimiter, params string[] values) =>
        string.Join(delimiter, values.Where(value => !string.IsNullOrWhiteSpace(value)));

    private static string WithArrow(string text, char arrow)
    {
        if (text == "")
            return "";
        return text.StartsWith(arrow) ? $"  {text}" : $"  {arrow}{text}";
    }

    private static readonly Dictionary<string, Geometry> Logos = new(StringComparer.Ordinal);

    internal static Geometry? ReadLogo(string path)
    {
        if (path == "")
            return null;
        if (Logos.TryGetValue(path, out var existing))
            return existing;
        try
        {
            var geometry = Geometry.Parse(path);
            geometry.Freeze();
            Logos.Add(path, geometry);
            return geometry;
        }
        catch (FormatException)
        {
            return null;
        }
    }

    private static Brush Solid(string color)
    {
        var brush = new SolidColorBrush((Color)ColorConverter.ConvertFromString(color));
        brush.Freeze();
        return brush;
    }

    internal static Brush ReadColor(string color)
    {
        if (color == "")
            return Palette.Brush("YaacText");
        try
        {
            return Solid(color);
        }
        catch (FormatException)
        {
            return Palette.Brush("YaacText");
        }
    }
}

internal sealed record AgentsAction(string Id, string Label, bool Enabled, bool? Checked, string Detail,
    IReadOnlyList<AgentsAction> Children)
{
    public bool IsSeparator => Id == "" && Label == "" && Children.Count == 0;

    public static AgentsAction? Read(JsonElement action)
    {
        if (action.ValueKind != JsonValueKind.Object)
            return null;
        string id = action.StringField("id");
        return new(id, action.StringField("label", id), action.BoolField("enabled"), action.OptionalFlag("checked"), action.StringField("detail"),
            action.Array("children").Select(Read).OfType<AgentsAction>().ToArray());
    }

    public static AgentsAction? Find(IReadOnlyList<AgentsAction> actions, string id)
    {
        foreach (AgentsAction action in actions)
        {
            if (!action.Enabled)
                continue;
            if (action.Id == id)
                return action;
            if (Find(action.Children, id) is { } found)
                return found;
        }
        return null;
    }
}

internal static class AgentsJson
{
    public static JsonElement Field(this JsonElement item, string key) =>
        item.ValueKind == JsonValueKind.Object && item.TryGetProperty(key, out JsonElement found) ? found : default;

    public static string StringField(this JsonElement item, string key, string fallback = "")
    {
        var value = item.Field(key);
        string? text = value.ValueKind == JsonValueKind.String ? value.GetString() : null;
        return string.IsNullOrWhiteSpace(text) ? fallback : text;
    }

    public static IEnumerable<JsonElement> Array(this JsonElement item, string key)
    {
        var value = item.Field(key);
        return value.ValueKind == JsonValueKind.Array ? value.EnumerateArray() : [];
    }

    public static IEnumerable<string> Ids(this JsonElement item, string key) => item.Array(key)
        .Where(value => value.ValueKind == JsonValueKind.String)
        .Select(value => value.GetString() ?? "")
        .Where(id => id.Length > 0);

    public static int? OptionalNumber(this JsonElement item, string key)
    {
        var value = item.Field(key);
        return value.ValueKind == JsonValueKind.Number && value.TryGetInt32(out int number) ? number : null;
    }

    public static int Number(this JsonElement item, string key, int fallback = 0) =>
        item.OptionalNumber(key) ?? fallback;

    public static string CountText(this JsonElement item, string key)
    {
        string text = item.StringField(key);
        if (text != "")
            return text;
        int count = item.Number(key);
        return count != 0 ? count.ToString(CultureInfo.CurrentCulture) : "";
    }

    public static bool? OptionalFlag(this JsonElement item, string key) => item.Field(key).ValueKind switch
    {
        JsonValueKind.True => true,
        JsonValueKind.False => false,
        _ => null
    };

    public static bool BoolField(this JsonElement item, string key, bool fallback = false) =>
        item.OptionalFlag(key) ?? fallback;
}
