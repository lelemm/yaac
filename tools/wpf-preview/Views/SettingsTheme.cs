using System;
using System.Collections.Generic;
using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public partial class SettingsView
{
    private static void SetTextSize(TextBlock block, double size)
    {
        block.SetBinding(TextBlock.FontSizeProperty, new Binding(nameof(FontSize))
        {
            RelativeSource = new RelativeSource(RelativeSourceMode.FindAncestor, typeof(SettingsView), 1),
            Converter = FontScale.Instance,
            ConverterParameter = size,
            FallbackValue = size
        });
    }

    private sealed class FontScale : IValueConverter
    {
        public static FontScale Instance { get; } = new();

        public object Convert(object value, Type targetType, object parameter, CultureInfo culture) =>
            (double)parameter * (double)value / 13.5;

        public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) =>
            Binding.DoNothing;
    }
}
