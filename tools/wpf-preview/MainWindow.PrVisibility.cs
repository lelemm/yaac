using System.Windows;
using System.Windows.Threading;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private DispatcherOperation? pendingPrVisibility;
    private string[] visiblePrRows = [];
    private bool agentRowsVisible;
    private string? requestVisibility;

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

        var cards = IsActive && IsVisible && WindowState != WindowState.Minimized && !settingsOpen
            ? (TranscriptPreview.IsVisible ? TranscriptPreviewPane : TranscriptPane).VisibleRequests().ToArray() : [];
        var fingerprint = System.Text.Json.JsonSerializer.Serialize(cards);
        if (requestVisibility != fingerprint)
        {
            requestVisibility = fingerprint;
            Send("shell.visibleRequests", new { cards });
        }
        string[] rows = [];
        bool agentsVisible = false;
        if (IsVisible && WindowState != WindowState.Minimized && !settingsOpen && !firstRun)
        {
            var visible = new SortedSet<string>(SidebarPane.VisiblePrRows(), StringComparer.Ordinal);
            visible.UnionWith(AgentsPane.VisiblePrRows());
            agentsVisible = AgentsPane.AreRowsVisible;
            if (SidebarRail.IsVisible && VisibleRailHoverThreadId is { } railThread)
                visible.Add(railThread);
            rows = visible.ToArray();
        }
        if (agentRowsVisible == agentsVisible && visiblePrRows.SequenceEqual(rows))
            return;

        visiblePrRows = rows;
        agentRowsVisible = agentsVisible;
        Send("shell.prVisible", new { ids = rows, agentsVisible });
    }
}
