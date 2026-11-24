using System.Collections.ObjectModel;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class AgentsView : UserControl
{
    private readonly ObservableCollection<AgentsEntry> _pill = new();
    private readonly ObservableCollection<AgentsEntry> _card = new();
    private readonly ObservableCollection<AgentsEntry> _dock = new();
    private readonly ObservableCollection<AgentsEntry> _dockParent = new();
    private readonly Dictionary<string, AgentsEntry> _entries = new(StringComparer.Ordinal);
    private readonly Dictionary<string, AgentsRow> _rows = new(StringComparer.Ordinal);
    private readonly Dictionary<string, bool> _folds = new(StringComparer.Ordinal);
    private readonly DispatcherTimer _closeTimer;
    private string? _snapshot;
    private string _threadId = "";
    private string? _floatingDestination;
    private bool _dockMouseInside;
    private string _hoveredDockThread = "";
    private string _mode = "pill";
    private bool _dockFolded;
    private string _firstHidden = "";
    private string _budgetId = "";
    private AgentsAction? _continueBudget;
    private ContextMenu? _menu;
    private bool _suppressPillFocus;
    private bool _focusCard;
    private bool _scrollHidden;
    private bool _keyboardCard;
    private double? _heightLimit;
    private double _floatingOffset;
    private double _floatingViewport;
    private readonly TranslateTransform _pillPosition = new();

    public AgentsView()
    {
        InitializeComponent();
        PillItems.ItemsSource = _pill;
        CardRows.ItemsSource = _card;
        DockRows.ItemsSource = _dock;
        DockParent.ItemsSource = _dockParent;
        Docked.IsVisibleChanged += (_, _) =>
        {
            if (!Docked.IsVisible)
                SetDockHover(false, "");
        };
        DockRows.AddHandler(ScrollViewer.ScrollChangedEvent, new ScrollChangedEventHandler((_, _) => TrackDockHover()));
        Pill.RenderTransform = _pillPosition;
        Floating.CustomPopupPlacementCallback = (size, target, _) =>
        [
            new(new Point(target.Width - size.Width, (target.Height - size.Height) / 2), PopupPrimaryAxis.Vertical)
        ];
        Pill.SizeChanged += (_, _) => PositionFloating();
        Card.SizeChanged += (_, _) => PositionFloating();
        _closeTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(250) };
        _closeTimer.Tick += (_, _) =>
        {
            _closeTimer.Stop();
            if (!Pill.IsMouseOver && !Card.IsMouseOver && !FloatingGrip.IsDragging && !_keyboardCard &&
                _menu is null && FindChild<PullRequestBadge>(Card, badge => badge.IsFlyoutOpen) is null)
                CloseCard();
        };
        Loaded += (_, _) => ApplyMode();
        IsVisibleChanged += (_, _) =>
        {
            if (!IsVisible)
                CloseCard(force: true);
            else
                ApplyMode();
        };
    }

    public event Action<string, object?>? Command;

    internal void ObservePrRows(Action changed)
    {
        VisibleListRows.Observe(DockRows, changed);
        VisibleListRows.Observe(DockParent, changed);
        VisibleListRows.Observe(CardRows, changed);
        Floating.Opened += (_, _) => changed();
        Floating.Closed += (_, _) => changed();
    }

    internal IEnumerable<string> VisiblePrRows()
    {
        if (!AreRowsVisible)
            return [];
        var visible = IsDocked
            ? VisibleListRows.Read(DockRows).Concat(VisibleListRows.Read(DockParent))
            : VisibleListRows.Read(CardRows);
        return visible.OfType<AgentsEntry>()
            .Where(entry => entry.Kind is "dockRow" or "cardRow" && entry.Data.CanOpen)
            .Select(entry => entry.CommandId);
    }

    internal bool AreRowsVisible => IsVisible && (IsDocked || Floating.IsOpen);

    internal bool IsDocked => _mode == "docked" && !_dockFolded;

    internal void SetDockFolded(bool folded)
    {
        if (_dockFolded == folded)
            return;

        _dockFolded = folded;
        ApplyMode();
    }

    internal void SetFloatingViewport(double height)
    {
        _floatingViewport = height;
        if (Floating.IsOpen)
            UpdateFloatingHeight();
        PositionFloating();
    }

    private void UpdateFloatingHeight()
    {
        var height = _floatingViewport > 0 ? _floatingViewport : 600;
        Card.MaxHeight = Math.Min(_heightLimit ?? double.PositiveInfinity, Math.Max(170, height - 100));
    }

    private double FloatingTravel => Math.Max(0, (_floatingViewport - Math.Max(Card.ActualHeight, Pill.ActualHeight)) / 2 - 4);

    private void PositionFloating()
    {
        var travel = FloatingTravel;
        var offset = Math.Clamp(_floatingOffset, -travel, travel);
        _pillPosition.Y = offset;
        Floating.VerticalOffset = offset;
    }

    private void OnFloatingDrag(object sender, DragDeltaEventArgs e)
    {
        var travel = FloatingTravel;
        _floatingOffset = Math.Clamp(Floating.VerticalOffset + e.VerticalChange, -travel, travel);
        PositionFloating();
    }

    private void OnFloatingDragCompleted(object sender, DragCompletedEventArgs e)
    {
        if (!Card.IsMouseOver)
            _closeTimer.Start();
    }

    public void Render(JsonElement view)
    {
        string snapshot = view.ValueKind == JsonValueKind.Undefined ? "" : view.GetRawText();
        if (snapshot == _snapshot)
            return;

        _snapshot = snapshot;
        string threadId = view.StringField("threadId");
        if (threadId != _threadId)
        {
            _threadId = threadId;
            SetDockHover(false, "");
            if (threadId != _floatingDestination)
            {
                CloseCard(force: true);
                ScrollToTop(CardRows);
            }
            _floatingDestination = null;
            ScrollToTop(DockRows);
        }

        _mode = view.StringField("mode");
        if (_mode == "")
            _mode = "pill";

        int maxHeight = view.Number("maxHeight");
        _heightLimit = maxHeight > 0 ? maxHeight : null;

        JsonElement budget = view.Field("budget");
        _budgetId = budget.StringField("id");
        _continueBudget = AgentsAction.Read(budget.Field("action"));
        bool exhausted = budget.BoolField("exhausted");

        List<AgentsGroup> groups = ReadGroups(view);
        Dictionary<string, AgentsRow> rows = groups.SelectMany(group => group.Rows)
            .ToDictionary(row => row.Id, StringComparer.Ordinal);
        foreach (JsonElement sibling in view.Array("siblings"))
        {
            string id = sibling.StringField("id");
            if (id.Length > 0 && !rows.ContainsKey(id))
                rows.Add(id, ReadRow(sibling, id, id));
        }
        foreach (string id in _rows.Keys.Except(rows.Keys).ToArray())
            _rows.Remove(id);

        DockButton.IsEnabled = view.BoolField("canDock", true);
        UndockButton.IsEnabled = view.BoolField("canUndock", true);
        bool canMark = view.BoolField("canMarkRead");
        MarkCard.Visibility = Show(view.BoolField("canMarkReadCard", canMark));
        MarkDock.Visibility = Show(canMark);
        DockBudgetText.Text = budget.StringField("text");
        DockBudgetText.Foreground = (Brush)FindResource(exhausted ? "YaacFailure" : "YaacMuted");
        DockBudgetText.Visibility = Show(DockBudgetText.Text.Length > 0);
        CardBudget.Visibility = DockBudget.Visibility = Show(exhausted);
        foreach (Button button in new[] { ContinueCard, ContinueDock })
        {
            button.Content = _continueBudget?.Label ?? "Continue";
            button.IsEnabled = _continueBudget is { Enabled: true } && _budgetId.Length > 0;
            button.ToolTip = _continueBudget?.Detail;
        }

        HashSet<string> used = new(StringComparer.Ordinal);
        var pill = BuildPill(view.Field("pill"), groups, exhausted, DockBudgetText.Text.Length > 0, used);
        var card = BuildCard(view.Field("cardGroups"), groups, rows, used);
        var dock = BuildDock(view.Array("siblings").Select(s => s.StringField("id")), groups, rows, used);
        var dockParent = BuildDockParent(view.Field("root"), groups, used);
        Synchronize(_pill, pill);
        Synchronize(_card, card, CardRows);
        Synchronize(_dock, dock, DockRows);
        Synchronize(_dockParent, dockParent, DockParent);
        foreach (string id in _entries.Keys.Except(used).ToArray())
            _entries.Remove(id);

        Visibility = Show(view.BoolField("visible", rows.Count > 0));
        ApplyMode();
        TrackDockHover();
    }

    private List<AgentsGroup> ReadGroups(JsonElement view)
    {
        List<AgentsGroup> groups = new();
        HashSet<string> seenGroups = new(StringComparer.Ordinal);
        HashSet<string> seenRows = new(StringComparer.Ordinal);
        foreach (JsonElement group in view.Array("groups"))
        {
            string id = group.StringField("id");
            if (id.Length == 0)
                id = groups.Count.ToString();
            if (!seenGroups.Add(id))
                continue;

            (string heading, string glyph) = id switch
            {
                "spawnedBy" => ("Spawned by", "↑"),
                "workers" => ("Workers", "⑂"),
                "contacts" => ("In contact with", "⇄"),
                _ => ("", "")
            };
            List<AgentsRow> rows = new();
            foreach (JsonElement json in group.Array("rows"))
            {
                string rowId = json.StringField("id");
                string identity = rowId.Length == 0 ? $"@{id}/{rows.Count}" : rowId;
                if (!seenRows.Add(identity))
                    continue;
                rows.Add(ReadRow(json, identity, rowId));
            }
            groups.Add(new(id, group.StringField("label", heading), group.StringField("glyph", glyph), group.OptionalNumber("count"),
                rows, group.Field("docked")));
        }
        return groups;
    }

    private AgentsRow ReadRow(JsonElement json, string identity, string rowId)
    {
        string raw = json.GetRawText();
        if (!_rows.TryGetValue(identity, out AgentsRow? row) || row.Raw != raw)
        {
            row = AgentsRow.Read(identity, rowId, raw, json);
            _rows[identity] = row;
        }
        return row;
    }

    private List<AgentsEntry> BuildPill(JsonElement view, List<AgentsGroup> groups, bool exhausted,
        bool hasBudget, HashSet<string> used)
    {
        List<AgentsEntry> result = new();
        if (exhausted)
            result.Add(Entry("pill", "pillLabel", "budget", new() { Title = "Ⅱ", Tooltip = "Agent budget reached · Continue in this card", MarkColor = Palette.Brush("YaacFailure") }, used));

        var all = groups.SelectMany(group => group.Rows).ToArray();
        var displayed = (view.Field("rowIds").ValueKind == JsonValueKind.Array
            ? view.Ids("rowIds") : all.Take(7).Select(row => row.Id))
            .Distinct(StringComparer.Ordinal)
            .Select((id, index) => (id, index))
            .ToDictionary(row => row.id, row => row.index, StringComparer.Ordinal);
        bool hasGroup = false;
        foreach (AgentsGroup group in groups)
        {
            var visible = group.Rows.Where(row => displayed.ContainsKey(row.Id)).OrderBy(row => displayed[row.Id]).ToArray();
            if (visible.Length == 0)
                continue;
            if (hasGroup)
                result.Add(Entry("pill", "rule", group.Id, new(), used));
            hasGroup = true;
            if (group.Glyph != "")
                result.Add(Entry("pill", "pillLabel", group.Id, new() { Title = group.Glyph, Tooltip = group.Label }, used));
            foreach (AgentsRow row in visible)
                result.Add(RowEntry("pill", "pillRow", row, used));
        }

        int hidden = view.Number("hidden", all.Count(row => !displayed.ContainsKey(row.Id)));
        int alive = view.Number("alive");
        _firstHidden = view.StringField("firstHiddenId", all.FirstOrDefault(row => !displayed.ContainsKey(row.Id))?.Id ?? "");
        if (hidden > 0)
        {
            if (hasGroup)
                result.Add(Entry("pill", "rule", "more", new(), used));
            var data = new AgentsData
            {
                Title = alive > 0 ? $"more\n({hidden})\n{alive} alive" : $"more\n({hidden})",
                Tooltip = alive > 0 ? $"more ({hidden} · {alive} alive)" : $"more ({hidden})",
                Attention = view.Number("needsYou") > 0
            };
            result.Add(Entry("pill", "more", "", data, used));
        }
        if (result.Count == 0 && hasBudget)
            result.Add(Entry("pill", "pillLabel", "budget", new() { Title = "⇄", Tooltip = "Related agents" }, used));
        return result;
    }

    private List<AgentsEntry> BuildCard(JsonElement view, List<AgentsGroup> groups,
        Dictionary<string, AgentsRow> rows, HashSet<string> used)
    {
        List<AgentsEntry> result = new();
        if (view.ValueKind == JsonValueKind.Array)
        {
            foreach (JsonElement group in view.EnumerateArray())
            {
                string label = group.StringField("label");
                AddGroup(label, label, group.Ids("rowIds").Where(rows.ContainsKey).Select(id => rows[id]));
            }
        }
        else
        {
            foreach (AgentsGroup group in groups)
                AddGroup(group.Id, group.Label, group.Rows);
        }
        return result;

        void AddGroup(string id, string label, IEnumerable<AgentsRow> entries)
        {
            var visible = entries.ToArray();
            if (visible.Length == 0)
                return;
            result.Add(Entry("card", "heading", id, new() { Title = label }, used));
            foreach (AgentsRow row in visible)
                result.Add(RowEntry("card", "cardRow", row, used));
        }
    }

    private List<AgentsEntry> BuildDock(IEnumerable<string> siblingIds, List<AgentsGroup> groups, Dictionary<string, AgentsRow> rows,
        HashSet<string> used)
    {
        List<AgentsEntry> result = new();
        var siblings = siblingIds.Where(rows.ContainsKey).Select(id => rows[id]).ToArray();
        if (siblings.Length > 0)
        {
            result.Add(Entry("dock", "heading", "siblings", new() { Title = "Siblings" }, used));
            foreach (var sibling in siblings)
                result.Add(RowEntry("dock", "dockRow", sibling, used));
        }
        foreach (AgentsGroup group in groups)
        {
            if (group.Id == "spawnedBy" || group.Rows.Count == 0 || group.Docked.Field("rowIds").ValueKind == JsonValueKind.Array
                && !group.Docked.Ids("rowIds").Any() && !group.Docked.Ids("foldIds").Any())
                continue;

            string heading = (group.Docked.OptionalNumber("count") ?? group.Count) is int count
                ? $"{group.Label} ({count})"
                : group.Label;
            result.Add(Entry("dock", "heading", group.Id, new() { Title = heading }, used));

            var groupRows = group.Rows.Select(row => row.Id).ToHashSet(StringComparer.Ordinal);
            var foldedIds = group.Docked.Ids("foldIds").Where(groupRows.Contains).Distinct(StringComparer.Ordinal).ToArray();
            IEnumerable<string> rowIds = group.Docked.Field("rowIds").ValueKind == JsonValueKind.Array
                ? group.Docked.Ids("rowIds")
                : group.Rows.Select(row => row.Id);
            foreach (string id in rowIds.Where(groupRows.Contains).Except(foldedIds, StringComparer.Ordinal))
                result.Add(RowEntry("dock", "dockRow", rows[id], used));

            List<AgentsEntry> folded = new();
            foreach (string id in foldedIds)
                folded.Add(RowEntry("dock", "dockRow", rows[id], used));
            if (folded.Count == 0)
                continue;
            bool expanded = _folds.GetValueOrDefault(group.Id);
            var fold = Entry("dock", "fold", group.Id, new() { Title = group.Docked.StringField("foldLabel", $"{folded.Count} inactive") }, used);
            fold.GroupId = group.Id;
            fold.FoldRows = folded;
            fold.Expanded = expanded;
            result.Add(fold);
            if (expanded)
                result.AddRange(folded);
        }
        return result;
    }

    private List<AgentsEntry> BuildDockParent(JsonElement root, List<AgentsGroup> groups, HashSet<string> used)
    {
        List<AgentsEntry> result = [Entry("dock", "heading", "spawnedBy", new() { Title = "Spawned by" }, used)];
        AddRoot(result, root, used);
        var parent = groups.FirstOrDefault(group => group.Id == "spawnedBy")?.Rows.FirstOrDefault();
        result.Add(parent is null
            ? Entry("dock", "info", "you", new() { Title = "started by you" }, used)
            : RowEntry("dock", "dockRow", parent, used));
        return result;
    }

    private void AddRoot(List<AgentsEntry> result, JsonElement root, HashSet<string> used)
    {
        string title = root.StringField("title");
        if (title == "")
            return;
        string id = root.StringField("id");
        bool canOpen = root.BoolField("canOpen") && id != "";
        var data = new AgentsData { Title = $"root: {title}", CanOpen = canOpen };
        var entry = Entry("dock", canOpen ? "root" : "info", "root", data, used);
        entry.CommandId = id;
        result.Add(entry);
    }

    private AgentsEntry RowEntry(string surface, string kind, AgentsRow row, HashSet<string> used)
    {
        var entry = Entry(surface, kind, row.Id, row.Data, used);
        entry.CommandId = row.CommandId;
        entry.RowId = row.Id;
        entry.Actions = row.Actions;
        return entry;
    }

    private AgentsEntry Entry(string surface, string kind, string id, AgentsData data, HashSet<string> used)
    {
        string key = $"{surface}/{kind}/{id}";
        used.Add(key);
        if (!_entries.TryGetValue(key, out AgentsEntry? entry))
        {
            entry = new AgentsEntry(kind);
            _entries.Add(key, entry);
        }
        entry.Data = data;
        return entry;
    }

    private static void Synchronize(ObservableCollection<AgentsEntry> collection, List<AgentsEntry> desired,
        ListBox? list = null)
    {
        if (collection.SequenceEqual(desired))
            return;

        var desiredSet = desired.ToHashSet();
        var focused = Keyboard.FocusedElement as FrameworkElement;
        var focusedEntry = list is not null && focused is not null && list.IsAncestorOf(focused)
            ? EntryOf(focused)
            : null;
        for (int i = collection.Count - 1; i >= 0; i--)
        {
            if (!desiredSet.Contains(collection[i]))
                collection.RemoveAt(i);
        }

        for (int i = 0; i < desired.Count; i++)
        {
            var next = desired[i];
            if (i < collection.Count && collection[i] == next)
                continue;
            int existing = collection.IndexOf(next);
            if (existing < 0)
                collection.Insert(i, next);
            else
                collection.Move(existing, i);
        }

        if (list is not null && focused is not null && focusedEntry is not null && desiredSet.Contains(focusedEntry))
        {
            list.Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(() =>
            {
                if (focused.IsKeyboardFocused || Keyboard.FocusedElement is not (null or ListBoxItem))
                    return;
                if (list.ItemContainerGenerator.ContainerFromItem(focusedEntry) is ListBoxItem container)
                    FindChild<FrameworkElement>(container, element => element.Name == focused.Name && element.Focusable)?.Focus();
            }));
        }
    }

    private void ApplyMode()
    {
        bool docked = IsDocked;
        Pill.Visibility = docked ? Visibility.Collapsed : Floating.IsOpen ? Visibility.Hidden : Visibility.Visible;
        Docked.Visibility = Show(docked);
        if (!IsVisible || docked && Floating.IsOpen)
            CloseCard(force: true);
        else if (_mode == "card" && IsLoaded)
            OpenCard();
    }

    private void OpenCard(bool focus = false, bool hidden = false)
    {
        if (!IsLoaded || !IsVisible || IsDocked)
            return;

        _closeTimer.Stop();
        _focusCard |= focus;
        _scrollHidden |= hidden;
        _keyboardCard |= focus;
        UpdateFloatingHeight();
        if (!Floating.IsOpen)
            Floating.IsOpen = true;
        else if (_focusCard || _scrollHidden)
            ScheduleCardPosition();
    }

    private void CloseCard(bool force = false, bool focusPill = false)
    {
        _closeTimer.Stop();
        if (!force && _mode == "card")
            return;
        _floatingDestination = null;
        _focusCard = false;
        _scrollHidden = false;
        _keyboardCard = false;
        if (_menu is not null)
            _menu.IsOpen = false;
        Floating.IsOpen = false;
        Pill.Visibility = Show(!IsDocked);
        if (focusPill && Pill.IsVisible)
        {
            _suppressPillFocus = true;
            Pill.Focus();
            _suppressPillFocus = false;
        }
    }

    private void ScheduleCardPosition()
    {
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(() =>
        {
            if (!Floating.IsOpen)
                return;

            bool focus = _focusCard;
            bool hidden = _scrollHidden;
            _focusCard = _scrollHidden = false;
            if (hidden && _card.FirstOrDefault(item => item.RowId == _firstHidden && item.RowId != "") is { } row)
                ScrollToRow(CardRows, row, focus && row.Data.CanOpen);
            else if (focus)
                Card.MoveFocus(new TraversalRequest(FocusNavigationDirection.First));
        }));
    }

    private static void ScrollToRow(ListBox list, AgentsEntry row, bool focus)
    {
        list.ScrollIntoView(row);
        list.Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(() =>
        {
            if (list.ItemContainerGenerator.ContainerFromItem(row) is not ListBoxItem container || !container.IsVisible)
                return;
            if (FindChild<ScrollViewer>(list) is { } scroll
                && FindChild<ScrollContentPresenter>(scroll) is { } viewport
                && viewport.IsAncestorOf(container))
            {
                var offset = container.TransformToAncestor(viewport).Transform(new Point()).Y;
                scroll.ScrollToVerticalOffset(scroll.VerticalOffset + offset);
            }
            if (focus)
                FindChild<Button>(container)?.Focus();
        }));
    }

    private static void ScrollToTop(ListBox list) => FindChild<ScrollViewer>(list)?.ScrollToTop();

    private static T? FindChild<T>(DependencyObject parent, Predicate<T>? match = null) where T : DependencyObject
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is T value && (match is null || match(value)))
                return value;
            if (FindChild(child, match) is { } descendant)
                return descendant;
        }
        return null;
    }

    internal void DismissFloatingCard() => CloseCard(force: true);

    private void OnUnloaded(object sender, RoutedEventArgs e) => DismissFloatingCard();
    private void OnFloatingOpened(object sender, EventArgs e)
    {
        Pill.Visibility = Visibility.Hidden;
        ScheduleCardPosition();
    }
    private void OnPillEnter(object sender, MouseEventArgs e) => OpenCard();
    private void OnCardEnter(object sender, MouseEventArgs e) => _closeTimer.Stop();
    private void OnCardLeave(object sender, MouseEventArgs e) => _closeTimer.Start();
    private void OnPillLeave(object sender, MouseEventArgs e) => _closeTimer.Start();

    private void OnPillFocus(object sender, KeyboardFocusChangedEventArgs e)
    {
        if (!_suppressPillFocus)
            OpenCard(focus: true);
    }

    private void OnPillFocusLost(object sender, KeyboardFocusChangedEventArgs e)
    {
        Dispatcher.BeginInvoke(DispatcherPriority.Input, new Action(() =>
        {
            if (!Card.IsKeyboardFocusWithin && !Pill.IsKeyboardFocusWithin && _menu is null)
            {
                _keyboardCard = false;
                _closeTimer.Start();
            }
        }));
    }

    private void OnMoreEnter(object sender, MouseEventArgs e) => OpenCard(hidden: true);
    private void OnMoreClick(object sender, RoutedEventArgs e) => OpenCard(hidden: true);

    private void OnFloatingKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape && Floating.IsOpen)
        {
            if (FindChild<PullRequestBadge>(Card, badge => badge.IsFlyoutFocused) is not null)
                return;
            CloseCard(force: true, focusPill: true);
            e.Handled = true;
        }
        else if (sender == Pill && e.Key is Key.Enter or Key.Space or Key.Left or Key.Down)
        {
            OpenCard(focus: true, hidden: e.Key == Key.Down && _firstHidden != "");
            e.Handled = true;
        }
    }

    private void OnDock(object sender, RoutedEventArgs e) => Command?.Invoke("agents.dock", null);
    private void OnDockHover(object sender, MouseEventArgs e) => TrackDockHover();
    private void OnDockLeave(object sender, MouseEventArgs e) => SetDockHover(false, "");

    private void TrackDockHover()
    {
        if (!Docked.IsVisible || !Docked.IsMouseOver)
            return;
        var list = DockParent.IsMouseOver ? DockParent : DockRows;
        var source = list.IsMouseOver ? list.InputHitTest(Mouse.GetPosition(list)) as DependencyObject : null;
        while (source is not null && source != list)
        {
            if (source is ListBoxItem { Content: AgentsEntry { Kind: "dockRow", Data.CanOpen: true } row })
            {
                SetDockHover(true, row.CommandId);
                return;
            }
            source = source is Visual ? VisualTreeHelper.GetParent(source) : LogicalTreeHelper.GetParent(source);
        }
        SetDockHover(true, "");
    }

    private void SetDockHover(bool inside, string id)
    {
        if (_dockMouseInside == inside && _hoveredDockThread == id)
            return;
        _dockMouseInside = inside;
        _hoveredDockThread = id;
        Command?.Invoke("agents.hover", new { owner = _threadId, inside, id });
    }

    private void OnUndock(object sender, RoutedEventArgs e) => Command?.Invoke("agents.undock", null);
    private void OnMarkRead(object sender, RoutedEventArgs e) =>
        Command?.Invoke("agents.markRead", new { floating = ReferenceEquals(sender, MarkCard) });
    private void OnContinue(object sender, RoutedEventArgs e) => InvokeAction(_budgetId, _continueBudget);

    private void OpenAgent(object sender, RoutedEventArgs e)
    {
        if (EntryOf(sender) is { Data.CanOpen: true, CommandId.Length: > 0 } entry)
        {
            if (Floating.IsOpen && entry.Kind == "cardRow" && entry.CommandId != _threadId)
                _floatingDestination = entry.CommandId;
            Command?.Invoke("agents.open", new { id = entry.CommandId });
        }
    }

    private void ToggleFold(object sender, RoutedEventArgs e)
    {
        if (EntryOf(sender) is not { } fold)
            return;
        int index = _dock.IndexOf(fold);
        if (index < 0)
            return;

        fold.Expanded = !fold.Expanded;
        _folds[fold.GroupId] = fold.Expanded;
        foreach (AgentsEntry row in fold.FoldRows)
        {
            if (fold.Expanded)
                _dock.Insert(++index, row);
            else
                _dock.Remove(row);
        }
    }

    private void OnOptions(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement anchor && EntryOf(anchor) is { } entry)
            OpenMenu(anchor, entry, PlacementMode.Bottom);
    }

    private void OnRowRightClick(object sender, MouseButtonEventArgs e)
    {
        if (!PullRequestBadge.IsSource(e.OriginalSource) &&
            sender is FrameworkElement row && EntryOf(row) is { Actions.Count: > 0 } entry)
        {
            OpenMenu(row, entry, PlacementMode.MousePoint);
            e.Handled = true;
        }
    }

    private void OnRowKeyDown(object sender, KeyEventArgs e)
    {
        if ((e.Key == Key.Apps || e.Key == Key.F10 && Keyboard.Modifiers == ModifierKeys.Shift)
            && !PullRequestBadge.IsSource(e.OriginalSource)
            && sender is FrameworkElement row && EntryOf(row) is { Actions.Count: > 0 } entry)
        {
            OpenMenu(row, entry, PlacementMode.Bottom);
            e.Handled = true;
        }
    }

    private void OnPrAction(object? sender, PullRequestBadgeEventArgs e)
    {
        if (sender is PullRequestBadge badge)
            Command?.Invoke("pr.action", new { id = badge.ThreadId, action = e.Action, number = e.Number });
    }

    private void OnPrFlyoutChanged(object? sender, EventArgs e)
    {
        if (Floating.IsOpen && sender is PullRequestBadge { IsFlyoutOpen: false })
            _closeTimer.Start();
    }

    private void OpenMenu(FrameworkElement anchor, AgentsEntry entry, PlacementMode placement)
    {
        if (_menu is not null)
            _menu.IsOpen = false;
        var menu = new ContextMenu { PlacementTarget = anchor, Placement = placement };
        foreach (AgentsAction action in entry.Actions)
            menu.Items.Add(action.IsSeparator ? new Separator() : MenuOption(entry, action));
        entry.MenuOpen = true;
        menu.Closed += (_, _) =>
        {
            entry.MenuOpen = false;
            if (_menu == menu)
                _menu = null;
            if (!Card.IsKeyboardFocusWithin && !Pill.IsKeyboardFocusWithin)
                _keyboardCard = false;
            _closeTimer.Start();
        };
        _menu = menu;
        menu.IsOpen = true;
    }

    private MenuItem MenuOption(AgentsEntry entry, AgentsAction action)
    {
        var option = new MenuItem
        {
            Header = action.Label,
            IsEnabled = action.Enabled && (action.Id != "" || action.Children.Count > 0),
            IsCheckable = action.Checked.HasValue,
            IsChecked = action.Checked ?? false,
            ToolTip = action.Detail != "" ? action.Detail : null
        };
        ToolTipService.SetShowOnDisabled(option, true);
        foreach (AgentsAction child in action.Children)
            option.Items.Add(child.IsSeparator ? new Separator() : MenuOption(entry, child));
        if (action.Children.Count == 0)
        {
            option.Click += (_, _) =>
            {
                if (_rows.TryGetValue(entry.RowId, out AgentsRow? row))
                    InvokeAction(row.CommandId, AgentsAction.Find(row.Actions, action.Id));
            };
        }
        return option;
    }

    private void InvokeAction(string id, AgentsAction? action)
    {
        if (id != "" && action is { Enabled: true, Id.Length: > 0 })
            Command?.Invoke("agents.action", new { id, action = action.Id });
    }

    private static AgentsEntry? EntryOf(object sender) => (sender as FrameworkElement)?.DataContext as AgentsEntry;
    private static Visibility Show(bool show) => show ? Visibility.Visible : Visibility.Collapsed;
}
