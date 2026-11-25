using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;

namespace Yaac.WpfPreview.Views;

internal static class PaneIcon
{
    private static readonly Dictionary<string, Geometry> Shapes = new()
    {
        ["Browser"] = Shape("M8 1 A7 7 0 1 1 7.99 1 M1 8H15 M8 1 C4 4 4 12 8 15 C12 12 12 4 8 1"),
        ["Files"] = Shape("M1 4 Q1 3 2 3 H6 L8 5 H14 Q15 5 15 6 V13 H1 Z"),
        ["File"] = Shape("M3 1 H9 L13 5 V15 H3 Z M9 1 V5 H13"),
        ["Back"] = Shape("M8 2 L2 8 L8 14 M2 8 H15"),
        ["Forward"] = Shape("M8 2 L14 8 L8 14 M14 8 H1"),
        ["Reload"] = Shape("M14 6 A6 6 0 1 0 14 10 M14 1 V6 H9"),
        ["Stop"] = Shape("M3 3 H13 V13 H3 Z"),
        ["Open in browser"] = Shape("M7 3 H2 V14 H13 V9 M8 8 L15 1 M9 1 H15 V7"),
        ["Inspect"] = Shape("M5 3 L1 8 L5 13 M11 3 L15 8 L11 13"),
        ["Full screen"] = Shape("M2 2 H14 V14 H2 Z"),
        ["Restore"] = Shape("M2 5 H11 V14 H2 Z M5 5 V2 H14 V11 H11"),
        ["New tab"] = Shape("M8 2 V14 M2 8 H14"),
        ["Close"] = Shape("M3 3 L13 13 M13 3 L3 13"),
        ["Show ignored"] = Shape("M1 8 Q8 -2 15 8 Q8 18 1 8 Z M10 8 A2 2 0 1 1 6 8 A2 2 0 1 1 10 8"),
        ["Collapse all"] = Shape("M3 8 L8 3 L13 8 M3 13 L8 8 L13 13"),
        ["Previous change"] = Shape("M8 14 V2 M3 7 L8 2 L13 7"),
        ["Next change"] = Shape("M8 2 V14 M3 9 L8 14 L13 9"),
        ["Chevron"] = Shape("M5 2 L11 8 L5 14"),
        ["Chat"] = Shape("M2 2 H14 V11 H7 L3 15 V11 H2 Z"),
    };

    private static Geometry Shape(string data)
    {
        var geometry = Geometry.Parse(data);
        geometry.Freeze();
        return geometry;
    }

    internal static FrameworkElement Draw(string name, double size = 16)
    {
        var path = new Path { Data = Shapes[name], StrokeThickness = 1.45,
            StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round,
            StrokeLineJoin = PenLineJoin.Round };
        path.SetResourceReference(Path.StrokeProperty, "YaacMuted");
        if (name == "Stop") path.SetResourceReference(Path.FillProperty, "YaacMuted");
        var canvas = new Canvas { Width = 16, Height = 16 };
        canvas.Children.Add(path);
        return new Viewbox { Width = size, Height = size, Child = canvas };
    }

    internal static void Set(Button button, string name, string? tip = null)
    {
        button.Content = Draw(name);
        button.Width = button.Height = 28;
        button.Padding = new Thickness(0);
        button.ToolTip = tip ?? name;
        button.SetResourceReference(FrameworkElement.StyleProperty, "YaacQuietButton");
        AutomationProperties.SetName(button, name);
    }
}
