using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class BrowserView : UserControl
{
    internal event Action<string, object?>? Command;
    internal Func<int>? ComposerCaret { get; set; }

    private string thread = "";
    private ulong? active;
    private bool full, busy, focusNewTab, focusWhenConnected;
    private bool windowShown;
    private bool addressDirty, updatingAddress, composing;
    private bool browserActive, chat;
    private string chatTitle = "", chatLogo = "", chatLogoColor = "", dismissedError = "";
    private bool chatMarked;
    private string chatTone = "";
    private Border? selectedTab;
    private string filesReason = "Open a thread to use Files.";
    private readonly FilesPane files = new();
    private string displayedUrl = "";
    private string tabSignature = "", portSignature = "";
    private (ulong Tab, int Width, int Height, double Dpi)? viewport;
    private readonly Dictionary<ulong, BitmapImage> icons = new();

    internal bool OwnsKeyboard => PageInput.IsKeyboardFocused || AddressField.IsKeyboardFocused || LandingAddress.IsKeyboardFocused || files.OwnsKeyboard;

    public BrowserView()
    {
        InitializeComponent();
        PaneIcon.Set(AddTabButton, "New tab");
        AddTabButton.Click += (_, _) => OpenAddMenu(AddTabButton);
        PaneIcon.Set(Back, "Back", "Back (Alt+Left)");
        PaneIcon.Set(Forward, "Forward", "Forward (Alt+Right)");
        PaneIcon.Set(External, "Open in browser");
        PaneIcon.Set(Inspect, "Inspect", "Inspect (F12)");
        PaneIcon.Set(DismissError, "Close", "Dismiss error");
        AutomationProperties.SetName(DismissError, "Dismiss error");
        DismissError.Click += (_, _) => { dismissedError = ErrorBanner.Text; ErrorRow.Visibility = Visibility.Collapsed; };
        DiscoverySpinner.Content = new RunningSpinner();
        TabAvailable.SizeChanged += (_, _) => SizeTabs();
        TabScroll.ScrollChanged += (_, _) =>
        {
            LeftFade.Visibility = TabScroll.HorizontalOffset > .5 ? Visibility.Visible : Visibility.Collapsed;
            RightFade.Visibility = TabScroll.HorizontalOffset + .5 < TabScroll.ScrollableWidth ? Visibility.Visible : Visibility.Collapsed;
        };
        TabScroll.PreviewMouseWheel += (_, e) =>
        {
            TabScroll.ScrollToHorizontalOffset(TabScroll.HorizontalOffset - e.Delta);
            e.Handled = true;
        };
        AddressField.GotKeyboardFocus += (_, _) => AddressField.SelectAll();
        AddressField.PreviewMouseLeftButtonDown += (_, e) =>
        {
            if (AddressField.IsKeyboardFocused) return;
            AddressField.Focus();
            e.Handled = true;
        };
        LandingAddress.TextChanged += (_, _) => AddressHint.Visibility =
            LandingAddress.Text.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        LandingAddress.PreviewKeyDown += (_, e) =>
        {
            if (e.Key != Key.Enter) return;
            focusWhenConnected = true;
            Send("navigate", new { url = LandingAddress.Text });
            e.Handled = true;
        };
        FilesContent.Content = files;
        files.Command += (command, data) =>
        {
            if (active is ulong tab)
            {
                if (command == "add")
                    Command?.Invoke("files.add", new { thread, tab, data, caret = ComposerCaret?.Invoke() });
                else
                    Command?.Invoke("files." + command, new { thread, tab, data });
            }
        };
        PreviewKeyDown += (_, e) =>
        {
            if (!browserActive) return;
            var key = e.Key == Key.System ? e.SystemKey : e.Key;
            if (key == Key.L && Keyboard.Modifiers == ModifierKeys.Control)
            {
                AddressField.Focus();
                AddressField.SelectAll();
            }
            else if (key == Key.Left && Keyboard.Modifiers == ModifierKeys.Alt && Back.IsEnabled) SendInput("Back");
            else if (key == Key.Right && Keyboard.Modifiers == ModifierKeys.Alt && Forward.IsEnabled) SendInput("Forward");
            else if (Reload.IsEnabled && (key == Key.F5 && Keyboard.Modifiers == ModifierKeys.None ||
                key == Key.R && Keyboard.Modifiers == ModifierKeys.Control))
                SendInput(busy ? "Stop" : "Reload");
            else if (key == Key.F12 && Inspect.IsEnabled)
            {
                if (!e.IsRepeat) Send("inspect");
            }
            else return;
            e.Handled = true;
        };
        Back.Click += (_, _) => SendInput("Back");
        Forward.Click += (_, _) => SendInput("Forward");
        Reload.Click += (_, _) => SendInput(busy ? "Stop" : "Reload");
        External.Click += (_, _) => Send("external");
        Inspect.Click += (_, _) => Send("inspect");
        Fullscreen.Click += (_, _) => Command?.Invoke("browser.fullScreen", new { thread, fullScreen = !full });

        AddressField.GotKeyboardFocus += (_, _) => AddressDisplay.Visibility = Visibility.Collapsed;
        AddressField.LostKeyboardFocus += (_, _) => AddressDisplay.Visibility = Visibility.Visible;
        AddressField.TextChanged += (_, _) => addressDirty |= !updatingAddress;
        AddressField.PreviewKeyDown += (_, e) =>
        {
            if (e.Key == Key.Enter)
            {
                focusWhenConnected = !Surface.IsVisible;
                Send("navigate", new { url = AddressField.Text });
                FocusPage();
                e.Handled = true;
            }
            else if (e.Key == Key.Escape)
            {
                SetAddress(displayedUrl);
                if (Surface.IsVisible) FocusPage(); else LandingAddress.Focus();
                e.Handled = true;
            }
        };

        Surface.SizeChanged += (_, _) => UpdateViewport();
        Surface.IsVisibleChanged += (_, _) =>
        {
            if (!Surface.IsVisible) DismissPageUi();
            viewport = null;
            UpdateViewport();
        };
        Surface.MouseMove += (_, e) => Pointer(e, "Move");
        Surface.MouseLeave += (_, e) => Pointer(e, "Leave");
        Surface.MouseDown += (_, e) => Button(e, false);
        Surface.MouseUp += (_, e) => Button(e, true);
        Surface.MouseWheel += (_, e) =>
        {
            bool horizontal = Keyboard.Modifiers.HasFlag(ModifierKeys.Shift);
            Pointer(e, new { Wheel = horizontal ? new[] { e.Delta, 0 } : new[] { 0, e.Delta } });
            e.Handled = true;
        };

        PageInput.PreviewKeyDown += (_, e) => KeyEvent(e, true);
        PageInput.PreviewKeyUp += (_, e) => KeyEvent(e, false);
        PageInput.GotKeyboardFocus += (_, _) => SendInput(new { Focus = true });
        PageInput.LostKeyboardFocus += (_, _) =>
        {
            if (composing) SendInput("CancelComposition");
            composing = false;
            SendInput(new { Focus = false });
        };
        TextCompositionManager.AddPreviewTextInputStartHandler(PageInput, (_, e) => Compose(e));
        TextCompositionManager.AddPreviewTextInputUpdateHandler(PageInput, (_, e) => Compose(e));
        TextCompositionManager.AddPreviewTextInputHandler(PageInput, (_, e) =>
        {
            var text = e.Text.Length > 0 ? e.Text : e.SystemText.Length > 0 ? e.SystemText : e.ControlText;
            if (composing) SendInput(new { Commit = text });
            else if (text.Length > 0) SendInput(new { Text = text });
            composing = false;
            e.Handled = true;
        });
    }

    internal void Render(JsonElement state)
    {
        var nextThread = Text(state, "thread");
        if (thread != nextThread) focusNewTab = false;
        var selected = state.Value("active");
        ulong? next = selected.ValueKind == JsonValueKind.Number ? selected.GetUInt64() : null;
        bool nextChat = Flag(state, "fullScreen") && Flag(state, "chat");
        bool switched = chat != nextChat || next != active || nextThread != thread;
        bool hadPageFocus = PageInput.IsKeyboardFocused;
        if (switched)
        {
            LandingAddress.Clear();
            dismissedError = "";
            focusWhenConnected = false;
            DismissPageUi();
            active = next;
            viewport = null;
            Page.Source = null;
            Surface.Cursor = Cursors.Arrow;
        }

        thread = nextThread;
        full = Flag(state, "fullScreen");
        chat = nextChat;
        chatTitle = Text(state, "chatTitle");
        chatLogo = Text(state, "chatLogo");
        chatLogoColor = Text(state, "chatLogoColor");
        chatMarked = Flag(state, "chatMarked");
        chatTone = Text(state, "chatTone");
        ChatHost.Visibility = chat ? Visibility.Visible : Visibility.Collapsed;
        if ((string?)Fullscreen.ToolTip != (full ? "Restore" : "Full screen"))
            PaneIcon.Set(Fullscreen, full ? "Restore" : "Full screen");
        var tabs = state.GetProperty("tabs");
        tabState = tabs.Clone();
        tabReason = Text(state, "reason");
        filesReason = Text(state, "filesReason");
        RenderTabs(tabs, tabReason);
        var current = active is ulong tabId
            ? tabs.EnumerateArray().FirstOrDefault(t => t.GetProperty("id").GetUInt64() == tabId)
            : default;
        var kind = Text(current, "kind");
        browserActive = !chat && active is not null && (kind is "" or "browser");
        BrowserToolbar.Visibility = browserActive ? Visibility.Visible : Visibility.Collapsed;
        if (!browserActive)
        {
            Surface.Visibility = Landing.Visibility = Visibility.Collapsed;
            FilesContent.Visibility = active is null || chat ? Visibility.Collapsed : Visibility.Visible;
            if (!chat && active is not null) files.Render(state, current, thread);
            else files.Clear();
            return;
        }
        FilesContent.Visibility = Visibility.Collapsed;
        files.Clear();
        var tab = active!.Value;
        var url = Text(current, "url");
        if (switched || !addressDirty || displayedUrl != url) SetAddress(url);
        displayedUrl = url;
        busy = Flag(current, "loading");
        var error = Text(current, "error");
        var hasSurface = Flag(current, "surface");
        if (ErrorBanner.Text != error) dismissedError = "";
        ErrorBanner.Text = error;
        ErrorRow.Visibility = error != dismissedError && error.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        Surface.Visibility = hasSurface ? Visibility.Visible : Visibility.Collapsed;
        Landing.Visibility = hasSurface ? Visibility.Collapsed : Visibility.Visible;
        Reload.IsEnabled = hasSurface;
        if ((string?)Reload.ToolTip != (busy ? "Stop (F5 or Ctrl+R)" : "Reload (F5 or Ctrl+R)"))
        {
            PaneIcon.Set(Reload, busy ? "Stop" : "Reload", busy ? "Stop (F5 or Ctrl+R)" : "Reload (F5 or Ctrl+R)");
            AddressIcon.Content = busy ? new RunningSpinner { Width = 16, Height = 16 } : PaneIcon.Draw("Browser");
        }
        ProgressLine.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        Back.IsEnabled = Flag(current, "back");
        Forward.IsEnabled = Flag(current, "forward");
        External.IsEnabled = url.Length > 0;
        Inspect.IsEnabled = hasSurface;

        if (!hasSurface)
            RenderPorts(state, url.Length > 0 && error.Length == 0 ? "Connecting…" : "");
        if (switched && hasSurface && hadPageFocus) SendInput(new { Focus = true });
        if (hasSurface && focusWhenConnected)
        {
            focusWhenConnected = false;
            FocusPage();
        }
        UpdateViewport();
        if (focusNewTab && switched && url.Length == 0)
        {
            focusNewTab = false;
            Dispatcher.BeginInvoke(DispatcherPriority.Input, () =>
            {
                if (active == tab && IsVisible)
                {
                    LandingAddress.Focus();
                }
            });
        }
    }

    internal bool FilesShortcut(Key key, ModifierKeys modifiers)
    {
        if (!FilesContent.IsVisible) return false;
        return files.Shortcut(key, modifiers);
    }

    internal void OpenAddMenu(FrameworkElement target)
    {
        var menu = new ContextMenu { PlacementTarget = target,
            Placement = System.Windows.Controls.Primitives.PlacementMode.Bottom };
        Add("Browser", tabReason, OpenNewTab);
        Add("Files", filesReason, () => Command?.Invoke("files.new", new { thread }));
        menu.IsOpen = true;

        void Add(string title, string unavailable, Action choose)
        {
            var item = new MenuItem { Header = title, IsEnabled = unavailable.Length == 0,
                ToolTip = unavailable.Length == 0 ? null : unavailable };
            item.Icon = PaneIcon.Draw(title);
            ((FrameworkElement)item.Icon).Margin = new Thickness(0,0,6,0);
            ToolTipService.SetShowOnDisabled(item, true);
            item.Click += (_, _) => choose();
            menu.Items.Add(item);
        }
    }

    internal void ReceiveFileDocument(IncomingFileDocument document) => files.ReceiveDocument(document);
    internal void FileDocumentFailed(ulong id) => files.DocumentFailed(id);
    internal void NavigateDiff(ulong id, int row, int ordinal) => files.Navigate(id, row, ordinal);

    internal void OpenNewTab()
    {
        focusNewTab = true;
        Command?.Invoke("browser.new", new { thread });
    }

    private void RenderPorts(JsonElement state, string status)
    {
        var ports = state.GetProperty("ports");
        var machine = Text(state,"machine");
        var project = Text(state,"project").TrimEnd('/','\\');
        var discovering = Flag(state, "discovering");
        var portsError = Text(state, "portsError");
        var signature = $"{thread}:{active}:{machine}:{project}:{discovering}:{status}:{portsError}:{ports.GetRawText()}";
        if (signature == portSignature) return;
        portSignature = signature;
        if (machine.Length == 0) machine = "this target";
        bool empty = ports.GetArrayLength() == 0 && !discovering && status.Length == 0 && portsError.Length == 0;
        LandingStatus.Text = status.Length > 0 ? status : portsError.Length > 0 ? portsError :
            empty ? $"No servers are listening on {machine}." : $"Servers on {machine}";
        DiscoverySpinner.Visibility = discovering ? Visibility.Visible : Visibility.Collapsed;
        NoServersHint.Visibility = empty ? Visibility.Visible : Visibility.Collapsed;
        PortList.Children.Clear();
        foreach (var port in ports.EnumerateArray())
        {
            var number = port.GetProperty("port").GetInt32();
            var command = Text(port, "command");
            var cwd = Text(port, "cwd");
            if (project.Length > 0 && (cwd.StartsWith(project + "/") || cwd.StartsWith(project + "\\")))
                cwd = cwd[(project.Length + 1)..];
            else if (cwd == project) cwd = "project root";
            var label = $"localhost:{number}";
            var row = new DockPanel();
            var globe = PaneIcon.Draw("Browser");
            globe.Margin = new Thickness(0,0,10,0);
            row.Children.Add(globe);
            var chevron = PaneIcon.Draw("Chevron",12);
            DockPanel.SetDock(chevron, Dock.Right);
            row.Children.Add(chevron);
            row.Children.Add(new TextBlock { Text = label, FontFamily = YaacFonts.Mono, VerticalAlignment = VerticalAlignment.Center });
            row.Children.Add(new TextBlock { Text = string.Join("  ·  ", new[]{command, cwd}.Where(x => x.Length > 0)),
                Margin = new Thickness(12,0,6,0), VerticalAlignment = VerticalAlignment.Center,
                Foreground = Palette.Brush("YaacMuted"), TextTrimming = TextTrimming.CharacterEllipsis });
            var button = QuietButton(row, label);
            button.ToolTip = $"{label}  {command}  {cwd}".TrimEnd();
            button.Height = 36;
            button.HorizontalContentAlignment = HorizontalAlignment.Stretch;
            var url = Text(port, "url");
            button.Click += (_, _) => Send("navigate", new { url });
            PortList.Children.Add(button);
        }
    }

    private Button QuietButton(object label, string description)
    {
        var button = new Button { Content = label, Padding = new Thickness(6, 2, 6, 2), ToolTip = description };
        button.SetResourceReference(StyleProperty, "YaacQuietButton");
        AutomationProperties.SetName(button, description);
        return button;
    }

    private void SetAddress(string url)
    {
        updatingAddress = true;
        AddressField.Text = url;
        updatingAddress = false;
        addressDirty = false;
    }

    private void FocusPage()
    {
        if (Surface.IsVisible) PageInput.Focus();
    }

    private void Send(string command, object? data = null)
    {
        if (active is ulong tab) SendFor(tab, command, data);
    }

    private void SendFor(ulong tab, string command, object? data = null) =>
        Command?.Invoke("browser." + command, new { thread, tab, data });

    private void SendInput(object input) => Send("input", input);

    internal void SetWindowShown(bool shown)
    {
        if (windowShown == shown) return;
        windowShown = shown;
        if (!shown) DismissPageUi();
        else
        {
            viewport = null;
            UpdateViewport();
        }
    }

    private double LayoutScale => (TryFindResource("YaacUiScale") as ScaleTransform)?.ScaleX ?? 1;

    internal void UpdateViewport()
    {
        if (!windowShown || !Surface.IsVisible || active is not ulong tab || Surface.ActualWidth < 1 || Surface.ActualHeight < 1) return;
        var scale = LayoutScale;
        var width = Math.Max(1, (int)Math.Round(Surface.ActualWidth * scale));
        var height = Math.Max(1, (int)Math.Round(Surface.ActualHeight * scale));
        var dpi = VisualTreeHelper.GetDpi(Surface).DpiScaleX;
        var current = (tab, width, height, dpi);
        if (viewport == current) return;
        viewport = current;
        ArrangePopup();
        PositionImeInput();
        SendInput(new { Bounds = new { width, height, scale = dpi } });
    }

    private void Button(MouseButtonEventArgs e, bool up)
    {
        var button = e.ChangedButton switch
        {
            MouseButton.Left => 0, MouseButton.Middle => 1, MouseButton.Right => 2, _ => -1,
        };
        if (button < 0)
        {
            if (up) SendInput(e.ChangedButton == MouseButton.XButton1 ? "Back" : "Forward");
        }
        else
        {
            var count = up ? clicks[button] : e.ClickCount;
            if (!up)
            {
                clicks[button] = count;
                FocusPage();
                Surface.CaptureMouse();
            }
            Pointer(e, new { Button = new { button, up, count } });
            if (up && (InputFlags() & (16 | 32 | 64)) == 0) Surface.ReleaseMouseCapture();
        }
        e.Handled = true;
    }

    private readonly int[] clicks = new int[3];

    private void Pointer(MouseEventArgs e, object kind)
    {
        var pos = e.GetPosition(Surface);
        SendInput(new { Pointer = new { x = (int)Math.Floor(pos.X * LayoutScale),
            y = (int)Math.Floor(pos.Y * LayoutScale), flags = InputFlags(), kind } });
    }

    private static int InputFlags()
    {
        var mods = Keyboard.Modifiers;
        return (Keyboard.IsKeyToggled(Key.CapsLock) ? 1 : 0)
             | (mods.HasFlag(ModifierKeys.Shift) ? 2 : 0)
             | (mods.HasFlag(ModifierKeys.Control) ? 4 : 0)
             | (mods.HasFlag(ModifierKeys.Alt) ? 8 : 0)
             | (Mouse.LeftButton == MouseButtonState.Pressed ? 16 : 0)
             | (Mouse.MiddleButton == MouseButtonState.Pressed ? 32 : 0)
             | (Mouse.RightButton == MouseButtonState.Pressed ? 64 : 0)
             | (mods.HasFlag(ModifierKeys.Windows) ? 128 : 0)
             | (Keyboard.IsKeyToggled(Key.NumLock) ? 256 : 0);
    }

    private void KeyEvent(KeyEventArgs e, bool pressed)
    {
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        if (key == Key.F12)
        {
            e.Handled = true;
            return;
        }
        if (key is Key.ImeProcessed or Key.DeadCharProcessed or Key.None) return;
        var vk = KeyInterop.VirtualKeyFromKey(key);
        if (vk == 0) return;
        var flags = InputFlags();
        if (key is >= Key.NumPad0 and <= Key.Divide) flags |= 512;
        if (key is Key.LeftShift or Key.LeftCtrl or Key.LeftAlt or Key.LWin) flags |= 1024;
        if (key is Key.RightShift or Key.RightCtrl or Key.RightAlt or Key.RWin) flags |= 2048;
        SendInput(new { Key = new { vk, flags, pressed } });
        // Let WPF translate printable keys and feed the IME. Consume keys which
        // would otherwise navigate the WPF control tree or invoke a TextBox command.
        e.Handled = !pressed || key is Key.Tab or Key.Escape or Key.Enter or Key.Back or Key.Delete
            or Key.Up or Key.Down or Key.Left or Key.Right or Key.PageUp or Key.PageDown or Key.Home or Key.End
            || Keyboard.Modifiers.HasFlag(ModifierKeys.Control) && !Keyboard.Modifiers.HasFlag(ModifierKeys.Alt);
    }

    private void Compose(TextCompositionEventArgs e)
    {
        var text = e.TextComposition.CompositionText;
        if (text.Length == 0 && !composing) return;
        composing = true;
        SendInput(new { Compose = text });
    }

    private static string Text(JsonElement source, string name) => source.Value(name) is
        { ValueKind: JsonValueKind.String } value ? value.GetString()! : "";

    private static bool Flag(JsonElement source, string name) => source.Value(name).ValueKind == JsonValueKind.True;
}
