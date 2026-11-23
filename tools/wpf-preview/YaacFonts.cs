using System.Windows.Media;

namespace Yaac.WpfPreview;

public static class YaacFonts
{
    private static readonly Uri AppResources = new("pack://application:,,,/");
    public static readonly FontFamily Ui = new(AppResources, "./Assets/Fonts/#CaskaydiaCove Nerd Font Propo, Segoe UI");
    public static readonly FontFamily Mono = new(AppResources, "./Assets/Fonts/#CaskaydiaCove Nerd Font Mono, Consolas");
}
