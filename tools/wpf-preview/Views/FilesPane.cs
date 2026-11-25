using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;

namespace Yaac.WpfPreview.Views;

internal sealed class FilesPane : DockPanel
{
    private readonly WrapPanel tools = new() { Margin = new Thickness(4) };
    private readonly TextBlock status = new() { Margin = new Thickness(12, 8, 12, 8), TextWrapping = TextWrapping.Wrap };
    private readonly FilesTree tree = new();
    private readonly FilesEditor editor = new();
    private string kind = "";
    private bool full, sideBySide, ignored, canNavigate;
    private ulong? expected, displayed;
    private string treeSignature = "";

    internal event Action<string, object?>? Command;
    internal bool OwnsKeyboard => editor.IsKeyboardFocusWithin;

    internal FilesPane()
    {
        var toolbar = new Border { Child = tools, BorderThickness = new Thickness(0, 0, 0, 1) };
        toolbar.SetResourceReference(BackgroundProperty, "YaacPanel");
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

    internal void Render(JsonElement state, JsonElement tab, string thread, bool fullscreen)
    {
        var files = state.Value("files");
        var nextKind = tab.Value("kind").GetString()!;
        var nextMode = files.Value("sideBySide").ValueKind == JsonValueKind.True;
        var showIgnored = files.Value("showIgnored").ValueKind == JsonValueKind.True;
        var next = FileDocumentReader.Expected(state);
        if (expected != next)
        {
            expected = next;
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
        if (kind == nextKind && full == fullscreen && sideBySide == nextMode && ignored == showIgnored
            && canNavigate == navigate) return;
        kind = nextKind;
        full = fullscreen;
        sideBySide = nextMode;
        ignored = showIgnored;
        canNavigate = navigate;
        RenderToolbar();
    }

    internal void ReceiveDocument(IncomingFileDocument value)
    {
        if (expected != value.Id) return;
        displayed = value.Id;
        editor.Render(value);
        editor.Visibility = Visibility.Visible;
        status.Visibility = Visibility.Collapsed;
    }

    internal void DocumentFailed(ulong id)
    {
        if (expected != id) return;
        status.Text = "Could not display the document.";
        status.Visibility = Visibility.Visible;
    }

    internal void Navigate(ulong id, int row) => editor.GoTo(id, row);

    internal void Clear()
    {
        if (expected is not null) editor.Clear();
        expected = displayed = null;
    }

    private void RenderToolbar()
    {
        tools.Children.Clear();
        if (kind == "files")
        {
            Button("Refresh", () => Command?.Invoke("refresh", null));
            Button("Collapse all", () => Command?.Invoke("collapse", null));
            var toggle = new CheckBox { Content = "Show ignored", IsChecked = ignored, Margin = new Thickness(6, 1, 6, 1),
                VerticalAlignment = VerticalAlignment.Center };
            toggle.Click += (_, _) => Command?.Invoke("ignored", new { show = toggle.IsChecked == true });
            tools.Children.Add(toggle);
        }
        else
        {
            Button("Reload", () => Command?.Invoke("reload", null));
            if (kind == "diff")
            {
                var mode = Button(sideBySide ? "Side by side" : "Unified",
                    () => Command?.Invoke("mode", new { sideBySide = !sideBySide }));
                mode.ToolTip = sideBySide ? "Switch to unified" : "Switch to side by side";
                Button("Previous change", () => editor.Navigate(false)).IsEnabled = canNavigate;
                Button("Next change", () => editor.Navigate(true)).IsEnabled = canNavigate;
            }
        }
        Button(full ? "Restore" : "Full screen", () => Command?.Invoke("fullScreen", null));
    }

    private Button Button(string title, Action action)
    {
        var button = new Button { Content = title };
        button.SetResourceReference(StyleProperty, "YaacQuietButton");
        AutomationProperties.SetName(button, title);
        button.Click += (_, _) => action();
        tools.Children.Add(button);
        return button;
    }
}
