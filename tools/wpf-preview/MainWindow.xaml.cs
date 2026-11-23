using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;

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
    private bool sidebarCollapsed;
    private double restoredSidebarWidth = 280;
    private bool sidebarPreference;
    private bool firstRun;
    private JsonElement lastSidebar;
    private string? railSignature;
    private readonly Dictionary<(string Key, ModifierKeys Modifiers), Shortcut> shortcuts = new();
    private sealed record Shortcut(string Id, bool InText, bool Repeat);

    internal MainWindow(Func<Controller> connect, bool demo, bool tray)
    {
        InitializeComponent();
        Title = demo ? "YAAC · WPF demo" : "YAAC";
        if (tray) this.tray = WindowTray.TryCreate(ShowFromTray, QuitWindow);
        SidebarPane.Command += Send;
        SidebarPane.BackButton.Click += BackClicked;
        SidebarPane.ForwardButton.Click += ForwardClicked;
        SidebarPane.NeedsButton.Click += NeedsClicked;
        SidebarPane.CollapseButton.Click += SidebarClicked;
        SidebarPane.SettingsButton.Click += SettingsClicked;
        SidebarPane.ScenesButton.Click += ScenesClicked;
        TranscriptPane.Command += Send;
        ComposerPane.Command += Send;
        AgentsPane.Command += Send;
        SettingsPane.Command += Send;
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
        SizeChanged += (_, _) =>
        {
            UpdateClientArea();
            UpdateSidebar();
            PositionFirstRun();
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
        if (ClientArea.Margin != inset)
            ClientArea.Margin = inset;
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
                frame => Dispatcher.InvokeAsync(() => Receive(frame),
                    System.Windows.Threading.DispatcherPriority.Background).Task));
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

    private void Receive(JsonElement frame)
    {
        if (closed) return;
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
        if (views.TryGetProperty("shell", out var shell)) RenderShell(shell);
        if (views.TryGetProperty("captionUpdate", out var update)) RenderCaptionUpdate(update);
        if (views.TryGetProperty("shortcuts", out var bindings)) RenderShortcuts(bindings);
        if (views.TryGetProperty("sidebar", out var sidebar))
        {
            SidebarPane.Render(sidebar);
            RailUsage.Render(sidebar.TryGetProperty("usage", out var usage) ? usage : default);
            lastSidebar = sidebar.Clone();
            if (sidebarCollapsed) RenderRail();
        }
        if (views.TryGetProperty("transcript", out var transcript)) TranscriptPane.Render(transcript);
        if (views.TryGetProperty("composer", out var composer))
        {
            ComposerPane.Render(composer);
            if (Flag(composer, "focus")) Send("shell.focusComposerDone");
        }
        if (views.TryGetProperty("agents", out var agents))
        {
            bool docked = Text(agents, "mode") == "docked";
            Grid.SetColumn(AgentsPane, docked ? 3 : 2);
            AgentsPane.Width = docked ? 300 : double.NaN;
            AgentsPane.HorizontalAlignment = docked ? HorizontalAlignment.Stretch : HorizontalAlignment.Right;
            AgentsPane.VerticalAlignment = docked ? VerticalAlignment.Stretch : VerticalAlignment.Center;
            AgentsPane.Margin = docked ? new Thickness(0, 42, 0, 0) : new Thickness(0, 0, 20, 0);
            AgentsPane.Render(agents);
        }
        if (views.TryGetProperty("settings", out var settings)) SettingsPane.Render(settings);
    }

    private void RenderShell(JsonElement shell)
    {
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
        if (shell.TryGetProperty("commander", out var commands)) RenderCommander(commands);
        FontSize = shell.TryGetProperty("fontSize", out var size) ? size.GetDouble() : FontSize;
        Palette.SetTheme(Text(shell, "theme"));
        RenderHistory(SidebarPane.BackButton, shell, "back");
        RenderHistory(SidebarPane.ForwardButton, shell, "forward");
        RenderHistory(RailBack, shell, "back");
        RenderHistory(RailForward, shell, "forward");
        RenderScenes(shell);
        sidebarPreference = Flag(shell, "sidebarCollapsed");
        UpdateSidebar();
        if (Flag(shell, "focusSearch"))
        {
            SidebarPane.FocusSearch();
            Send("shell.focusSearchDone");
        }
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

    private void UpdateVisibility() =>
        Send("shell.visible", new { shown = IsVisible && WindowState != WindowState.Minimized, hidden = !IsVisible });

    private void HideToTray()
    {
        AgentsPane.DismissFloatingCard();
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
        SettingsPane.Margin = new Thickness(firstRun ? SidebarColumn.ActualWidth + 1 : 0, 0, 0, 0);
    }

    private void UpdateSidebar()
    {
        var collapsed = sidebarPreference || ActualWidth > 0 && ActualWidth < 1000;
        if (sidebarCollapsed == collapsed) return;
        sidebarCollapsed = collapsed;
        if (collapsed)
        {
            if (SidebarColumn.ActualWidth >= 220) restoredSidebarWidth = SidebarColumn.ActualWidth;
            SidebarColumn.MinWidth = 48;
            SidebarColumn.Width = new GridLength(48);
        }
        else
        {
            SidebarColumn.MinWidth = 220;
            SidebarColumn.Width = new GridLength(restoredSidebarWidth);
        }
        SidebarPane.Visibility = SidebarSplitter.Visibility = collapsed ? Visibility.Collapsed : Visibility.Visible;
        SidebarRail.Visibility = collapsed ? Visibility.Visible : Visibility.Collapsed;
        if (collapsed) RenderRail();
        PositionFirstRun();
    }

    private void RenderRail()
    {
        if (lastSidebar.ValueKind != JsonValueKind.Object
            || !lastSidebar.TryGetProperty("sections", out var sections)) return;
        var rows = sections.EnumerateArray().SelectMany(section => section.GetProperty("rows").EnumerateArray())
            .Select(row => (Id: Text(row, "id"), Title: Text(row, "title"), Tone: Text(row, "tone"), Selected: Flag(row, "selected"), Fast: Flag(row, "fast"), Ultrafast: Flag(row, "ultrafast")))
            .ToArray();
        var signature = JsonSerializer.Serialize(rows.Select(row => new { row.Id, row.Title, row.Tone, row.Selected, row.Fast, row.Ultrafast }));
        if (railSignature == signature) return;
        railSignature = signature;
        RailItems.Children.Clear();
        foreach (var row in rows)
        {
            var button = new Button
            {
                Style = (Style)FindResource("RailButton"),
                Content = row.Tone == "running"
                    ? new RunningSpinner { Width = 15, Height = 15, Fast = row.Fast, Ultrafast = row.Ultrafast, Foreground = RunningSpinner.Ink(row.Fast, row.Ultrafast) }
                    : string.IsNullOrEmpty(MarkGlyph(row.Tone)) && row.Title.Length > 0 ? row.Title[..1] : MarkGlyph(row.Tone),
                ToolTip = row.Title,
            };
            if (row.Selected) button.Background = Palette.Brush("YaacSelected");
            System.Windows.Automation.AutomationProperties.SetName(button, row.Title);
            button.Click += (_, _) => Send("sidebar.open", new { id = row.Id });
            RailItems.Children.Add(button);
        }
    }

    private void SidebarClicked(object sender, RoutedEventArgs args) => Send("shell.sidebar");
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
        if (viewport <= 0 || ComposerColumn is null || TranscriptPane is null) return;
        var max = viewport * .95;
        var width = Math.Clamp(preferredChatWidth, Math.Min(360, max), max);
        var gutter = Math.Min(28, (viewport - width) / 2);
        ComposerColumn.Width = width;
        TranscriptPane.Width = width + gutter * 2;
        TranscriptPane.SetChatGutter(gutter);
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
        if (SettingsPane.IsRecordingShortcut) return;
        var key = args.Key == Key.System ? args.SystemKey : args.Key;
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
        args.Handled = InvokeShortcut(logical, Keyboard.Modifiers, args.IsRepeat);
    }

    private void WindowMouse(object sender, MouseButtonEventArgs args)
    {
        if (SettingsPane.IsRecordingShortcut || CommanderOverlay.IsVisible) return;
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
            }
        }
        Send("shell.shortcut", new { id = shortcut.Id, inText });
        return true;
    }

    private static string Text(JsonElement element, string key) =>
        element.ValueKind == JsonValueKind.Object && element.TryGetProperty(key, out var value)
            && value.ValueKind == JsonValueKind.String ? value.GetString()! : "";

    private static bool Flag(JsonElement element, string key) =>
        element.ValueKind == JsonValueKind.Object && element.TryGetProperty(key, out var value)
            && value.ValueKind == JsonValueKind.True;
}
