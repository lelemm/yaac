using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Documents;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

/// <summary>A transcript range whose endpoints survive row virtualization.</summary>
internal sealed class TranscriptSelection
{
    private readonly ListBox rows;
    private readonly Func<ScrollContentPresenter?> viewport;
    private readonly Func<IEnumerable<(TranscriptRow Row, ListBoxItem Container)>> realized;
    private readonly Action<double> scrollBy;
    private readonly DispatcherTimer autoScroll;
    private readonly Dictionary<(TranscriptRow Row, int Part), CachedText> copied = new();
    private readonly Dictionary<FrameworkElement, RangeHighlight> highlights = new();
    private Endpoint? anchor;
    private Endpoint? head;
    private TranscriptText? pressedEditor;
    private Point downAt;
    private bool dragging;
    private bool refreshQueued;
    private Cursor? originalCursor;

    private record struct Endpoint(TranscriptRow Row, int Part, int Offset) : IComparable<Endpoint>
    {
        public int CompareTo(Endpoint other)
        {
            var order = Row.Index.CompareTo(other.Row.Index);
            if (order != 0) return order;
            order = Part.CompareTo(other.Part);
            return order != 0 ? order : Offset.CompareTo(other.Offset);
        }
    }

    private sealed record CachedText(int Revision, int Start, int End, string Text);

    public TranscriptSelection(ListBox rows, Func<ScrollContentPresenter?> viewport,
        Func<IEnumerable<(TranscriptRow Row, ListBoxItem Container)>> realized, Action<double> scrollBy)
    {
        this.rows = rows;
        this.viewport = viewport;
        this.realized = realized;
        this.scrollBy = scrollBy;
        autoScroll = new DispatcherTimer(TimeSpan.FromMilliseconds(32), DispatcherPriority.Input, AutoScroll, rows.Dispatcher);
        autoScroll.Stop();
        rows.AddHandler(UIElement.PreviewMouseLeftButtonDownEvent, new MouseButtonEventHandler(MouseDown), true);
        rows.AddHandler(UIElement.PreviewMouseMoveEvent, new MouseEventHandler(MouseMove), true);
        rows.AddHandler(UIElement.PreviewMouseLeftButtonUpEvent, new MouseButtonEventHandler(MouseUp), true);
        rows.LostMouseCapture += (_, e) => { if (e.OriginalSource == rows && dragging) EndDrag(); };
        rows.PreviewKeyDown += KeyDown;
        CommandManager.AddPreviewCanExecuteHandler(rows, (_, e) =>
        {
            if (e.Command != ApplicationCommands.Copy || !HasRange()) return;
            e.CanExecute = e.Handled = true;
        });
        CommandManager.AddPreviewExecutedHandler(rows, (_, e) =>
        {
            if (e.Command == ApplicationCommands.Copy && Copy()) e.Handled = true;
        });
        rows.Unloaded += (_, _) => Clear();
    }

    private void MouseDown(object sender, MouseButtonEventArgs e)
    {
        if (e.ClickCount != 1 || Keyboard.Modifiers != ModifierKeys.None)
        {
            Clear();
            return;
        }
        if (viewport() is not { } area) return;
        var point = e.GetPosition(area);
        if (!new Rect(area.RenderSize).Contains(point)) return;
        Clear();
        if (e.OriginalSource is not DependencyObject source || SourceFragment(source) is not { } element)
            return;

        foreach (var (row, container) in realized())
        {
            var part = 0;
            foreach (var fragment in Fragments(container))
            {
                if (fragment == element)
                {
                    anchor = new Endpoint(row, part, OffsetAt(fragment, e.GetPosition(fragment)));
                    pressedEditor = fragment as TranscriptText;
                    downAt = point;
                    return;
                }
                part++;
            }
        }
    }

    private void MouseMove(object sender, MouseEventArgs e)
    {
        if (anchor is null || e.LeftButton != MouseButtonState.Pressed || viewport() is not { } area) return;
        var point = e.GetPosition(area);
        if (!dragging)
        {
            if (Math.Abs(point.X - downAt.X) < SystemParameters.MinimumHorizontalDragDistance
                && Math.Abs(point.Y - downAt.Y) < SystemParameters.MinimumVerticalDragDistance)
                return;
            if (!Mouse.Capture(rows)) return;
            dragging = true;
            originalCursor = rows.Cursor;
            rows.Cursor = Cursors.IBeam;
            rows.Focus();
            pressedEditor?.PaintSelection(anchor.Value.Offset, anchor.Value.Offset);
        }
        UpdateHead(area, point);
        UpdateTimer(area, point);
        e.Handled = true;
    }

    private void MouseUp(object sender, MouseButtonEventArgs e)
    {
        if (!dragging)
        {
            anchor = null;
            pressedEditor = null;
            return;
        }
        if (viewport() is { } area) UpdateHead(area, e.GetPosition(area));
        EndDrag();
        e.Handled = true;
    }

    private void EndDrag()
    {
        dragging = false;
        autoScroll.Stop();
        pressedEditor = null;
        rows.Cursor = originalCursor;
        if (Mouse.Captured == rows) rows.ReleaseMouseCapture();
    }

    private void KeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.C && Keyboard.Modifiers == ModifierKeys.Control && Copy())
            e.Handled = true;
        else if (e.Key == Key.Escape && HasRange())
        {
            Clear();
            e.Handled = true;
        }
    }

    public void Clear()
    {
        if (dragging) EndDrag();
        anchor = head = null;
        pressedEditor = null;
        copied.Clear();
        ClearPaint();
    }

    public void QueueRefresh()
    {
        if (anchor is null || head is null || refreshQueued) return;
        refreshQueued = true;
        rows.Dispatcher.BeginInvoke(DispatcherPriority.Loaded, () =>
        {
            refreshQueued = false;
            if (anchor is null || head is null) return;
            if (!rows.Items.Contains(anchor.Value.Row) || !rows.Items.Contains(head.Value.Row))
            {
                Clear();
                return;
            }
            if (dragging && viewport() is { } area) UpdateHead(area, Mouse.GetPosition(area));
            else Paint();
            foreach (var visible in highlights.Values) visible.InvalidateVisual();
        });
    }

    private void UpdateTimer(FrameworkElement area, Point point)
    {
        if (ScrollDelta(area, point) == 0) autoScroll.Stop();
        else if (!autoScroll.IsEnabled) autoScroll.Start();
    }

    private void AutoScroll(object? sender, EventArgs e)
    {
        if (!dragging || Mouse.LeftButton != MouseButtonState.Pressed || viewport() is not { } area)
        {
            EndDrag();
            return;
        }
        var amount = ScrollDelta(area, Mouse.GetPosition(area));
        if (amount != 0) scrollBy(amount);
    }

    private static double ScrollDelta(FrameworkElement area, Point point) =>
        point.Y < 0 ? Math.Clamp(point.Y, -44, -6)
        : point.Y > area.ActualHeight ? Math.Clamp(point.Y - area.ActualHeight, 6, 44) : 0;

    private void UpdateHead(FrameworkElement area, Point point)
    {
        var y = Math.Clamp(point.Y, 0, Math.Max(0, area.ActualHeight - 1));
        var distance = double.PositiveInfinity;
        (TranscriptRow Row, int Part, FrameworkElement Fragment, Rect Rect)? nearest = null;
        foreach (var (row, container) in realized())
        {
            var part = 0;
            foreach (var fragment in Fragments(container))
            {
                var index = part++;
                var topLeft = fragment.TranslatePoint(new Point(), area);
                var rect = new Rect(topLeft, fragment.RenderSize);
                if (rect.Bottom < 0 || rect.Top >= area.ActualHeight) continue;
                var vertical = Math.Max(0, Math.Max(rect.Top - y, y - rect.Bottom));
                var horizontal = Math.Max(0, Math.Max(rect.Left - point.X, point.X - rect.Right));
                // Prefer text on the same line before considering horizontal distance.
                var score = vertical * (area.ActualWidth + 1) + horizontal;
                if (score >= distance) continue;
                distance = score;
                nearest = (row, index, fragment, rect);
            }
        }
        if (nearest is not { } next) return;
        var offset = point.Y < 0 && next.Rect.Top >= 0 ? 0
            : point.Y > area.ActualHeight && next.Rect.Bottom <= area.ActualHeight ? Length(next.Fragment)
            : OffsetAt(next.Fragment, area.TranslatePoint(new Point(point.X, y), next.Fragment));
        head = new Endpoint(next.Row, next.Part, offset);
        Paint();
    }

    private bool HasRange() => anchor is { } a && head is { } b && a.CompareTo(b) != 0;

    private bool Copy()
    {
        if (!HasRange()) return false;
        var (first, last) = Ordered();
        Paint();
        var text = copied.OrderBy(entry => entry.Key.Row.Index).ThenBy(entry => entry.Key.Part)
            .Where(entry => InRange(entry.Key.Row, entry.Key.Part, first, last))
            .Select(entry => entry.Value.Text)
            .Where(value => value.Length > 0);
        var value = string.Join(Environment.NewLine, text);
        if (value.Length == 0) return false;
        TranscriptText.Copy(value);
        return true;
    }

    private (Endpoint First, Endpoint Last) Ordered() => anchor!.Value.CompareTo(head!.Value) <= 0
        ? (anchor.Value, head.Value) : (head.Value, anchor.Value);

    private static bool InRange(TranscriptRow row, int part, Endpoint first, Endpoint last)
    {
        var position = new Endpoint(row, part, 0);
        return position.CompareTo(first with { Offset = 0 }) >= 0
            && position.CompareTo(last with { Offset = 0 }) <= 0;
    }

    private void Paint()
    {
        if (!HasRange())
        {
            ClearPaint();
            copied.Clear();
            return;
        }

        var (first, last) = Ordered();
        var wanted = new HashSet<FrameworkElement>();
        foreach (var (row, container) in realized())
        {
            if (row.Index < first.Row.Index || row.Index > last.Row.Index) continue;
            var part = 0;
            foreach (var fragment in Fragments(container))
            {
                var index = part++;
                if (!InRange(row, index, first, last)) continue;
                var length = Length(fragment);
                var start = ReferenceEquals(row, first.Row) && index == first.Part ? Math.Min(first.Offset, length) : 0;
                var end = ReferenceEquals(row, last.Row) && index == last.Part ? Math.Min(last.Offset, length) : length;
                var key = (row, index);
                if (start >= end)
                {
                    copied.Remove(key);
                    continue;
                }
                wanted.Add(fragment);
                PaintFragment(fragment, start, end, row.Revision);
                if (!copied.TryGetValue(key, out var cached)
                    || cached.Revision != row.Revision || cached.Start != start || cached.End != end)
                {
                    var value = fragment is TranscriptText editor ? editor.SelectionText(start, end).TrimEnd('\r', '\n')
                        : ((TextBlock)fragment).Text[start..end];
                    copied[key] = new CachedText(row.Revision, start, end, value);
                }
            }
        }

        foreach (var fragment in highlights.Keys.Where(fragment => !wanted.Contains(fragment)).ToArray())
            RemovePaint(fragment);
    }

    private void ClearPaint()
    {
        foreach (var fragment in highlights.Keys.ToArray()) RemovePaint(fragment);
    }

    private void PaintFragment(FrameworkElement fragment, int start, int end, int revision)
    {
        if (!highlights.TryGetValue(fragment, out var adorner))
        {
            if (AdornerLayer.GetAdornerLayer(fragment) is not { } layer) return;
            adorner = new RangeHighlight(fragment, layer, viewport);
            highlights.Add(fragment, adorner);
            layer.Add(adorner);
        }
        adorner.SetRange(start, end, revision);
    }

    private void RemovePaint(FrameworkElement fragment)
    {
        if (highlights.Remove(fragment, out var adorner)) adorner.Layer.Remove(adorner);
    }

    private static FrameworkElement? SourceFragment(DependencyObject source)
    {
        // The inner visual tree of a read-only editor may include a code-copy button.
        var blocked = TranscriptView.FindAncestor<ButtonBase>(source);
        if (blocked is not null) return null;
        if (TranscriptView.FindAncestor<TranscriptText>(source) is { } editor) return editor;
        return TranscriptView.FindAncestor<TextBlock>(source);
    }

    private static IEnumerable<FrameworkElement> Fragments(DependencyObject root)
    {
        if (root is UIElement { IsVisible: false } || root is ButtonBase or TextBoxBase or PasswordBox) yield break;
        if (root is TranscriptText editor)
        {
            if (editor.SelectionLength > 0) yield return editor;
            yield break;
        }
        if (root is TextBlock label)
        {
            if (label.Text.Length > 0) yield return label;
            yield break;
        }
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
            foreach (var fragment in Fragments(VisualTreeHelper.GetChild(root, i)))
                yield return fragment;
    }

    private static int Length(FrameworkElement fragment) => fragment is TranscriptText editor
        ? editor.SelectionLength : ((TextBlock)fragment).Text.Length;

    private static int OffsetAt(FrameworkElement fragment, Point point)
    {
        if (fragment is TranscriptText editor) return editor.SelectionOffset(point);
        var label = (TextBlock)fragment;
        var best = double.PositiveInfinity;
        var index = 0;
        for (var i = 0; i <= label.Text.Length; i++)
        {
            var rect = CaretRect(label, i);
            if (rect.IsEmpty || rect.Left > label.ActualWidth + 1) continue;
            var vertical = Math.Max(0, Math.Max(rect.Top - point.Y, point.Y - rect.Bottom));
            var score = vertical * (label.ActualWidth + 1) + Math.Abs(rect.Left - point.X);
            if (score < best)
            {
                best = score;
                index = i;
            }
        }
        return index;
    }

    private static Rect CaretRect(TextBlock label, int offset)
    {
        var start = label.ContentStart.GetInsertionPosition(LogicalDirection.Forward);
        var pointer = start.GetPositionAtOffset(offset, LogicalDirection.Forward);
        return pointer?.GetCharacterRect(LogicalDirection.Forward) ?? Rect.Empty;
    }

    private sealed class RangeHighlight : Adorner
    {
        private readonly FrameworkElement element;
        private readonly Func<ScrollContentPresenter?> viewport;
        private (int Start, int End, int Revision) range;
        private static readonly Brush Fill = new SolidColorBrush(Color.FromArgb(100, 49, 107, 191));

        static RangeHighlight() => Fill.Freeze();

        public RangeHighlight(FrameworkElement element, AdornerLayer layer, Func<ScrollContentPresenter?> viewport) : base(element)
        {
            this.element = element;
            this.viewport = viewport;
            Layer = layer;
            IsHitTestVisible = false;
        }

        public AdornerLayer Layer { get; }

        public void SetRange(int start, int end, int revision)
        {
            if (range == (start, end, revision)) return;
            range = (start, end, revision);
            InvalidateVisual();
        }

        protected override void OnRender(DrawingContext drawingContext)
        {
            if (!element.IsVisible || viewport() is not { } area) return;
            var clip = new Rect(element.RenderSize);
            clip.Intersect(new Rect(area.TranslatePoint(new Point(), element), area.RenderSize));
            if (clip.IsEmpty) return;

            var length = Length(element);
            var start = Math.Min(range.Start, length);
            var end = Math.Min(range.End, length);
            var rectangles = element is TranscriptText editor
                ? editor.SelectionRectangles(start, end, clip)
                : LabelRectangles((TextBlock)element, start, end);
            drawingContext.PushClip(new RectangleGeometry(clip));
            // Coalesce adjacent character rectangles before painting to keep long code lines cheap.
            var line = Rect.Empty;
            foreach (var rectangle in rectangles)
            {
                if (!line.IsEmpty && Math.Abs(line.Top - rectangle.Top) < .5 && Math.Abs(line.Height - rectangle.Height) < .5
                    && rectangle.Left <= line.Right + .5 && rectangle.Right >= line.Left - .5)
                {
                    line.Union(rectangle);
                }
                else
                {
                    if (!line.IsEmpty) drawingContext.DrawRectangle(Fill, null, line);
                    line = rectangle;
                }
            }
            if (!line.IsEmpty) drawingContext.DrawRectangle(Fill, null, line);
            drawingContext.Pop();
        }

        private static IEnumerable<Rect> LabelRectangles(TextBlock label, int start, int end)
        {
            for (var i = start; i < end; i++)
            {
                var rect = TranscriptText.CharacterBounds(CaretRect(label, i), CaretRect(label, i + 1));
                if (!rect.IsEmpty) yield return rect;
            }
        }
    }
}
