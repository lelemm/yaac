using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;

namespace Yaac.WpfPreview;

public static class DropdownPlacement
{
    internal static ContextMenu CreateMenu(FrameworkElement anchor, bool preferAbove = false) => new()
    {
        PlacementTarget = anchor,
        Placement = PlacementMode.Custom,
        CustomPopupPlacementCallback = preferAbove ? Above : Below,
    };

    public static CustomPopupPlacementCallback Above { get; } = (popup, target, _) =>
    [
        new(new Point(0, -popup.Height), PopupPrimaryAxis.Horizontal),
        new(new Point(0, target.Height), PopupPrimaryAxis.Horizontal),
    ];

    public static CustomPopupPlacementCallback Below { get; } = (popup, target, _) =>
    [
        new(new Point(0, target.Height), PopupPrimaryAxis.Horizontal),
        new(new Point(0, -popup.Height), PopupPrimaryAxis.Horizontal),
    ];

    public static CustomPopupPlacementCallback Beside { get; } = (popup, target, _) =>
    [
        new(new Point(target.Width, 0), PopupPrimaryAxis.Vertical),
        new(new Point(-popup.Width, 0), PopupPrimaryAxis.Vertical),
    ];
}
