using System.Windows.Media;

namespace Yaac.WpfPreview;

public static class YaacFonts
{
    private static readonly Uri AppResources = new("pack://application:,,,/");
    private const string NerdSymbols = "./Assets/#Symbols Nerd Font Mono";

    public static readonly FontFamily Ui = WithSymbols("Segoe UI");
    public static readonly FontFamily Mono = WithSymbols("Cascadia Mono, Consolas");
    public static readonly FontFamily Symbol = WithSymbols("Segoe UI Symbol");

    private static FontFamily WithSymbols(string family) => new(AppResources, $"{family}, {NerdSymbols}");
}
