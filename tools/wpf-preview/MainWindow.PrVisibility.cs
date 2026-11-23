using System.Windows;
using System.Windows.Threading;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private DispatcherOperation? pendingPrVisibility;
    private string[] visiblePrRows = [];

    private void QueueVisiblePrRows()
    {
        if (pendingPrVisibility is { Status: DispatcherOperationStatus.Pending })
            return;
        pendingPrVisibility = Dispatcher.InvokeAsync(ReportVisiblePrRows, DispatcherPriority.Background);
    }

    private void ReportVisiblePrRows()
    {
        if (closed || controller is null)
            return;

        string[] rows = [];
        if (IsVisible && WindowState != WindowState.Minimized && !settingsOpen && !firstRun)
        {
            var visible = new SortedSet<string>(SidebarPane.VisiblePrRows(), StringComparer.Ordinal);
            visible.UnionWith(AgentsPane.VisiblePrRows());
            if (SidebarRail.IsVisible && VisibleRailHoverThreadId is { } railThread)
                visible.Add(railThread);
            rows = visible.ToArray();
        }
        if (visiblePrRows.SequenceEqual(rows))
            return;

        visiblePrRows = rows;
        Send("shell.prVisible", new { ids = rows });
    }
}
