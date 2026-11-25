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
    private bool browserActive;
    private string filesReason = "Open a thread to use Files.";
    private readonly FilesPane files = new();
    private string displayedUrl = "";
    private string tabSignature = "", portSignature = "";
    private (ulong Tab, int Width, int Height, double Dpi)? viewport;
    private readonly Dictionary<ulong, BitmapImage> icons = new();

    internal bool OwnsKeyboard => PageInput.IsKeyboardFocused || AddressField.IsKeyboardFocused || files.OwnsKeyboard;

    public BrowserView()
    {
        InitializeComponent();
        FilesContent.Content = files;
        files.Command += (command, data) =>
        {
            if (command == "fullScreen")
                Command?.Invoke("browser.fullScreen", new { thread, fullScreen = !full });
            else if (active is ulong tab)
            {
                if (command == "add")
                    Command?.Invoke("files.add", new { thread, tab, data, caret = ComposerCaret?.Invoke() });
                else
                    Command?.Invoke("files." + command, new { thread, tab, data });
            }
        };
        PreviewKeyDown += (_, e) =>
        {
            if (!browserActive || e.Key != Key.F12 || !Inspect.IsEnabled) return;
            if (!e.IsRepeat) Send("inspect");
            e.Handled = true;
        };
        Back.Click += (_, _) => SendInput("Back");
        Forward.Click += (_, _) => SendInput("Forward");
        Reload.Click += (_, _) => SendInput(busy ? "Stop" : "Reload");
        External.Click += (_, _) => Send("external");
        Inspect.Click += (_, _) => Send("inspect");
        Fullscreen.Click += (_, _) => Command?.Invoke("browser.fullScreen", new { thread, fullScreen = !full });

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
        bool switched = next != active || nextThread != thread;
        bool hadPageFocus = PageInput.IsKeyboardFocused;
        if (switched)
        {
            focusWhenConnected = false;
            DismissPageUi();
            active = next;
            viewport = null;
            Page.Source = null;
            Surface.Cursor = Cursors.Arrow;
        }

        thread = nextThread;
        full = Flag(state, "fullScreen");
        Fullscreen.Content = full ? "Restore" : "Full screen";
        var tabs = state.GetProperty("tabs");
        tabState = tabs.Clone();
        tabReason = Text(state, "reason");
        filesReason = Text(state, "filesReason");
        RenderTabs(tabs, tabReason, switched);
        var current = active is ulong tabId
            ? tabs.EnumerateArray().FirstOrDefault(t => t.GetProperty("id").GetUInt64() == tabId)
            : default;
        var kind = Text(current, "kind");
        browserActive = active is not null && (kind is "" or "browser");
        BrowserToolbar.Visibility = browserActive ? Visibility.Visible : Visibility.Collapsed;
        if (!browserActive)
        {
            Surface.Visibility = Landing.Visibility = Visibility.Collapsed;
            FilesContent.Visibility = active is null ? Visibility.Collapsed : Visibility.Visible;
            if (active is not null) files.Render(state, current, thread, full);
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
        ErrorBanner.Text = error;
        ErrorBanner.Visibility = hasSurface && error.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        Surface.Visibility = hasSurface ? Visibility.Visible : Visibility.Collapsed;
        Landing.Visibility = hasSurface ? Visibility.Collapsed : Visibility.Visible;
        Reload.IsEnabled = hasSurface;
        Reload.Content = busy ? "×" : "⟳";
        Reload.ToolTip = busy ? "Stop" : "Reload";
        AutomationProperties.SetName(Reload, (string)Reload.ToolTip);
        Back.IsEnabled = Flag(current, "back");
        Forward.IsEnabled = Flag(current, "forward");
        External.IsEnabled = url.Length > 0;
        Inspect.IsEnabled = hasSurface;

        if (!hasSurface)
            RenderPorts(state, error.Length > 0 ? error : url.Length > 0 ? "Connecting…" : Text(state, "portsError"));
        if (switched && hasSurface && hadPageFocus) SendInput(new { Focus = true });
        if (hasSurface && focusWhenConnected)
        {
            focusWhenConnected = false;
            if (AddressField.IsKeyboardFocused) FocusPage();
        }
        UpdateViewport();
        if (focusNewTab && switched && url.Length == 0)
        {
            focusNewTab = false;
            Dispatcher.BeginInvoke(DispatcherPriority.Input, () =>
            {
                if (active == tab && IsVisible)
                {
                    AddressField.Focus();
                    AddressField.SelectAll();
                }
            });
        }
    }

    private void RenderTabs(JsonElement tabs, string reason, bool bringSelectedIntoView)
    {
        // Address and loading updates do not replace tab controls or steal their focus.
        var ids = new HashSet<ulong>();
        var labels = new List<(ulong Id, string Title, string AgentLogo, string AgentLogoColor, string Path)>();
        foreach (var tab in tabs.EnumerateArray())
        {
            var id = tab.GetProperty("id").GetUInt64();
            var title = Text(tab, "title");
            if (title.Length == 0) title = Text(tab, "url");
            labels.Add((id, title.Length == 0 ? "New tab" : title, Text(tab, "agentLogo"), Text(tab, "agentLogoColor"), Text(tab, "path")));
            ids.Add(id);
        }
        foreach (var id in icons.Keys.Where(id => !ids.Contains(id)).ToArray()) icons.Remove(id);
        var signature = JsonSerializer.Serialize(new
        {
            active, reason, filesReason, tabs = labels.Select(t => new { t.Id, t.Title, t.AgentLogo, t.AgentLogoColor, t.Path }), favicon = icons.Keys
        });
        if (signature == tabSignature) return;
        tabSignature = signature;
        TabStrip.Children.Clear();

        foreach (var (id, title, agentLogo, agentLogoColor, path) in labels)
        {
            var row = new DockPanel { LastChildFill = true, Margin = new Thickness(0, 0, 2, 0) };
            if (active == id) row.SetResourceReference(BackgroundProperty, "YaacSelected");
            row.PreviewMouseDown += (_, e) =>
            {
                if (e.ChangedButton != MouseButton.Middle) return;
                SendFor(id, "close");
                e.Handled = true;
            };
            var close = QuietButton("×", $"Close {title}");
            DockPanel.SetDock(close, Dock.Right);
            close.Click += (_, _) => SendFor(id, "close");
            row.Children.Add(close);

            var label = new DockPanel();
            if (icons.TryGetValue(id, out var icon))
                label.Children.Add(new Image { Source = icon, Width = 14, Height = 14, Margin = new Thickness(0, 0, 5, 0) });
            if (agentLogo.Length > 0)
            {
                var mark = new System.Windows.Shapes.Path
                {
                    Data = Geometry.Parse(agentLogo), Width = 14, Height = 14, Stretch = Stretch.Uniform,
                    Margin = new Thickness(0, 0, 5, 0),
                };
                if (agentLogoColor.Length > 0)
                    mark.Fill = new SolidColorBrush((Color)ColorConverter.ConvertFromString(agentLogoColor));
                else
                    mark.SetResourceReference(System.Windows.Shapes.Path.FillProperty, "YaacText");
                label.Children.Add(mark);
            }
            label.Children.Add(new TextBlock { Text = title, MaxWidth = 156, TextTrimming = TextTrimming.CharacterEllipsis });
            var pick = QuietButton(label, title);
            if (path.Length > 0) pick.ToolTip = path;
            if (agentLogo.Length > 0)
            {
                const string status = "Agent is using this tab";
                pick.ToolTip = $"{title} · {status}";
                AutomationProperties.SetItemStatus(pick, status);
            }
            pick.Click += (_, _) => SendFor(id, "select");
            row.Children.Add(pick);
            TabStrip.Children.Add(row);
            if (bringSelectedIntoView && active == id)
                Dispatcher.BeginInvoke(DispatcherPriority.Loaded, () =>
                {
                    if (active == id) row.BringIntoView();
                });
        }
        var add = QuietButton("+", "New tab");
        add.IsEnabled = reason.Length == 0 || filesReason.Length == 0;
        if (!add.IsEnabled) add.ToolTip = reason;
        ToolTipService.SetShowOnDisabled(add, true);
        add.Click += (_, _) => OpenAddMenu(add);
        TabStrip.Children.Add(add);
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
            ToolTipService.SetShowOnDisabled(item, true);
            item.Click += (_, _) => choose();
            menu.Items.Add(item);
        }
    }

    internal void ReceiveFileDocument(IncomingFileDocument document) => files.ReceiveDocument(document);
    internal void FileDocumentFailed(ulong id) => files.DocumentFailed(id);
    internal void NavigateDiff(ulong id, int row) => files.Navigate(id, row);

    internal void OpenNewTab()
    {
        focusNewTab = true;
        Command?.Invoke("browser.new", new { thread });
    }

    private void RenderPorts(JsonElement state, string status)
    {
        var ports = state.GetProperty("ports");
        var signature = $"{active}:{status}:{ports.GetRawText()}";
        if (signature == portSignature) return;
        portSignature = signature;
        LandingStatus.Text = status.Length > 0 ? status :
            ports.GetArrayLength() > 0 ? "Listening ports" : "No listening ports found";
        PortList.Children.Clear();
        foreach (var port in ports.EnumerateArray())
        {
            var number = port.GetProperty("port").GetInt32();
            var command = Text(port, "command");
            var cwd = Text(port, "cwd");
            var label = command.Length == 0 ? $"{number}" : $"{number}   {command}";
            var button = QuietButton(new TextBlock { Text = label, TextTrimming = TextTrimming.CharacterEllipsis }, label);
            if (cwd.Length > 0) button.ToolTip = cwd;
            button.HorizontalContentAlignment = HorizontalAlignment.Left;
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
