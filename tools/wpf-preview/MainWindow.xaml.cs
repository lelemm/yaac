using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Shell;
using Yaac.WpfPreview.Views;

namespace Yaac.WpfPreview;

public partial class MainWindow : Window
{
    private Controller? controller;
    private WindowTray? tray;
    private bool trayOnClose;
    private bool quitting;
    private WindowState visibleState = WindowState.Maximized;
    private bool settingsOpen;
    private bool closed;
    private string? activeDialog;
    private Window? dialog;
    private double fontScale = 1;
    private bool firstRun;
    private readonly Dictionary<(string Key, ModifierKeys Modifiers), Shortcut> shortcuts = new();
    private sealed record Shortcut(string Id, bool InText, bool Repeat);

    internal MainWindow(Func<Controller> connect, bool demo, bool tray)
    {
        InitializeComponent();
        InitializeRail();
        Title = demo ? "YAAC · WPF demo" : "YAAC";
        if (tray) this.tray = WindowTray.TryCreate(ShowFromTray, QuitWindow);
        SidebarPane.Command += (command, payload) =>
        {
            Send(command, payload);
            if (command == "sidebar.open") CloseNarrowSidebar();
        };
        SidebarPane.BackButton.Click += BackClicked;
        SidebarPane.ForwardButton.Click += ForwardClicked;
        SidebarPane.NeedsButton.Click += NeedsClicked;
        SidebarPane.CollapseButton.Click += SidebarClicked;
        SidebarPane.SettingsButton.Click += SettingsClicked;
        SidebarPane.ScenesButton.Click += ScenesClicked;
        TranscriptPane.Command += Send;
        ImageOverlay.CloseRequested += () => Send("image.close");
        ChatSurface.AddHandler(MouseLeftButtonDownEvent, new MouseButtonEventHandler(ChatBackgroundClicked),
            handledEventsToo: true);
        ComposerPane.Command += Send;
        ComposerPane.DropOverlayChanged += visible =>
            ComposerDropOverlay.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        AgentsPane.Command += Send;
        SettingsPane.Command += Send;
        SidebarPane.ObservePrRows(QueueVisiblePrRows);
        AgentsPane.ObservePrRows(QueueVisiblePrRows);
        Loaded += async (_, _) => await StartAsync(connect);
        SourceInitialized += (_, _) => UpdateClientArea();
        LocationChanged += (_, _) => UpdateClientArea();
        DpiChanged += (_, _) => UpdateClientArea();
        StateChanged += (_, _) =>
        {
            if (WindowState != WindowState.Minimized) visibleState = WindowState;
            UpdateClientArea();
            UpdateCaption();
            UpdateVisibility();
        };
        SizeChanged += (_, _) => UpdateClientArea();
        Workspace.SizeChanged += (_, e) =>
        {
            if (e.WidthChanged) UpdatePaneLayout();
        };
        UpdateCaption();
        IsVisibleChanged += (_, _) => UpdateVisibility();
        Closing += (_, e) =>
        {
            if (quitting || !trayOnClose || this.tray is null) return;
            e.Cancel = true;
            HideToTray();
        };
        Closed += (_, _) =>
        {
            closed = true;
            this.tray?.Dispose();
            this.tray = null;
            controller?.Dispose();
        };
    }

    private void UpdateClientArea()
    {
        var inset = WindowState == WindowState.Maximized
            ? WindowInsets.ForMaximized(this)
            : new Thickness();
        if (Padding != inset)
            Padding = inset;
    }

    private void ApplyFontScale(JsonElement shell)
    {
        if (!shell.TryGetProperty("fontScale", out var value) || value.ValueKind != JsonValueKind.Number
            || !value.TryGetDouble(out var scale) || !double.IsFinite(scale) || scale <= 0 || fontScale == scale)
            return;

        fontScale = scale;
        Application.Current.Resources["YaacUiScale"] = new ScaleTransform(scale, scale);
        WindowChrome.GetWindowChrome(this).CaptionHeight = 42 * scale;
        UpdateDialogWidth();

        // In-tree popups copy the accumulated layout transform when they open.
        ComposerPane.DismissPickers();
        AgentsPane.DismissFloatingCard();
        CloseRailHover();
    }

    private async Task StartAsync(Func<Controller> connect)
    {
        try
        {
            var session = await Task.Run(connect);
            controller = session;
            if (closed)
            {
                session.Dispose();
                await session.RunAsync(_ => Task.CompletedTask);
                return;
            }
            Send("shell.ready", new { tray = tray is not null });
            UpdateVisibility();
            await Task.Run(() => session.RunAsync(
                frame =>
                {
                    var image = ImageViewerFrame.Read(frame.Value("imageViewer"));
                    return Dispatcher.InvokeAsync(() => Receive(frame, image),
                        System.Windows.Threading.DispatcherPriority.Background).Task;
                }));
        }
        catch (Exception error) when (!closed)
        {
            Status.Text = $"Controller disconnected · {error.Message}";
            Workspace.IsEnabled = false;
            SettingsPane.IsEnabled = false;
        }
        catch (Exception) when (closed)
        {
        }
    }

    private void Receive(JsonElement frame, ImageViewerFrame? image)
    {
        if (closed) return;
        if (image is { } viewer)
            ImageOverlay.Render(viewer);
        if (frame.TryGetProperty("window", out var command))
        {
            switch (command.GetString())
            {
                case "show": ShowFromTray(); break;
                case "hide": HideToTray(); break;
                case "quit": QuitWindow(); break;
            }
            if (closed) return;
        }
        if (frame.TryGetProperty("error", out var error))
        {
            if (dialog is { IsVisible: true }) MessageBox.Show(dialog, error.GetString(), "YAAC");
            else Status.Text = error.GetString();
        }
        if (frame.TryGetProperty("copy", out var copy)) Send("shell.copy", copy.GetString());
        if (!frame.TryGetProperty("views", out var views)) return;
        if (views.TryGetProperty("paneSizing", out var panes)) RenderPaneSizing(panes);
        bool hasShell = views.TryGetProperty("shell", out var shell);
        if (hasShell) RenderShell(shell);
        if (views.TryGetProperty("captionUpdate", out var update)) RenderCaptionUpdate(update);
        if (views.TryGetProperty("shortcuts", out var bindings)) RenderShortcuts(bindings);
        if (views.TryGetProperty("sidebar", out var sidebar))
        {
            SidebarPane.Render(sidebar);
            RailUsage.Render(sidebar.TryGetProperty("usage", out var usage) ? usage : default);
            SetRailData(sidebar);
        }
        if (views.TryGetProperty("transcript", out var transcript)) TranscriptPane.Render(transcript);
        if (views.TryGetProperty("transcriptPreview", out var preview))
        {
            bool visible = preview.ValueKind == JsonValueKind.Object;
            TranscriptPreview.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
            if (visible)
            {
                TranscriptPreviewTitle.Text = $"Previewing {preview.GetProperty("previewTitle").GetString()}";
                TranscriptPreviewPane.Render(preview);
            }
        }
        if (views.TryGetProperty("composer", out var composer))
        {
            ComposerPane.Render(composer);
            if (Flag(composer, "focus")) Send("shell.focusComposerDone");
        }
        if (views.TryGetProperty("agents", out var agents))
        {
            AgentsPane.Render(agents);
            UpdateDockLayout();
        }
        if (views.TryGetProperty("settings", out var settings)) SettingsPane.Render(settings);
        if (hasShell && Flag(shell, "focusSearch"))
        {
            if (!settingsOpen && SidebarPane.SearchEnabled)
            {
                if (NarrowWorkspace) OpenNarrowSidebar();
                SidebarPane.FocusSearch();
            }
            Send("shell.focusSearchDone");
        }
        if (views.TryGetProperty("sidebar", out _) || views.TryGetProperty("agents", out _)
            || views.TryGetProperty("shell", out _))
            QueueVisiblePrRows();
    }

    private void RenderShell(JsonElement shell)
    {
        ApplyFontScale(shell);
        RenderHeader(shell);
        settingsOpen = Flag(shell, "settings");
        firstRun = Flag(shell, "firstRun") && !settingsOpen;
        Workspace.Visibility = settingsOpen ? Visibility.Collapsed : Visibility.Visible;
        ThreadContent.Visibility = firstRun ? Visibility.Collapsed : Visibility.Visible;
        SettingsPane.Visibility = settingsOpen || firstRun ? Visibility.Visible : Visibility.Collapsed;
        PositionFirstRun();
        SidebarPane.BackButton.IsEnabled = RailBack.IsEnabled = Flag(shell, "canBack");
        SidebarPane.ForwardButton.IsEnabled = RailForward.IsEnabled = Flag(shell, "canForward");
        var count = shell.TryGetProperty("needs", out var needs) ? needs.GetInt32() : 0;
        trayOnClose = Flag(shell, "trayOnClose");
        tray?.Update(count, Text(shell, "trayTooltip"));
        SidebarPane.NeedsButton.Content = RailNeeds.Content = $"!  {count}";
        SidebarPane.NeedsButton.Visibility = RailNeeds.Visibility = count > 0 ? Visibility.Visible : Visibility.Collapsed;
        Status.Text = Text(shell, "status");
        if (shell.TryGetProperty("stoppedWsl", out var stoppedWsl)) RenderWslNotifications(stoppedWsl);
        if (shell.TryGetProperty("commander", out var commands)) RenderCommander(commands);
        Palette.SetTheme(Text(shell, "theme"));
        RenderHistory(SidebarPane.BackButton, shell, "back");
        RenderHistory(SidebarPane.ForwardButton, shell, "forward");
        RenderHistory(RailBack, shell, "back");
        RenderHistory(RailForward, shell, "forward");
        RenderScenes(shell);
        if (shell.TryGetProperty("dialog", out var dialogView))
            RenderDialog(dialogView);
    }

    private void Send(string command, object? payload = null)
    {
        if (command == "shell.copy")
        {
            if (payload is string text)
            {
                try { Clipboard.SetText(text); }
                catch (System.Runtime.InteropServices.ExternalException) { Status.Text = "Clipboard is busy."; }
            }
            return;
        }

        controller?.Send(command, payload);
    }

    private void RenderHistory(Button button, JsonElement shell, string key)
    {
        if (!shell.TryGetProperty(key, out var places) || places.GetArrayLength() == 0)
        {
            button.ContextMenu = null;
            return;
        }
        var menu = new ContextMenu();
        foreach (var place in places.EnumerateArray())
        {
            int steps = place.GetProperty("steps").GetInt32();
            var item = new MenuItem { Header = Text(place, "label"), ToolTip = Text(place, "detail") };
            item.Click += (_, _) => Send("shell.history", new { steps });
            menu.Items.Add(item);
        }
        button.ContextMenu = menu;
    }

    private void SettingsClicked(object sender, RoutedEventArgs args) =>
        Send(settingsOpen ? "settings.close" : "shell.settings");
    private void BackClicked(object sender, RoutedEventArgs args) => Send("shell.back");
    private void ForwardClicked(object sender, RoutedEventArgs args) => Send("shell.forward");
    private void NeedsClicked(object sender, RoutedEventArgs args) => Send("shell.needs");

    private void UpdateVisibility()
    {
        bool shown = IsVisible && WindowState != WindowState.Minimized;
        if (!shown)
            visiblePrRows = [];
        Send("shell.visible", new { shown, hidden = !IsVisible });
        QueueVisiblePrRows();
    }

    private void HideToTray()
    {
        AgentsPane.DismissFloatingCard();
        CloseRailHover();
        dialog?.Hide();
        Hide();
    }

    private void ShowFromTray()
    {
        if (closed) return;
        if (!IsVisible) Show();
        if (WindowState == WindowState.Minimized) WindowState = visibleState;
        Activate();
        if (dialog is { IsVisible: false }) dialog.Show();
    }

    private void QuitWindow()
    {
        if (closed) return;
        quitting = true;
        Close();
    }

    private void PositionFirstRun()
    {
        SettingsPane.Margin = new Thickness(firstRun ? SidebarColumn.Width.Value + SidebarGap.Width.Value : 0, 0, 0, 0);
    }

    private void RailNewClicked(object sender, RoutedEventArgs args) => Send("shell.new");
    private double preferredChatWidth = 860;

    private void ThreadContentSizeChanged(object sender, SizeChangedEventArgs e)
    {
        ApplyChatWidth(e.NewSize.Width);
        AgentsPane?.SetFloatingViewport(e.NewSize.Height);
    }

    private void ComposerResize(object sender, System.Windows.Controls.Primitives.DragDeltaEventArgs e)
    {
        var max = ThreadContent.ActualWidth * .95;
        if (max <= 0) return;
        preferredChatWidth = Math.Clamp(ComposerColumn.ActualWidth + e.HorizontalChange * 2, Math.Min(360, max), max);
        ApplyChatWidth(ThreadContent.ActualWidth);
    }

    private void ApplyChatWidth(double viewport)
    {
        if (viewport <= 0 || ComposerColumn is null || TranscriptPane is null || TranscriptPreviewPane is null) return;
        var max = viewport * .95;
        var width = Math.Clamp(preferredChatWidth, Math.Min(360, max), max);
        var gutter = Math.Min(28, (viewport - width) / 2);
        ComposerColumn.Width = width;
        TranscriptPane.Width = width + gutter * 2;
        TranscriptPane.SetChatGutter(gutter);
        TranscriptPreviewPane.Width = TranscriptPane.Width;
        TranscriptPreviewPane.SetChatGutter(gutter);
    }

    private void MinimizeClicked(object sender, RoutedEventArgs args) => SystemCommands.MinimizeWindow(this);
    private void CaptionUpdateClicked(object sender, RoutedEventArgs args)
    {
        if (CaptionUpdateButton.Tag is string action)
            Send("shell.update", new { action });
    }

    private void RenderCaptionUpdate(JsonElement update)
    {
        var visible = update.ValueKind == JsonValueKind.Object;
        CaptionUpdate.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        CaptionThreadHeader.Margin = new Thickness(28, 0, 132 + (visible ? CaptionUpdate.Width : 0), 0);
        if (!visible) return;

        var action = Text(update, "action");
        var tooltip = Text(update, "tooltip");
        var downloading = action.Length == 0;
        CaptionUpdate.ToolTip = tooltip;
        CaptionUpdateButton.Tag = action;
        CaptionUpdateButton.Content = action == "install" ? "\uEB37" : "\uEA78";
        CaptionUpdateButton.Visibility = downloading ? Visibility.Collapsed : Visibility.Visible;
        System.Windows.Automation.AutomationProperties.SetName(CaptionUpdateButton, tooltip);
        CaptionUpdateProgress.Visibility = downloading ? Visibility.Visible : Visibility.Collapsed;
        CaptionUpdateProgress.Value = update.GetProperty("progress").GetDouble();
        System.Windows.Automation.AutomationProperties.SetName(CaptionUpdateProgress, tooltip);
    }

    private void CloseClicked(object sender, RoutedEventArgs args) => SystemCommands.CloseWindow(this);
    private void MaximizeClicked(object sender, RoutedEventArgs args)
    {
        if (WindowState == WindowState.Maximized) SystemCommands.RestoreWindow(this);
        else SystemCommands.MaximizeWindow(this);
    }

    private void UpdateCaption()
    {
        var restore = WindowState == WindowState.Maximized;
        MaximizeButton.Content = restore ? "\uEABB" : "\uEAB9";
        MaximizeButton.ToolTip = restore ? "Restore" : "Maximize";
        System.Windows.Automation.AutomationProperties.SetName(MaximizeButton, (string)MaximizeButton.ToolTip);
    }

    private void RenderScenes(JsonElement shell)
    {
        SidebarPane.ScenesButton.Visibility = Flag(shell, "demo") ? Visibility.Visible : Visibility.Collapsed;
        if (SidebarPane.ScenesButton.Visibility != Visibility.Visible) return;
        if (SidebarPane.ScenesButton.ContextMenu != null || !shell.TryGetProperty("scenes", out var scenes)) return;
        var menu = new ContextMenu { PlacementTarget = SidebarPane.ScenesButton, Placement = PlacementMode.Bottom };
        foreach (var scene in scenes.EnumerateArray())
        {
            var id = scene.GetProperty("id").GetInt32();
            var item = new MenuItem { Header = Text(scene, "label") };
            item.Click += (_, _) => Send("shell.scene", new { id });
            menu.Items.Add(item);
        }
        menu.Items.Add(new Separator());
        var stream = new MenuItem { Header = "Stream a reply in this thread" };
        stream.Click += (_, _) => Send("shell.stream");
        menu.Items.Add(stream);
        var unread = new MenuItem { Header = "Mark finished agents unread" };
        unread.Click += (_, _) => Send("shell.unreadDemo");
        menu.Items.Add(unread);
        var reset = new MenuItem { Header = "Reset demo" };
        reset.Click += (_, _) => Send("shell.reset");
        menu.Items.Add(reset);
        SidebarPane.ScenesButton.ContextMenu = menu;
    }

    private void ScenesClicked(object sender, RoutedEventArgs args)
    {
        if (SidebarPane.ScenesButton.ContextMenu != null) SidebarPane.ScenesButton.ContextMenu.IsOpen = true;
    }

    private void RenderShortcuts(JsonElement bindings)
    {
        shortcuts.Clear();
        foreach (var binding in bindings.EnumerateArray())
        {
            var modifiers = ModifierKeys.None;
            if (Flag(binding, "ctrl")) modifiers |= ModifierKeys.Control;
            if (Flag(binding, "alt")) modifiers |= ModifierKeys.Alt;
            if (Flag(binding, "shift")) modifiers |= ModifierKeys.Shift;
            shortcuts.TryAdd((Text(binding, "key"), modifiers),
                new Shortcut(Text(binding, "id"), Flag(binding, "inText"), Flag(binding, "repeat")));
        }
    }

    private void WindowKey(object sender, KeyEventArgs args)
    {
        var key = args.Key == Key.System ? args.SystemKey : args.Key;
        if (ImageOverlay.IsVisible)
        {
            if (key == Key.Escape)
            {
                ImageOverlay.CloseViewer();
                args.Handled = true;
            }
            else if (!ImageOverlay.IsKeyboardFocusWithin)
                args.Handled = true;
            return;
        }
        if (SettingsPane.IsRecordingShortcut) return;
        var logical = Views.SettingsView.CanonicalKey(key, Keyboard.Modifiers) ?? key.ToString();
        if (CommanderOverlay.IsVisible)
        {
            if (!args.IsRepeat && shortcuts.TryGetValue((logical, Keyboard.Modifiers), out var shortcut)
                && shortcut.Id == "commander")
            {
                OpenCommander();
                args.Handled = true;
            }
            return;
        }
        if (key == Key.Escape && Keyboard.Modifiers == ModifierKeys.None &&
            narrowSidebarOpen && SidebarPane.IsKeyboardFocusWithin && !SidebarPane.SearchHasText)
        {
            CloseNarrowSidebar();
            RailExpand.Focus();
            args.Handled = true;
            return;
        }
        args.Handled = InvokeShortcut(logical, Keyboard.Modifiers, args.IsRepeat);
    }

    private void ChatBackgroundClicked(object sender, MouseButtonEventArgs args)
    {
        var source = args.OriginalSource as DependencyObject;
        if (TranscriptView.FindAncestor<TextBoxBase>(source) is not null ||
            TranscriptView.FindAncestor<ButtonBase>(source) is not null ||
            TranscriptView.FindAncestor<ScrollBar>(source) is not null)
            return;
        TranscriptPane.FocusBackground();
    }

    private void WindowMouse(object sender, MouseButtonEventArgs args)
    {
        if (SettingsPane.IsRecordingShortcut || CommanderOverlay.IsVisible || ImageOverlay.IsVisible) return;
        var logical = args.ChangedButton switch
        {
            MouseButton.XButton1 => "MouseBack",
            MouseButton.XButton2 => "MouseForward",
            _ => null,
        };
        if (logical != null) args.Handled = InvokeShortcut(logical, ModifierKeys.None, false);
    }

    private bool InvokeShortcut(string key, ModifierKeys modifiers, bool repeat)
    {
        if (!shortcuts.TryGetValue((key, modifiers), out var shortcut)
            && ((modifiers & ModifierKeys.Shift) == 0
                || !shortcuts.TryGetValue((key, modifiers & ~ModifierKeys.Shift), out shortcut)))
            return false;
        var inText = Keyboard.FocusedElement is TextBoxBase or PasswordBox;
        if (inText && !shortcut.InText) return false;
        if (repeat && !shortcut.Repeat) return true;
        if (shortcut.Id == "commander") { OpenCommander(); return true; }
        if (!settingsOpen)
        {
            switch (shortcut.Id)
            {
                case "jump-to-bottom": TranscriptPane.JumpToBottom(); return true;
                case "prev-message": TranscriptPane.NavigatePrompt(-1); return true;
                case "next-message": TranscriptPane.NavigatePrompt(1); return true;
                case "search-threads" when NarrowWorkspace: Send("shell.focusSearch"); return true;
                case "toggle-sidebar" when NarrowWorkspace: ToggleNarrowSidebar(); return true;
            }
        }
        var approval = TranscriptPane.FocusedApproval;
        Send("shell.shortcut", new { id = shortcut.Id, inText, card = approval?.ActionId, threadId = approval?.ThreadId });
        return true;
    }

    private static string Text(JsonElement element, string key) =>
        element.ValueKind == JsonValueKind.Object && element.TryGetProperty(key, out var value)
            && value.ValueKind == JsonValueKind.String ? value.GetString()! : "";

    private static bool Flag(JsonElement element, string key) =>
        element.ValueKind == JsonValueKind.Object && element.TryGetProperty(key, out var value)
            && value.ValueKind == JsonValueKind.True;
}
