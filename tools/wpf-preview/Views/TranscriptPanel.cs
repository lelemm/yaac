using System.Collections.Specialized;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

/// <summary>Recycles transcript rows while preserving the reader's pixel position as heights change.</summary>
public sealed class TranscriptPanel : VirtualizingPanel, IScrollInfo
{
    private const double EstimatedHeight = 44;
    private sealed class Measurement
    {
        public double Width { get; set; }
        public double Height { get; set; }
    }

    private readonly ConditionalWeakTable<TranscriptRow, Measurement> heights = new();
    private TranscriptRow[] rows = [];
    private double[] tops = [0];
    private TranscriptRow? firstVisible;
    private double firstTop;
    private TranscriptRow? requestedRow;
    private double requestedTop;
    private double? requestedOffset;
    private bool stickToEnd;

    internal void Pin(TranscriptRow? row, double top, double offset, bool bottom)
    {
        requestedRow = row;
        requestedTop = top;
        requestedOffset = row is null && !bottom ? offset : null;
        stickToEnd = bottom;
        InvalidateMeasure();
    }

    internal void CancelPin()
    {
        requestedRow = null;
        requestedOffset = null;
        stickToEnd = false;
    }

    protected override Size MeasureOverride(Size availableSize)
    {
        var children = InternalChildren;
        var width = availableSize.Width;
        var viewport = availableSize.Height;
        rows = ItemsControl.GetItemsOwner(this).Items.Cast<TranscriptRow>().ToArray();
        tops = new double[rows.Length + 1];
        var anchor = requestedRow ?? (requestedOffset is null ? firstVisible : null);
        var anchorIndex = anchor is null ? -1 : Array.IndexOf(rows, anchor);
        var anchorTop = requestedRow is null ? firstTop : requestedTop;
        var offset = requestedOffset ?? VerticalOffset;
        requestedRow = null;
        requestedOffset = null;

        void RefreshTops()
        {
            for (var i = 0; i < rows.Length; i++)
                tops[i + 1] = tops[i] + Estimate(rows[i], width);
            if (stickToEnd)
                offset = tops[^1] - viewport;
            else if (anchorIndex >= 0)
            {
                var height = tops[anchorIndex + 1] - tops[anchorIndex];
                offset = tops[anchorIndex] - Math.Max(anchorTop, 1 - height);
            }
            offset = Math.Clamp(offset, 0, Math.Max(0, tops[^1] - viewport));
        }

        // A clipped row can be much taller than its estimate, especially after rewrapping.
        if (anchorIndex >= 0 && !stickToEnd)
            MeasureRange(anchorIndex, anchorIndex, width);
        RefreshTops();
        if (anchorIndex < 0 && !stickToEnd && rows.Length > 0)
        {
            anchorIndex = IndexAt(offset);
            anchorTop = tops[anchorIndex] - offset;
        }

        if (rows.Length > 0)
        {
            var cache = viewport * 0.5;
            var first = IndexAt(Math.Max(0, offset - cache));
            var last = IndexAt(offset + viewport + cache);
            RecycleOutside(children, first, last);
            MeasureRange(first, last, width);
            // Measurements can expose more rows. Fill the gap before arranging the viewport.
            while (true)
            {
                RefreshTops();
                var nextFirst = IndexAt(Math.Max(0, offset - cache));
                var nextLast = IndexAt(offset + viewport + cache);
                if (nextFirst >= first && nextLast <= last)
                    break;
                if (nextFirst < first)
                    MeasureRange(nextFirst, first - 1, width);
                if (nextLast > last)
                    MeasureRange(last + 1, nextLast, width);
                first = Math.Min(first, nextFirst);
                last = Math.Max(last, nextLast);
            }
        }

        if (viewport > 0 && rows.Length != 0)
        {
            var first = IndexAt(offset);
            firstVisible = rows[first];
            firstTop = tops[first] - offset;
        }
        else
        {
            firstVisible = null;
        }

        var changed = ExtentHeight != tops[^1] || ViewportHeight != viewport || VerticalOffset != offset
            || ViewportWidth != width;
        ExtentHeight = tops[^1];
        ViewportHeight = viewport;
        ViewportWidth = width;
        VerticalOffset = offset;
        if (changed)
            ScrollOwner?.InvalidateScrollInfo();
        return new Size(ViewportWidth, ViewportHeight);
    }

    private double Estimate(TranscriptRow row, double width) =>
        heights.TryGetValue(row, out var entry) && entry.Width == width ? entry.Height : EstimatedHeight;

    private int IndexAt(double y)
    {
        var index = Array.BinarySearch(tops, y);
        return Math.Clamp(index >= 0 ? index : ~index - 1, 0, rows.Length - 1);
    }

    private void MeasureRange(int first, int last, double width)
    {
        var generator = ItemContainerGenerator;
        var position = generator.GeneratorPositionFromIndex(first);
        var childIndex = position.Index + (position.Offset == 0 ? 0 : 1);
        using (generator.StartAt(position, GeneratorDirection.Forward, true))
        {
            for (var i = first; i <= last; i++, childIndex++)
            {
                var child = (UIElement)generator.GenerateNext(out _);
                if (VisualTreeHelper.GetParent(child) != this)
                {
                    InsertInternalChild(childIndex, child);
                    generator.PrepareItemContainer(child);
                }
                child.Measure(new Size(width, double.PositiveInfinity));
                var measurement = heights.GetOrCreateValue(rows[i]);
                measurement.Width = width;
                measurement.Height = Math.Max(1, child.DesiredSize.Height);
            }
        }
    }

    private void RecycleOutside(UIElementCollection children, int first, int last)
    {
        var generator = (IRecyclingItemContainerGenerator)ItemContainerGenerator;
        for (var childIndex = children.Count - 1; childIndex >= 0; childIndex--)
        {
            var position = new GeneratorPosition(childIndex, 0);
            var index = generator.IndexFromGeneratorPosition(position);
            if (index >= first && index <= last || children[childIndex].IsKeyboardFocusWithin)
                continue;
            generator.Recycle(position, 1);
            RemoveInternalChildRange(childIndex, 1);
        }
    }

    protected override void OnItemsChanged(object sender, ItemsChangedEventArgs args)
    {
        base.OnItemsChanged(sender, args);
        if (args.ItemUICount == 0 || args.Action is not (NotifyCollectionChangedAction.Remove or NotifyCollectionChangedAction.Move))
            return;
        var position = args.Action == NotifyCollectionChangedAction.Move ? args.OldPosition : args.Position;
        RemoveInternalChildRange(position.Index + (position.Offset > 0 ? 1 : 0), args.ItemUICount);
    }

    protected override Size ArrangeOverride(Size finalSize)
    {
        var children = InternalChildren;
        for (var childIndex = 0; childIndex < children.Count; childIndex++)
        {
            var index = ItemContainerGenerator.IndexFromGeneratorPosition(new GeneratorPosition(childIndex, 0));
            children[childIndex].Arrange(new Rect(0, tops[index] - VerticalOffset, finalSize.Width, tops[index + 1] - tops[index]));
        }
        return finalSize;
    }

    public bool CanHorizontallyScroll { get; set; }
    public bool CanVerticallyScroll { get; set; }
    public double ExtentWidth => ViewportWidth;
    public double ExtentHeight { get; private set; }
    public double ViewportWidth { get; private set; }
    public double ViewportHeight { get; private set; }
    public double HorizontalOffset => 0;
    public double VerticalOffset { get; private set; }
    public ScrollViewer? ScrollOwner { get; set; }

    public void SetHorizontalOffset(double offset) { }

    public void SetVerticalOffset(double offset)
    {
        var limit = Math.Max(0, ExtentHeight - ViewportHeight);
        var atEnd = double.IsPositiveInfinity(offset) || limit > 0 && offset >= limit;
        if (offset != VerticalOffset || atEnd)
            Pin(null, 0, Math.Max(0, offset), atEnd);
    }

    public void LineUp() => SetVerticalOffset(VerticalOffset - 16);
    public void LineDown() => SetVerticalOffset(VerticalOffset + 16);
    public void PageUp() => SetVerticalOffset(VerticalOffset - ViewportHeight);
    public void PageDown() => SetVerticalOffset(VerticalOffset + ViewportHeight);
    public void MouseWheelUp() => SetVerticalOffset(VerticalOffset - 100);
    public void MouseWheelDown() => SetVerticalOffset(VerticalOffset + 100);
    public void LineLeft() { }
    public void LineRight() { }
    public void PageLeft() { }
    public void PageRight() { }
    public void MouseWheelLeft() { }
    public void MouseWheelRight() { }

    public Rect MakeVisible(Visual visual, Rect rectangle)
    {
        if (rectangle.IsEmpty || visual == this || !IsAncestorOf(visual))
            return Rect.Empty;
        var bounds = visual.TransformToAncestor(this).TransformBounds(rectangle);
        if (bounds.Top < 0 || bounds.Bottom > ViewportHeight)
        {
            var delta = bounds.Top < 0 ? bounds.Top : Math.Min(bounds.Top, bounds.Bottom - ViewportHeight);
            SetVerticalOffset(VerticalOffset + delta);
            bounds.Offset(0, -delta);
        }
        bounds.Intersect(new Rect(0, 0, ViewportWidth, ViewportHeight));
        return bounds;
    }
}
