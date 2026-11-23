using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;

namespace Yaac.WpfPreview.Views;

public sealed class UsageMeter : WrapPanel
{
    private JsonElement previous;
    public bool Compact { get; set; }

    public void Render(JsonElement data)
    {
        if (TranscriptJson.Same(previous, data)) return;
        previous = data.Snapshot();
        Children.Clear();
        foreach (var account in data.Elements())
            Children.Add(Account(account));
        Visibility = Children.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private FrameworkElement Account(JsonElement account)
    {
        var tooltip = account.Value("tooltip").Text();
        var bars = account.Value("bars").Elements().ToArray();
        if (Compact && account.Value("tightest").TryGetInt32(out var tightest) && tightest < bars.Length)
            bars = [bars[tightest]];

        var meter = new StackPanel
        {
            Orientation = Compact ? Orientation.Vertical : Orientation.Horizontal,
            Margin = Compact ? new Thickness(4, 4, 4, 3) : new Thickness(2, 3, 6, 3),
            Opacity = account.Value("stale").Flag() ? .55 : 1,
            ToolTip = tooltip,
        };
        var label = account.Value("label").Text();
        if (AgentsRow.ReadLogo(account.Value("logo").Text()) is { } geometry)
        {
            meter.Children.Add(new Path
            {
                Data = geometry, Fill = BrushFrom(account.Value("logoColor").Text()), Stretch = Stretch.Uniform,
                Width = 14, Height = 14, VerticalAlignment = VerticalAlignment.Center,
                Margin = Compact ? new Thickness(0, 0, 0, 3) : new Thickness(0, 0, 4, 0),
            });
        }
        else
            meter.Children.Add(new TextBlock { Text = Compact ? label[..Math.Min(1, label.Length)] : label,
                Foreground = TranscriptColors.Muted, FontSize = 10, Margin = new Thickness(0, 0, 4, 0) });

        var windows = new StackPanel();
        foreach (var bar in bars)
        {
            var brand = BrushFrom(account.Value("color").Text());
            var textColor = bar.Value("near").Flag() ? TranscriptColors.Error : brand;
            var row = new StackPanel { Orientation = Orientation.Horizontal };
            foreach (var segment in bar.Value("segments").Elements())
            {
                var fill = segment.GetDouble();
                row.Children.Add(new Border
                {
                    Width = Compact ? 4 : 5, Height = Compact ? 3 : 4,
                    CornerRadius = new CornerRadius(1), Margin = new Thickness(0, 0, 1.5, 0),
                    VerticalAlignment = VerticalAlignment.Center, Background = fill == 0
                        ? TranscriptColors.Muted : Compact ? textColor : brand,
                    Opacity = fill == 0 ? .22 : fill,
                });
            }
            if (!Compact)
                row.Children.Add(new TextBlock { Text = bar.Value("text").Text(), Foreground = textColor,
                    FontSize = 12, Margin = new Thickness(3, 0, 0, 0) });
            windows.Children.Add(row);
        }
        meter.Children.Add(windows);
        AutomationProperties.SetName(meter, $"{label} usage · {string.Join(" · ", bars.Select(bar => bar.Value("text").Text()))}");
        return meter;
    }

    private static Brush BrushFrom(string color) => color.Length == 7
        ? (Brush)new BrushConverter().ConvertFromString(color)!
        : TranscriptColors.Muted;
}
