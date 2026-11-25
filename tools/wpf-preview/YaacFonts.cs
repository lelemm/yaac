using System.Text.Json;
using System.Windows;
using System.Windows.Media;

namespace Yaac.WpfPreview;

public static class YaacFonts
{
    private static readonly Uri AppResources = new("pack://application:,,,/Yaac.WpfPreview;component/");
    private static readonly FontFamily DefaultUi = new(AppResources, "./Assets/Fonts/#UbuntuSans Nerd Font Propo, Segoe UI");
    private static readonly FontFamily DefaultCode = new(AppResources, "./Assets/Fonts/#CaskaydiaCove Nerd Font Mono, Consolas, Segoe UI Emoji, Segoe UI Symbol");
    internal const string UiResource = "YaacUiFont";
    internal const string CodeResource = "YaacCodeFont";
    internal const string TerminalResource = "YaacTerminalFont";
    private static string? applied;

    public static FontFamily Ui => Application.Current?.Resources[UiResource] as FontFamily ?? DefaultUi;
    public static FontFamily Mono => Application.Current?.Resources[CodeResource] as FontFamily ?? DefaultCode;

    internal static void Initialize()
    {
        Application.Current.Resources[UiResource] = DefaultUi;
        Application.Current.Resources[CodeResource] = DefaultCode;
        Application.Current.Resources[TerminalResource] = DefaultCode;
        applied = null;
    }

    internal static void Apply(JsonElement fonts)
    {
        if (fonts.ValueKind != JsonValueKind.Array || fonts.GetArrayLength() != 2) return;
        var signature = fonts.GetRawText();
        if (signature == applied) return;
        applied = signature;
        Application.Current.Resources[UiResource] = Selected(fonts[0], DefaultUi, fallback: true);
        Application.Current.Resources[CodeResource] = Selected(fonts[1], DefaultCode, fallback: true);
        Application.Current.Resources[TerminalResource] = Selected(fonts[1], DefaultCode, fallback: false);
    }

    internal static FontFamily Preview(JsonElement selection, bool code) =>
        Selected(selection, code ? DefaultCode : DefaultUi, fallback: true);

    private static FontFamily Selected(JsonElement selection, FontFamily defaults, bool fallback)
    {
        if (selection.ValueKind != JsonValueKind.Object || !selection.TryGetProperty("name", out var value) || value.GetString() is not { Length: > 0 } name)
            return defaults;
        // Imported fonts live alone in a directory under the YAAC data folder.
        if (selection.TryGetProperty("folder", out var folder) && folder.ValueKind == JsonValueKind.String
            && Uri.TryCreate(folder.GetString()?.TrimEnd('\\', '/') + "/", UriKind.Absolute, out var uri) && uri.IsFile)
            name = uri.AbsoluteUri + "#" + name;
        return new FontFamily(AppResources, fallback ? name + ", " + defaults.Source : name);
    }
}
