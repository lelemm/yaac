using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;
using Yaac.WpfPreview.Views;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private readonly Dictionary<(string Kind, string Id), FrameworkElement> railControls = new();
    private readonly DispatcherTimer railHoverTimer = new() { Interval = TimeSpan.FromMilliseconds(250) };
    private JsonElement railSections;
    private ContentControl railPreview = null!;
    private Button? railHoverAnchor;
    private SidebarItem? RailHoverRow => railPreview.Content as SidebarItem;
    private string? VisibleRailHoverThreadId => RailHover.IsOpen ? RailHoverRow?.Id : null;

    private void InitializeRail()
    {
        railPreview = SidebarPane.CreateRailPreview();
        RailHoverHost.Content = railPreview;
        SidebarPane.RailPrFlyoutChanged += () =>
        {
            if (RailHover.IsOpen && RailPrBadge?.IsFlyoutOpen != true)
                railHoverTimer.Start();
        };
        RailHover.CustomPopupPlacementCallback = (popup, target, _) =>
        [
            new(new Point(target.Width + 7, (target.Height - popup.Height) / 2), PopupPrimaryAxis.Vertical),
            new(new Point(-popup.Width - 7, (target.Height - popup.Height) / 2), PopupPrimaryAxis.Vertical),
        ];
        railHoverTimer.Tick += (_, _) =>
        {
            railHoverTimer.Stop();
            if (railHoverAnchor?.IsMouseOver != true && !RailHoverCard.IsMouseOver &&
                railHoverAnchor?.ContextMenu?.IsOpen != true && RailHoverCard.ContextMenu?.IsOpen != true &&
                RailPrBadge?.IsFlyoutOpen != true)
                CloseRailHover();
        };
        RailHoverCard.MouseLeftButtonUp += (_, e) =>
        {
            if (RailHoverRow is not { } row) return;
            OpenRailThread(row);
            e.Handled = true;
        };
        RailHoverCard.MouseRightButtonUp += (_, e) =>
        {
            if (RailHoverRow is not { } row) return;
            OpenRailRowMenu(row, RailHoverCard, PlacementMode.MousePoint);
            e.Handled = true;
        };
        SidebarRail.IsVisibleChanged += (_, _) =>
        {
            if (!SidebarRail.IsVisible) CloseRailHover();
        };
        Deactivated += (_, _) =>
        {
            if (railHoverAnchor?.ContextMenu?.IsOpen != true && RailHoverCard.ContextMenu?.IsOpen != true &&
                RailPrBadge?.IsFlyoutOpen != true)
                CloseRailHover();
        };
    }

    private void SetRailData(JsonElement sidebar)
    {
        RailExpand.IsEnabled = SidebarPane.SearchEnabled;
        var sections = sidebar.Get("railSections");
        if (sections.ValueKind != JsonValueKind.Array)
            sections = sidebar.Get("sections");
        if (sections.ValueKind != JsonValueKind.Array ||
            (railSections.ValueKind == JsonValueKind.Array && JsonElement.DeepEquals(railSections, sections)))
            return;
        railSections = sections.Clone();
        if (sidebarCollapsed) RenderRail();
    }

    private void RenderRail()
    {
        var next = new HashSet<(string Kind, string Id)>();
        int index = 0;
        bool rule = false;
        foreach (var section in railSections.Items())
        {
            var id = section.Text("id");
            if (rule)
            {
                Place("rule", id, () => new Border
                {
                    Width = 24, Height = 1, Margin = new Thickness(0, 4, 0, 4),
                    Background = Palette.Brush("YaacRule"),
                });
                rule = false;
            }

            var header = section.Get("header");
            bool collapsed = section.Flag("collapsed");
            if (header.ValueKind == JsonValueKind.Object)
            {
                var heading = (Button)Place("header", id, () => CreateRailHeading(id));
                var text = header.Text("label").Split(" · ", 2)[0];
                var label = (TextBlock)heading.Content;
                label.Text = id == "pinned" ? (collapsed ? "\uF435  ▸" : "\uF435") : (collapsed ? "▸  " : "") + text;
                heading.ToolTip = collapsed ? $"{text} (collapsed)" : text;
                AutomationProperties.SetName(heading, $"{(collapsed ? "Expand" : "Collapse")} {text}");
                heading.IsEnabled = header.Get("toggle").Flag("enabled");
                bool canStart = header.Get("actions").Items()
                    .Any(action => action.Text("id") == "section.newThread" && action.Flag("enabled"));
                if (canStart && heading.ContextMenu is null)
                {
                    var item = new MenuItem { Header = "New Thread" };
                    item.Click += (_, _) => Send("sidebar.action", new { action = "section.newThread", sectionId = id });
                    heading.ContextMenu = new ContextMenu { Items = { item } };
                }
                else if (!canStart && heading.ContextMenu is not null)
                    heading.ClearValue(ContextMenuProperty);
            }
            if (collapsed) continue;

            foreach (var data in section.Get("rows").Items())
            {
                var row = new SidebarItem(SidebarItemKind.Row, data.Text("id"));
                var button = (Button)Place("row", row.Id, () => CreateRailButton(row));
                row = (SidebarItem)button.Content;
                row.Update(data, id);
                if (RailHoverRow == row) railPreview.Height = row.Height;
                AutomationProperties.SetName(button, row.State.Title);
                AutomationProperties.SetItemStatus(button, row.State.AccessibilityStatus);
                if (row.State.Selected) button.SetResourceReference(Control.BackgroundProperty, "YaacSelected");
                else button.ClearValue(Control.BackgroundProperty);
                rule = true;
            }
        }

        while (RailItems.Children.Count > index)
            RailItems.Children.RemoveAt(index);
        foreach (var key in railControls.Keys.Where(key => !next.Contains(key)).ToArray())
        {
            if (railControls[key] == railHoverAnchor) CloseRailHover();
            railControls.Remove(key);
        }

        FrameworkElement Place(string kind, string id, Func<FrameworkElement> create)
        {
            var key = (kind, id);
            if (!railControls.TryGetValue(key, out var control))
                railControls.Add(key, control = create());
            if (!next.Add(key)) return control;
            if (index == RailItems.Children.Count || RailItems.Children[index] != control)
            {
                if (RailItems.Children.Contains(control))
                {
                    if (control == railHoverAnchor) CloseRailHover();
                    RailItems.Children.Remove(control);
                }
                RailItems.Children.Insert(index, control);
            }
            index++;
            return control;
        }
    }

    private Button CreateRailHeading(string id)
    {
        var label = new TextBlock { FontSize = 10.5, MaxWidth = 110, TextTrimming = TextTrimming.CharacterEllipsis };
        label.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
        if (id == "pinned") label.FontSize = 14;
        else label.LayoutTransform = new RotateTransform(-90);
        var heading = new Button
        {
            Style = (Style)FindResource("RailButton"),
            Padding = new Thickness(0, 5, 0, 5),
            MinHeight = 22,
            Content = label,
        };
        heading.Click += (_, _) => Send("sidebar.railToggle", new { sectionId = id });
        return heading;
    }

    private Button CreateRailButton(SidebarItem row)
    {
        var button = new Button
        {
            Style = (Style)FindResource("RailButton"),
            Height = 30,
            ContentTemplate = (DataTemplate)FindResource("RailThreadMark"),
            Content = row,
        };
        button.Click += (_, _) => OpenRailThread(row);
        button.MouseEnter += (_, _) => ShowRailHover(button, row);
        button.MouseMove += (_, _) =>
        {
            if (!RailHover.IsOpen && button.ContextMenu?.IsOpen != true) ShowRailHover(button, row);
        };
        button.MouseLeave += RailHoverLeave;
        button.MouseRightButtonUp += (_, e) =>
        {
            OpenRailRowMenu(row, button, PlacementMode.MousePoint);
            e.Handled = true;
        };
        button.PreviewKeyDown += (_, e) =>
        {
            if (e.Key != Key.Apps && !(e.Key == Key.F10 && Keyboard.Modifiers == ModifierKeys.Shift)) return;
            OpenRailRowMenu(row, button, PlacementMode.Right);
            e.Handled = true;
        };
        return button;
    }

    private void OpenRailThread(SidebarItem row)
    {
        CloseRailHover();
        Send("sidebar.open", new { id = row.Id });
    }

    private void OpenRailRowMenu(SidebarItem row, FrameworkElement anchor, PlacementMode placement)
    {
        SidebarPane.OpenRailMenu(row, anchor, placement);
        if (anchor.ContextMenu is { IsOpen: true } menu)
            menu.Closed += (_, _) => railHoverTimer.Start();
    }

    private void ShowRailHover(Button anchor, SidebarItem row)
    {
        if (!SidebarRail.IsVisible || RailHoverCard.ContextMenu?.IsOpen == true ||
            railHoverAnchor?.ContextMenu?.IsOpen == true)
            return;
        railHoverTimer.Stop();
        if (RailHover.IsOpen && railHoverAnchor == anchor) return;
        railHoverAnchor = anchor;
        railPreview.Height = row.Height;
        railPreview.Content = row;
        RailHover.PlacementTarget = anchor;
        RailHover.IsOpen = true;
        QueueVisiblePrRows();
    }

    private PullRequestBadge? RailPrBadge => SidebarPane.RailPrBadge(railPreview);

    private void CloseRailHover()
    {
        RailPrBadge?.CloseFlyouts();
        if (railHoverAnchor?.ContextMenu is { } anchorMenu) anchorMenu.IsOpen = false;
        railHoverAnchor = null;
        if (RailHoverCard.ContextMenu is { } menu) menu.IsOpen = false;
        RailHover.IsOpen = false;
        railHoverTimer.Stop();
        railPreview.Content = null;
    }

    private void RailHoverEnter(object sender, MouseEventArgs e) => railHoverTimer.Stop();
    private void RailHoverLeave(object sender, MouseEventArgs e) => railHoverTimer.Start();
    private void RailHoverChanged(object sender, EventArgs e) => QueueVisiblePrRows();

    private void RailScrolled(object sender, ScrollChangedEventArgs e)
    {
        if (RailHover.IsOpen && e.VerticalChange != 0)
            CloseRailHover();
    }
}
