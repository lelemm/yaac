using System.Collections.ObjectModel;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class SidebarView : UserControl
{
    private const string DragFormat = "Yaac.WpfPreview.SidebarThreads";
    private const double HeaderHeight = 26;
    private readonly ObservableCollection<SidebarItem> items = [];
    private readonly Dictionary<(SidebarItemKind, string), SidebarItem> models = [];
    private readonly HashSet<SidebarItem> selected = [];
    private readonly SidebarActions grouping = new();
    private readonly SidebarActions ordering = new();
    private readonly SidebarActions bulk = new();
    private readonly SidebarActions footer = new();
    private readonly SidebarActions notice = new();
    private readonly SidebarItem searchError = new(SidebarItemKind.Status, "searchError");
    private readonly SidebarItem searchStatus = new(SidebarItemKind.Status, "searchStatus");
    private readonly SidebarItem searchEmpty = new(SidebarItemKind.Status, "searchEmpty");
    private readonly SidebarItem searchMore = new(SidebarItemKind.More, "searchMore");
    private static readonly SidebarAction LocalClear = new("clearSelection", "Clear selection",
        true, null, "Esc", []);

    private List<SidebarItem> desired = [];
    private Dictionary<string, SidebarAction> dropActions = new(StringComparer.Ordinal);
    private readonly Dictionary<SidebarItem, (string Section, int Rank)> shownRanks = [];
    private SidebarItem[] rows = [];
    private double[] tops = [0];
    private int[] headerIndices = [];
    private string[] sentSelection = [];
    private SidebarItem? rangeAnchor;
    private SidebarItem? rangeHead;
    private SidebarItem? pressedRow;
    private Point pressedAt;
    private ModifierKeys pressedModifiers;
    private string[]? dragIds;
    private string? dropSection;
    private SidebarItem[] highlighted = [];
    private ScrollViewer? scroll;
    private ScrollContentPresenter? scrollContent;
    private ContextMenu? activeMenu;
    private Action? refreshActiveMenu;
    private bool menuHoldsLayout;
    private bool syncingSearch;
    private bool syncingSelection;
    private bool searchResults;
    private string? pendingSearch;
    private string? selectionReset;
    private SidebarAction? newThread;
    private string noticeLabel = "";
    private string noticeDetail = "";
    private DispatcherOperation? restoreOffset;
    private JsonElement lastView;

    public SidebarView()
    {
        InitializeComponent();
        Threads.ItemsSource = items;
        Threads.SelectionChanged += OnNativeSelectionChanged;
    }

    public event Action<string, object?>? Command;

    internal void ObservePrRows(Action changed) => VisibleListRows.Observe(Threads, changed);

    internal IEnumerable<string> VisiblePrRows()
    {
        if (searchResults)
            return [];
        var coveredTop = StickyHeader.IsVisible ? Math.Max(0, StickyHeader.ActualHeight + StickyOffset.Y) : 0;
        return VisibleListRows.Read(Threads, coveredTop).OfType<SidebarItem>()
            .Where(item => item.Kind == SidebarItemKind.Row).Select(item => item.Id);
    }
    internal bool SearchEnabled => SearchBox.IsEnabled;
    internal bool SearchHasText => SearchBox.Text.Length > 0;

    private JsonElement hubs;

    /// <summary>The desktop's saved hubs (shell.hubs, P412): the badge left of the mark, only
    /// with more than one hub saved, and its drop-down.</summary>
    internal void RenderHubs(JsonElement view)
    {
        hubs = view.ValueKind == JsonValueKind.Object ? view.Clone() : default;
        var badge = hubs.Text("badge");
        HubBadge.Visibility = VisibilityOf(badge.Length > 0);
        HubBadge.Content = badge;
        HubBadge.FontSize = badge.Length > 1 ? 9.5 : 11;
        var current = hubs.Get("list").Items().FirstOrDefault(h => h.Flag("selected"));
        HubBadge.ToolTip = $"Hub: {current.Text("name")}. Switch hub";
    }

    private void OnHubBadge(object sender, RoutedEventArgs e)
    {
        if (hubs.ValueKind != JsonValueKind.Object)
            return;
        var menu = new ContextMenu { PlacementTarget = HubBadge, Placement = PlacementMode.Bottom };
        if (TryFindResource("YaacContextMenu") is Style menuStyle)
            menu.Style = menuStyle;
        var itemStyle = TryFindResource("YaacMenuItem") as Style;
        foreach (var hub in hubs.Get("list").Items())
        {
            var index = hub.Get("index").GetInt32();
            var chosen = hub.Flag("selected");
            var item = new MenuItem
            {
                Header = $"{hub.Text("name")}   {hub.Text("address")} · {hub.Text("status")}".Replace("_", "__"),
                IsChecked = chosen,
            };
            if (itemStyle is not null)
                item.Style = itemStyle;
            item.Click += (_, _) =>
            {
                if (!chosen)
                    Command?.Invoke("hubs.select", new { index });
            };
            menu.Items.Add(item);
        }
        menu.Items.Add(new Separator());
        var all = new MenuItem { Header = "Hubs…" };
        if (itemStyle is not null)
            all.Style = itemStyle;
        all.Click += (_, _) => Command?.Invoke("hubs.open", new { });
        menu.Items.Add(all);
        menu.Closed += (_, _) => HubBadge.ClearValue(ContextMenuProperty);
        HubBadge.ContextMenu = menu;
        menu.IsOpen = true;
    }

    public void Render(JsonElement view)
    {
        if (view.ValueKind == JsonValueKind.Object && lastView.ValueKind == JsonValueKind.Object
            && JsonElement.DeepEquals(view, lastView))
            return;
        lastView = view.ValueKind == JsonValueKind.Object ? view.Clone() : default;
        RenderSearch(view);
        newThread = SidebarAction.Read(view.Get("newThread"));
        SetButton(NewButton, newThread);
        grouping.Update(view.Get("grouping"));
        ordering.Update(view.Get("ordering"));
        GroupButton.Visibility = VisibilityOf(SearchBox.IsEnabled && grouping.Items.Count > 0);
        OrderButton.Visibility = VisibilityOf(SearchBox.IsEnabled && ordering.Items.Count > 0);

        var footerData = view.Get("footer");
        var footerLabel = footerData.Text("label");
        bool footerChanged = footer.Update(footerData.Get("actions"));
        if (footerChanged)
            FooterActions.ItemsSource = Buttons(footer.Items);
        FooterLabel.Text = footerLabel;
        FooterLabel.ToolTip = footerData.Text("detail").Tip();
        Usage.Render(view.Get("usage"));
        SidebarFooter.Visibility = VisibilityOf(footerLabel.Length > 0 || footer.Items.Count > 0);

        var noticeData = view.Get("notice");
        noticeLabel = noticeData.Text("label");
        noticeDetail = noticeData.Text("detail");
        bulk.Update(view.Get("bulkActions"));
        notice.Update(noticeData.Get("actions"));
        bool wasSearching = searchResults;
        searchResults = view.Flag("searchResults");
        var reset = view.Get("selectionReset");
        string nextReset = reset.ValueKind == JsonValueKind.Number
            ? reset.GetRawText() : view.Text("selectionReset");
        if ((nextReset.Length > 0 && nextReset != selectionReset) || (searchResults && !wasSearching))
        {
            rangeAnchor = rangeHead = null;
            UpdateSelection([], notify: false);
        }
        selectionReset = nextReset;

        var next = new List<SidebarItem>();
        var nextDrops = new Dictionary<string, SidebarAction>(StringComparer.Ordinal);
        var seen = new HashSet<(SidebarItemKind, string)>();
        var error = view.Text("searchError");
        if (error.Length > 0)
        {
            searchError.UpdateStatus(error, tone: "failed");
            next.Add(searchError);
        }
        var status = view.Text("searchStatus");
        if (status.Length > 0)
        {
            searchStatus.UpdateStatus(status);
            next.Add(searchStatus);
        }
        if (searchResults)
        {
            AddRows(view.Get("searchRows"), "hubSearch");
            if (view.Flag("searchEmpty"))
            {
                searchEmpty.UpdateStatus("No thread matches.");
                next.Add(searchEmpty);
            }
            var more = view.Get("searchMore");
            if (more.ValueKind == JsonValueKind.Object)
            {
                searchMore.Update(more, "");
                next.Add(searchMore);
            }
        }
        foreach (var section in (searchResults ? default : view.Get("sections")).Items())
        {
            string sectionId = section.Text("id");
            if (sectionId.Length == 0 || !seen.Add((SidebarItemKind.Header, sectionId)))
                continue;
            var drop = section.Get("drop");
            if (SidebarAction.Read(drop.Get("action")) is { } move)
            {
                string detail = drop.Text("detail");
                nextDrops.Add(sectionId, detail.Length > 0 ? move with { Detail = detail } : move);
            }
            bool collapsed = section.Flag("collapsed");
            if (section.Get("header") is { ValueKind: JsonValueKind.Object } header)
            {
                var item = GetItem(SidebarItemKind.Header, sectionId);
                item.Update(header, sectionId, collapsed, next.Count > 0);
                next.Add(item);
            }
            if (!collapsed)
                AddRows(section.Get("rows"), sectionId);
        }
        desired = next;
        dropActions = nextDrops;
        ApplyLayout(HoldLayout);
        UpdateBar();
        refreshActiveMenu?.Invoke();

        void AddRows(JsonElement rows, string sectionId)
        {
            foreach (var data in rows.Items())
            {
                string id = data.Text("id");
                if (id.Length == 0 || !seen.Add((SidebarItemKind.Row, id)))
                    continue;
                var row = GetItem(SidebarItemKind.Row, id);
                row.Update(data, sectionId);
                next.Add(row);
            }
        }
    }

    public void FocusSearch() => SearchBox.Focus();

    internal event Action? RailPrFlyoutChanged;
    internal PullRequestBadge? RailPrBadge(ContentControl preview) => FindVisualChild<PullRequestBadge>(preview);

    internal ContentControl CreateRailPreview()
    {
        return new ContentControl
        {
            ContentTemplate = (DataTemplate)Resources["SidebarRowTemplate"],
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
        };
    }

    internal void OpenRailMenu(SidebarItem row, FrameworkElement anchor, PlacementMode placement) =>
        ShowMenu(anchor, () => row.State.Actions, action => InvokeRowAction(row, action, rail: true), placement, row);

    private void RenderSearch(JsonElement view)
    {
        SearchBox.IsEnabled = view.Get("searchEnabled").ValueKind != JsonValueKind.False;
        if (!SearchBox.IsEnabled)
            pendingSearch = null;
        if (view.Get("search").ValueKind != JsonValueKind.String)
            return;
        var incoming = view.Text("search");
        // Snapshots for earlier keystrokes can arrive while the next query is in flight.
        if (pendingSearch is { } pending)
        {
            if (incoming != pending)
                return;
            pendingSearch = null;
        }
        if (SearchBox.Text == incoming)
            return;
        syncingSearch = true;
        SearchBox.SetCurrentValue(TextBox.TextProperty, incoming);
        syncingSearch = false;
    }

    private SidebarItem GetItem(SidebarItemKind kind, string id)
    {
        var key = (kind, id);
        if (!models.TryGetValue(key, out var item))
        {
            item = new SidebarItem(kind, id);
            models.Add(key, item);
        }
        return item;
    }

    private bool HoldLayout => !searchResults && (ListHost.IsMouseOver || menuHoldsLayout || dragIds is not null);

    private IReadOnlyList<SidebarItem> SteadyOrder()
    {
        if (shownRanks.Count == 0)
            return desired;
        var result = new List<SidebarItem>(desired.Count);
        for (int i = 0; i < desired.Count;)
        {
            var item = desired[i];
            if (item.Kind != SidebarItemKind.Row)
            {
                result.Add(item);
                i++;
                continue;
            }
            int end = i + 1;
            while (end < desired.Count && desired[end].Kind == SidebarItemKind.Row
                   && desired[end].SectionId == item.SectionId)
                end++;
            result.AddRange(desired.GetRange(i, end - i)
                .OrderBy(row => shownRanks.TryGetValue(row, out var rank) && rank.Section == row.SectionId
                    ? rank.Rank : int.MaxValue));
            i = end;
        }
        return result;
    }

    private void ApplyLayout(bool hold)
    {
        bool atDesiredOrder = items.SequenceEqual(desired);
        IReadOnlyList<SidebarItem> next = hold && !atDesiredOrder ? SteadyOrder() : desired;
        bool orderChanged = !atDesiredOrder && !items.SequenceEqual(next);
        bool resized = orderChanged;
        for (int i = 0; !resized && i < items.Count; i++)
            resized = tops[i + 1] - tops[i] != items[i].Height;

        if (!resized)
        {
            RememberOrder();
            NotifySelection();
            return;
        }

        var keep = new HashSet<SidebarItem>(next);
        var anchored = ViewportAnchor(keep);
        var focused = ContainerAt(Keyboard.FocusedElement as DependencyObject)?.Content as SidebarItem;
        syncingSelection = true;
        if (orderChanged)
        {
            for (int i = items.Count - 1; i >= 0; i--)
            {
                if (!keep.Contains(items[i]))
                    items.RemoveAt(i);
            }

            for (int i = 0; i < next.Count; i++)
            {
                var item = next[i];
                if (i < items.Count && items[i] == item)
                    continue;
                int from = items.IndexOf(item);
                if (from < 0)
                    items.Insert(i, item);
                else
                    items.Move(from, i);
            }
        }
        syncingSelection = false;

        tops = new double[items.Count + 1];
        for (int i = 0; i < items.Count; i++)
            tops[i + 1] = tops[i] + items[i].Height;
        headerIndices = Enumerable.Range(0, items.Count)
            .Where(i => items[i].Kind == SidebarItemKind.Header).ToArray();
        rows = items.Where(item => item.Kind == SidebarItemKind.Row).ToArray();
        RememberOrder();

        if (rangeAnchor is not null && !keep.Contains(rangeAnchor))
            rangeAnchor = null;
        if (rangeHead is not null && !keep.Contains(rangeHead))
            rangeHead = null;
        UpdateSelection(selected.Where(keep.Contains).ToHashSet(), notify: false);
        foreach (var key in models.Keys.Where(key => !keep.Contains(models[key])).ToArray())
            models.Remove(key);

        RestoreViewport(anchored, focused);
        UpdateStickyHeader();
        NotifySelection();
    }

    private void RememberOrder()
    {
        shownRanks.Clear();
        for (int i = 0; i < rows.Length; i++)
            shownRanks.Add(rows[i], (rows[i].SectionId, i));
    }

    private (SidebarItem Item, double Delta)? ViewportAnchor(HashSet<SidebarItem> keep)
    {
        if (scroll is null || scroll.VerticalOffset <= 0 || items.Count == 0)
            return null;
        double offset = scroll.VerticalOffset;
        int index = Math.Clamp(AtOrBefore(tops, offset), 0, items.Count - 1);
        if (keep.Contains(items[index]))
            return (items[index], offset - tops[index]);
        for (int i = index + 1; i < items.Count; i++)
        {
            if (keep.Contains(items[i]))
                return (items[i], 0);
        }
        return null;
    }

    private void RestoreViewport((SidebarItem Item, double Delta)? anchored, SidebarItem? focused)
    {
        restoreOffset?.Abort();
        if (anchored is null && focused is null)
            return;
        restoreOffset = Dispatcher.InvokeAsync(() =>
        {
            if (scroll is not null && anchored is { } a)
            {
                int i = items.IndexOf(a.Item);
                if (i >= 0)
                {
                    double target = tops[i] + Math.Min(a.Delta, a.Item.Height - 1);
                    if (Math.Abs(scroll.VerticalOffset - target) >= 1)
                        scroll.ScrollToVerticalOffset(target);
                }
            }
            if (focused is not null && (Keyboard.FocusedElement is null || Threads.IsKeyboardFocused)
                && Threads.ItemContainerGenerator.ContainerFromItem(focused) is ListBoxItem item)
                item.Focus();
        }, DispatcherPriority.Loaded);
    }

    private static int AtOrBefore(double[] offsets, double value)
    {
        int index = Array.BinarySearch(offsets, value);
        return index >= 0 ? index : ~index - 1;
    }

    private void UpdateStickyHeader()
    {
        if (scroll is null || scrollContent is null || headerIndices.Length == 0)
        {
            StickyHeader.Visibility = Visibility.Collapsed;
            return;
        }
        double offset = scroll.VerticalOffset;
        int index = AtOrBefore(tops, offset);
        int found = Array.BinarySearch(headerIndices, index);
        int header = found >= 0 ? found : ~found - 1;
        double natural = header >= 0 ? HeaderTop(header) : double.PositiveInfinity;
        if (natural >= offset)
        {
            StickyHeader.Visibility = Visibility.Collapsed;
            return;
        }
        double next = header + 1 < headerIndices.Length ? HeaderTop(header + 1) : double.PositiveInfinity;
        StickyHeader.Content = items[headerIndices[header]];
        StickyHeader.Width = Math.Max(0, scrollContent.ActualWidth - Threads.Padding.Right);
        StickyOffset.Y = Math.Min(0, next - offset - HeaderHeight);
        StickyHeader.Visibility = Visibility.Visible;
    }

    private double HeaderTop(int header)
    {
        int index = headerIndices[header];
        return tops[index] + items[index].Height - HeaderHeight;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        scroll = FindVisualChild<ScrollViewer>(Threads);
        scrollContent = scroll is null ? null : FindVisualChild<ScrollContentPresenter>(scroll);
        UpdateStickyHeader();
    }

    private void OnScrolled(object sender, ScrollChangedEventArgs e)
    {
        if (e.OriginalSource is not ScrollViewer viewer || Ancestor(viewer, Threads) is null)
            return;
        if (viewer != scroll)
        {
            scroll = viewer;
            scrollContent = FindVisualChild<ScrollContentPresenter>(viewer);
        }
        UpdateStickyHeader();
    }

    private void OnStickyWheel(object sender, MouseWheelEventArgs e)
    {
        if (scroll is null)
            return;
        scroll.ScrollToVerticalOffset(scroll.VerticalOffset - (e.Delta / 120.0 * 72));
        e.Handled = true;
    }

    private void OnListLeave(object sender, MouseEventArgs e)
    {
        if (!HoldLayout)
            ApplyLayout(false);
    }

    private static T? FindVisualChild<T>(DependencyObject parent) where T : DependencyObject
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is T match)
                return match;
            if (FindVisualChild<T>(child) is { } descendant)
                return descendant;
        }
        return null;
    }

    private static T? Ancestor<T>(DependencyObject? source, T? match = null) where T : DependencyObject
    {
        for (var current = source; current is not null; current = current is Visual
                 ? VisualTreeHelper.GetParent(current) : LogicalTreeHelper.GetParent(current))
        {
            if (current is T ancestor && (match is null || ancestor == match))
                return ancestor;
        }
        return null;
    }

    private ListBoxItem? ContainerAt(DependencyObject? source) =>
        source is null ? null : ItemsControl.ContainerFromElement(Threads, source) as ListBoxItem;

    private SidebarItem? ItemAt(DependencyObject? source) =>
        ContainerAt(source)?.Content as SidebarItem
        ?? (Ancestor(source, StickyHeader) is not null ? StickyHeader.Content as SidebarItem : null);

    private void OnListDown(object sender, MouseButtonEventArgs e)
    {
        pressedRow = null;
        var source = e.OriginalSource as DependencyObject;
        if (Ancestor<ButtonBase>(source) is not null || Ancestor<ScrollBar>(source) is not null)
            return;
        var container = ContainerAt(source);
        if (container?.Content is not SidebarItem { Kind: SidebarItemKind.Row } row)
            return;
        container.Focus();
        pressedRow = row;
        pressedAt = e.GetPosition(ListHost);
        pressedModifiers = Keyboard.Modifiers;
        e.Handled = true;
    }

    private void OnListUp(object sender, MouseButtonEventArgs e)
    {
        var row = pressedRow;
        pressedRow = null;
        if (row is null || row != ItemAt(e.OriginalSource as DependencyObject))
            return;
        e.Handled = true;
        ActivateRow(row, pressedModifiers);
    }

    private void ActivateRow(SidebarItem row, ModifierKeys modifiers)
    {
        if (!searchResults && modifiers.HasFlag(ModifierKeys.Control))
        {
            var next = new HashSet<SidebarItem>(selected);
            if (!next.Remove(row))
                next.Add(row);
            rangeAnchor = rangeHead = row;
            UpdateSelection(next);
            return;
        }
        if (!searchResults && modifiers.HasFlag(ModifierKeys.Shift))
        {
            SelectRange(rangeAnchor ?? OpenRow() ?? row, row);
            return;
        }
        rangeAnchor = rangeHead = row;
        UpdateSelection([]);
        Command?.Invoke("sidebar.open", new { id = row.Id });
    }

    private SidebarItem? OpenRow() => rows.FirstOrDefault(row => row.State.Selected);

    private void SelectRange(SidebarItem anchor, SidebarItem head)
    {
        int start = Array.IndexOf(rows, anchor);
        int end = Array.IndexOf(rows, head);
        if (end < 0)
            return;
        if (start < 0)
        {
            anchor = head;
            start = end;
        }
        rangeAnchor = anchor;
        rangeHead = head;
        var next = new HashSet<SidebarItem>();
        for (int i = Math.Min(start, end); i <= Math.Max(start, end); i++)
            next.Add(rows[i]);
        UpdateSelection(next);
    }

    private void StepRange(int delta)
    {
        if (rows.Length == 0)
            return;
        var start = rangeHead ?? rangeAnchor ?? OpenRow();
        int index = start is null ? -1 : Array.IndexOf(rows, start);
        int next = index < 0 ? (delta > 0 ? 0 : rows.Length - 1)
            : Math.Clamp(index + delta, 0, rows.Length - 1);
        var head = rows[next];
        SelectRange(rangeAnchor ?? start ?? head, head);
        Threads.ScrollIntoView(head);
    }

    private void UpdateSelection(HashSet<SidebarItem> next, bool notify = true)
    {
        syncingSelection = true;
        foreach (var row in selected.Except(next).ToArray())
        {
            row.SetMulti(false);
            Threads.SelectedItems.Remove(row);
        }
        foreach (var row in next)
        {
            if (selected.Contains(row))
                continue;
            row.SetMulti(true);
            Threads.SelectedItems.Add(row);
        }
        syncingSelection = false;
        selected.Clear();
        selected.UnionWith(next);
        UpdateBar();
        if (notify)
            NotifySelection();
    }

    private void OnNativeSelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (syncingSelection || e.OriginalSource != Threads)
            return;
        if (searchResults)
        {
            syncingSelection = true;
            Threads.UnselectAll();
            syncingSelection = false;
            return;
        }
        var next = Threads.SelectedItems.OfType<SidebarItem>()
            .Where(item => item.Kind == SidebarItemKind.Row).ToHashSet();
        UpdateSelection(next);
    }

    private string[] OrderedSelection() =>
        selected.Count == 0 ? [] : rows.Where(selected.Contains).Select(row => row.Id).ToArray();

    private void NotifySelection()
    {
        if (Command is null)
            return;
        var ids = OrderedSelection();
        if (sentSelection.SequenceEqual(ids))
            return;
        sentSelection = ids;
        Command.Invoke("sidebar.selection", new { ids });
    }

    private void ClearLocalSelection()
    {
        rangeAnchor = rangeHead = null;
        UpdateSelection([]);
    }

    private void OnListKeyDown(object sender, KeyEventArgs e)
    {
        var source = e.OriginalSource as DependencyObject;
        var modifiers = Keyboard.Modifiers;
        var row = ItemAt(source);
        if (e.Key == Key.A && modifiers == ModifierKeys.Control && !searchResults)
        {
            rangeAnchor = rows.FirstOrDefault();
            rangeHead = rows.LastOrDefault();
            UpdateSelection(rows.ToHashSet());
        }
        else if (e.Key is Key.Up or Key.Down && modifiers == ModifierKeys.Shift && !searchResults)
        {
            StepRange(e.Key == Key.Up ? -1 : 1);
        }
        else if (e.Key == Key.Escape && modifiers == ModifierKeys.None && selected.Count > 0)
        {
            ClearLocalSelection();
        }
        else if (e.Key is Key.Enter or Key.Space && row?.Kind == SidebarItemKind.Row
                 && Ancestor<ButtonBase>(source) is null)
        {
            ActivateRow(row, modifiers);
        }
        else if (e.Key == Key.Apps || (e.Key == Key.F10 && modifiers == ModifierKeys.Shift))
        {
            if (row is null || Ancestor<PullRequestBadge>(source) is not null)
                return;
            var target = (FrameworkElement?)ContainerAt(source) ?? StickyHeader;
            OpenItemMenu(row, target, PlacementMode.Bottom);
        }
        else
        {
            return;
        }
        e.Handled = true;
    }

    private SidebarItem? MenuItemAt(DependencyObject? source) =>
        Ancestor<ScrollBar>(source) is null && Ancestor<PullRequestBadge>(source) is null ? ItemAt(source) : null;

    private void OnListMenuDown(object sender, MouseButtonEventArgs e)
    {
        // WPF's Multiple selection mode toggles the row on right-button down. The context
        // menu uses the existing selection when it opens on button up.
        if (MenuItemAt(e.OriginalSource as DependencyObject) is { Kind: SidebarItemKind.Row })
            e.Handled = true;
    }

    private void OnListMenu(object sender, MouseButtonEventArgs e)
    {
        var source = e.OriginalSource as DependencyObject;
        if (MenuItemAt(source) is not { } item)
            return;
        var anchor = (FrameworkElement?)ContainerAt(source) ?? StickyHeader;
        OpenItemMenu(item, anchor, PlacementMode.MousePoint);
        e.Handled = true;
    }

    private void OpenItemMenu(SidebarItem row, FrameworkElement anchor, PlacementMode placement)
    {
        bool forBulk = row.Kind == SidebarItemKind.Row && selected.Count >= 2 && selected.Contains(row);
        ShowMenu(anchor,
            () => !items.Contains(row) ? [] : forBulk ? SelectionMenu() : row.State.Actions,
            action =>
            {
                if (forBulk)
                    InvokeBulk(action);
                else if (row.Kind == SidebarItemKind.Header)
                    InvokeAction(action, [], row.SectionId);
                else
                    InvokeRowAction(row, action);
            }, placement, row);
    }

    private IReadOnlyList<SidebarAction> SelectionMenu() =>
        bulk.Items.Any(action => action.Id == LocalClear.Id) ? bulk.Items : [.. bulk.Items, LocalClear];

    private void ShowMenu(FrameworkElement anchor, Func<IReadOnlyList<SidebarAction>> getActions,
        Action<SidebarAction> invoke, PlacementMode placement = PlacementMode.Bottom,
        SidebarItem? highlight = null, bool keepChecksOpen = false)
    {
        var actions = getActions();
        if (actions.Count == 0)
            return;
        if (activeMenu is not null)
            activeMenu.IsOpen = false;
        var menu = new ContextMenu { PlacementTarget = anchor, Placement = placement };
        var refresh = new List<Action>();
        AddEntries(menu, actions, getActions, invoke, keepChecksOpen, refresh);
        menuHoldsLayout = highlight is not null;
        highlight?.SetMenuOpen(true);
        menu.Closed += (_, _) =>
        {
            highlight?.SetMenuOpen(false);
            if (anchor.ContextMenu == menu)
                anchor.ClearValue(ContextMenuProperty);
            if (activeMenu != menu)
                return;
            activeMenu = null;
            refreshActiveMenu = null;
            menuHoldsLayout = false;
            if (!HoldLayout)
                ApplyLayout(false);
        };
        // Assign the menu before opening it so WPF can resolve the placement/owner theme.
        anchor.ContextMenu = menu;
        activeMenu = menu;
        refreshActiveMenu = () =>
        {
            foreach (var update in refresh)
                update();
        };
        menu.IsOpen = true;
    }

    private static void AddEntries(ItemsControl parent, IReadOnlyList<SidebarAction> actions,
        Func<IReadOnlyList<SidebarAction>> getLatest, Action<SidebarAction> invoke, bool keepChecksOpen,
        List<Action> refresh)
    {
        foreach (var action in actions)
        {
            if (action.IsSeparator)
            {
                parent.Items.Add(new Separator());
                continue;
            }
            var item = new MenuItem();
            ToolTipService.SetShowOnDisabled(item, true);
            SidebarAction? Latest() => getLatest().FirstOrDefault(a => a.Id == action.Id);
            void Apply(SidebarAction? choice)
            {
                item.Visibility = VisibilityOf(choice is not null);
                if (choice is null)
                    return;
                item.Header = choice.Label.Replace("_", "__");
                item.IsEnabled = choice.Enabled;
                item.IsCheckable = choice.Checked.HasValue;
                item.IsChecked = choice.Checked.GetValueOrDefault();
                item.StaysOpenOnClick = keepChecksOpen && choice.Checked.HasValue;
                item.ToolTip = choice.ToolTip;
                if (choice.Danger)
                    item.SetResourceReference(Control.ForegroundProperty, "SystemFillColorCriticalBrush");
                else
                    item.ClearValue(Control.ForegroundProperty);
            }

            Apply(action);
            refresh.Add(() => Apply(Latest()));
            if (action.Children.Count > 0)
            {
                AddEntries(item, action.Children, () => Latest()?.Children ?? [], invoke, keepChecksOpen, refresh);
            }
            else
            {
                item.Click += (_, e) =>
                {
                    if (Latest() is { Enabled: true } choice)
                        invoke(choice);
                    e.Handled = true;
                };
            }
            parent.Items.Add(item);
        }
    }

    private void InvokeBulk(SidebarAction action)
    {
        if (action.Id == LocalClear.Id)
        {
            ClearLocalSelection();
            return;
        }
        string[] ids = OrderedSelection();
        if (ids.Length > 0)
            InvokeAction(action, ids);
    }

    private void InvokeRowAction(SidebarItem row, SidebarAction action, bool rail = false)
    {
        if (!action.Enabled || (!rail && !rows.Contains(row)))
            return;
        if (action.Id == "copyId")
        {
            try
            {
                Clipboard.SetText(row.State.CopyId.Length > 0 ? row.State.CopyId : row.Id);
            }
            catch (ExternalException)
            {
                // The clipboard may be locked by another application.
            }
            return;
        }
        if (rail)
            Command?.Invoke("sidebar.railAction", new { id = row.Id, action = action.Id });
        else
            InvokeAction(action, [row.Id]);
    }

    private void InvokeAction(SidebarAction? choice, string[] ids, string? sectionId = null)
    {
        if (choice is not { Enabled: true } || choice.Id.Length == 0)
            return;
        object payload = sectionId is null
            ? new { action = choice.Id, ids }
            : new { action = choice.Id, ids, sectionId };
        Command?.Invoke("sidebar.action", payload);
    }

    private static Visibility VisibilityOf(bool value) => value ? Visibility.Visible : Visibility.Collapsed;

    private static void SetButton(Button button, SidebarAction? action)
    {
        button.Visibility = VisibilityOf(action is not null);
        button.Content = action?.Label;
        button.IsEnabled = action?.Enabled == true;
        button.ToolTip = action?.ToolTip;
    }

    private static IEnumerable<SidebarAction> Buttons(IReadOnlyList<SidebarAction> actions) =>
        actions.Where(action => !action.IsSeparator);

    private void UpdateBar()
    {
        bool hasSelection = selected.Count > 0;
        bool showNotice = !hasSelection && (noticeLabel.Length > 0 || notice.Items.Count > 0);
        SelectionBar.Visibility = VisibilityOf(hasSelection || showNotice);
        var actions = hasSelection ? bulk.Items : notice.Items;
        var buttons = Buttons(actions).Where(action => action.Id != LocalClear.Id).ToArray();
        var inline = buttons.Where(action => action.Children.Count == 0).Take(2).ToArray();
        if (SelectionActions.ItemsSource is not IEnumerable<SidebarAction> old || !old.SequenceEqual(inline))
            SelectionActions.ItemsSource = inline;
        SelectionLabel.Text = hasSelection ? $"{selected.Count} selected" : noticeLabel;
        SelectionLabel.ToolTip = hasSelection ? null : noticeDetail.Tip();
        ClearSelection.Visibility = VisibilityOf(hasSelection);
        MoreBulkActions.Visibility = VisibilityOf(buttons.Length > inline.Length);
    }

    private void OnBarAction(object sender, RoutedEventArgs e)
    {
        if (sender is not Button { DataContext: SidebarAction action } button)
            return;
        if (Ancestor<ItemsControl>(VisualTreeHelper.GetParent(button)) == FooterActions)
            InvokeAction(action, []);
        else if (selected.Count > 0)
            InvokeBulk(action);
        else
            InvokeAction(action, []);
    }

    private void OnMoreBulk(object sender, RoutedEventArgs e) =>
        ShowMenu(MoreBulkActions, () => selected.Count > 0 ? SelectionMenu() : notice.Items,
            action =>
            {
                if (selected.Count > 0)
                    InvokeBulk(action);
                else
                    InvokeAction(action, []);
            });

    private void OnClearSelection(object sender, RoutedEventArgs e)
    {
        ClearLocalSelection();
        Threads.Focus();
    }

    private void OnNew(object sender, RoutedEventArgs e)
    {
        if (newThread?.Enabled == true)
            Command?.Invoke("sidebar.new", null);
    }

    private void OnGroup(object sender, RoutedEventArgs e) =>
        ShowMenu(GroupButton, () => grouping.Items, choice => InvokeAction(choice, []), keepChecksOpen: true);

    private void OnOrder(object sender, RoutedEventArgs e) =>
        ShowMenu(OrderButton, () => ordering.Items, choice => InvokeAction(choice, []), keepChecksOpen: true);

    private void OnHeaderToggle(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: SidebarItem item })
            InvokeAction(item.State.Toggle, [], item.SectionId);
    }

    private void OnHeaderAction(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: SidebarItem item })
            InvokeAction(item.State.Primary, [], item.SectionId);
    }

    private void OnPrAction(object? sender, PullRequestBadgeEventArgs e)
    {
        if (sender is PullRequestBadge badge)
            Command?.Invoke("pr.action", new { id = badge.ThreadId, action = e.Action, number = e.Number });
    }

    private void OnPrFlyoutChanged(object? sender, EventArgs e)
    {
        if (sender is PullRequestBadge { DataContext: SidebarItem item } && !rows.Contains(item))
            RailPrFlyoutChanged?.Invoke();
    }

    private void OnSearchMore(object sender, RoutedEventArgs e)
    {
        if (pendingSearch is null && searchMore.State.Primary?.Enabled == true)
            Command?.Invoke("sidebar.searchMore", new { query = SearchBox.Text });
    }

    private void OnSearchChanged(object sender, TextChangedEventArgs e)
    {
        if (syncingSearch || Command is null)
            return;
        string text = ((TextBox)sender).Text;
        pendingSearch = text;
        Command.Invoke("sidebar.search", new { text });
    }

    private void OnSearchKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape && SearchBox.Text.Length > 0)
        {
            SearchBox.Clear();
            e.Handled = true;
        }
    }

    private void OnClearSearch(object sender, RoutedEventArgs e)
    {
        SearchBox.Clear();
        SearchBox.Focus();
    }

    private void OnListMove(object sender, MouseEventArgs e)
    {
        if (pressedRow is not { } row || !row.State.Draggable || searchResults)
            return;
        if (e.LeftButton != MouseButtonState.Pressed)
        {
            pressedRow = null;
            return;
        }
        var point = e.GetPosition(ListHost);
        if (Math.Abs(point.X - pressedAt.X) < SystemParameters.MinimumHorizontalDragDistance
            && Math.Abs(point.Y - pressedAt.Y) < SystemParameters.MinimumVerticalDragDistance)
            return;

        pressedRow = null;
        dragIds = selected.Count >= 2 && selected.Contains(row) ? OrderedSelection() : [row.Id];
        try
        {
            DragDrop.DoDragDrop(ListHost, new DataObject(DragFormat, dragIds), DragDropEffects.Move);
        }
        finally
        {
            dragIds = null;
            ClearDropHint();
            if (!HoldLayout)
                ApplyLayout(false);
        }
        e.Handled = true;
    }

    private SidebarAction? DropAction(string section)
    {
        if (dragIds is [var id]
            && models.TryGetValue((SidebarItemKind.Row, id), out var row)
            && row.State.Drops.TryGetValue(section, out var individual))
            return individual;
        return dropActions.GetValueOrDefault(section);
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        if (dragIds is null || !e.Data.GetDataPresent(DragFormat))
            return;
        var item = ItemAt(e.OriginalSource as DependencyObject);
        var action = item is not null ? DropAction(item.SectionId) : null;
        e.Effects = action?.Enabled == true ? DragDropEffects.Move : DragDropEffects.None;
        e.Handled = true;
        var section = e.Effects == DragDropEffects.Move ? item?.SectionId : null;
        HighlightDrop(section);

        if (action is null || (action.Detail.Length == 0 && action.Label.Length == 0))
        {
            DropHint.IsOpen = false;
            return;
        }
        DropText.Text = action.Detail.Length == 0 ? action.Label : action.Detail;
        Point point = e.GetPosition(ListHost);
        DropHint.HorizontalOffset = point.X + 14;
        DropHint.VerticalOffset = point.Y + 16;
        DropHint.IsOpen = true;
    }

    private void OnDrop(object sender, DragEventArgs e)
    {
        if (dragIds is not { } ids || !e.Data.GetDataPresent(DragFormat))
            return;
        e.Handled = true;
        var item = ItemAt(e.OriginalSource as DependencyObject);
        var action = item is null ? null : DropAction(item.SectionId);
        e.Effects = action?.Enabled == true ? DragDropEffects.Move : DragDropEffects.None;
        if (item is not null)
            InvokeAction(action, ids, item.SectionId);
        ClearDropHint();
    }

    private void OnDragLeave(object sender, DragEventArgs e)
    {
        var point = e.GetPosition(ListHost);
        if (!new Rect(ListHost.RenderSize).Contains(point))
            ClearDropHint();
    }

    private void HighlightDrop(string? section)
    {
        if (dropSection == section)
            return;
        foreach (var item in highlighted)
            item.SetDropTarget(false);
        highlighted = section is null ? [] : items.Where(item => item.SectionId == section).ToArray();
        foreach (var item in highlighted)
            item.SetDropTarget(true);
        dropSection = section;
    }

    private void ClearDropHint()
    {
        HighlightDrop(null);
        DropHint.IsOpen = false;
    }
}
