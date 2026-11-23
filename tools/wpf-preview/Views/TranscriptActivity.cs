using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal sealed class TranscriptActivity : StackPanel
{
    private readonly DockPanel line = new() { MinHeight = 22, ClipToBounds = true };
    private readonly DockPanel expandedArea = new() { Margin = new Thickness(0, 2, 8, 6) };
    private readonly StackPanel expanded = new();
    private readonly WrapPanel peers = new();
    private readonly TextBlock receipt = Label("");
    private readonly TextBlock resultLabel = Label("Result");
    private readonly TranscriptText message;
    private readonly TranscriptText result;
    private readonly TranscriptRowState state;
    private readonly Action<JsonElement> open;
    private readonly Button disclosure;
    private readonly TextBlock staticMark = Label("");
    private readonly bool call;
    private readonly Action showDetails;
    private JsonElement current;
    private bool canExpand;
    private string mark = "";

    public TranscriptActivity(TranscriptRowState state, Func<string?> copy, Action<JsonElement> open,
        FrameworkElement details, Action showDetails, bool call)
    {
        this.state = state;
        this.call = call;
        this.open = open;
        this.showDetails = showDetails;
        Children.Add(line);
        Children.Add(expandedArea);
        line.Background = Brushes.Transparent;
        line.MouseEnter += (_, _) => UpdateMark();
        line.MouseLeave += (_, _) => UpdateMark();
        line.MouseLeftButtonUp += (_, _) => Toggle();
        disclosure = new Button
        {
            Style = (Style)FindResource("YaacQuietButton"),
            Width = 22,
            MinHeight = 22,
            Padding = new Thickness(0),
            FontFamily = new FontFamily("Segoe UI Symbol"),
        };
        disclosure.Click += (_, _) => Toggle();
        disclosure.IsKeyboardFocusWithinChanged += (_, _) => UpdateMark();
        staticMark.Width = 22;
        staticMark.Margin = new Thickness(0);
        staticMark.TextAlignment = TextAlignment.Center;
        staticMark.FontFamily = disclosure.FontFamily;
        var rule = new Border
        {
            Width = 1, Margin = new Thickness(0, 1, 0, 2),
            Background = TranscriptColors.Rule, HorizontalAlignment = HorizontalAlignment.Center,
        };
        var guide = new Border
        {
            Width = 18, Margin = new Thickness(0, 0, 4, 0), Background = Brushes.Transparent,
            Child = rule, Cursor = Cursors.Hand, ToolTip = "Collapse",
        };
        guide.MouseEnter += (_, _) => rule.Background = TranscriptColors.Muted;
        guide.MouseLeave += (_, _) => rule.Background = TranscriptColors.Rule;
        guide.MouseLeftButtonUp += (_, _) => Toggle();
        expandedArea.Children.Add(guide);
        expandedArea.Children.Add(expanded);
        expanded.Children.Add(peers);
        message = new TranscriptText("call-message", state, copy) { Foreground = TranscriptColors.Text };
        result = new TranscriptText("call-result", state, copy) { Foreground = TranscriptColors.Text };
        expanded.Children.Add(message);
        expanded.Children.Add(receipt);
        expanded.Children.Add(resultLabel);
        expanded.Children.Add(result);
        expanded.Children.Add(details);
        expandedArea.Visibility = Visibility.Collapsed;
    }

    public void Render(JsonElement data)
    {
        current = data;
        var tone = TranscriptColors.Tone(data.Value("tone").Text(), TranscriptColors.Muted);
        mark = data.Value("mark").Text();
        var pills = data.Value("pills").Elements().ToArray();
        canExpand = pills.Length > 0 || data.Value("parts").Elements().Any(part => part.Value("text").OptionalText() is not null)
            || new[] { "detail", "message", "result", "receipt" }.Any(field => data.Value(field).OptionalText() is not null);
        disclosure.Foreground = tone;
        staticMark.Foreground = tone;
        staticMark.Text = mark;
        staticMark.FontSize = mark == "•" ? 9 : 12;
        line.Cursor = canExpand ? Cursors.Hand : null;
        line.Children.Clear();
        line.Children.Add(canExpand ? disclosure : staticMark);
        var tail = data.Value("tail").OptionalText();
        if (tail is not null)
        {
            var status = Label($"· {tail}", tone);
            status.MaxWidth = 210;
            status.ToolTip = tail;
            DockPanel.SetDock(status, Dock.Right);
            line.Children.Add(status);
        }
        AddLabel(line, data.Value("lead").Text(), tone);
        foreach (var pill in pills.Take(3))
            line.Children.Add(Pill(pill));
        if (pills.Length > 3)
            AddLabel(line, $"+{pills.Length - 3}", tone);
        AddLabel(line, data.Value("after").Text(), tone);
        var preview = data.Value("preview").OptionalText();
        if (call)
            line.Children.Add(Label(preview is null ? "" : $"“{preview}”", tone));
        else
        {
            // The last child gets the remaining width, so long titles elide before the counts.
            var counts = new StackPanel { Orientation = Orientation.Horizontal };
            AddLabel(counts, data.Value("added").Text(), TranscriptColors.Success);
            AddLabel(counts, data.Value("removed").Text(), TranscriptColors.Error);
            DockPanel.SetDock(counts, Dock.Right);
            line.Children.Insert(1, counts);
        }

        ShowExpanded(canExpand && state.Expanded.GetValueOrDefault("activity"));
        if (IsExpanded)
            RenderDetails(data);
    }

    private void RenderDetails(JsonElement data)
    {
        var tone = TranscriptColors.Tone(data.Value("tone").Text(), TranscriptColors.Muted);
        var pills = data.Value("pills").Elements().ToArray();
        peers.Children.Clear();
        if (pills.Length > 0)
        {
            AddLabel(peers, data.Value("heading").Text(), tone);
            foreach (var pill in pills)
                peers.Children.Add(Pill(pill));
        }
        message.Show(data.Value("message").Text(), "markdown", data.Value("messageBlocks"));
        receipt.Text = data.Value("receipt").Text();
        receipt.Foreground = tone;
        receipt.Visibility = receipt.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        var resultText = data.Value("result").Text();
        resultLabel.Visibility = resultText.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        result.Show(resultText, "markdown", data.Value("resultBlocks"));
    }

    public void DetachDetails(FrameworkElement details) => expanded.Children.Remove(details);

    public bool IsExpanded => expandedArea.Visibility == Visibility.Visible;

    private void Toggle()
    {
        if (!canExpand) return;
        ShowExpanded(!IsExpanded);
        if (IsExpanded)
        {
            RenderDetails(current);
            showDetails();
        }
    }

    private void ShowExpanded(bool value)
    {
        expandedArea.Visibility = value ? Visibility.Visible : Visibility.Collapsed;
        state.Expanded["activity"] = value;
        var verb = value ? "Collapse" : "Expand";
        var subject = current.Value("disclosureName").OptionalText() ?? (call ? "agent call" : "tool details");
        var name = $"{verb} {subject}";
        AutomationProperties.SetName(disclosure, name);
        disclosure.ToolTip = name;
        UpdateMark();
    }

    private void UpdateMark()
    {
        var active = canExpand && (line.IsMouseOver || disclosure.IsKeyboardFocusWithin || IsExpanded);
        line.Background = active ? Palette.Brush("YaacHover") : Brushes.Transparent;
        var symbol = !call && mark == "•" && active ? IsExpanded ? "⌄" : "›" : mark;
        disclosure.Content = symbol;
        disclosure.FontSize = symbol == "•" ? 9 : 12;
    }

    private FrameworkElement Pill(JsonElement data)
    {
        var title = data.Value("label").Text();
        var target = data.Value("machine").OptionalText();
        var clickable = data.Value("enabled").Flag();
        var text = Label(target is null ? title : $"{title} · {target}",
            clickable ? TranscriptColors.Text : TranscriptColors.Muted);
        text.Margin = new Thickness(0);
        text.MaxWidth = 185;
        var face = new DockPanel();
        if (AgentsRow.ReadLogo(data.Value("logo").Text()) is { } logo)
        {
            face.Children.Add(new System.Windows.Shapes.Path
            {
                Data = logo, Fill = TranscriptColors.Muted, Stretch = Stretch.Uniform,
                Width = 12, Height = 12, Margin = new Thickness(0, 0, 5, 0),
                VerticalAlignment = VerticalAlignment.Center,
            });
        }
        face.Children.Add(text);
        var plate = new Border
        {
            Child = face, Background = Palette.Brush("YaacInput"), CornerRadius = new CornerRadius(10),
            Padding = new Thickness(7, 1, 7, 1), VerticalAlignment = VerticalAlignment.Center,
        };
        if (!clickable)
        {
            plate.Margin = new Thickness(0, 0, 5, 0);
            return plate;
        }
        var button = new Button
        {
            Style = (Style)FindResource("YaacQuietButton"), Content = plate, Padding = new Thickness(0),
            MinHeight = 0, Margin = new Thickness(0, 0, 5, 0), ToolTip = text.Text,
        };
        AutomationProperties.SetName(button, title);
        button.Click += (_, _) => open(data);
        return button;
    }

    private static void AddLabel(Panel panel, string text, Brush tone)
    {
        if (text.Length > 0) panel.Children.Add(Label(text, tone));
    }

    private static TextBlock Label(string text, Brush? tone = null) => new()
    {
        Text = text, Foreground = tone ?? TranscriptColors.Muted, TextTrimming = TextTrimming.CharacterEllipsis,
        Margin = new Thickness(0, 0, 6, 0), VerticalAlignment = VerticalAlignment.Center, FontSize = 12,
    };
}
