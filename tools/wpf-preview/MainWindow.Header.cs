using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private string? headerLinks;
    private string? headerBanners;
    private string headerKey = "";

    private static string MarkGlyph(string mark) => mark switch
    {
        "done" => "✓",
        "needsYou" => "!",
        "failed" => "×",
        "paused" => "Ⅱ",
        "waiting" => "⌛",
        _ => "",
    };

    private static Brush MarkColor(string tone) => Palette.Brush(tone switch
    {
        "needsYou" or "failed" => "YaacFailure",
        "paused" => "YaacWarning",
        "done" => "YaacSuccess",
        "running" => "YaacText",
        _ => "YaacMuted",
    });

    private void RenderHeader(JsonElement shell)
    {
        ThreadTitle.Text = Text(shell, "title");
        ThreadLocation.Text = Text(shell, "location");
        var mark = Text(shell, "mark");
        var tag = Text(shell, "tag");
        var glyph = MarkGlyph(mark);
        ThreadMark.Text = glyph.Length == 0 && Flag(shell, "readOnly") ? "▫" : glyph;
        ThreadMark.Foreground = MarkColor(mark);
        ThreadMarkSlot.Visibility = Flag(shell, "hasThread") ? Visibility.Visible : Visibility.Collapsed;
        ThreadSpinner.Visibility = mark == "running" ? Visibility.Visible : Visibility.Collapsed;
        ThreadSpinner.Foreground = Palette.Brush(Flag(shell, "fast") ? "YaacFast" : "YaacText");
        ThreadTagText.Text = tag;
        ThreadTagText.Foreground = MarkColor(Text(shell, "tagTone"));
        ThreadTag.Visibility = tag.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        HeaderBudget.Visibility = Flag(shell, "budget") ? Visibility.Visible : Visibility.Collapsed;
        headerKey = Text(shell, "threadId");

        if (shell.TryGetProperty("links", out var links) && headerLinks != links.GetRawText())
        {
            headerLinks = links.GetRawText();
            HeaderLinks.Children.Clear();
            foreach (var entry in links.EnumerateArray())
            {
                var id = Text(entry, "id");
                var label = Text(entry, "label");
                if (id.Length == 0)
                {
                    HeaderLinks.Children.Add(new TextBlock
                    {
                        Text = label, Margin = new Thickness(0, 0, 5, 0),
                        VerticalAlignment = VerticalAlignment.Center, Foreground = Palette.Brush("YaacMuted"),
                    });
                }
                else
                {
                    var link = new Button { Content = label, Padding = new Thickness(3, 1, 3, 1), MinHeight = 21 };
                    link.Click += (_, _) => Send("sidebar.open", new { id });
                    HeaderLinks.Children.Add(link);
                }
            }
        }
        if (shell.TryGetProperty("banners", out var banners) && headerBanners != banners.GetRawText())
        {
            headerBanners = banners.GetRawText();
            HeaderBanners.Children.Clear();
            foreach (var entry in banners.EnumerateArray()) AddHeaderBanner(entry);
        }
    }

    private void AddHeaderBanner(JsonElement entry)
    {
        var row = new DockPanel { Margin = new Thickness(12, 7, 12, 7) };
        var buttons = new StackPanel { Orientation = Orientation.Horizontal };
        DockPanel.SetDock(buttons, Dock.Right);
        if (entry.TryGetProperty("actions", out var actions))
        {
            foreach (var action in actions.EnumerateArray())
            {
                var id = Text(action, "id");
                var button = new Button
                {
                    Content = Text(action, "label"), ToolTip = Text(action, "detail"), IsEnabled = Flag(action, "enabled"),
                    Margin = new Thickness(5, 0, 0, 0),
                };
                if (Flag(action, "primary")) button.Style = (Style)FindResource("YaacPrimaryButton");
                button.Click += (_, _) => Send("header.action", new { id, threadId = headerKey });
                buttons.Children.Add(button);
            }
        }
        row.Children.Add(buttons);
        var text = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
        text.Children.Add(new TextBlock { Text = Text(entry, "label"), TextWrapping = TextWrapping.Wrap });
        if (Text(entry, "detail") is { Length: > 0 } detail)
            text.Children.Add(new TextBlock { Text = detail, TextWrapping = TextWrapping.Wrap, Foreground = Palette.Brush("YaacMuted") });
        row.Children.Add(text);
        HeaderBanners.Children.Add(new Border
        {
            CornerRadius = new CornerRadius(6), Background = Palette.Brush("YaacInput"),
            Margin = new Thickness(0, 5, 0, 0), Child = row,
        });
    }

    private void ContinueClicked(object sender, RoutedEventArgs args) =>
        Send("header.action", new { id = "continue", threadId = headerKey });
}
