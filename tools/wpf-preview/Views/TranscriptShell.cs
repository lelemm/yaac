using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

// A `!command` the user ran (P346): not a chat bubble. One row — the prompt icon and the command
// in mono, a status badge at the right — then the output on a code surface, folded to about 12
// lines with "Show all N lines". The controller sends the folded head; unfolding is local.
internal sealed class TranscriptShell : StackPanel
{
    private const string OpenKey = "shell:open";
    private readonly TranscriptRowState state;
    private readonly TranscriptText output;
    private readonly Action<JsonElement> submit;
    private readonly TextBlock command = new()
    {
        FontSize = 13, TextTrimming = TextTrimming.CharacterEllipsis,
        VerticalAlignment = VerticalAlignment.Center,
    };
    private readonly TextBlock mark = new() { FontSize = 13, Margin = new Thickness(0, 0, 4, 0), VerticalAlignment = VerticalAlignment.Center };
    private readonly TextBlock badge = new() { FontSize = 12, VerticalAlignment = VerticalAlignment.Center };
    private readonly RunningSpinner spinner = new() { Width = 12, Height = 12, Margin = new Thickness(0, 0, 6, 0), VerticalAlignment = VerticalAlignment.Center };
    private readonly Button stop = new() { Content = "Stop", Padding = new Thickness(8, 1, 8, 1), Margin = new Thickness(8, 0, 0, 0) };
    private readonly Button showAll = new() { HorizontalAlignment = HorizontalAlignment.Left, Padding = new Thickness(6, 1, 6, 1), Margin = new Thickness(0, 4, 0, 0) };
    private readonly TextBlock status = new() { FontSize = 12, Margin = new Thickness(0, 3, 0, 0) };
    private JsonElement stopAction;
    private JsonElement data;

    public TranscriptShell(TranscriptRowState state, TranscriptText output, Action<JsonElement> submit)
    {
        this.state = state;
        this.output = output;
        this.submit = submit;
        command.SetResourceReference(TextBlock.FontFamilyProperty, YaacFonts.CodeResource);
        Focusable = false;

        var header = new DockPanel { LastChildFill = true };
        var right = new StackPanel { Orientation = Orientation.Horizontal, VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(10, 0, 0, 0) };
        right.Children.Add(spinner);
        right.Children.Add(mark);
        right.Children.Add(badge);
        right.Children.Add(stop);
        DockPanel.SetDock(right, Dock.Right);
        header.Children.Add(right);
        var icon = PaneIcon.Draw("Terminal", 14);
        icon.Margin = new Thickness(0, 0, 7, 0);
        icon.VerticalAlignment = VerticalAlignment.Center;
        if (icon is Viewbox { Child: Canvas { Children: [System.Windows.Shapes.Path path] } })
            path.SetResourceReference(System.Windows.Shapes.Shape.StrokeProperty, "YaacShell");
        DockPanel.SetDock(icon, Dock.Left);
        header.Children.Add(icon);
        header.Children.Add(command);
        Children.Add(header);

        output.Margin = new Thickness(0, 6, 0, 0);
        Children.Add(output);
        showAll.SetResourceReference(StyleProperty, "YaacQuietButton");
        showAll.SetResourceReference(Control.ForegroundProperty, "YaacMuted");
        showAll.Click += (_, _) =>
        {
            state.Expanded[OpenKey] = !state.Expanded.GetValueOrDefault(OpenKey);
            Render(data);
        };
        Children.Add(showAll);
        status.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
        Children.Add(status);

        stop.Click += (_, _) =>
        {
            if (stopAction.ValueKind == JsonValueKind.Object)
                submit(stopAction);
        };
        AutomationProperties.SetName(stop, "Stop the command");
    }

    public void Render(JsonElement value)
    {
        data = value;
        var text = value.Value("command").Text();
        command.Text = text;
        command.ToolTip = value.Value("cwd").OptionalText() is { } cwd ? $"{text}\n{cwd}" : text;
        command.SetResourceReference(TextBlock.ForegroundProperty, "YaacText");
        AutomationProperties.SetName(command, "Command " + text);

        var running = value.Value("running").Flag();
        spinner.Visibility = running ? Visibility.Visible : Visibility.Collapsed;
        spinner.Foreground = Palette.Brush("YaacShell");
        var ok = value.Value("ok");
        mark.Visibility = ok.ValueKind is JsonValueKind.True or JsonValueKind.False ? Visibility.Visible : Visibility.Collapsed;
        mark.Text = ok.ValueKind == JsonValueKind.True ? "✓" : "✗";
        mark.Foreground = ok.ValueKind == JsonValueKind.True ? TranscriptColors.Success : TranscriptColors.Error;
        var badgeText = running ? "running" : value.Value("badge").OptionalText() ?? "";
        badge.Text = badgeText;
        badge.Foreground = ok.ValueKind == JsonValueKind.False ? TranscriptColors.Error : TranscriptColors.Muted;
        badge.Visibility = badgeText.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        stopAction = value.Value("stop");
        stop.Visibility = stopAction.ValueKind == JsonValueKind.Object ? Visibility.Visible : Visibility.Collapsed;
        stop.IsEnabled = stopAction.Value("enabled").Flag();

        var all = value.Value("output").Text();
        var shown = value.Value("shown").OptionalText();
        var open = state.Expanded.GetValueOrDefault(OpenKey);
        output.Show(shown is not null && !open ? shown : all, "terminal");
        var label = value.Value("showAll").OptionalText();
        showAll.Visibility = label is null ? Visibility.Collapsed : Visibility.Visible;
        showAll.Content = open ? "Show less" : label;
        var notes = new List<string>();
        if (value.Value("truncated").Flag())
            notes.Add("Output truncated");
        if (value.Value("status").OptionalText() is { Length: > 0 } sending)
            notes.Add(sending);
        status.Text = string.Join(" · ", notes);
        status.Visibility = notes.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
    }
}
