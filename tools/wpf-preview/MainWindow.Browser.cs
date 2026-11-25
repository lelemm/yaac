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
    private string? browserDragThread;
    private bool browserCanSwitchTabs;
    private ModifierKeys? tabSwitchModifiers;

    private void InitializeBrowser()
    {
        PaneIcon.Set(ComposerPane.TerminalToggle, "Terminal");
        ComposerPane.TerminalToggle.Click += (_, _) => Send("shell.shortcut", new { id = ToggleTerminal });
        ChatRegion.SizeChanged += (_, e) => { if (e.HeightChanged) SizeTerminalPanel(); };
        ComposerColumn.SizeChanged += (_, e) => { if (panelOpen && e.HeightChanged) SizeTerminalPanel(); };
        BrowserPane.Command += Send;
        BrowserPane.ComposerCaret = () => ComposerPane.CaretIndex;
        DpiChanged += (_, _) => Dispatcher.BeginInvoke(DispatcherPriority.Loaded, BrowserPane.UpdateViewport);
        Deactivated += (_, _) => FinishTabSwitch(false);
        PreviewKeyUp += (_, _) =>
        {
            if (tabSwitchModifiers is { } held && (Keyboard.Modifiers & held) != held)
                FinishTabSwitch(true);
        };
    }

    private void CaptionAddClicked(object sender, RoutedEventArgs e) => BrowserPane.OpenAddMenu(CaptionAddPane);

    private void RenderBrowser(JsonElement state)
    {
        var thread = Text(state, "thread");
        if (browserThread != thread)
        {
            browserDragThread = null;
            tabSwitchModifiers = null;
        }
        browserThread = thread;
        // Side pane tabs belong to a thread: without one open there is nothing to add a tab to.
        CaptionAddPane.Visibility = string.IsNullOrEmpty(thread) ? Visibility.Collapsed : Visibility.Visible;
        BrowserExternalFallback.Visibility = Flag(state, "external") ? Visibility.Visible : Visibility.Collapsed;
        var active = state.Value("active").ValueKind == JsonValueKind.Number;
        var full = active && Flag(state, "fullScreen");
        browserCanSwitchTabs = full && state.Value("tabs").Elements().Any();
        if (!browserCanSwitchTabs) tabSwitchModifiers = null;
        RenderTabSwitcher(state.Value("tabSwitcher"));
        BrowserPane.Visibility = active ? Visibility.Visible : Visibility.Collapsed;
        bool showChatTab = full && Flag(state, "chat");
        if (showChatTab && ThreadColumns.Children.Contains(ChatRegion))
        {
            ThreadColumns.Children.Remove(ChatRegion);
            BrowserPane.ChatHost.Content = ChatRegion;
        }
        else if (!showChatTab && BrowserPane.ChatHost.Content != null)
        {
            BrowserPane.ChatHost.Content = null;
            ThreadColumns.Children.Insert(0, ChatRegion);
        }
        ChatRegion.Visibility = full && !showChatTab ? Visibility.Collapsed : Visibility.Visible;
        BrowserDivider.Visibility = active && !full ? Visibility.Visible : Visibility.Collapsed;
        BrowserDividerColumn.Width = new GridLength(active && !full ? 5 : 0);
        if (browserDragThread is null)
        {
            var ratio = state.Value("ratio").ValueKind == JsonValueKind.Number
                ? state.GetProperty("ratio").GetDouble() : .5;
            BrowserChatColumn.Width = new GridLength(full ? 0 : active ? ratio : 1, GridUnitType.Star);
            BrowserColumn.Width = new GridLength(full ? 1 : active ? 1 - ratio : 0, GridUnitType.Star);
        }
        ThreadLocation.Visibility = showChatTab ? Visibility.Collapsed : Visibility.Visible;
        UpdateDockLayout();
        BrowserPane.Render(state);
        RenderTerminalPanel(state);
    }

    private bool CycleFullscreenTabs(string key, ModifierKeys modifiers, bool repeat)
    {
        if (!browserCanSwitchTabs || settingsOpen || SettingsPane.IsRecordingShortcut || CommanderOverlay.IsVisible
            || (dialog?.IsVisible ?? false)) return false;
        if (key == "Escape" && tabSwitchModifiers.HasValue)
        {
            FinishTabSwitch(false);
            return true;
        }
        if (!shortcuts.TryGetValue((key, modifiers), out var shortcut) || shortcut.Id != "cycle-fullscreen-tabs") return false;
        if (TerminalFocused ? !shortcut.InTerminal : Keyboard.FocusedElement is (TextBoxBase or PasswordBox) && !shortcut.InText) return false;
        if (repeat) return true;
        tabSwitchModifiers = modifiers;
        Send("browser.switchTab", new { thread = browserThread, mode = "cycle" });
        if (modifiers == ModifierKeys.None) FinishTabSwitch(true);
        return true;
    }

    private void FinishTabSwitch(bool activate)
    {
        if (!tabSwitchModifiers.HasValue) return;
        tabSwitchModifiers = null;
        TabSwitchOverlay.Visibility = Visibility.Collapsed;
        Send("browser.switchTab", new { thread = browserThread, mode = activate ? "commit" : "cancel" });
    }

    private void RenderTabSwitcher(JsonElement state)
    {
        if (!tabSwitchModifiers.HasValue || state.ValueKind != JsonValueKind.Object)
        {
            TabSwitchOverlay.Visibility = Visibility.Collapsed;
            return;
        }
        TabSwitchRows.ItemsSource = state.Value("tabs").Elements().Select(item => item.Text()).ToArray();
        TabSwitchRows.SelectedIndex = state.GetProperty("index").GetInt32();
        TabSwitchOverlay.Visibility = Visibility.Visible;
        if (TabSwitchRows.SelectedItem is { } selected) TabSwitchRows.ScrollIntoView(selected);
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
