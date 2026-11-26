using System.ComponentModel;
using System.Text.Json;
using System.Windows;
using System.Windows.Data;
using System.Windows.Media;

namespace Yaac.WpfPreview;

internal static class Palette
{
    private static readonly (string Name, uint Dark, uint Light)[] Colors =
    [
        ("YaacCanvas",      0x16181D, 0xFFFFFF),
        ("YaacPanel",       0x1B1E24, 0xF5F6F8),
        ("YaacCard",        0x22262E, 0xF7F8FA),
        ("YaacRaised",      0x22262E, 0xF7F8FA),
        ("YaacInput",       0x1E2128, 0xF3F4F7),
        ("YaacRule",        0x2E333D, 0xDADDE3),
        ("YaacText",        0xD8DADE, 0x1A1D23),
        ("YaacMuted",       0x9AA2AF, 0x5B6472),
        ("YaacAccent",      0xE6E8EC, 0x1A1D23),
        ("YaacOnAccent",    0x16181D, 0xFFFFFF),
        ("YaacLink",        0xE6E8EC, 0x1A1D23),
        ("YaacSelected",    0x2A2E37, 0xE6E8EC),
        ("YaacHover",       0x1E2128, 0xF3F4F7),
        ("YaacUnread",      0x6495ED, 0x4A7FE0),
        ("YaacWarning",     0xF2B24C, 0x8F5600),
        ("YaacFailure",     0xFF7C87, 0xBF2636),
        ("YaacSuccess",     0x4CC38A, 0x17754D),
        ("YaacUser",        0x2D333E, 0xE3E7EE),
        ("YaacCodeKeyword", 0xC68CFF, 0x8A2BC2),
        ("YaacCodeString",  0x9ECE6A, 0x2E7D32),
        ("YaacCodeComment", 0x7E9C6E, 0x4E7A3A),
        ("YaacCodeNumber",  0xF09A5C, 0xB35000),
        ("YaacCodeType",    0x6CC4FF, 0x1A5FB4),
        ("YaacDiffAdded",   0x1B322C, 0xE7F5EB),
        ("YaacDiffRemoved", 0x39252D, 0xFBEAEC),
        ("YaacDiffAddWord", 0x285943, 0xB9E7C8),
        ("YaacDiffDelWord", 0x663947, 0xF6BEC5),
        ("YaacFast",        0xF5C518, 0x9A7000),
        ("YaacUltrafast",   0x5BA8FF, 0x1764BA),
        // `!command` mode and command blocks (P346); the same values as egui's Palette.shell.
        ("YaacShell",       0x9DBF7A, 0x4A6E2C),
    ];

    private static readonly Dictionary<string, (SolidColorBrush Brush, Swatch Swatch)> Brushes = new(StringComparer.Ordinal);
    private static string requested = "dark";

    internal static Brush Brush(string name) => Brushes[name].Brush;
    internal static bool FollowsSystem => requested == "system";

    private static JsonElement custom;

    internal static void SetTheme(string theme, JsonElement palette = default)
    {
        custom = theme == "custom" ? palette : default;
        requested = custom.ValueKind == JsonValueKind.Object ? (custom.GetProperty("dark").GetBoolean() ? "dark" : "light") : theme;
        var app = Application.Current;
#pragma warning disable WPF0001 // Preview uses WPF's Fluent control resources.
        var mode = requested switch { "light" => ThemeMode.Light, "system" => ThemeMode.System, _ => ThemeMode.Dark };
        if (app.ThemeMode != mode)
            app.ThemeMode = mode;
#pragma warning restore WPF0001
        Refresh();
    }

    internal static void Refresh()
    {
        var app = Application.Current;
        var light = requested == "light" || requested == "system"
            && app.TryFindResource("ApplicationBackgroundBrush") is SolidColorBrush background
            && background.Color.R + background.Color.G + background.Color.B > 381;
        foreach (var (name, darkValue, lightValue) in Colors)
        {
            var value = light ? lightValue : darkValue;
            if (custom.ValueKind == JsonValueKind.Object && custom.TryGetProperty(name, out var rgb))
                value = rgb.GetUInt32();
            var color = Color.FromRgb((byte)(value >> 16), (byte)(value >> 8), (byte)value);
            if (Brushes.TryGetValue(name, out var current))
            {
                current.Swatch.Set(color);
                continue;
            }
            var source = new Swatch(color);
            var brush = new SolidColorBrush();
            // A binding keeps brushes mutable even when a template shares them by reference.
            BindingOperations.SetBinding(brush, SolidColorBrush.ColorProperty, new Binding(nameof(Swatch.Color)) { Source = source });
            Brushes[name] = (brush, source);
            app.Resources[name] = brush;
            // Fluent inputs share this focus brush. Use the same outline as YAAC controls.
            if (name == "YaacAccent")
                app.Resources["TextControlBorderBrushFocused"] = brush;
        }
    }

    private sealed class Swatch(Color color) : INotifyPropertyChanged
    {
        public Color Color { get; private set; } = color;
        public event PropertyChangedEventHandler? PropertyChanged;

        public void Set(Color value)
        {
            if (Color == value) return;
            Color = value;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Color)));
        }
    }
}
