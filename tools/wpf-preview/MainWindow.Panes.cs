using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    // The controller uses the same limits in yaac_core::strip and yaac_core::prefs.
    private const double NarrowWorkspaceWidth = 1000;
    private const double SidebarStripWidth = 48;
    private const double SidebarMinWidth = 220;
    private const double SidebarMaxWidth = 480;
    private const double SidebarCollapseBelow = 160;
    private const double DockMinWidth = 240;
    private const double DockMaxWidth = 520;

    private bool paneSizingReady;
    private bool sidebarCollapsed;
    private bool sidebarPreference;
    private bool narrowSidebarOpen;
    private double sidebarWidth = 280;
    private double dockWidth = 300;
    private PaneDrag? paneDrag;
    private sealed record PaneDrag(bool Sidebar, double Origin, double Width);

    private bool NarrowWorkspace => Workspace.ActualWidth is > 0 and < NarrowWorkspaceWidth;

    private void RenderPaneSizing(JsonElement panes)
    {
        sidebarPreference = Flag(panes, "sidebarCollapsed");
        sidebarWidth = panes.GetProperty("sidebarWidth").GetDouble();
        dockWidth = panes.GetProperty("dockWidth").GetDouble();
        paneSizingReady = true;
        UpdatePaneLayout();
    }

    private void UpdatePaneLayout()
    {
        if (!NarrowWorkspace) narrowSidebarOpen = false;
        var requested = paneDrag is { Sidebar: true } drag ? DraggedWidth(drag) : null;
        bool collapsed = NarrowWorkspace ? !narrowSidebarOpen
            : requested is double width ? width < SidebarCollapseBelow : sidebarPreference;
        SidebarColumn.Width = new GridLength(collapsed
            ? SidebarStripWidth
            : Math.Clamp(requested ?? sidebarWidth, SidebarMinWidth, SidebarMaxWidth));
        SidebarSplitter.IsHitTestVisible = paneSizingReady && !NarrowWorkspace;
        if (collapsed != sidebarCollapsed)
        {
            sidebarCollapsed = collapsed;
            SidebarPane.Visibility = collapsed ? Visibility.Collapsed : Visibility.Visible;
            SidebarRail.Visibility = collapsed ? Visibility.Visible : Visibility.Collapsed;
            if (collapsed) RenderRail();
            else CloseRailHover();
        }
        PositionFirstRun();
        UpdateDockLayout();
    }

    private void OpenNarrowSidebar()
    {
        narrowSidebarOpen = true;
        UpdatePaneLayout();
    }

    private void CloseNarrowSidebar()
    {
        if (!narrowSidebarOpen) return;
        narrowSidebarOpen = false;
        UpdatePaneLayout();
    }

    private void ToggleNarrowSidebar()
    {
        if (narrowSidebarOpen)
        {
            bool focused = SidebarPane.IsKeyboardFocusWithin;
            CloseNarrowSidebar();
            if (focused) RailExpand.Focus();
        }
        else
        {
            bool focused = RailExpand.IsKeyboardFocused;
            OpenNarrowSidebar();
            if (focused) SidebarPane.CollapseButton.Focus();
        }
    }

    private void SidebarClicked(object sender, RoutedEventArgs args)
    {
        if (NarrowWorkspace) ToggleNarrowSidebar();
        else Send("shell.sidebar");
    }

    private void UpdateDockLayout()
    {
        double room = Math.Max(0, ThreadColumns.ActualWidth - 5);
        bool split = BrowserPane.Visibility == Visibility.Visible && BrowserDivider.Visibility == Visibility.Visible;
        const double minimumPane = 280, minimumChat = 360, pillGutter = 64;
        BrowserColumn.MinWidth = split ? Math.Min(minimumPane, Math.Max(0, room - minimumChat)) : 0;
        BrowserChatColumn.MinWidth = split ? Math.Min(minimumChat, room) : 0;

        // Fold the dock before shrinking the conversation, and preserve the saved dock preference.
        var available = ChatRegion.ActualWidth;
        AgentsPane.SetDockFolded(NarrowWorkspace || split && available < minimumChat + DockMinWidth);
        bool docked = AgentsPane.IsDocked && AgentsPane.Visibility == Visibility.Visible;
        var requested = paneDrag is { Sidebar: false } drag ? DraggedWidth(drag) : null;
        var width = Math.Clamp(requested ?? dockWidth, DockMinWidth, DockMaxWidth);
        DockColumn.Width = new GridLength(AgentsPane.Visibility != Visibility.Visible ? 0 : docked
            ? Math.Min(width, Math.Min(Workspace.ActualWidth * .4, Math.Max(DockMinWidth, available - minimumChat)))
            : Math.Min(pillGutter, available));
        DockSplitter.Visibility = docked ? Visibility.Visible : Visibility.Collapsed;
        DockSplitter.IsHitTestVisible = paneSizingReady;
        AgentsPane.HorizontalAlignment = docked ? HorizontalAlignment.Stretch : HorizontalAlignment.Center;
        AgentsPane.VerticalAlignment = docked ? VerticalAlignment.Stretch : VerticalAlignment.Center;
        AgentsPane.Margin = new Thickness(0);
    }

    private double? DraggedWidth(PaneDrag drag)
    {
        // Relative to the workspace so moving the thumb during a drag does not move its origin.
        var delta = Mouse.GetPosition(Workspace).X - drag.Origin;
        return delta == 0 ? null : drag.Width + (drag.Sidebar ? delta : -delta);
    }

    private void PaneDragStarted(object sender, DragStartedEventArgs e)
    {
        bool sidebar = sender == SidebarSplitter;
        var width = sidebar ? SidebarColumn.Width.Value : DockColumn.Width.Value;
        paneDrag = new PaneDrag(sidebar, Mouse.GetPosition(Workspace).X, width);
    }

    private void PaneDragDelta(object sender, DragDeltaEventArgs e) => UpdatePaneLayout();

    private void PaneDragCompleted(object sender, DragCompletedEventArgs e)
    {
        var drag = paneDrag;
        paneDrag = null;
        if (!e.Canceled && !NarrowWorkspace && drag is not null && DraggedWidth(drag) is double width)
        {
            if (drag.Sidebar)
            {
                sidebarPreference = width < SidebarCollapseBelow;
                if (!sidebarPreference) sidebarWidth = Math.Clamp(width, SidebarMinWidth, SidebarMaxWidth);
                Send("shell.resizeSidebar", new { width });
            }
            else
            {
                dockWidth = Math.Clamp(width, DockMinWidth, DockMaxWidth);
                Send("agents.resizeDock", new { width });
            }
        }
        UpdatePaneLayout();
    }
}
