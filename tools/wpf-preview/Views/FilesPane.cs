using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;

namespace Yaac.WpfPreview.Views;

internal sealed class FilesPane : DockPanel
{
    private readonly DockPanel tools = new() { Margin = new Thickness(4,0,4,0), LastChildFill = false };
    private string path = "";
    private readonly TextBlock counter = new() { VerticalAlignment = VerticalAlignment.Center };
    private int changeCount;
    private int ordinal;
    private int? changeRow;
    private readonly TextBlock status = new() { Margin = new Thickness(12, 8, 12, 8), TextWrapping = TextWrapping.Wrap };
    private readonly FilesTree tree = new();
    private readonly FilesEditor editor = new();
    private string kind = "";
    private bool sideBySide, ignored, canNavigate, canCollapse;
    private ulong? expected, displayed;
    private string treeSignature = "";

    internal event Action<string, object?>? Command;
    internal bool OwnsKeyboard => editor.IsKeyboardFocusWithin;

    internal FilesPane()
    {
        var toolbar = new Border { Height = 36, Child = tools, BorderThickness = new Thickness(0, 0, 0, 1) };
        toolbar.SetResourceReference(BackgroundProperty, "YaacCanvas");
        toolbar.SetResourceReference(Border.BorderBrushProperty, "YaacRule");
        SetDock(toolbar, Dock.Top);
        status.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
        SetDock(status, Dock.Top);
        Children.Add(toolbar);
        Children.Add(status);
        var body = new Grid();
        body.Children.Add(tree);
        body.Children.Add(editor);
        Children.Add(body);
        tree.Command += (command, data) => Command?.Invoke(command, data);
        editor.Command += (command, data) => Command?.Invoke(command, data);
    }

    internal void Render(JsonElement state, JsonElement tab, string thread)
    {
        var files = state.Value("files");
        var nextKind = tab.Value("kind").GetString()!;
        var nextMode = files.Value("sideBySide").ValueKind == JsonValueKind.True;
        var showIgnored = files.Value("showIgnored").ValueKind == JsonValueKind.True;
        bool expanded = files.Value("canCollapse").ValueKind == JsonValueKind.True;
        var next = FileDocumentReader.Expected(state);
        if (expected != next)
        {
            expected = next;
            ordinal = 0;
            changeRow = files.Value("firstChange") is { ValueKind: JsonValueKind.Number } first ? first.GetInt32() : null;
            displayed = null;
            editor.Clear();
        }
        bool treeTab = nextKind == "files";
        bool navigate = files.Value("changes").ValueKind == JsonValueKind.True;
        var message = files.Value("status") is { ValueKind: JsonValueKind.String } text ? text.GetString()! : "";
        if (!treeTab && message.Length == 0)
            message = displayed == expected && displayed is not null ? "" : "Loading…";
        status.Text = message;
        status.Visibility = message.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        tree.Visibility = treeTab ? Visibility.Visible : Visibility.Collapsed;
        editor.Visibility = displayed == expected && displayed is not null ? Visibility.Visible : Visibility.Collapsed;

        if (treeTab)
        {
            var signature = thread + ":" + files.Value("folders").GetRawText() + files.Value("errors").GetRawText();
            if (treeSignature != signature)
            {
                treeSignature = signature;
                tree.Render(files, thread);
            }
        }
        int count = files.Value("changeCount") is { ValueKind: JsonValueKind.Number } n ? n.GetInt32() : 0;
        var nextPath = tab.Value("path").OptionalText() ?? "";
        counter.Text = $"{(ordinal == 0 && count > 0 ? 1 : ordinal)} of {count}";
        if (kind == nextKind && path == nextPath && changeCount == count && sideBySide == nextMode && ignored == showIgnored
            && canNavigate == navigate && canCollapse == expanded) return;
        kind = nextKind;
        path = nextPath;
        changeCount = count;
        sideBySide = nextMode;
        ignored = showIgnored;
        canNavigate = navigate;
        canCollapse = expanded;
        RenderToolbar();
    }

    internal void ReceiveDocument(IncomingFileDocument value)
    {
        if (expected != value.Id) return;
        displayed = value.Id;
        editor.Render(value);
        if (changeRow is int row) editor.GoTo(value.Id, row);
        editor.Visibility = Visibility.Visible;
        status.Visibility = Visibility.Collapsed;
    }

    internal void DocumentFailed(ulong id)
    {
        if (expected != id) return;
        status.Text = "Could not display the document.";
        status.Visibility = Visibility.Visible;
    }

    internal void Navigate(ulong id, int row, int change)
    {
        if (id != expected) return;
        changeRow = row;
        if (id == displayed) editor.GoTo(id, row);
        ordinal = change;
        counter.Text = $"{ordinal} of {changeCount}";
    }

    internal bool Shortcut(Key key, ModifierKeys modifiers)
    {
        if (key == Key.F5 && modifiers == ModifierKeys.None)
        {
            Command?.Invoke(kind == "files" ? "refresh" : "reload", null);
            return true;
        }
        if (kind == "diff" && canNavigate && key == Key.F7 &&
            modifiers is ModifierKeys.None or ModifierKeys.Shift)
        {
            editor.Navigate(modifiers == ModifierKeys.None);
            return true;
        }
        return false;
    }

    internal void Clear()
    {
        if (expected is not null) editor.Clear();
        expected = displayed = null;
    }

    private void RenderToolbar()
    {
        tools.Children.Clear();
        tools.LastChildFill = kind != "files";
        if (kind == "files")
        {
            var eye = ActionButton("Show ignored", () => Command?.Invoke("ignored", new { show = !ignored }));
            if (ignored) eye.SetResourceReference(BackgroundProperty, "YaacSelected");
            AutomationProperties.SetItemStatus(eye, ignored ? "On" : "Off");
            ActionButton("Reload", () => Command?.Invoke("refresh", null), "Refresh (F5)");
            ActionButton("Collapse all", () => Command?.Invoke("collapse", null)).IsEnabled = canCollapse;
            return;
        }

        // Dock from the right so the path yields space to the controls.
        var reload = ActionButton("Reload", () => Command?.Invoke("reload", null), "Reload (F5)");
        SetDock(reload, Dock.Right);
        if (kind == "diff")
        {
            var next = ActionButton("Next change", () => editor.Navigate(true), "Next change (F7)");
            SetDock(next, Dock.Right);
            next.IsEnabled = canNavigate;
            SetDock(counter, Dock.Right);
            tools.Children.Add(counter);
            var prev = ActionButton("Previous change", () => editor.Navigate(false), "Previous change (Shift+F7)");
            SetDock(prev, Dock.Right);
            prev.IsEnabled = canNavigate;
            var modes = new StackPanel { Orientation = Orientation.Horizontal, VerticalAlignment = VerticalAlignment.Center };
            foreach (bool paired in new[] { false, true })
            {
                var label = paired ? "Side by side" : "Unified";
                var segment = new Button { Content = label, ToolTip = label, Padding = new Thickness(7,3,7,3) };
                segment.SetResourceReference(StyleProperty, "YaacQuietButton");
                if (sideBySide == paired) segment.SetResourceReference(BackgroundProperty, "YaacSelected");
                segment.Click += (_, _) => Command?.Invoke("mode", new { sideBySide = paired });
                modes.Children.Add(segment);
            }
            var group = new Border { Child = modes, BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(5),
                BorderBrush = Palette.Brush("YaacRule"), VerticalAlignment = VerticalAlignment.Center };
            SetDock(group, Dock.Right);
            tools.Children.Add(group);
        }
        var text = new TextBlock { Text = path, FlowDirection = FlowDirection.LeftToRight,
            Foreground = Palette.Brush("YaacMuted"), VerticalAlignment = VerticalAlignment.Center };
        var clip = new Grid { ClipToBounds = true, ToolTip = path, Margin = new Thickness(5,0,6,0) };
        clip.Children.Add(text);
        clip.SizeChanged += (_, _) =>
        {
            text.Text = path;
            text.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
            if (text.DesiredSize.Width <= clip.ActualWidth) return;
            var basename = path.Split('/', (char)92)[^1];
            text.Text = basename;
            text.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
            if (text.DesiredSize.Width > clip.ActualWidth)
            {
                text.Text = "";
                return;
            }
            int low = basename.Length, high = path.Length;
            while (low < high)
            {
                int length = (low + high + 1) / 2;
                text.Text = "…" + path[^length..];
                text.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
                if (text.DesiredSize.Width <= clip.ActualWidth) low = length;
                else high = length - 1;
            }
            text.Text = low == basename.Length ? basename : "…" + path[^low..];
        };
        tools.Children.Add(clip);
    }

    private Button ActionButton(string name, Action action, string? tip = null)
    {
        var button = new Button();
        PaneIcon.Set(button, name, tip);
        if (tip != null) AutomationProperties.SetName(button, tip.Split(" (")[0]);
        button.Click += (_, _) => action();
        tools.Children.Add(button);
        return button;
    }
}
