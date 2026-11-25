using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class BrowserView
{
    private void RenderTabs(JsonElement tabs, string reason)
    {
        var signature = JsonSerializer.Serialize(new { thread, active, full, chat, chatTitle, chatLogo, chatLogoColor, chatMarked, chatTone,
            reason, filesReason, tabs = tabs.EnumerateArray().Select(t => new[] { Text(t, "title"), Text(t, "kind"),
                Text(t, "url"), Text(t, "agentLogo"), Text(t, "agentLogoColor"), Text(t, "path"), Text(t, "mark"),
                t.GetProperty("id").ToString(), Flag(t, "loading").ToString() }) });
        if (signature == tabSignature) return;
        tabSignature = signature;
        object? focused = TabStrip.IsKeyboardFocusWithin && Keyboard.FocusedElement is Button old ? old.Tag : null;
        TabStrip.Children.Clear();
        selectedTab = null;
        var ids = tabs.EnumerateArray().Select(t => t.GetProperty("id").GetUInt64()).ToHashSet();
        foreach (var id in icons.Keys.Where(id => !ids.Contains(id)).ToArray()) icons.Remove(id);
        if (full) AddTab(null, chatTitle.Length == 0 ? "Chat" : chatTitle, default, chat);
        foreach (var tab in tabs.EnumerateArray())
        {
            var id = tab.GetProperty("id").GetUInt64();
            var title = Text(tab, "title");
            if (title.Length == 0) title = Text(tab, "url");
            AddTab(id, title.Length == 0 ? "New tab" : title, tab, !chat && active == id);
        }
        AddTabButton.IsEnabled = reason.Length == 0 || filesReason.Length == 0;
        AddTabButton.ToolTip = AddTabButton.IsEnabled ? "New tab" : reason;
        SizeTabs();

        void AddTab(ulong? id, string title, JsonElement tab, bool selected)
        {
            var row = new Border { ClipToBounds = true, Height = 32, VerticalAlignment = VerticalAlignment.Bottom,
                CornerRadius = new CornerRadius(5, 5, 0, 0), BorderThickness = new Thickness(1, 1, 1, 0),
                BorderBrush = selected ? Palette.Brush("YaacRule") : Brushes.Transparent };
            var body = new DockPanel();
            row.Child = body;
            var close = new Button { VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(0, 0, 2,0) };
            close.Tag = (id, true);
            PaneIcon.Set(close, "Close");
            close.Content = PaneIcon.Draw("Close", 12);
            close.Width = close.Height = 20;
            AutomationProperties.SetName(close, $"Close {title}");
            close.ToolTip = $"Close {title} (middle-click tab)";
            if (id is ulong value)
            {
                close.Click += (_, _) => SendFor(value, "close");
                row.PreviewMouseDown += (_, e) =>
                {
                    if (e.ChangedButton != MouseButton.Middle) return;
                    SendFor(value, "close");
                    e.Handled = true;
                };
                DockPanel.SetDock(close, Dock.Right);
                body.Children.Add(close);
            }
            var label = new DockPanel();
            var icon = new Grid { Width = 16, Height = 16, Margin = new Thickness(0, 0, 6,0) };
            FrameworkElement glyph = id is null && chatLogo.Length > 0 ? Logo(chatLogo, chatLogoColor) :
                id is null ? PaneIcon.Draw("Chat") : Text(tab, "kind") switch
                {
                    "files" => PaneIcon.Draw("Files"), "file" => PaneIcon.Draw("File"),
                    "diff" => GitMark(Text(tab, "mark")),
                    _ when Flag(tab, "loading") => new RunningSpinner(),
                    _ when icons.TryGetValue(id.Value, out var image) => new Image { Source = image },
                    _ => PaneIcon.Draw("Browser"),
                };
            icon.Children.Add(glyph);
            if (id is null && chatMarked)
                icon.Children.Add(new Border { Width = 9, Height = 9, CornerRadius = new CornerRadius(5),
                    Child = chatTone == "running" ? new RunningSpinner() : null,
                    HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom,
                    Background = Palette.Brush(chatTone == "needsYou" ? "YaacFailure" : chatTone == "running" ? "YaacPanel" : "YaacUnread") });
            if (Text(tab, "agentLogo") is { Length: > 0 } path)
            {
                var mark = Logo(path, Text(tab, "agentLogoColor"));
                icon.Children.Add(new Border { Child = mark, Width = 10, Height = 10,
                    Background = Palette.Brush("YaacPanel"), HorizontalAlignment = HorizontalAlignment.Right,
                    VerticalAlignment = VerticalAlignment.Bottom });
            }
            label.Children.Add(icon);
            label.Children.Add(new TextBlock { Text = title, TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = Palette.Brush(selected ? "YaacText" : "YaacMuted"), VerticalAlignment = VerticalAlignment.Center });
            var pick = QuietButton(label, id is null ? $"Chat · {title}" : title);
            pick.Tag = (id, false);
            pick.HorizontalContentAlignment = HorizontalAlignment.Stretch;
            if (Text(tab, "path") is { Length: > 0 } filePath) pick.ToolTip = filePath;
            if (Text(tab, "agentLogo").Length > 0)
            {
                pick.ToolTip = $"{title} · Agent is using this tab";
                AutomationProperties.SetItemStatus(pick, "Agent is using this tab");
            }
            pick.Click += (_, _) =>
            {
                if (id is ulong page) SendFor(page, "select");
                else Command?.Invoke("browser.chat", new { thread });
            };
            body.Children.Add(pick);
            TabStrip.Children.Add(row);
            if (selected)
            {
                selectedTab = row;
                var measure = new TextBlock { Text = title, FontFamily = FontFamily, FontSize = FontSize };
                measure.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
                row.Tag = Math.Min(200, Math.Ceiling(measure.DesiredSize.Width) + (id.HasValue ? 60 : 36));
            }
            void Update()
            {
                close.Visibility = selected || row.IsMouseOver || row.IsKeyboardFocusWithin
                    ? Visibility.Visible : Visibility.Collapsed;
                row.Background = selected ? Palette.Brush("YaacCanvas") : row.IsMouseOver ? Palette.Brush("YaacHover") : Brushes.Transparent;
            }
            row.MouseEnter += (_, _) => Update();
            row.MouseLeave += (_, _) => Update();
            row.IsKeyboardFocusWithinChanged += (_, _) => Update();
            row.SizeChanged += (_, _) => Update();
            Update();
            if (Equals(focused, pick.Tag)) pick.Focus();
            else if (id.HasValue && Equals(focused, close.Tag)) close.Focus();
        }
    }

    private void SizeTabs()
    {
        int count = TabStrip.Children.Count;
        if (count < 1) return;
        double room = Math.Max(0, TabAvailable.ActualWidth - 32);
        double width = Math.Clamp(room / count, 110, 200);
        double selectedWidth = Math.Max(width, selectedTab?.Tag is double needed ? needed : 0);
        if (count > 1) width = Math.Clamp((room - selectedWidth) / (count - 1), 110, 200);
        double total = selectedWidth + (count - 1) * width;
        TabGroup.Width = Math.Min(TabAvailable.ActualWidth, total + 32);
        foreach (var row in TabStrip.Children.OfType<Border>())
            row.Width = row == selectedTab ? selectedWidth : width;
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, () => selectedTab?.BringIntoView());
    }

    private static FrameworkElement Logo(string data, string color) => new System.Windows.Shapes.Path
    {
        Data = Geometry.Parse(data), Stretch = Stretch.Uniform,
        Fill = color.Length > 0 ? new SolidColorBrush((Color)ColorConverter.ConvertFromString(color)) : Palette.Brush("YaacText"),
    };

    private static FrameworkElement GitMark(string mark)
    {
        if (mark.Length == 0) mark = "M";
        var ink = Palette.Brush(mark switch { "A" or "U" => "YaacSuccess", "D" or "!" => "YaacFailure",
            "R" => "YaacUnread", _ => "YaacWarning" });
        return new Border { CornerRadius = new CornerRadius(3), BorderBrush = ink, BorderThickness = new Thickness(1),
            Child = new TextBlock { Text = mark, Foreground = ink, FontSize = 10, FontWeight = FontWeights.Bold,
                HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center } };
    }

}
