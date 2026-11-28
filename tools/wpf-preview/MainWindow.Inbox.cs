using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using Yaac.WpfPreview.Views;

namespace Yaac.WpfPreview;

/// External dialogs without a thread (P418, docs/16-external-dialogs.md): their rows above the
/// sidebar's threads, and the one opened from them (or from a notification) as a modal card.
/// The card's controls are built at runtime from the layout in the frame
/// (<see cref="DialogLayoutView"/>). Closing the card dismisses the dialog (the integrator is told
/// it was cancelled), as Esc does; **Later** only hides it — it stays in Needs you.
public partial class MainWindow
{
    private Window? inboxWindow;
    private string? inboxKey;
    private DialogLayoutView? inboxLayout;
    private TextBlock? inboxSource;
    private TextBlock? inboxGone;
    private TextBlock? inboxStatus;
    private WrapPanel? inboxActions;
    private readonly Dictionary<string, object?> inboxValues = new();
    private string inboxRowsSignature = "";
    private bool inboxClosing;
    private bool inboxPending;

    private void RenderInbox(JsonElement rows)
    {
        var signature = rows.ValueKind == JsonValueKind.Array ? rows.GetRawText() : "";
        if (signature == inboxRowsSignature)
            return;
        inboxRowsSignature = signature;
        var panel = SidebarPane.InboxRows;
        panel.Children.Clear();
        foreach (var row in rows.Elements())
        {
            var target = Text(row, "target");
            var requestId = Text(row, "requestId");
            var lines = new StackPanel();
            var title = new TextBlock { Text = "!  " + Text(row, "title"), TextTrimming = TextTrimming.CharacterEllipsis };
            title.SetResourceReference(TextBlock.ForegroundProperty, "YaacText");
            var detail = new TextBlock { Text = $"{Text(row, "source")} · {Text(row, "targetName")}", FontSize = 12, TextTrimming = TextTrimming.CharacterEllipsis, Margin = new Thickness(17, 0, 0, 0) };
            detail.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
            lines.Children.Add(title);
            lines.Children.Add(detail);
            var button = new Button
            {
                Content = lines,
                Style = (Style)FindResource("YaacQuietButton"),
                HorizontalContentAlignment = HorizontalAlignment.Left,
                Padding = new Thickness(6, 4, 6, 4),
                ToolTip = Text(row, "detail"),
            };
            System.Windows.Automation.AutomationProperties.SetName(button, $"{Text(row, "title")} · {Text(row, "detail")}");
            button.Click += (_, _) => Send("inbox.open", new { target, requestId });
            panel.Children.Add(button);
        }
        panel.Visibility = panel.Children.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void RenderInboxDialog(JsonElement data)
    {
        var key = data.ValueKind == JsonValueKind.Object ? $"{Text(data, "target")}\n{Text(data, "requestId")}" : null;
        if (key != inboxKey)
        {
            var previous = inboxWindow;
            inboxWindow = null;
            inboxLayout = null;
            inboxValues.Clear();
            if (previous is not null)
            {
                inboxClosing = true;
                previous.Close();
                inboxClosing = false;
            }
            inboxKey = key;
        }
        if (key is null)
            return;
        var target = Text(data, "target");
        var requestId = Text(data, "requestId");
        if (inboxWindow is null)
        {
            inboxWindow = Overlay(Text(data, "title"), Math.Max(360, ActualHeight - 50));
            var body = new StackPanel();
            var heading = new TextBlock { Text = Text(data, "title"), FontSize = 17, FontWeight = FontWeights.SemiBold, TextWrapping = TextWrapping.Wrap };
            inboxSource = new TextBlock { TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 2, 0, 0) };
            inboxSource.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
            inboxGone = new TextBlock { TextWrapping = TextWrapping.Wrap };
            inboxGone.SetResourceReference(TextBlock.ForegroundProperty, "YaacWarning");
            inboxStatus = new TextBlock { TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 8, 0, 0) };
            inboxActions = new WrapPanel { Margin = new Thickness(0, 10, 0, 0) };
            inboxLayout = new DialogLayoutView(inboxValues);
            inboxLayout.Pressed += button => Send("inbox.action", new { target, requestId, action = $"dialog:{button}", values = inboxLayout.Values });
            body.Children.Add(heading);
            body.Children.Add(inboxSource);
            body.Children.Add(inboxGone);
            body.Children.Add(inboxLayout);
            body.Children.Add(inboxStatus);
            body.Children.Add(inboxActions);
            var surface = new Border { Padding = new Thickness(16), Child = new ScrollViewer { Content = body, VerticalScrollBarVisibility = ScrollBarVisibility.Auto } };
            surface.SetResourceReference(LayoutTransformProperty, "YaacUiScale");
            inboxWindow.Content = surface;
            var window = inboxWindow;
            window.PreviewKeyDown += (_, e) =>
            {
                // Esc closes it, as the window's × does: a cancellation.
                if (e.Key == Key.Escape)
                {
                    e.Handled = true;
                    Send("inbox.action", inboxPending
                        ? new { target, requestId, action = "dialogDismiss" }
                        : new { target, requestId, action = "later" });
                }
            };
            window.Closing += (_, e) =>
            {
                if (inboxClosing || closed || !ReferenceEquals(inboxWindow, window))
                    return;
                e.Cancel = true;
                Send("inbox.action", inboxPending
                    ? new { target, requestId, action = "dialogDismiss" }
                    : new { target, requestId, action = "later" });
            };
            window.Show();
        }
        inboxPending = Flag(data, "pending");
        inboxSource!.Text = Text(data, "source");
        inboxGone!.Text = Text(data, "gone");
        inboxGone.Visibility = inboxGone.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        var status = Text(data, "status");
        inboxStatus!.Text = status;
        inboxStatus.Visibility = status.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        inboxStatus.SetResourceReference(TextBlock.ForegroundProperty, Text(data, "tone") == "warning" ? "YaacWarning" : "YaacMuted");
        inboxLayout!.Visibility = Flag(data, "pending") ? Visibility.Visible : Visibility.Collapsed;
        if (Flag(data, "pending"))
            inboxLayout.Render(data);
        inboxActions!.Children.Clear();
        foreach (var action in data.Value("actions").Elements())
        {
            var id = Text(action, "id");
            var button = new Button { Content = Text(action, "label"), IsEnabled = Flag(action, "enabled"), MinWidth = 80, Margin = new Thickness(0, 0, 8, 0) };
            button.Click += (_, _) => Send("inbox.action", new { target, requestId, action = id });
            inboxActions.Children.Add(button);
        }
    }
}
