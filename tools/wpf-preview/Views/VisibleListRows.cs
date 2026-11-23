using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal static class VisibleListRows
{
    internal static void Observe(ListBox list, Action changed)
    {
        list.Loaded += (_, _) => changed();
        list.SizeChanged += (_, _) => changed();
        list.IsVisibleChanged += (_, _) => changed();
        list.ItemContainerGenerator.ItemsChanged += (_, _) => changed();
        list.ItemContainerGenerator.StatusChanged += (_, _) => changed();
        list.AddHandler(ScrollViewer.ScrollChangedEvent,
            new ScrollChangedEventHandler((_, _) => changed()), handledEventsToo: true);
    }

    internal static IEnumerable<object> Read(ListBox list, double coveredTop = 0)
    {
        if (!list.IsVisible || Find<ScrollContentPresenter>(list) is not { } viewport
            || Find<Panel>(viewport, panel => panel.IsItemsHost) is not { } itemsHost)
            yield break;

        var clip = new Rect(0, coveredTop, viewport.ActualWidth, Math.Max(0, viewport.ActualHeight - coveredTop));
        // The host can keep containers alive outside the viewport for recycling and focus.
        // Walk only its realized children and require a positive intersection with the viewport.
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(itemsHost); i++)
        {
            if (VisualTreeHelper.GetChild(itemsHost, i) is not ListBoxItem { IsVisible: true } row)
                continue;
            var bounds = row.TransformToAncestor(viewport).TransformBounds(new Rect(row.RenderSize));
            bounds.Intersect(clip);
            if (!bounds.IsEmpty && bounds.Width > 0 && bounds.Height > 0
                && list.ItemContainerGenerator.ItemFromContainer(row) is { } item)
                yield return item;
        }
    }

    private static T? Find<T>(DependencyObject parent, Predicate<T>? match = null) where T : DependencyObject
    {
        for (int i = 0; i < VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = VisualTreeHelper.GetChild(parent, i);
            if (child is T value && (match is null || match(value)))
                return value;
            if (Find(child, match) is { } descendant)
                return descendant;
        }
        return null;
    }
}
