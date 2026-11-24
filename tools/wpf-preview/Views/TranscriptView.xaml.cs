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
    private sealed record Prompt(string Id, string Title, string? Stamp);
    private Prompt[] prompts = [];
    private Prompt[]? promptIndex;
    private readonly Dictionary<string, TranscriptRow> promptRows = new(StringComparer.Ordinal);
    private readonly Dictionary<string, int> promptNumbers = new(StringComparer.Ordinal);
    private (int Number, int Row)[] loadedPrompts = [];
    private string? timelineTarget;
    private string? requestedPage;
    private bool jumpingLatest;
    private ThreadViewState? thread;
    private ScrollViewer? scroll;
    private double chatGutter = 28;
    private ScrollContentPresenter? viewport;
    private TranscriptPanel? panel;
    private ScrollPin? pendingPin;
    private bool pinQueued;
    private bool reportAfterPin;
    private bool follow = true;
    private bool scrollingBar;
    private string? pageDirection;
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
        Rows.AddHandler(ScrollBar.ScrollEvent, new ScrollEventHandler(ScrollbarScrolled), handledEventsToo: true);
        Rows.PreviewMouseLeftButtonUp += (_, _) => EndScrollbarScroll();
        Rows.LostMouseCapture += (_, _) =>
        {
            if (Mouse.LeftButton == MouseButtonState.Released)
                EndScrollbarScroll();
        };
        Unloaded += (_, _) => reportTimer.Stop();
        Loaded += (_, _) =>
        {
            Rows.ApplyTemplate();
            scroll = FindVisual<ScrollViewer>(Rows);
            scroll?.ApplyTemplate();
            ApplyChatPadding();
            viewport = scroll is null ? null : FindVisual<ScrollContentPresenter>(scroll);
            panel = viewport is null ? null : FindVisual<TranscriptPanel>(viewport);
            PinPanel();
            RebuildTimeline();
            QueuePin();
        };
    }

    internal void FocusBackground() => Rows.Focus();

    internal TranscriptRow? FocusedApproval => IsVisible && IsKeyboardFocusWithin
        ? FindAncestor<TranscriptRowView>(Keyboard.FocusedElement as DependencyObject)?.FocusedApproval
        : null;

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
        var anchored = pin.Id is not null ? byId.GetValueOrDefault(pin.Id) : null;
        var oldIndex = anchored?.Index ?? -1;
        if (switched)
        {
            selection.Clear();
            scrollingBar = false;
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
            anchorToken = null;
            requestedPin = null;
            promptIndex = null;
            timelineTarget = requestedPage = pageDirection = null;
            jumpingLatest = false;
            byId.Clear();
            items.Clear();
            pin = thread.Pin ?? new ScrollPin(null, 0, 0, true);
            follow = pin.Bottom;
        }

        historyAway = view.Value("historyAway").Flag();
        var pageData = view.Value("page");
        var pageChanged = !TranscriptJson.Same(page, pageData);
        if (pageChanged)
            page = pageData.Snapshot();

        var indexData = view.Value("prompts");
        var indexChanged = indexData.ValueKind != JsonValueKind.Undefined;
        if (indexChanged)
        {
            promptIndex = indexData.ValueKind == JsonValueKind.Array
                ? indexData.EnumerateArray().Select(data => new Prompt(data.Value("id").Text(), data.Value("title").Text(), data.Value("stamp").OptionalText())).ToArray()
                : null;
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

        if (structureChanged || dataChanged || indexChanged || switched)
        {
            var nextPrompts = promptIndex ?? items.Where(row => row.Kind is "user" or "task")
                .Select(row => new Prompt(row.ActionId, row.Prompt, row.Data.Value("stamp").OptionalText())).ToArray();
            var timelineChanged = !nextPrompts.SequenceEqual(prompts);
            prompts = nextPrompts;
            if (timelineChanged)
            {
                promptNumbers.Clear();
                for (var i = 0; i < prompts.Length; i++)
                    promptNumbers.TryAdd(prompts[i].Id, i);
            }
            promptRows.Clear();
            var loaded = new List<(int Number, int Row)>();
            foreach (var row in items)
            {
                if (promptNumbers.TryGetValue(row.ActionId, out var number) && promptRows.TryAdd(row.ActionId, row))
                    loaded.Add((number, row.Index));
            }
            loadedPrompts = loaded.ToArray();
            messages = items.Count(row => row.Kind is "user" or "agent");
            if (timelineChanged)
                RebuildTimeline();
            else
                UpdateTimeline();
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
        if (nextAnchorToken is not null && nextAnchorToken != anchorToken)
        {
            var anchorId = incomingAnchor.OptionalText() ?? incomingAnchor.Value("id").OptionalText();
            if (anchorId is not null)
            {
                var offsetData = incomingAnchor.Value("offset");
                var offset = offsetData.ValueKind == JsonValueKind.Number && offsetData.TryGetDouble(out var pixels)
                    && double.IsFinite(pixels) ? Math.Max(0, pixels) : 0;
                requestedPin = new ScrollPin(anchorId, -offset, 0, false);
                anchorToken = nextAnchorToken;
            }
        }
        else if (nextAnchorToken is null)
        {
            anchorToken = null;
        }
        if (jumpingLatest && !historyAway && !page.Value("loading").Flag())
        {
            jumpingLatest = false;
            SetPin(new ScrollPin(null, 0, 0, true), report: true);
        }
        else if (!focusApplied && FindRow(focusId) is { } requestedFocusRow)
        {
            focusApplied = true;
            pageDirection = null;
            requestedPin = null;
            seen = messages;
            var fromTimeline = requestedFocusRow.ActionId == timelineTarget;
            timelineTarget = null;
            SetPin(new ScrollPin(requestedFocusRow.Id, fromTimeline ? 10 : 60, 0, false, Focus: !fromTimeline), report: true);
        }
        else if (requestedPin is { } toRestore && FindRow(toRestore.Id) is { } anchorRow)
        {
            requestedPin = null;
            pageDirection = null;
            seen = messages;
            SetPin(toRestore with { Id = anchorRow.Id }, report: true);
        }
        else if (structureChanged || dataChanged)
        {
            var removedAnchor = !pin.Bottom && pin.Id is not null && !byId.ContainsKey(pin.Id) && items.Count > 0;
            if (removedAnchor)
            {
                var run = anchored?.Data.Value("foldRun").OptionalText();
                var turn = anchored?.Data.Value("turnId").OptionalText();
                var fallback = (run is null ? null : byId.GetValueOrDefault(run))
                    ?? (turn is null ? null : byId.GetValueOrDefault($"~worked:{turn}"));
                pin = pin with
                {
                    Id = switched ? null : fallback?.Id ?? items[Math.Clamp(oldIndex, 0, items.Count - 1)].Id,
                    Top = fallback is null ? 0 : Math.Max(0, pin.Top),
                    Focus = false,
                };
            }
            if (switched)
                seen = messages;
            SetPin(pin, report: switched || removedAnchor);
        }

        selectingThread = false;
        if (structureChanged || dataChanged)
            selection.QueueRefresh();
        UpdateLatest();
        MaybePage();
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
        pageDirection = null;
        if (historyAway)
        {
            jumpingLatest = true;
            timelineTarget = null;
            Command?.Invoke("transcript.page", new { id = "latest", threadId });
            return;
        }
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
        var candidates = direction > 0 ? loadedPrompts.AsEnumerable() : loadedPrompts.Reverse();
        foreach (var (number, index) in candidates)
        {
            if (direction > 0 ? index <= rowIndex : index > rowIndex || (index == rowIndex && anchor.Top > -2))
                continue;
            ScrollToPrompt(prompts[number]);
            return true;
        }
        var adjacent = loadedPrompts.Length == 0 ? (direction < 0 && !historyAway ? prompts.Length - 1 : -1)
            : direction > 0 ? loadedPrompts[^1].Number + 1 : loadedPrompts[0].Number - 1;
        if (adjacent < 0 || adjacent >= prompts.Length)
            return false;
        ScrollToPrompt(prompts[adjacent]);
        return true;
    }

    private void ScrollToPrompt(Prompt prompt)
    {
        jumpingLatest = false;
        pageDirection = null;
        if (promptRows.TryGetValue(prompt.Id, out var row))
        {
            SetPin(new ScrollPin(row.Id, 10, 0, false), report: true);
            return;
        }

        timelineTarget = prompt.Id;
        Command?.Invoke("transcript.prompt", new { id = prompt.Id, threadId });
    }

    private void EmitAction(TranscriptRow row, object payload)
    {
        if (byId.TryGetValue(row.Id, out var current) && ReferenceEquals(row, current))
            Command?.Invoke("transcript.action", payload);
    }

    private void MaybePage()
    {
        if (scroll is null || pendingPin is not null || selectingThread || jumpingLatest || page.Value("loading").Flag())
            return;
        const double prefetch = 220;
        var direction = pageDirection;
        var cursor = direction switch
        {
            "older" when scroll.VerticalOffset < prefetch => page.Value("older").OptionalText(),
            "newer" when scroll.ScrollableHeight - scroll.VerticalOffset < prefetch => page.Value("newer").OptionalText(),
            _ => null,
        };
        if (cursor is null || cursor == requestedPage)
            return;
        pageDirection = null;
        requestedPage = cursor;
        Command?.Invoke("transcript.page", new { id = direction, threadId });
    }

    private void LatestClicked(object sender, RoutedEventArgs e) => JumpToBottom();

    private void SetPin(ScrollPin pin, bool report = false)
    {
        pendingPin = pin;
        follow = pin.Bottom;
        reportAfterPin |= report;
        PinPanel();
        QueuePin();
    }

    private void PinPanel()
    {
        if (panel is null || pendingPin is not { } pin)
            return;
        var row = pin.Id is null ? null : byId.GetValueOrDefault(pin.Id);
        panel.Pin(row, pin.Top, pin.Offset, pin.Bottom);
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
        if (pin.Bottom)
            seen = messages;
        if (pin.Focus && FindRow(pin.Id) is { } row
            && Rows.ItemContainerGenerator.ContainerFromItem(row) is FrameworkElement container)
            FindVisual<TranscriptRowView>(container)?.FocusAction();
        pendingPin = null;
        if (reportAfterPin)
        {
            reportAfterPin = false;
            QueueAnchorReport();
        }
        UpdateLatest();
        UpdateTimeline();
        MaybePage();
    }

    private bool AtBottom() => scroll is null || scroll.ScrollableHeight - scroll.VerticalOffset < 1;

    private ScrollPin CapturePin()
    {
        var offset = scroll?.VerticalOffset ?? 0;
        // Geometry may be between two layout passes when a streamed patch arrives.
        // Remember the user's follow intent independently of the current offset.
        if (items.Count == 0 || follow && !historyAway)
            return new ScrollPin(null, 0, offset, true);
        if (FirstVisibleRow() is { } visible)
            return new ScrollPin(visible.Row.Id, visible.Top, offset, false);
        return new ScrollPin(null, 0, offset, false);
    }

    private (TranscriptRow Row, double Top, double Bottom)? FirstVisibleRow()
    {
        var height = panel?.ViewportHeight ?? 0;
        foreach (var visible in RealizedRows())
        {
            if (visible.Bottom > 0 && visible.Top < height)
                return visible;
        }
        return null;
    }

    private IEnumerable<(TranscriptRow Row, double Top, double Bottom)> RealizedRows()
    {
        foreach (var (row, item) in RealizedContainers())
        {
            var top = item.TranslatePoint(new Point(), panel).Y;
            yield return (row, top, top + item.ActualHeight);
        }
    }

    private IEnumerable<(TranscriptRow Row, ListBoxItem Container)> RealizedContainers()
    {
        if (viewport is null)
            yield break;
        panel ??= FindVisual<TranscriptPanel>(viewport);
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

    private void ScrollSelectionBy(double delta) => ScrollManuallyBy(delta, resumeAtBottom: false);

    private void BeginManualScroll()
    {
        pendingPin = null;
        panel?.CancelPin();
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
            ScrollManuallyBy(-wheel.Delta * (100.0 / 120.0));
            e.Handled = true;
        }
        else if (e is MouseButtonEventArgs { ChangedButton: MouseButton.Left }
            && FindAncestor<ScrollBar>(e.OriginalSource as DependencyObject) is not null)
        {
            BeginManualScroll();
            scrollingBar = true;
        }
    }

    private void ScrollKey(object sender, KeyEventArgs e)
    {
        if (scroll is null || Keyboard.Modifiers != ModifierKeys.None || e.Key is not (Key.PageUp or Key.PageDown))
            return;
        ScrollManuallyBy(e.Key == Key.PageUp ? -scroll.ViewportHeight : scroll.ViewportHeight);
        e.Handled = true;
    }

    private void ScrollManuallyBy(double delta, bool resumeAtBottom = true)
    {
        if (scroll is null || delta == 0)
            return;
        pageDirection = delta < 0 ? "older" : "newer";
        var offset = Math.Clamp(scroll.VerticalOffset + delta, 0, scroll.ScrollableHeight);
        if (resumeAtBottom && !historyAway && (delta > 0 && offset >= scroll.ScrollableHeight - 1 ||
            scroll.ScrollableHeight < 1 && page.Value("older").OptionalText() is null))
            SetPin(new ScrollPin(null, 0, 0, true), report: true);
        else
        {
            BeginManualScroll();
            scroll.ScrollToVerticalOffset(offset);
            if (offset == scroll.VerticalOffset)
                MaybePage();
        }
    }

    private void ScrollbarScrolled(object sender, ScrollEventArgs e)
    {
        if (scrollingBar && scroll is not null && e.NewValue != scroll.VerticalOffset)
            pageDirection = e.NewValue < scroll.VerticalOffset ? "older" : "newer";
    }

    private void EndScrollbarScroll()
    {
        if (!scrollingBar)
            return;
        scrollingBar = false;
        var selected = thread;
        Dispatcher.BeginInvoke(DispatcherPriority.Background, new Action(() =>
        {
            if (ReferenceEquals(selected, thread) && !scrollingBar && !historyAway && AtBottom()
                && pendingPin is null)
                SetPin(new ScrollPin(null, 0, 0, true), report: true);
        }));
    }

    private void RowsScrolled(object sender, ScrollChangedEventArgs e)
    {
        if (scroll is null || e.OriginalSource != scroll || selectingThread)
            return;
        ApplyChatPadding();
        selection.QueueRefresh();
        if (pendingPin is null)
        {
            if (follow && !historyAway && !AtBottom())
                SetPin(new ScrollPin(null, 0, 0, true));
            if (e.VerticalChange != 0)
                QueueAnchorReport();
        }
        if (follow)
            seen = messages;
        UpdateLatest();
        UpdateTimeline();
        MaybePage();
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
        if (follow && !historyAway)
        {
            if (thread.LastReported is not { Id: "" })
            {
                thread.LastReported = ("", 0);
                Command?.Invoke("transcript.anchor", new { id = "", offset = 0, threadId });
            }
            return;
        }
        if (FirstVisibleRow() is not { } visible)
            return;
        var (row, top, _) = visible;
        var offset = Math.Max(0, -top);
        if (thread.LastReported is { } last && row.Id == last.Id && Math.Abs(offset - last.Offset) < 1)
            return;
        thread.LastReported = (row.Id, offset);
        Command?.Invoke("transcript.anchor", new { id = row.Id, itemId = row.ActionId, offset, threadId });
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
                Width = Math.Min(28, Timeline.ActualWidth),
                Height = step,
                Cursor = Cursors.Hand,
                IsTabStop = false,
                Content = new Border { Width = 7, Height = 2, CornerRadius = new CornerRadius(1), Background = TranscriptColors.Muted },
                Tag = (first, end),
            };
            button.Click += (_, _) => PickPrompt(button, first, end);
            Timeline.Children.Add(button);
            Canvas.SetTop(button, top + i * step);
        }
        UpdateTimeline(labels: true);
    }

    private void PickPrompt(Button button, int first, int end)
    {
        if (end - first == 1)
        {
            ScrollToPrompt(prompts[first]);
            return;
        }
        var menu = new ContextMenu
        {
            PlacementTarget = button, Placement = PlacementMode.Right, MaxHeight = 320,
        };
        for (var i = first; i < end; i++)
        {
            var prompt = prompts[i];
            var item = new MenuItem { Header = prompt.Title, ToolTip = prompt.Stamp };
            item.Click += (_, _) => ScrollToPrompt(prompt);
            menu.Items.Add(item);
        }
        menu.IsOpen = true;
    }

    private void UpdateTimeline(bool labels = false)
    {
        if (Timeline is null || Timeline.Children.Count == 0)
            return;
        var firstVisible = -1;
        var lastVisible = -1;
        foreach (var (row, top, bottom) in RealizedRows())
        {
            if (bottom <= 0 || top >= (panel?.ViewportHeight ?? 0))
                continue;
            var index = row.Index;
            if (firstVisible < 0)
                firstVisible = index;
            lastVisible = index;
        }
        var firstPrompt = -1;
        var lastPrompt = -1;
        foreach (var (number, row) in loadedPrompts)
        {
            if (row <= firstVisible || firstPrompt < 0 && row <= lastVisible)
                firstPrompt = number;
            if (row <= lastVisible)
                lastPrompt = number;
        }
        foreach (Button button in Timeline.Children)
        {
            var (first, end) = ((int, int))button.Tag;
            var prompt = prompts[first];
            if (labels)
            {
                var stamp = prompt.Stamp;
                var line = prompt.Title;
                var label = (stamp is null ? line : $"{line}\n{stamp}") + (end - first > 1 ? $"\nand {end - first - 1} more" : "");
                if (!Equals(button.ToolTip, label))
                {
                    button.ToolTip = label;
                    AutomationProperties.SetName(button, label);
                }
            }

            var lit = firstPrompt >= 0 && first <= lastPrompt && end > firstPrompt;
            var dash = (Border)button.Content;
            dash.Width = Math.Min(lit ? 12 : 7, Math.Max(0, Timeline.ActualWidth - 2));
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
