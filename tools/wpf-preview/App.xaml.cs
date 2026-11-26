using System.IO;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;

namespace Yaac.WpfPreview;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs args)
    {
        RenderOptions.ProcessRenderMode = RenderMode.SoftwareOnly;
        base.OnStartup(args);
        YaacFonts.Initialize();

        string? Option(string name)
        {
            var index = Array.IndexOf(args.Args, name);
            return index >= 0 && index + 1 < args.Args.Length && !args.Args[index + 1].StartsWith("--")
                ? args.Args[index + 1] : null;
        }

        Palette.SetTheme(Option("--theme") ?? "dark");
        SystemParameters.StaticPropertyChanged += SystemThemeChanged;
        var piped = args.Args.Contains("--pipe");
        var scenario = Option("--demo");
        Func<Controller> connect = piped ? Controller.FromPipe : () => new Controller(
            Option("--controller") ?? Path.Combine(AppContext.BaseDirectory, "yaac.exe"),
            scenario == "fresh" ? "fresh" : "fleet", Option("--theme") ?? "dark");
        var target = Option("--capture-ui");
        var display = new MainWindow(connect, demo: !piped || scenario is not null,
            tray: target is null && args.Args.Contains("--tray"));
        MainWindow = display;
        if (target is null)
            display.Show();
        else
            _ = display.CaptureDemoAsync(connect, target);
    }
    protected override void OnExit(ExitEventArgs args)
    {
        SystemParameters.StaticPropertyChanged -= SystemThemeChanged;
        base.OnExit(args);
    }

    private void SystemThemeChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs args)
    {
        Dispatcher.BeginInvoke(() =>
        {
            if (Palette.FollowsSystem) Palette.Refresh();
            (MainWindow as MainWindow)?.ReportSystemTheme();
        });
    }
}
