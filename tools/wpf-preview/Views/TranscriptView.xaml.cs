using System.Collections.ObjectModel;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Media3D;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class TranscriptView : UserControl
{
    private sealed record ScrollPin(string? Id, double Top, double Offset, bool Bottom, bool Focus = false);
    private sealed class ThreadViewState
    {
        public ScrollPin? Pin { get; set; }
        public (string Id, double Offset)? LastReported { get; set; }
        public Dictionary<string, TranscriptRowState> RowState { get; } = new();
    }

    private readonly ObservableCollection<TranscriptRow> items = new();
    private readonly Dictionary<string, TranscriptRow> byId = new();
    private readonly Dictionary<string, JsonElement> projectedRows = new();
    private IReadOnlyList<string> projectedOrder = [];
    private readonly Dictionary<string, ThreadViewState> threads = new();
    private readonly DispatcherTimer reportTimer;
    private readonly TranscriptSelection selection;
    private TranscriptRow[] prompts = [];
    private ThreadViewState? thread;
    private ScrollViewer? scroll;
    private double chatGutter = 28;
    private ScrollContentPresenter? viewport;
    private VirtualizingStackPanel? panel;
    private ScrollPin? pendingPin;
    private int pinPasses;
    private bool pinQueued;
    private bool reportAfterPin;
    private bool follow = true;
    private bool selectingThread;
    private int messages;
    private int seen;
    private string? threadId;
    private string? focusId;
    private string? focusToken;
    private string? anchorToken;
    private ScrollPin? requestedPin;
    private bool focusApplied;
    private JsonElement page;
    private bool historyAway;

    public event Action<string, object?>? Command;

    public TranscriptView()
    {
        InitializeComponent();
        Rows.ItemsSource = items;
        selection = new TranscriptSelection(Rows, () => viewport, RealizedContainers, ScrollSelectionBy);
        reportTimer = new DispatcherTimer(DispatcherPriority.Background, Dispatcher)
        {
            Interval = TimeSpan.FromMilliseconds(120),
        };
        reportTimer.Tick += (_, _) => ReportAnchor();
        Unloaded += (_, _) => reportTimer.Stop();
        Loaded += (_, _) =>
        {
            Rows.ApplyTemplate();
            scroll = FindVisual<ScrollViewer>(Rows);
            scroll?.ApplyTemplate();
            ApplyChatPadding();
            viewport = scroll is null ? null : FindVisual<ScrollContentPresenter>(scroll);
            panel = viewport is null ? null : FindVisual<VirtualizingStackPanel>(viewport);
            RebuildTimeline();
            QueuePin();
        };
    }

    public void SetChatGutter(double width)
    {
        chatGutter = width;
        TimelineColumn.Width = new GridLength(width);
        ApplyChatPadding();
    }

    private void ApplyChatPadding()
    {
        var scrollbar = scroll?.ComputedVerticalScrollBarVisibility == Visibility.Visible
            ? (scroll.Template.FindName("PART_VerticalScrollBar", scroll) as ScrollBar)?.Width ?? 0
            : 0;
        Rows.Padding = new Thickness(0, 10, Math.Max(0, chatGutter - scrollbar), 16);
    }

    public void Render(JsonElement view)
    {
        var nextId = view.Value("threadId").Text();
        var switched = thread is null || nextId != threadId;
        var pin = pendingPin ?? CapturePin();
        var oldIndex = pin.Id is not null && byId.TryGetValue(pin.Id, out var anchored) ? anchored.Index : -1;
        if (switched)
        {
            selection.Clear();
            if (thread is not null)
                thread.Pin = pin with { Focus = false };

            selectingThread = true;
            reportTimer.Stop();
            reportAfterPin = false;
            threadId = nextId;
            if (!threads.TryGetValue(nextId, out thread))
                threads.Add(nextId, thread = new ThreadViewState());

            focusId = null;
            focusToken = null;
            focusApplied = false;
            byId.Clear();
            items.Clear();
            pin = thread.Pin ?? new ScrollPin(null, 0, 0, true);
            follow = pin.Bottom;
        }

        historyAway = view.Value("historyAway").Flag();
        var pageData = view.Value("page");
        var pageChanged = !TranscriptJson.Same(page, pageData);
        if (pageChanged)
        {
            page = pageData.Snapshot();
            Older.Content = page.Value("label").Text();
            Older.IsEnabled = page.Value("enabled").Flag();
            Older.ToolTip = page.Value("detail").OptionalText();
            Older.Visibility = page.Value("id").Text().Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        }

        var state = thread!;
        var next = new List<TranscriptRow>();
        var present = new HashSet<string>();
        var added = new HashSet<TranscriptRow>();
        var dataChanged = false;
        foreach (var data in ReadRows(view))
        {
            if (data.ValueKind != JsonValueKind.Object)
                continue;
            var id = data.Value("id").OptionalText() ?? $"~row:{next.Count}";
            if (!present.Add(id))
                continue;
            if (!byId.TryGetValue(id, out var item))
            {
                item = new TranscriptRow(id, nextId, data, () =>
                {
                    if (!state.RowState.TryGetValue(id, out var saved))
                        state.RowState.Add(id, saved = new TranscriptRowState());
                    return saved;
                }, EmitAction);
                byId.Add(id, item);
                added.Add(item);
            }
            else
            {
                dataChanged |= item.Update(data);
            }
            item.Index = next.Count;
            next.Add(item);
        }

        var structureChanged = switched;
        for (var i = items.Count - 1; i >= 0; i--)
        {
            var item = items[i];
            if (present.Contains(item.Id))
                continue;
            byId.Remove(item.Id);
            items.RemoveAt(i);
            structureChanged = true;
        }
        for (var i = 0; i < next.Count; i++)
        {
            var item = next[i];
            if (i < items.Count && ReferenceEquals(items[i], item))
                continue;
            if (added.Contains(item))
                items.Insert(i, item);
            else
                items.Move(items.IndexOf(item), i);
            structureChanged = true;
        }

        var empty = items.Count == 0
            ? view.Value("loading").Flag() ? "Loading…" : view.Value("emptyText").OptionalText() ?? ""
            : "";
        Empty.Text = empty;
        Empty.Visibility = empty.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

        if (structureChanged || dataChanged)
        {
            var nextPrompts = items.Where(row => row.Kind is "user" or "task").ToArray();
            var timelineChanged = !nextPrompts.SequenceEqual(prompts);
            prompts = nextPrompts;
            messages = items.Count(row => row.Kind is "user" or "agent");
            if (timelineChanged)
                RebuildTimeline();
            else
                UpdateTimeline(labels: true);
        }

        var requestedFocus = view.Value("focusId").OptionalText();
        var token = view.Value("focusToken").Scalar();
        if (requestedFocus != focusId || token != focusToken)
        {
            focusId = requestedFocus;
            focusToken = token;
            focusApplied = false;
        }
        var nextAnchorToken = view.Value("anchorToken").Scalar();
        var incomingAnchor = view.Value("anchor");
        if (switched || nextAnchorToken is not null && nextAnchorToken != anchorToken)
        {
            requestedPin = null;
            var anchorId = incomingAnchor.OptionalText() ?? incomingAnchor.Value("id").OptionalText();
            if (anchorId is not null)
            {
                var offsetData = incomingAnchor.Value("offset");
                var offset = offsetData.ValueKind == JsonValueKind.Number && offsetData.TryGetDouble(out var pixels)
                    && double.IsFinite(pixels) ? Math.Max(0, pixels) : 0;
                requestedPin = new ScrollPin(anchorId, -offset, 0, false);
            }
        }
        anchorToken = nextAnchorToken;
        if (!focusApplied && FindRow(focusId) is { } requestedFocusRow)
        {
            focusApplied = true;
            requestedPin = null;
            seen = messages;
            SetPin(new ScrollPin(requestedFocusRow.Id, 60, 0, false, Focus: true), report: true);
        }
        else if (requestedPin is { } toRestore && FindRow(toRestore.Id) is { } anchorRow)
        {
            requestedPin = null;
            seen = messages;
            SetPin(toRestore with { Id = anchorRow.Id }, report: true);
        }
        else if (structureChanged || dataChanged || pageChanged)
        {
            if (!pin.Bottom && pin.Id is not null && !byId.ContainsKey(pin.Id) && items.Count > 0)
                pin = pin with { Id = switched ? null : items[Math.Clamp(oldIndex, 0, items.Count - 1)].Id, Top = 0 };
            if (switched)
                seen = messages;
            SetPin(pin, report: switched);
        }

        selectingThread = false;
        if (structureChanged || dataChanged)
            selection.QueueRefresh();
        UpdateLatest();
    }

    private TranscriptRow? FindRow(string? id) =>
        id is null ? null : byId.GetValueOrDefault(id) ?? items.FirstOrDefault(row => row.ActionId == id);

    private IEnumerable<JsonElement> ReadRows(JsonElement view)
    {
        var changes = view.Value("rowChanges");
        if (changes.ValueKind != JsonValueKind.Object)
        {
            projectedRows.Clear();
            var order = new List<string>();
            foreach (var row in view.Value("rows").Elements())
            {
                var id = row.Value("id").OptionalText() ?? $"~row:{order.Count}";
                if (projectedRows.TryAdd(id, row.Snapshot())) order.Add(id);
            }
            projectedOrder = order;
        }
        else
        {
            foreach (var row in changes.Value("rows").Elements())
                if (row.Value("id").OptionalText() is { } id) projectedRows[id] = row.Snapshot();
            var order = changes.Value("order");
            if (order.ValueKind == JsonValueKind.Array)
            {
                projectedOrder = order.EnumerateArray().Select(id => id.Text()).ToArray();
                var present = projectedOrder.ToHashSet();
                foreach (var id in projectedRows.Keys.Except(present).ToArray()) projectedRows.Remove(id);
            }
        }
        foreach (var id in projectedOrder)
            if (projectedRows.TryGetValue(id, out var row)) yield return row;
    }

    public void JumpToBottom()
    {
        if (historyAway)
            Command?.Invoke("transcript.page", new { id = "latest", threadId });
        SetPin(new ScrollPin(null, 0, 0, true), report: true);
    }

    public bool NavigatePrompt(int direction)
    {
        if (direction == 0 || prompts.Length == 0)
            return false;
        var anchor = CapturePin();
        var rowIndex = anchor.Id is not null && byId.TryGetValue(anchor.Id, out var current)
            ? current.Index
            : anchor.Bottom ? items.Count : 0;
        var candidates = direction > 0 ? prompts.AsEnumerable() : prompts.Reverse();
        foreach (var prompt in candidates)
        {
            var index = prompt.Index;
            if (direction > 0 ? index <= rowIndex : index > rowIndex || (index == rowIndex && anchor.Top > -2))
                continue;
            SetPin(new ScrollPin(prompt.Id, 10, 0, false), report: true);
            return true;
        }
        return false;
    }

    private void EmitAction(TranscriptRow row, object payload)
    {
        if (byId.TryGetValue(row.Id, out var current) && ReferenceEquals(row, current))
            Command?.Invoke("transcript.action", payload);
    }

    private void PageClicked(object sender, RoutedEventArgs e)
    {
        if (page.Value("enabled").Flag() && page.Value("id").OptionalText() is { } id)
        {
            follow = false;
            pendingPin = CapturePin();
            Command?.Invoke("transcript.page", new { id, threadId });
        }
    }

    private void LatestClicked(object sender, RoutedEventArgs e) => JumpToBottom();

    private void SetPin(ScrollPin pin, bool report = false)
    {
        pendingPin = pin;
        pinPasses = 0;
        follow = pin.Bottom;
        reportAfterPin |= report;
        QueuePin();
    }

    private void QueuePin()
    {
        if (pinQueued || pendingPin is null || scroll is null || !IsLoaded)
            return;
        pinQueued = true;
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(ApplyPin));
    }

    private void ApplyPin()
    {
        pinQueued = false;
        if (scroll is null || pendingPin is not { } pin)
            return;
        pinPasses++;
        if (pin.Bottom)
        {
            if (!AtBottom())
            {
                scroll.ScrollToBottom();
                if (pinPasses < 4)
                {
                    QueuePin();
                    return;
                }
            }
            seen = messages;
        }
        else if (pin.Id is not null && byId.TryGetValue(pin.Id, out var row))
        {
            if (Rows.ItemContainerGenerator.ContainerFromItem(row) is FrameworkElement container
                && container.IsArrangeValid && viewport is not null)
            {
                var top = container.TranslatePoint(new Point(), viewport).Y;
                var offset = Math.Clamp(scroll.VerticalOffset + top - pin.Top, 0, scroll.ScrollableHeight);
                if (Math.Abs(offset - scroll.VerticalOffset) > 0.75)
                {
                    scroll.ScrollToVerticalOffset(offset);
                    if (pinPasses < 5)
                    {
                        QueuePin();
                        return;
                    }
                }
                if (pin.Focus)
                    FindVisual<TranscriptRowView>(container)?.FocusAction();
            }
            else if (pinPasses < 5)
            {
                Rows.ScrollIntoView(row);
                QueuePin();
                return;
            }
        }
        else
        {
            scroll.ScrollToVerticalOffset(pin.Offset);
        }
        pendingPin = null;
        if (reportAfterPin)
        {
            reportAfterPin = false;
            QueueAnchorReport();
        }
        UpdateLatest();
        UpdateTimeline();
    }

    private bool AtBottom() => scroll is null || scroll.ScrollableHeight - scroll.VerticalOffset < 1;

    private ScrollPin CapturePin()
    {
        var offset = scroll?.VerticalOffset ?? 0;
        if (items.Count == 0 || follow && AtBottom())
            return new ScrollPin(null, 0, offset, true);
        foreach (var (row, top, bottom) in RealizedRows())
        {
            if (bottom > 0)
                return new ScrollPin(row.Id, top, offset, false);
        }
        return new ScrollPin(null, 0, offset, false);
    }

    private IEnumerable<(TranscriptRow Row, double Top, double Bottom)> RealizedRows()
    {
        foreach (var (row, item) in RealizedContainers())
        {
            var top = item.TranslatePoint(new Point(), viewport).Y;
            yield return (row, top, top + item.ActualHeight);
        }
    }

    private IEnumerable<(TranscriptRow Row, ListBoxItem Container)> RealizedContainers()
    {
        if (viewport is null)
            yield break;
        panel ??= FindVisual<VirtualizingStackPanel>(viewport);
        if (panel is null)
            yield break;
        var count = VisualTreeHelper.GetChildrenCount(panel);
        for (var i = 0; i < count; i++)
        {
            if (VisualTreeHelper.GetChild(panel, i) is not ListBoxItem { Content: TranscriptRow row } item)
                continue;
            yield return (row, item);
        }
    }

    private void ScrollSelectionBy(double delta)
    {
        if (scroll is null) return;
        BeginManualScroll();
        scroll.ScrollToVerticalOffset(Math.Clamp(scroll.VerticalOffset + delta, 0, scroll.ScrollableHeight));
    }

    private void BeginManualScroll()
    {
        pendingPin = null;
        requestedPin = null;
        reportAfterPin = false;
        follow = false;
    }

    private void ScrollInput(object sender, InputEventArgs e)
    {
        if (scroll is null)
            return;
        if (e is MouseWheelEventArgs wheel)
        {
            BeginManualScroll();
            var delta = -wheel.Delta * (100.0 / 120.0);
            scroll.ScrollToVerticalOffset(Math.Clamp(scroll.VerticalOffset + delta, 0, scroll.ScrollableHeight));
            e.Handled = true;
        }
        else if (FindAncestor<ScrollBar>(e.OriginalSource as DependencyObject) is not null)
        {
            BeginManualScroll();
        }
    }

    private void ScrollKey(object sender, KeyEventArgs e)
    {
        if (scroll is null || Keyboard.Modifiers != ModifierKeys.None || e.Key is not (Key.PageUp or Key.PageDown))
            return;
        BeginManualScroll();
        if (e.Key == Key.PageUp) scroll.PageUp();
        else scroll.PageDown();
        e.Handled = true;
    }

    private void RowsScrolled(object sender, ScrollChangedEventArgs e)
    {
        if (scroll is null || e.OriginalSource != scroll || selectingThread)
            return;
        ApplyChatPadding();
        selection.QueueRefresh();
        if (pendingPin is null)
        {
            if (follow && e.VerticalChange == 0 && (e.ExtentHeightChange != 0 || e.ViewportHeightChange != 0) && !AtBottom())
                SetPin(new ScrollPin(null, 0, 0, true));
            else
                follow = AtBottom();
            if (e.VerticalChange != 0 && e.ExtentHeightChange == 0 && e.ViewportHeightChange == 0)
                QueueAnchorReport();
        }
        if (follow)
            seen = messages;
        UpdateLatest();
        UpdateTimeline();
    }

    private void QueueAnchorReport()
    {
        reportTimer.Stop();
        reportTimer.Start();
    }

    private void ReportAnchor()
    {
        reportTimer.Stop();
        if (thread is null || pendingPin is not null)
            return;
        foreach (var (row, top, bottom) in RealizedRows())
        {
            if (bottom <= 0)
                continue;
            var offset = Math.Max(0, top * -1);
            if (thread.LastReported is { } last && row.Id == last.Id && Math.Abs(offset - last.Offset) < 1)
                break;
            thread.LastReported = (row.Id, offset);
            Command?.Invoke("transcript.anchor", new { id = row.ActionId, offset, threadId });
            break;
        }
    }

    private void UpdateLatest()
    {
        var fresh = Math.Max(0, messages - seen);
        Latest.Content = historyAway ? "Latest messages" : fresh switch { 0 => "Jump to bottom", 1 => "1 new message", _ => $"{fresh} new messages" };
        Latest.Visibility = historyAway || !follow && items.Count > 0 && !AtBottom() ? Visibility.Visible : Visibility.Collapsed;
    }

    private void TimelineSizeChanged(object sender, SizeChangedEventArgs e) => RebuildTimeline();

    private void RebuildTimeline()
    {
        if (Timeline is null)
            return;
        Timeline.Children.Clear();
        if (prompts.Length < 2 || Timeline.ActualHeight < 6)
            return;
        var slots = Math.Min(prompts.Length, Math.Max(1, (int)(Timeline.ActualHeight / 5)));
        var step = Math.Min(10, Timeline.ActualHeight / slots);
        var top = (Timeline.ActualHeight - step * slots) / 2;
        for (var i = 0; i < slots; i++)
        {
            var first = i * prompts.Length / slots;
            var end = (i + 1) * prompts.Length / slots;
            var button = new Button
            {
                Style = (Style)FindResource("TimelineButton"),
                Width = 28,
                Height = step,
                Cursor = Cursors.Hand,
                IsTabStop = false,
                Content = new Border { Width = 7, Height = 2, CornerRadius = new CornerRadius(1), Background = TranscriptColors.Muted },
                Tag = (first, end),
            };
            var id = prompts[first].Id;
            button.Click += (_, _) => SetPin(new ScrollPin(id, 10, 0, false), report: true);
            Timeline.Children.Add(button);
            Canvas.SetTop(button, top + i * step);
        }
        UpdateTimeline(labels: true);
    }

    private void UpdateTimeline(bool labels = false)
    {
        if (Timeline is null || Timeline.Children.Count == 0)
            return;
        var firstVisible = -1;
        var lastVisible = -1;
        foreach (var (row, top, bottom) in RealizedRows())
        {
            if (bottom <= 0 || top >= (viewport?.ActualHeight ?? 0))
                continue;
            var index = row.Index;
            if (firstVisible < 0)
                firstVisible = index;
            lastVisible = index;
        }
        foreach (Button button in Timeline.Children)
        {
            var (first, end) = ((int, int))button.Tag;
            var row = prompts[first];
            if (labels)
            {
                var stamp = row.Data.Value("stamp").OptionalText();
                var line = row.Prompt;
                var label = (stamp is null ? line : $"{line}\n{stamp}") + (end - first > 1 ? $"\nand {end - first - 1} more" : "");
                if (!Equals(button.ToolTip, label))
                {
                    button.ToolTip = label;
                    AutomationProperties.SetName(button, label);
                }
            }

            var startRow = row.Index;
            var endRow = end < prompts.Length ? prompts[end].Index : items.Count;
            var lit = firstVisible >= 0 && startRow <= lastVisible && endRow > firstVisible;
            var dash = (Border)button.Content;
            dash.Width = lit ? 12 : 7;
            dash.Background = lit ? TranscriptColors.Text : TranscriptColors.Muted;
        }
    }

    internal static T? FindVisual<T>(DependencyObject parent) where T : DependencyObject
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is T match)
                return match;
            if (FindVisual<T>(child) is { } nested)
                return nested;
        }
        return null;
    }

    internal static T? FindAncestor<T>(DependencyObject? child) where T : DependencyObject
    {
        while (child is not null)
        {
            if (child is T match)
                return match;
            child = child switch
            {
                Visual or Visual3D => VisualTreeHelper.GetParent(child),
                FrameworkContentElement content => content.Parent,
                _ => LogicalTreeHelper.GetParent(child)
            };
        }
        return null;
    }
}
