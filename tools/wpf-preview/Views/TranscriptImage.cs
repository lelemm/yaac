using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal sealed class TranscriptImage : StackPanel
{
    private readonly Button open;
    private readonly Image thumbnail = new() { MaxWidth = 420, MaxHeight = 240, Stretch = Stretch.Uniform };
    private readonly TextBlock caption = new() { TextWrapping = TextWrapping.Wrap };
    private readonly TextBlock status = new() { FontSize = 12, TextWrapping = TextWrapping.Wrap };
    private readonly Action<JsonElement> submit;
    private JsonElement action;
    private JsonElement request;
    private string? requested;
    private string? displayedPng;
    private bool waitingForLayout;

    internal TranscriptImage(Action<JsonElement> submit)
    {
        this.submit = submit;
        var picture = new StackPanel();
        picture.Children.Add(thumbnail);
        picture.Children.Add(caption);
        open = new Button
        {
            Content = picture, HorizontalAlignment = HorizontalAlignment.Left,
            HorizontalContentAlignment = HorizontalAlignment.Left, Padding = new Thickness(6, 4, 6, 4),
        };
        open.Click += (_, _) => submit(action);
        status.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
        status.Margin = new Thickness(6, 2, 0, 0);
        Children.Add(open);
        Children.Add(status);
        Loaded += (_, _) => WatchVisibility();
        Unloaded += (_, _) => StopWatching();
    }

    internal void Render(JsonElement part)
    {
        action = part.Value("action");
        request = part.Value("request");
        var label = part.Value("label").OptionalText() ?? "Image";
        var preview = part.Value("preview");
        var png = preview.Value("png").OptionalText();
        var error = preview.Value("error").OptionalText();
        if (png != displayedPng)
        {
            displayedPng = png;
            thumbnail.Source = png is { Length: <= 524288 } ? ImageViewer.TryDecodePng(png) : null;
        }
        thumbnail.Visibility = thumbnail.Source is null ? Visibility.Collapsed : Visibility.Visible;
        caption.Text = thumbnail.Source is null ? $"▧  {label}" : label;
        open.IsEnabled = action.Value("enabled").Flag();
        var openLabel = action.Value("label").OptionalText() ?? $"Open {label}";
        open.ToolTip = openLabel;
        AutomationProperties.SetName(open, openLabel);
        status.Text = error ?? (png is not null && thumbnail.Source is null ? "Could not show the thumbnail." : "");
        status.Visibility = status.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        if (png is not null || error is not null)
        {
            request = default;
            requested = null;
        }
        WatchVisibility();
    }

    private void WatchVisibility()
    {
        StopWatching();
        if (!IsLoaded || !request.Value("enabled").Flag() || request.Value("id").OptionalText() is not { } id
            || requested == id)
            return;
        waitingForLayout = true;
        LayoutUpdated += RequestWhenVisible;
        if (IsArrangeValid)
            RequestWhenVisible(this, EventArgs.Empty);
    }

    private void RequestWhenVisible(object? sender, EventArgs e)
    {
        if (!IsVisible || RenderSize.Width <= 0 || RenderSize.Height <= 0
            || TranscriptView.FindAncestor<ScrollContentPresenter>(this) is not { } viewport)
            return;
        var bounds = TransformToAncestor(viewport).TransformBounds(new Rect(RenderSize));
        if (!bounds.IntersectsWith(new Rect(viewport.RenderSize)))
            return;

        StopWatching();
        requested = request.Value("id").OptionalText();
        submit(request);
    }

    private void StopWatching()
    {
        if (!waitingForLayout) return;
        LayoutUpdated -= RequestWhenVisible;
        waitingForLayout = false;
    }
}
