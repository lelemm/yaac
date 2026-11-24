using System.IO;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media.Imaging;

namespace Yaac.WpfPreview.Views;

internal readonly record struct ImageViewerFrame(ulong Id, string Name, string? Png, string? Error)
{
    public static ImageViewerFrame? Read(JsonElement data)
    {
        var id = data.Value("id");
        return id.ValueKind == JsonValueKind.Number && id.TryGetUInt64(out var serial)
            ? new(serial, data.Value("name").OptionalText() ?? "Image",
                data.Value("png").OptionalText(), data.Value("error").OptionalText())
            : null;
    }
}

public partial class ImageViewer : UserControl
{
    private ulong? latestId;
    private int revision;
    private bool hasPayload;
    private BitmapSource? bitmap;
    private IInputElement? previousFocus;
    private double zoom = 1;
    private Vector pan;
    private Point? lastDrag;

    internal event Action? CloseRequested;

    public ImageViewer()
    {
        InitializeComponent();
        Unloaded += (_, _) => Clear();
    }

    internal async void Render(ImageViewerFrame frame)
    {
        if (latestId is { } latest && (frame.Id < latest || frame.Id == latest && Visibility != Visibility.Visible))
            return;
        if (latestId != frame.Id)
        {
            Clear();
            latestId = frame.Id;
            ImageName.Text = frame.Name;
            ImageName.ToolTip = frame.Name;
            AutomationProperties.SetName(Picture, frame.Name);
            ShowStatus("Loading image…");
            if (Visibility != Visibility.Visible)
            {
                previousFocus = Keyboard.FocusedElement;
                Visibility = Visibility.Visible;
                if (!CloseButton.Focus())
                {
                    _ = Dispatcher.InvokeAsync(() =>
                    {
                        if (Visibility == Visibility.Visible)
                            CloseButton.Focus();
                    }, System.Windows.Threading.DispatcherPriority.Input);
                }
            }
        }
        if (frame.Error is { } error)
        {
            Clear();
            hasPayload = true;
            ShowStatus(error);
            return;
        }
        if (frame.Png is not { } png || hasPayload)
            return;

        hasPayload = true;
        var request = ++revision;
        var decoded = await Task.Run(() => TryDecodePng(png));
        if (request != revision || frame.Id != latestId || Visibility != Visibility.Visible)
            return;
        if (decoded is null)
        {
            ShowStatus("Could not open this image.");
            return;
        }

        bitmap = decoded;
        Picture.Source = bitmap;
        ShowStatus("");
        PositionImage();
    }

    internal static BitmapSource? TryDecodePng(string png)
    {
        try
        {
            using var stream = new MemoryStream(Convert.FromBase64String(png), writable: false);
            var decoder = new PngBitmapDecoder(stream, BitmapCreateOptions.PreservePixelFormat, BitmapCacheOption.OnLoad);
            var image = decoder.Frames[0];
            image.Freeze();
            return image;
        }
        catch (Exception exception) when (exception is FormatException or IOException or
            NotSupportedException or ArgumentException or System.Runtime.InteropServices.COMException)
        {
            return null;
        }
    }

    internal void CloseViewer()
    {
        if (Visibility != Visibility.Visible) return;
        Clear();
        Visibility = Visibility.Collapsed;
        if (previousFocus is UIElement { IsVisible: true, IsEnabled: true } prior)
            prior.Focus();
        previousFocus = null;
        CloseRequested?.Invoke();
    }

    private void Clear()
    {
        ++revision;
        hasPayload = false;
        bitmap = null;
        Picture.Source = null;
        zoom = 1;
        pan = default;
        Viewport.ReleaseMouseCapture();
    }

    private void ShowStatus(string text)
    {
        ViewerStatus.Text = text;
        ViewerStatus.Visibility = text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        ZoomOut.IsEnabled = ZoomIn.IsEnabled = Fit.IsEnabled = bitmap is not null;
        Viewport.Cursor = bitmap is null ? Cursors.Arrow : Cursors.Hand;
    }

    private void PositionImage()
    {
        if (bitmap is null || Viewport.ActualWidth <= 0 || Viewport.ActualHeight <= 0)
            return;
        var fit = Math.Min(1, Math.Min(Viewport.ActualWidth / bitmap.PixelWidth,
            Viewport.ActualHeight / bitmap.PixelHeight));
        Picture.Width = bitmap.PixelWidth * fit * zoom;
        Picture.Height = bitmap.PixelHeight * fit * zoom;
        var dx = Math.Max(0, (Picture.Width - Viewport.ActualWidth) / 2);
        var dy = Math.Max(0, (Picture.Height - Viewport.ActualHeight) / 2);
        pan = new Vector(Math.Clamp(pan.X, -dx, dx), Math.Clamp(pan.Y, -dy, dy));
        Canvas.SetLeft(Picture, (Viewport.ActualWidth - Picture.Width) / 2 + pan.X);
        Canvas.SetTop(Picture, (Viewport.ActualHeight - Picture.Height) / 2 + pan.Y);
    }

    private void Scale(double factor, Point anchor)
    {
        if (bitmap is null) return;
        var center = new Point(Viewport.ActualWidth / 2, Viewport.ActualHeight / 2);
        var next = Math.Clamp(zoom * factor, 0.25, 16);
        var offset = anchor - center;
        pan = offset - (offset - pan) * (next / zoom);
        zoom = next;
        PositionImage();
    }

    private Point Center() => new(Viewport.ActualWidth / 2, Viewport.ActualHeight / 2);
    private void ViewportResized(object sender, SizeChangedEventArgs e) => PositionImage();
    private void ZoomOutClicked(object sender, RoutedEventArgs e) => Scale(0.8, Center());
    private void ZoomInClicked(object sender, RoutedEventArgs e) => Scale(1.25, Center());
    private void FitClicked(object sender, RoutedEventArgs e)
    {
        zoom = 1;
        pan = default;
        PositionImage();
    }
    private void CloseClicked(object sender, RoutedEventArgs e) => CloseViewer();

    private void ViewportWheel(object sender, MouseWheelEventArgs e)
    {
        Scale(Math.Pow(1.25, e.Delta / 120.0), e.GetPosition(Viewport));
        e.Handled = true;
    }

    private void PanStarted(object sender, MouseButtonEventArgs e)
    {
        if (bitmap is null || !Viewport.CaptureMouse()) return;
        lastDrag = e.GetPosition(Viewport);
        Viewport.Cursor = Cursors.SizeAll;
        e.Handled = true;
    }

    private void PanMoved(object sender, MouseEventArgs e)
    {
        if (lastDrag is not { } last) return;
        var current = e.GetPosition(Viewport);
        pan += current - last;
        lastDrag = current;
        PositionImage();
        e.Handled = true;
    }

    private void PanEnded(object sender, MouseButtonEventArgs e)
    {
        if (lastDrag is null) return;
        Viewport.ReleaseMouseCapture();
        e.Handled = true;
    }

    private void PanLost(object sender, MouseEventArgs e)
    {
        lastDrag = null;
        Viewport.Cursor = bitmap is null ? Cursors.Arrow : Cursors.Hand;
    }
}
