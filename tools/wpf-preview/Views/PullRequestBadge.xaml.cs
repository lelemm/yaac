using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public sealed class PullRequestBadgeState
{
    internal PullRequestBadgeState(JsonElement value)
    {
        Signature = value.GetRawText();
        Number = value.Get("number").GetUInt64();
        Detail = value.Text("detail");
        Hint = value.Text("hint");
        Checks = value.Text("checks");
        Enabled = value.Flag("enabled");
        More = value.Text("more");
        Actions = SidebarAction.ReadList(value.Get("actions"));
        Others = value.Get("others").Items().Select(pr =>
            new Other(pr.Get("number").GetUInt64(), pr.Text("label"), pr.Flag("enabled"))).ToArray();
    }

    internal string Signature { get; }
    internal ulong Number { get; }
    internal string Detail { get; }
    internal string Hint { get; }
    internal string Checks { get; }
    internal bool Enabled { get; }
    internal string More { get; }
    internal IReadOnlyList<SidebarAction> Actions { get; }
    internal IReadOnlyList<Other> Others { get; }

    internal sealed record Other(ulong Number, string Label, bool Enabled);

    internal static PullRequestBadgeState? Read(JsonElement value) =>
        value.Get("number").ValueKind == JsonValueKind.Number ? new(value) : null;
}

public sealed class PullRequestBadgeEventArgs(string action, ulong number) : EventArgs
{
    public string Action { get; } = action;
    public ulong Number { get; } = number;
}

public partial class PullRequestBadge : UserControl
{
    public static readonly DependencyProperty BadgeProperty = DependencyProperty.Register(
        nameof(Badge), typeof(PullRequestBadgeState), typeof(PullRequestBadge),
        new PropertyMetadata(null, (d, e) => ((PullRequestBadge)d).UpdateBadge(
            (PullRequestBadgeState?)e.OldValue, (PullRequestBadgeState?)e.NewValue)));

    public static readonly DependencyProperty ThreadIdProperty = DependencyProperty.Register(
        nameof(ThreadId), typeof(string), typeof(PullRequestBadge),
        new PropertyMetadata("", (d, _) => ((PullRequestBadge)d).CloseFlyouts()));

    private static readonly DoubleCollection Dashes = new([3, 2]);
    private static readonly DoubleCollection Dots = new([1, 1.5]);
    private static readonly Geometry MergeMark = Geometry.Parse("M2,1 V7 M6,1 V2 Q6,4 2,5 M2,1 L1,2 M2,1 L3,2");
    private static readonly Geometry FailingMark = Geometry.Parse("M1,1 L7,7 M1,7 L7,1");
    private static readonly Geometry PendingMark = new EllipseGeometry(new Point(4, 4), 3, 3);

    private readonly DispatcherTimer closeTimer = new() { Interval = TimeSpan.FromMilliseconds(180) };
    private ContextMenu? menu;

    public PullRequestBadge()
    {
        InitializeComponent();
        closeTimer.Tick += (_, _) =>
        {
            closeTimer.Stop();
            if (!More.IsMouseOver && !OtherPanel.IsMouseOver && !OtherPanel.IsKeyboardFocusWithin)
                Others.IsOpen = false;
        };
        Unloaded += (_, _) => CloseFlyouts();
        IsVisibleChanged += (_, _) =>
        {
            if (!IsVisible)
                CloseFlyouts();
        };
    }

    public PullRequestBadgeState? Badge
    {
        get => (PullRequestBadgeState?)GetValue(BadgeProperty);
        set => SetValue(BadgeProperty, value);
    }

    public string ThreadId
    {
        get => (string)GetValue(ThreadIdProperty);
        set => SetValue(ThreadIdProperty, value);
    }

    public event EventHandler<PullRequestBadgeEventArgs>? Invoked;
    public event EventHandler? FlyoutChanged;
    internal bool IsFlyoutOpen => Others.IsOpen || menu?.IsOpen == true;
    internal bool IsFlyoutFocused => OtherPanel.IsKeyboardFocusWithin || menu?.IsKeyboardFocusWithin == true;

    internal static bool IsSource(object source)
    {
        for (var current = source as DependencyObject; current is not null; current = current is Visual
                 ? VisualTreeHelper.GetParent(current) : LogicalTreeHelper.GetParent(current))
        {
            if (current is PullRequestBadge)
                return true;
        }
        return false;
    }

    internal void CloseFlyouts()
    {
        closeTimer.Stop();
        Others.IsOpen = false;
        if (menu is { } open)
            open.IsOpen = false;
    }

    private void UpdateBadge(PullRequestBadgeState? old, PullRequestBadgeState? badge)
    {
        if (old?.Signature == badge?.Signature)
            return;
        if (old is null || badge is null || old.Number != badge.Number)
        {
            CloseFlyouts();
        }
        else
        {
            if (Others.IsOpen && !old.Others.SequenceEqual(badge.Others))
                Others.IsOpen = false;
            if (menu is { IsOpen: true } open && !old.Actions.SequenceEqual(badge.Actions))
                UpdateMenuItems(open, badge);
        }
        Visibility = badge is null ? Visibility.Collapsed : Visibility.Visible;
        if (badge is null)
            return;

        bool muted = badge.Hint is "merged" or "closed";
        Number.Text = badge.Number.ToString(System.Globalization.CultureInfo.InvariantCulture);
        Number.SetResourceReference(TextBlock.ForegroundProperty, muted ? "YaacMuted" : "YaacText");
        Number.TextDecorations = badge.Hint == "closed" ? TextDecorations.Strikethrough : null;
        Outline.StrokeDashArray = badge.Hint == "draft" ? Dashes : null;
        Outline.SetResourceReference(System.Windows.Shapes.Shape.StrokeProperty,
            badge.Hint == "draft" ? "YaacMuted" : "YaacRule");
        Mark.Data = badge.Hint == "merged" ? MergeMark : badge.Checks switch
        {
            "failing" => FailingMark,
            "pending" => PendingMark,
            _ => null,
        };
        Mark.StrokeDashArray = ReferenceEquals(Mark.Data, PendingMark) ? Dots : null;
        Mark.Visibility = Mark.Data is null ? Visibility.Collapsed : Visibility.Visible;
        Primary.ToolTip = badge.Detail.Tip();
        Primary.IsEnabled = badge.Enabled;
        AutomationProperties.SetName(Primary, $"Open pull request #{Number.Text}");
        AutomationProperties.SetHelpText(Primary, badge.Detail);

        More.Visibility = MoreRule.Visibility = badge.More == "" ? Visibility.Collapsed : Visibility.Visible;
        More.Content = badge.More;
        AutomationProperties.SetName(More, $"Other pull requests from this branch ({badge.Others.Count})");
    }

    protected override void OnPreviewMouseRightButtonUp(MouseButtonEventArgs e)
    {
        base.OnPreviewMouseRightButtonUp(e);
        OpenMenu(PlacementMode.MousePoint);
        e.Handled = true;
    }

    protected override void OnPreviewKeyDown(KeyEventArgs e)
    {
        base.OnPreviewKeyDown(e);
        if (e.Key == Key.Escape && Others.IsOpen)
        {
            if (OtherPanel.IsKeyboardFocusWithin)
                More.Focus();
            Others.IsOpen = false;
            e.Handled = true;
            return;
        }
        if (e.Key != Key.Apps && !(e.Key == Key.F10 && Keyboard.Modifiers == ModifierKeys.Shift))
            return;
        OpenMenu(PlacementMode.Bottom);
        e.Handled = true;
    }

    private void OpenMenu(PlacementMode placement)
    {
        if (Badge is not { } badge || menu?.IsOpen == true)
            return;
        Others.IsOpen = false;
        var opened = new ContextMenu { PlacementTarget = this, Placement = placement };
        UpdateMenuItems(opened, badge);
        opened.Closed += (_, _) =>
        {
            menu = null;
            FlyoutChanged?.Invoke(this, EventArgs.Empty);
        };
        menu = opened;
        opened.IsOpen = true;
        FlyoutChanged?.Invoke(this, EventArgs.Empty);
        Invoke("refresh", badge.Number);
    }

    private void UpdateMenuItems(ContextMenu opened, PullRequestBadgeState badge)
    {
        opened.Items.Clear();
        foreach (var action in badge.Actions)
        {
            var item = new MenuItem { Header = action.Label, IsEnabled = action.Enabled, ToolTip = action.ToolTip };
            ToolTipService.SetShowOnDisabled(item, true);
            item.Click += (_, _) => Invoke(action.Id, badge.Number);
            opened.Items.Add(item);
        }
    }

    private void OnPrimary(object sender, RoutedEventArgs e)
    {
        if (Badge is { Enabled: true } badge)
            Invoke("open", badge.Number);
        e.Handled = true;
    }

    private void OnMoreEnter(object sender, MouseEventArgs e)
    {
        closeTimer.Stop();
        if (sender == More)
            OpenOthers();
    }

    private void OnMoreLeave(object sender, MouseEventArgs e) => closeTimer.Start();
    private void OnOtherFocusLost(object sender, KeyboardFocusChangedEventArgs e) => closeTimer.Start();

    private void OnMoreClick(object sender, RoutedEventArgs e)
    {
        OpenOthers();
        OtherLinks.Children.OfType<Button>().FirstOrDefault(link => link.IsEnabled)?.Focus();
        e.Handled = true;
    }

    private void OpenOthers()
    {
        if (Others.IsOpen || Badge is not { Others.Count: > 0 } badge || menu?.IsOpen == true)
            return;
        OtherLinks.Children.Clear();
        foreach (var other in badge.Others)
        {
            var link = new Button
            {
                Padding = new Thickness(5, 1, 5, 1),
                MinHeight = 20,
                HorizontalContentAlignment = HorizontalAlignment.Left,
                IsEnabled = other.Enabled,
                ToolTip = other.Label,
                Content = new TextBlock { Text = other.Label, TextTrimming = TextTrimming.CharacterEllipsis }
            };
            link.SetResourceReference(Control.FontFamilyProperty, YaacFonts.UiResource);
            AutomationProperties.SetName(link, other.Label);
            link.Click += (_, e) =>
            {
                if (OtherPanel.IsKeyboardFocusWithin)
                    More.Focus();
                Others.IsOpen = false;
                Invoke("openOther", other.Number);
                e.Handled = true;
            };
            OtherLinks.Children.Add(link);
        }
        Others.IsOpen = true;
    }

    private void OnOtherKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Escape)
            return;
        More.Focus();
        Others.IsOpen = false;
        e.Handled = true;
    }

    private void OnPopupChanged(object? sender, EventArgs e) => FlyoutChanged?.Invoke(this, e);

    private void Invoke(string action, ulong number)
    {
        if (ThreadId.Length > 0)
            Invoked?.Invoke(this, new(action, number));
    }
}
