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
    private readonly Dictionary<ulong, long> terminalBells = new();

    private void RenderTabs(JsonElement tabs, string reason)
    {
        var signature = JsonSerializer.Serialize(new { thread, active, full, chat, chatTitle, chatLogo, chatLogoColor, chatMarked, chatTone,
            reason, filesReason, terminalReason, tabs = tabs.EnumerateArray().Select(t => new[] { Text(t, "title"), Text(t, "kind"),
                Text(t, "url"), Text(t, "agentLogo"), Text(t, "agentLogoColor"), Text(t, "path"), Text(t, "mark"), t.Value("bell").ToString(), Flag(t, "unread").ToString(),
                t.GetProperty("id").ToString(), Flag(t, "loading").ToString(), Flag(t, "artifact").ToString() }) });
        if (signature == tabSignature) return;
        tabSignature = signature;
        object? focused = TabStrip.IsKeyboardFocusWithin && Keyboard.FocusedElement is Button old ? old.Tag : null;
        var ids = tabs.EnumerateArray().Select(t => t.GetProperty("id").GetUInt64()).ToHashSet();
        if (rename is { } edit && (edit.Thread != thread || !ids.Contains(edit.Title.Tab)
            || edit.Title.Selected != (!chat && edit.Title.Tab == active))) EndRename(false, false);
        renameTabs.Clear();
        // Keep the live TextBox parented. Recreating it would interrupt IME composition
        // and reset its selection whenever an OSC title or an unread marker changed.
        if (rename == null) TabStrip.Children.Clear();
        else
            for (int i = TabStrip.Children.Count - 1; i >= 0; i--)
                if (TabStrip.Children[i] != rename.Title.Row) TabStrip.Children.RemoveAt(i);
        int position = 0;
        selectedTab = null;
        foreach (var id in terminalBells.Keys.Where(id => !ids.Contains(id)).ToArray()) terminalBells.Remove(id);
        foreach (var id in icons.Keys.Where(id => !ids.Contains(id)).ToArray()) icons.Remove(id);
        if (full) AddTab(null, chatTitle.Length == 0 ? "Chat" : chatTitle, default, chat);
        foreach (var tab in tabs.EnumerateArray())
        {
            var id = tab.GetProperty("id").GetUInt64();
            var title = Text(tab, "title");
            if (title.Length == 0) title = Text(tab, "url");
            AddTab(id, title.Length == 0 ? "New tab" : title, tab, !chat && active == id);
        }
        AddTabButton.IsEnabled = reason.Length == 0 || filesReason.Length == 0 || terminalReason.Length == 0;
        AddTabButton.ToolTip = AddTabButton.IsEnabled ? (IsTerminalPanel ? "New terminal" : "New tab") : reason;
        SizeTabs();

        void AddTab(ulong? id, string title, JsonElement tab, bool selected)
        {
            if (rename?.Title is { } existing && existing.Tab == id)
            {
                existing.Label.Text = title;
                existing.Pick.ToolTip = TerminalTitleTip(tab, title);
                renameTabs[existing.Tab] = existing;
                if (selected) selectedTab = existing.Row;
                position++;
                return;
            }
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
                    "terminal" => PaneIcon.Draw("Terminal"),
                    "files" => PaneIcon.Draw("Files"), "file" => PaneIcon.Draw("File"),
                    "diff" => GitMark(Text(tab, "mark")),
                    _ when Flag(tab, "loading") => new RunningSpinner(),
                    _ when Flag(tab, "artifact") => PaneIcon.Draw("File"),
                    _ when icons.TryGetValue(id.Value, out var image) => new Image { Source = image },
                    _ => PaneIcon.Draw("Browser"),
                };
            icon.Children.Add(glyph);
            if (id is ulong termId && Text(tab, "kind") == "terminal")
            {
                var counter = tab.Value("bell");
                long bells = counter.ValueKind == JsonValueKind.Number ? counter.GetInt64() : 0;
                terminalBells.TryGetValue(termId, out long previous);
                terminalBells[termId] = bells;
                if (bells > previous)
                {
                    var flash = new Border { Background = Palette.Brush("YaacWarning"), CornerRadius = new CornerRadius(4) };
                    icon.Children.Add(flash);
                    flash.BeginAnimation(OpacityProperty, new System.Windows.Media.Animation.DoubleAnimation(
                        .85, 0, TimeSpan.FromMilliseconds(280)));
                }
                if (!selected && Flag(tab, "unread"))
                    icon.Children.Add(new Border { Width = 6, Height = 6, CornerRadius = new CornerRadius(3),
                        HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Bottom,
                        Background = Palette.Brush("YaacUnread") });
            }
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
            var caption = new TextBlock { Text = title, TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = Palette.Brush(selected ? "YaacText" : "YaacMuted"), VerticalAlignment = VerticalAlignment.Center };
            label.Children.Add(caption);
            var pick = QuietButton(label, id is null ? $"Chat · {title}" : title);
            pick.Tag = (id, false);
            pick.HorizontalContentAlignment = HorizontalAlignment.Stretch;
            if (Text(tab, "path") is { Length: > 0 } filePath) pick.ToolTip = filePath;
            if (Text(tab, "kind") == "terminal")
                pick.ToolTip = TerminalTitleTip(tab, title);
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
            if (id is ulong terminalId && Text(tab, "kind") == "terminal")
            {
                var menu = new ContextMenu();
                var slot = new Grid();
                slot.Children.Add(pick);
                body.Children.Add(slot);
                var terminalTitle = new TerminalTitle(terminalId, selected, row, slot, pick, caption);
                renameTabs[terminalId] = terminalTitle;
                pick.PreviewMouseDoubleClick += (_, e) =>
                {
                    if (e.ChangedButton != MouseButton.Left) return;
                    e.Handled = true;
                    BeginRename(terminalTitle);
                };
                var renameItem = new MenuItem { Header = "Rename", InputGestureText = "F2" };
                renameItem.Click += (_, _) => Dispatcher.BeginInvoke(DispatcherPriority.Input, () =>
                {
                    if (renameTabs.TryGetValue(terminalId, out var current)) BeginRename(current);
                });
                menu.Items.Add(renameItem);
                row.ContextMenu = menu;
            }
            else body.Children.Add(pick);
            if (IsTerminalPanel && terminalShells.ValueKind == JsonValueKind.Array && row.ContextMenu is { } panelMenu)
            {
                var choices = new MenuItem { Header = "New terminal with shell…" };
                foreach (var shell in terminalShells.EnumerateArray())
                {
                    var shellPath = Text(shell, "path");
                    var choice = new MenuItem { Header = Text(shell, "name") };
                    choice.Click += (_, _) => Command?.Invoke("terminal.panelNew", new { thread, shell = shellPath });
                    choices.Items.Add(choice);
                }
                choices.IsEnabled = choices.Items.Count > 0;
                panelMenu.Items.Add(choices);
            }
            TabStrip.Children.Insert(position++, row);
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
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, () => (rename?.Title.Row ?? selectedTab)?.BringIntoView());
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
