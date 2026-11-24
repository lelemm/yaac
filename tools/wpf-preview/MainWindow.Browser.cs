using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Controls.Primitives;
using System.Windows.Threading;
using Yaac.WpfPreview.Views;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private string browserThread = "";
    private string browserReason = "Open a thread to use the browser.";
    private string? browserDragThread;

    private void InitializeBrowser()
    {
        BrowserPane.Command += Send;
        DpiChanged += (_, _) => Dispatcher.BeginInvoke(DispatcherPriority.Loaded, BrowserPane.UpdateViewport);
    }

    private void CaptionAddClicked(object sender, RoutedEventArgs e)
    {
        var menu = new ContextMenu { PlacementTarget = CaptionAddPane, Placement = PlacementMode.Bottom };
        var browser = new MenuItem
        {
            Header = "Browser",
            ToolTip = browserReason.Length > 0 ? browserReason : null,
            IsEnabled = browserReason.Length == 0,
        };
        browser.Click += (_, _) => BrowserPane.OpenNewTab();
        menu.Items.Add(browser);
        menu.IsOpen = true;
    }

    private void RenderBrowser(JsonElement state)
    {
        var thread = Text(state, "thread");
        if (browserThread != thread) browserDragThread = null;
        browserThread = thread;
        browserReason = Text(state, "reason");
        BrowserExternalFallback.Visibility = Flag(state, "external") ? Visibility.Visible : Visibility.Collapsed;
        var active = state.Value("active").ValueKind == JsonValueKind.Number;
        var full = active && Flag(state, "fullScreen");
        BrowserPane.Visibility = active ? Visibility.Visible : Visibility.Collapsed;
        BrowserChat.Visibility = full ? Visibility.Collapsed : Visibility.Visible;
        BrowserDivider.Visibility = active && !full ? Visibility.Visible : Visibility.Collapsed;
        BrowserDividerColumn.Width = new GridLength(active && !full ? 5 : 0);
        if (browserDragThread is null)
        {
            var ratio = state.Value("ratio").ValueKind == JsonValueKind.Number
                ? state.GetProperty("ratio").GetDouble() : .5;
            BrowserChatColumn.Width = new GridLength(full ? 0 : active ? ratio : 1, GridUnitType.Star);
            BrowserColumn.Width = new GridLength(full ? 1 : active ? 1 - ratio : 0, GridUnitType.Star);
        }
        BrowserPane.Render(state);
    }

    private void BrowserExternalClicked(object sender, RoutedEventArgs e)
    {
        var thread = browserThread;
        var dialog = Overlay("Open in browser", 350);
        var panel = new StackPanel { Margin = new Thickness(16) };
        panel.Children.Add(new TextBlock { Text = "Enter an address or port on this thread’s target." });
        var address = new TextBox { Margin = new Thickness(0, 8, 0, 8) };
        System.Windows.Automation.AutomationProperties.SetName(address, "Address or port");
        panel.Children.Add(address);
        var open = new Button { Content = "Open in browser", IsDefault = true, HorizontalAlignment = HorizontalAlignment.Right };
        open.Click += (_, _) =>
        {
            if (address.Text.Trim().Length == 0) return;
            Send("browser.openExternal", new { thread, url = address.Text });
            dialog.Close();
        };
        panel.Children.Add(open);
        dialog.Content = panel;
        dialog.PreviewKeyDown += (_, args) =>
        {
            if (args.Key == Key.Escape) dialog.Close();
        };
        dialog.Show();
        address.Focus();
    }

    private void BrowserChatSizeChanged(object sender, SizeChangedEventArgs e)
    {
        if (e.WidthChanged) ApplyChatWidth(e.NewSize.Width);
    }

    private void BrowserDragStarted(object sender, DragStartedEventArgs e) => browserDragThread = browserThread;

    private void BrowserDragCompleted(object sender, DragCompletedEventArgs e)
    {
        if (!e.Canceled && browserDragThread == browserThread) PersistBrowserRatio();
        browserDragThread = null;
    }

    private void BrowserDividerKeyUp(object sender, KeyEventArgs e)
    {
        if (e.Key is Key.Left or Key.Right) PersistBrowserRatio();
    }

    private void PersistBrowserRatio()
    {
        var width = BrowserChatColumn.ActualWidth + BrowserColumn.ActualWidth;
        if (width > 0)
            Send("browser.ratio", new { thread = browserThread, ratio = BrowserChatColumn.ActualWidth / width });
    }
}
