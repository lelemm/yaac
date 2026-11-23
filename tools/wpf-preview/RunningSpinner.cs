using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Shapes;

namespace Yaac.WpfPreview;

public sealed class RunningSpinner : ContentControl
{
    private static readonly Geometry Arc = Geometry.Parse("M 15.5,5.5 A 7,7 0 1 1 7,2.3");
    private readonly RotateTransform rotation = new();

    public RunningSpinner()
    {
        Focusable = false;
        IsHitTestVisible = false;
        HorizontalContentAlignment = HorizontalAlignment.Stretch;
        VerticalContentAlignment = VerticalAlignment.Stretch;
        var path = new Path
        {
            Data = Arc,
            Stretch = Stretch.Uniform,
            StrokeThickness = 1.8,
            StrokeStartLineCap = PenLineCap.Round,
            StrokeEndLineCap = PenLineCap.Round,
            RenderTransform = rotation,
            RenderTransformOrigin = new Point(.5, .5),
            Margin = new Thickness(1),
        };
        path.SetBinding(Shape.StrokeProperty, new Binding(nameof(Foreground)) { Source = this });
        Content = path;
        Loaded += (_, _) => UpdateAnimation();
        Unloaded += (_, _) => rotation.BeginAnimation(RotateTransform.AngleProperty, null);
        IsVisibleChanged += (_, _) => UpdateAnimation();
    }

    private void UpdateAnimation()
    {
        if (!IsLoaded || !IsVisible || !SystemParameters.ClientAreaAnimation)
        {
            rotation.BeginAnimation(RotateTransform.AngleProperty, null);
            return;
        }

        var animation = new DoubleAnimation(0, 360, TimeSpan.FromMilliseconds(900))
        {
            RepeatBehavior = RepeatBehavior.Forever,
        };
        Timeline.SetDesiredFrameRate(animation, 24);
        rotation.BeginAnimation(RotateTransform.AngleProperty, animation);
    }
}
