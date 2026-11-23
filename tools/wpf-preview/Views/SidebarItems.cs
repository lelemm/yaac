using System.ComponentModel;
using System.Globalization;
using System.Text.Json;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal static class SidebarJson
{
    internal static JsonElement Get(this JsonElement value, string name) =>
        value.ValueKind == JsonValueKind.Object && value.TryGetProperty(name, out var property)
            ? property : default;

    internal static string Text(this JsonElement value, string name) =>
        value.Get(name) is { ValueKind: JsonValueKind.String } text ? text.GetString() ?? "" : "";

    internal static bool Flag(this JsonElement value, string name) =>
        value.Get(name).ValueKind == JsonValueKind.True;

    internal static IEnumerable<JsonElement> Items(this JsonElement value) =>
        value.ValueKind == JsonValueKind.Array ? value.EnumerateArray() : [];

    internal static object? Tip(this string value) => value.Length == 0 ? null : value;
}

internal sealed record SidebarAction(string Id, string Label, bool Enabled, bool? Checked,
    string Detail, IReadOnlyList<SidebarAction> Children, bool Danger = false)
{
    public bool IsSeparator => Id.Length == 0 && Label.Length == 0;
    public object? ToolTip => Detail.Tip();

    public static SidebarAction? Read(JsonElement value)
    {
        if (value.ValueKind != JsonValueKind.Object)
            return null;
        bool? @checked = value.Get("checked").ValueKind switch
        {
            JsonValueKind.True => true,
            JsonValueKind.False => false,
            _ => null,
        };
        return new(value.Text("id"), value.Text("label"), value.Flag("enabled"), @checked,
            value.Text("detail"), ReadList(value.Get("children")), value.Flag("danger"));
    }

    public static IReadOnlyList<SidebarAction> ReadList(JsonElement value) =>
        value.Items().Select(Read).OfType<SidebarAction>().ToArray();
}

internal sealed class SidebarActions
{
    private string? signature;
    public IReadOnlyList<SidebarAction> Items { get; private set; } = [];

    public bool Update(JsonElement value)
    {
        string? next = value.ValueKind == JsonValueKind.Array ? value.GetRawText() : null;
        if (next == signature)
            return false;
        signature = next;
        Items = SidebarAction.ReadList(value);
        return true;
    }
}

internal enum SidebarItemKind { Row, Header, Status, More }

internal sealed class SidebarItem(SidebarItemKind kind, string id) : INotifyPropertyChanged
{
    private string? signature;
    private bool collapsed;
    private bool multi;
    private bool menuOpen;
    private bool dropTarget;

    public event PropertyChangedEventHandler? PropertyChanged;
    public string Id { get; } = id;
    public SidebarItemKind Kind { get; } = kind;
    public string SectionId { get; private set; } = "";
    public bool GappedHeader { get; private set; }
    public double Height => Kind == SidebarItemKind.Header ? (GappedHeader ? 38 : 26)
        : State.HasLine2 ? 44 : 28;
    public SidebarItemState State { get; private set; } = new();
    public bool IsMulti => multi;
    public bool IsMenuOpen => menuOpen;
    public bool IsDropTarget => dropTarget;

    public void Update(JsonElement value, string sectionId, bool isCollapsed = false, bool gapped = false)
    {
        string next = value.GetRawText();
        SectionId = sectionId;
        if (signature == next && collapsed == isCollapsed && GappedHeader == gapped)
            return;
        bool gapChanged = GappedHeader != gapped;
        double oldHeight = Height;
        signature = next;
        collapsed = isCollapsed;
        GappedHeader = gapped;
        State = new(Kind, value, collapsed);
        Changed(nameof(State));
        if (Height != oldHeight)
            Changed(nameof(Height));
        if (gapChanged)
            Changed(nameof(GappedHeader));
    }

    public void UpdateStatus(string title)
    {
        if (State.Title == title)
            return;
        State = new SidebarItemState { Title = title };
        Changed(nameof(State));
    }

    public void SetMulti(bool value) => SetFlag(ref multi, value, nameof(IsMulti));
    public void SetMenuOpen(bool value) => SetFlag(ref menuOpen, value, nameof(IsMenuOpen));
    public void SetDropTarget(bool value) => SetFlag(ref dropTarget, value, nameof(IsDropTarget));

    private void SetFlag(ref bool field, bool value, string property)
    {
        if (field == value)
            return;
        field = value;
        Changed(property);
    }

    private void Changed(string property) => PropertyChanged?.Invoke(this, new(property));
}

internal sealed class SidebarItemState
{
    private static readonly IReadOnlyDictionary<string, SidebarAction> NoDrops =
        new Dictionary<string, SidebarAction>();

    public SidebarItemState() { }

    public SidebarItemState(SidebarItemKind kind, JsonElement value, bool collapsed)
    {
        if (kind == SidebarItemKind.More)
        {
            Primary = SidebarAction.Read(value);
            Title = Primary?.Label ?? "";
            Detail = Primary?.Detail ?? "";
            return;
        }

        Actions = SidebarAction.ReadList(value.Get("actions"));
        if (kind == SidebarItemKind.Header)
        {
            Title = value.Text("label");
            Machine = value.Text("machine");
            Connection = value.Text("connection");
            Tone = value.Text("tone");
            Detail = value.Text("detail");
            Temporary = value.Flag("temporary");
            OlderDaemon = value.Flag("olderDaemon");
            Toggle = SidebarAction.Read(value.Get("toggle"));
            Primary = SidebarAction.Read(value.Get("primary"));
            HeaderGlyph = Toggle is not null ? (collapsed ? "▸" : "▾") : Temporary ? "◌" : "";
            TemporaryGlyph = Toggle is not null && Temporary ? "◌" : "";
            return;
        }

        Title = value.Text("title");
        Right = value.Text("right");
        Status = value.Text("status");
        CopyId = value.Text("copyId");
        Tone = value.Text("tone");
        Glyph = value.Text("glyph");
        Fast = value.Flag("fast");
        Ultrafast = value.Flag("ultrafast");
        Detail = value.Text("detail");
        Selected = value.Flag("selected");
        Unseen = value.Flag("unseen");
        Muted = value.Flag("muted");
        ReadOnly = value.Flag("readOnly");
        Forked = value.Flag("forked");
        Draggable = value.Flag("draggable");
        Badge = SidebarAction.Read(value.Get("badge"));

        if (value.Get("drops") is { ValueKind: JsonValueKind.Object } drops)
        {
            var actions = new Dictionary<string, SidebarAction>(StringComparer.Ordinal);
            foreach (var destination in drops.EnumerateObject())
            {
                if (SidebarAction.Read(destination.Value) is { } action)
                    actions[destination.Name] = action;
            }
            Drops = actions;
        }

        if (value.Get("line2") is not { ValueKind: JsonValueKind.Object } line)
            return;
        HasLine2 = true;
        Provider = line.Text("provider");
        ProviderLogo = AgentsRow.ReadLogo(line.Text("logo"));
        ProviderLogoColor = line.Text("logoColor") is { Length: > 0 } color
            ? AgentsRow.ReadColor(color) : Palette.Brush("YaacMuted");
        Branch = line.Text("branch");
        Added = Count(line, "added", "+");
        Modified = Count(line, "modified", "~");
        Deleted = Count(line, "deleted", "−");
        Ahead = Count(line, "ahead", "↑");
        Behind = Count(line, "behind", "↓");
        Workers = Count(line, "workers", "♟ ");
    }

    public string Title { get; init; } = "";
    public string Detail { get; } = "";
    public string Right { get; } = "";
    public string Status { get; } = "";
    public string CopyId { get; } = "";
    public string Tone { get; } = "";
    public string Glyph { get; } = "";
    public bool Selected { get; }
    public bool Unseen { get; }
    public bool Muted { get; }
    public bool ReadOnly { get; }
    public bool Forked { get; }
    public bool Draggable { get; }
    public string Machine { get; } = "";
    public string Connection { get; } = "";
    public bool Temporary { get; }
    public bool OlderDaemon { get; }
    public string HeaderGlyph { get; } = "";
    public string TemporaryGlyph { get; } = "";
    public bool HasToggle => Toggle is not null;
    public SidebarAction? Toggle { get; }
    public SidebarAction? Primary { get; }
    public SidebarAction? Badge { get; }
    public IReadOnlyList<SidebarAction> Actions { get; } = [];
    public IReadOnlyDictionary<string, SidebarAction> Drops { get; } = NoDrops;
    public bool HasLine2 { get; }
    public string Provider { get; } = "";
    public Geometry? ProviderLogo { get; }
    public Brush? ProviderLogoColor { get; }
    public string Branch { get; } = "";
    public string BranchLabel => Branch.Length == 0 ? "" : $"⑂ {Branch}";
    public string Added { get; } = "";
    public string Modified { get; } = "";
    public string Deleted { get; } = "";
    public string Ahead { get; } = "";
    public string Behind { get; } = "";
    public string Workers { get; } = "";

    public string Mark => Glyph.Length > 0 ? Glyph : Tone switch
    {
        "waiting" => "⌛",
        "needs" or "needsYou" or "needs you" => "!",
        "paused" => "Ⅱ",
        "failed" => "×",
        "done" => "✓",
        _ => "",
    };

    public bool Fast { get; }
    public bool Ultrafast { get; }

    public bool MarkDanger => Tone is "needs" or "needsYou" or "needs you" or "failed";
    public bool Dimmed => Muted || Tone is "offline" or "off";
    public string AccessibilityStatus => string.Join(" · ", new[]
    {
        Status, Unseen ? "Unread" : "", ReadOnly ? "Read-only" : "", Muted ? "Muted" : "",
        Selected ? "Open thread" : "", Right,
    }.Where(part => part.Length > 0));
    public object? ToolTip => (Detail.Length > 0 ? Detail : AccessibilityStatus).Tip();

    private static string Count(JsonElement value, string name, string prefix) =>
        value.Get(name) is { ValueKind: JsonValueKind.Number } count
        && count.TryGetUInt32(out uint number) && number > 0
            ? prefix + number.ToString(CultureInfo.CurrentCulture) : "";

}
