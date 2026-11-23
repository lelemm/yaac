using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Media;
using System.Windows.Shapes;
using System.Windows.Threading;

namespace Yaac.WpfPreview;

public sealed class RunningSpinner : ContentControl
{
    private static readonly Geometry Arc = Geometry.Parse("M 15.5,5.5 A 7,7 0 1 1 7,2.3");
    private static readonly HashSet<RunningSpinner> Active = new();
    private static readonly DispatcherTimer Ticker = new(DispatcherPriority.Render)
    {
        Interval = TimeSpan.FromSeconds(1.0 / 30),
    };
    private readonly RotateTransform rotation = new();
    private Window? host;
    private long lastTick;

    static RunningSpinner() => Ticker.Tick += (_, _) =>
    {
        var now = Stopwatch.GetTimestamp();
        foreach (var spinner in Active)
            spinner.Advance(now);
    };

    public static readonly DependencyProperty FastProperty = DependencyProperty.Register(
        nameof(Fast), typeof(bool), typeof(RunningSpinner), new PropertyMetadata(false));
    public static readonly DependencyProperty UltrafastProperty = DependencyProperty.Register(
        nameof(Ultrafast), typeof(bool), typeof(RunningSpinner), new PropertyMetadata(false));

    public bool Fast { get => (bool)GetValue(FastProperty); set => SetValue(FastProperty, value); }
    public bool Ultrafast { get => (bool)GetValue(UltrafastProperty); set => SetValue(UltrafastProperty, value); }

    internal static Brush Ink(bool fast, bool ultrafast) =>
        Palette.Brush(ultrafast ? "YaacUltrafast" : fast ? "YaacFast" : "YaacText");

    public RunningSpinner()
    {
        Focusable = false;
        IsHitTestVisible = false;
        HorizontalContentAlignment = HorizontalAlignment.Stretch;
        VerticalContentAlignment = VerticalAlignment.Stretch;
        var path = new Path
        {
            Data = Arc,
            Stretch = Stretch.Uniform,
            StrokeThickness = 1.8,
            StrokeStartLineCap = PenLineCap.Round,
            StrokeEndLineCap = PenLineCap.Round,
            RenderTransform = rotation,
            RenderTransformOrigin = new Point(.5, .5),
            Margin = new Thickness(1),
        };
        path.SetBinding(Shape.StrokeProperty, new Binding(nameof(Foreground)) { Source = this });
        Content = path;
        Loaded += (_, _) => WatchWindow(Window.GetWindow(this) ?? Application.Current?.MainWindow);
        Unloaded += (_, _) => WatchWindow(null);
        IsVisibleChanged += (_, _) => UpdateTicking();
    }

    private void WatchWindow(Window? window)
    {
        if (host is not null)
        {
            host.Activated -= HostChanged;
            host.Deactivated -= HostChanged;
            host.StateChanged -= HostChanged;
        }
        host = window;
        if (host is not null)
        {
            host.Activated += HostChanged;
            host.Deactivated += HostChanged;
            host.StateChanged += HostChanged;
        }
        UpdateTicking();
    }

    private void HostChanged(object? sender, EventArgs e) => UpdateTicking();

    private void UpdateTicking()
    {
        // Windows can disable client-area animations in remote sessions. This live activity
        // indicator follows the same focused-window policy as egui in software rendering.
        if (IsLoaded && IsVisible && host is { IsActive: true, WindowState: not WindowState.Minimized })
        {
            if (Active.Add(this))
            {
                lastTick = Stopwatch.GetTimestamp();
                if (Active.Count == 1) Ticker.Start();
            }
        }
        else if (Active.Remove(this) && Active.Count == 0)
        {
            Ticker.Stop();
        }
    }

    private void Advance(long now)
    {
        var period = Ultrafast ? .35 : Fast ? 1.8 : 2.5;
        var elapsed = Stopwatch.GetElapsedTime(lastTick, now).TotalSeconds;
        rotation.Angle = (rotation.Angle + elapsed * 360 / period) % 360;
        lastTick = now;
    }
}
