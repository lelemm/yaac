using System.Globalization;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace Yaac.WpfPreview.Views;

/// <summary>
/// An external dialog (docs/16-external-dialogs.md): its controls are built at runtime from the
/// integrator's layout tree in the view frame — no per-dialog XAML. The values the user enters
/// stay in <see cref="Values"/> (in memory only; secret ones too) and go to the controller with
/// the button pressed, which validates them and sends the answer once. Problems of the last
/// press come back in the frame and are shown under their inputs.
/// </summary>
internal sealed class DialogLayoutView : StackPanel
{
    private readonly Dictionary<string, object?> values;
    private JsonElement shown;
    private bool enabled;
    private JsonElement problems;
    private string newerYaac = "This part of the dialog needs a newer YAAC";

    public DialogLayoutView(Dictionary<string, object?> values)
    {
        this.values = values;
        Margin = new Thickness(0, 4, 0, 4);
    }

    /// <summary>A button of the layout was pressed: its id, with <see cref="Values"/> to send.</summary>
    public event Action<string>? Pressed;

    public IReadOnlyDictionary<string, object?> Values => values;

    /// <summary>Rebuilds the controls when the frame's dialog changed; values entered are kept.</summary>
    public void Render(JsonElement dialog)
    {
        if (shown.ValueKind != JsonValueKind.Undefined && TranscriptJson.Same(shown, dialog))
            return;
        shown = dialog.Snapshot();
        enabled = dialog.Value("enabled").Flag();
        problems = dialog.Value("problems");
        newerYaac = dialog.Value("newerYaac").OptionalText() ?? newerYaac;
        Children.Clear();
        if (problems.Value("").OptionalText() is { } whole)
            Children.Add(Label(whole, color: TranscriptColors.Error));
        Node(shown.Value("layout"), Children, 0);
    }

    private static TextBlock Label(string text, bool semiBold = false, Brush? color = null, double? size = null)
    {
        var label = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap, FontWeight = semiBold ? FontWeights.SemiBold : FontWeights.Normal };
        if (color is not null) label.Foreground = color;
        if (size is { } s) label.FontSize = s;
        return label;
    }

    private static double Gap(JsonElement node, double fallback) =>
        double.TryParse(node.Value("gap").Scalar(), NumberStyles.Float, CultureInfo.InvariantCulture, out var g) ? Math.Clamp(g, 0, 64) : fallback;

    private bool On(JsonElement node) => enabled && !node.Value("disabled").Flag();

    private void Header(JsonElement node, UIElementCollection into)
    {
        if (node.Value("label").OptionalText() is { } label)
            into.Add(Label(label + (node.Value("required").Flag() ? " *" : ""), semiBold: true));
    }

    private void Footer(JsonElement node, UIElementCollection into)
    {
        if (node.Value("help").OptionalText() is { } help)
            into.Add(Label(help, color: TranscriptColors.Muted, size: 12));
        if (node.Value("id").OptionalText() is { } id && problems.Value(id).OptionalText() is { } problem)
            into.Add(Label(problem, color: TranscriptColors.Error, size: 12));
    }

    private string Text(string id, string fallback)
    {
        if (!values.TryGetValue(id, out var v))
            values[id] = v = fallback;
        return v as string ?? fallback;
    }

    private void Node(JsonElement node, UIElementCollection into, int depth)
    {
        if (node.ValueKind != JsonValueKind.Object || depth > 16)
            return;
        var type = node.Value("type").Text();
        var id = node.Value("id").OptionalText() ?? "";
        switch (type)
        {
            case "column":
            {
                var gap = Gap(node, 6);
                var column = new StackPanel();
                foreach (var child in node.Value("children").Elements())
                {
                    var before = column.Children.Count;
                    Node(child, column.Children, depth + 1);
                    for (var i = before; i < column.Children.Count; i++)
                        if (column.Children[i] is FrameworkElement e) e.Margin = new Thickness(0, 0, 0, gap);
                }
                into.Add(column);
                break;
            }
            case "row":
            {
                var gap = Gap(node, 8);
                var row = new WrapPanel();
                foreach (var child in node.Value("children").Elements())
                {
                    var holder = new StackPanel { Margin = new Thickness(0, 0, gap, 4), VerticalAlignment = VerticalAlignment.Center };
                    Node(child, holder.Children, depth + 1);
                    row.Children.Add(holder);
                }
                into.Add(row);
                break;
            }
            case "group":
            {
                var inner = new StackPanel();
                if (node.Value("label").OptionalText() is { } label)
                    inner.Children.Add(Label(label, semiBold: true));
                foreach (var child in node.Value("children").Elements())
                    Node(child, inner.Children, depth + 1);
                into.Add(new Border { BorderBrush = TranscriptColors.Rule, BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(6), Padding = new Thickness(10), Child = inner });
                break;
            }
            case "divider":
                into.Add(new Border { Height = 1, Background = TranscriptColors.Rule, Margin = new Thickness(0, 4, 0, 4) });
                break;
            case "text":
            {
                var style = node.Value("style").Text();
                var label = Label(node.Value("text").Text(), semiBold: style == "heading",
                    color: TranscriptColors.Tone(node.Value("tone").Text(), TranscriptColors.Text),
                    size: style switch { "heading" => 16, "caption" => 12, _ => null });
                if (style == "code") label.FontFamily = new FontFamily("Cascadia Mono, Consolas");
                into.Add(label);
                break;
            }
            case "markdown":
                into.Add(Label(node.Value("text").Text()));
                break;
            case "image":
                into.Add(Picture(node));
                break;
            case "link":
            {
                var url = node.Value("url").Text();
                var link = new Hyperlink(new Run(node.Value("label").Text())) { Foreground = TranscriptColors.Link, ToolTip = url };
                link.Click += (_, _) =>
                {
                    if (Uri.TryCreate(url, UriKind.Absolute, out var uri) && (uri.Scheme == Uri.UriSchemeHttps || uri.Scheme == Uri.UriSchemeHttp))
                        System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(uri.AbsoluteUri) { UseShellExecute = true });
                };
                into.Add(new TextBlock(link));
                break;
            }
            case "list":
                foreach (var item in node.Value("items").Elements())
                    into.Add(Label("•  " + item.Text()));
                break;
            case "table":
                into.Add(Table(node));
                break;
            case "progress":
            {
                var bar = new ProgressBar { Height = 6, Minimum = 0, Maximum = 1, MaxWidth = 480, HorizontalAlignment = HorizontalAlignment.Left, Width = 320 };
                if (double.TryParse(node.Value("value").Scalar(), NumberStyles.Float, CultureInfo.InvariantCulture, out var v)) bar.Value = Math.Clamp(v, 0, 1);
                else bar.IsIndeterminate = true;
                into.Add(bar);
                break;
            }
            case "textInput":
            {
                Header(node, into);
                Control editor;
                if (node.Value("secret").Flag())
                {
                    // Masked while typed; kept only in memory until the answer is sent.
                    var password = new PasswordBox { Password = Text(id, ""), IsEnabled = On(node) };
                    password.PasswordChanged += (_, _) => values[id] = password.Password;
                    editor = password;
                }
                else
                {
                    var multiline = node.Value("multiline").Flag();
                    var box = new TextBox { Text = Text(id, node.Value("default").Text()), IsEnabled = On(node), AcceptsReturn = multiline, TextWrapping = multiline ? TextWrapping.Wrap : TextWrapping.NoWrap, MinHeight = multiline ? 60 : 0 };
                    if (int.TryParse(node.Value("maxLength").Scalar(), out var max)) box.MaxLength = max;
                    box.TextChanged += (_, _) => values[id] = box.Text;
                    editor = box;
                }
                editor.MaxWidth = 480;
                editor.HorizontalAlignment = HorizontalAlignment.Stretch;
                AutomationProperties.SetName(editor, node.Value("label").OptionalText() ?? id);
                into.Add(editor);
                Footer(node, into);
                break;
            }
            case "number":
            case "date":
            {
                Header(node, into);
                var box = new TextBox { Text = Text(id, node.Value("default").Scalar() ?? ""), IsEnabled = On(node), Width = type == "date" ? 120 : 100, HorizontalAlignment = HorizontalAlignment.Left };
                if (type == "date") box.ToolTip = "YYYY-MM-DD";
                box.TextChanged += (_, _) => values[id] = box.Text;
                AutomationProperties.SetName(box, node.Value("label").OptionalText() ?? id);
                into.Add(box);
                Footer(node, into);
                break;
            }
            case "slider":
            {
                Header(node, into);
                double.TryParse(node.Value("min").Scalar(), NumberStyles.Float, CultureInfo.InvariantCulture, out var min);
                if (!double.TryParse(node.Value("max").Scalar(), NumberStyles.Float, CultureInfo.InvariantCulture, out var max)) max = min;
                var start = Text(id, node.Value("default").Scalar() ?? min.ToString(CultureInfo.InvariantCulture));
                double.TryParse(start, NumberStyles.Float, CultureInfo.InvariantCulture, out var value);
                var slider = new Slider { Minimum = min, Maximum = Math.Max(min, max), Value = value, Width = 260, IsEnabled = On(node), HorizontalAlignment = HorizontalAlignment.Left };
                if (double.TryParse(node.Value("step").Scalar(), NumberStyles.Float, CultureInfo.InvariantCulture, out var step) && step > 0)
                {
                    slider.TickFrequency = step;
                    slider.IsSnapToTickEnabled = true;
                }
                var shownValue = Label(start, color: TranscriptColors.Muted);
                slider.ValueChanged += (_, e) =>
                {
                    var text = e.NewValue.ToString(CultureInfo.InvariantCulture);
                    values[id] = text;
                    shownValue.Text = text;
                };
                var line = new StackPanel { Orientation = Orientation.Horizontal };
                line.Children.Add(slider);
                shownValue.Margin = new Thickness(8, 0, 0, 0);
                line.Children.Add(shownValue);
                into.Add(line);
                Footer(node, into);
                break;
            }
            case "checkbox":
            case "toggle":
            {
                if (!values.ContainsKey(id)) values[id] = node.Value("default").Flag();
                var box = new CheckBox { Content = Label(node.Value("label").Text()), IsChecked = values[id] is true, IsEnabled = On(node) };
                box.Checked += (_, _) => values[id] = true;
                box.Unchecked += (_, _) => values[id] = false;
                into.Add(box);
                Footer(node, into);
                break;
            }
            case "select":
            {
                Header(node, into);
                var current = Text(id, node.Value("default").OptionalText() ?? "");
                var combo = new ComboBox { IsEnabled = On(node), MinWidth = 200, MaxWidth = 480, HorizontalAlignment = HorizontalAlignment.Left };
                if (node.Value("placeholder").OptionalText() is { } placeholder) combo.ToolTip = placeholder;
                foreach (var option in node.Value("options").Elements())
                {
                    var value = option.Value("value").Text();
                    var item = new ComboBoxItem { Content = option.Value("label").Text(), Tag = value, ToolTip = option.Value("detail").OptionalText() };
                    combo.Items.Add(item);
                    if (value == current) combo.SelectedItem = item;
                }
                combo.SelectionChanged += (_, _) => values[id] = (combo.SelectedItem as ComboBoxItem)?.Tag as string;
                AutomationProperties.SetName(combo, node.Value("label").OptionalText() ?? id);
                into.Add(combo);
                Footer(node, into);
                break;
            }
            case "radio":
            {
                Header(node, into);
                var current = Text(id, node.Value("default").OptionalText() ?? "");
                var group = Guid.NewGuid().ToString("N");
                foreach (var option in node.Value("options").Elements())
                {
                    var value = option.Value("value").Text();
                    var content = new StackPanel();
                    content.Children.Add(Label(option.Value("label").Text()));
                    if (option.Value("detail").OptionalText() is { } detail) content.Children.Add(Label(detail, color: TranscriptColors.Muted, size: 12));
                    var radio = new RadioButton { GroupName = group, Content = content, IsChecked = value == current, IsEnabled = On(node), Margin = new Thickness(0, 0, 0, 3) };
                    radio.Checked += (_, _) => values[id] = value;
                    into.Add(radio);
                }
                Footer(node, into);
                break;
            }
            case "multiSelect":
            {
                Header(node, into);
                if (!values.TryGetValue(id, out var existing) || existing is not List<string>)
                    values[id] = node.Value("default").Elements().Select(e => e.Text()).ToList();
                var chosen = (List<string>)values[id]!;
                foreach (var option in node.Value("options").Elements())
                {
                    var value = option.Value("value").Text();
                    var box = new CheckBox { Content = Label(option.Value("label").Text()), IsChecked = chosen.Contains(value), IsEnabled = On(node), ToolTip = option.Value("detail").OptionalText() };
                    box.Checked += (_, _) => { if (!chosen.Contains(value)) chosen.Add(value); };
                    box.Unchecked += (_, _) => chosen.Remove(value);
                    into.Add(box);
                }
                Footer(node, into);
                break;
            }
            case "button":
            {
                var kind = node.Value("kind").Text();
                var button = new Button
                {
                    Content = node.Value("label").Text(),
                    IsEnabled = enabled,
                    Padding = new Thickness(10, 4, 10, 4),
                    Margin = new Thickness(0, 3, 5, 2),
                    FontWeight = kind == "primary" ? FontWeights.SemiBold : FontWeights.Normal,
                };
                if (kind == "primary")
                    button.Style = (Style)FindResource("YaacPrimaryButton");
                else if (kind == "danger")
                    button.Foreground = TranscriptColors.Error;
                button.Click += (_, _) => Pressed?.Invoke(id);
                into.Add(button);
                break;
            }
            default:
                into.Add(new TextBlock { Text = newerYaac, FontStyle = FontStyles.Italic, Foreground = TranscriptColors.Muted, TextWrapping = TextWrapping.Wrap });
                break;
        }
    }

    private static UIElement Picture(JsonElement node)
    {
        var src = node.Value("src").Text();
        var alt = node.Value("alt").Text();
        // An inline raster is shown; a remote image is not fetched by the window, only linked.
        if (src.StartsWith("data:image/", StringComparison.Ordinal) && src.IndexOf(";base64,", StringComparison.Ordinal) is var at and > 0)
        {
            try
            {
                var bitmap = new BitmapImage();
                bitmap.BeginInit();
                bitmap.CacheOption = BitmapCacheOption.OnLoad;
                bitmap.StreamSource = new System.IO.MemoryStream(Convert.FromBase64String(src[(at + 8)..]));
                bitmap.EndInit();
                var image = new Image { Source = bitmap, Stretch = Stretch.Uniform, HorizontalAlignment = HorizontalAlignment.Left, ToolTip = alt };
                image.MaxHeight = double.TryParse(node.Value("maxHeight").Scalar(), NumberStyles.Float, CultureInfo.InvariantCulture, out var h) ? Math.Clamp(h, 16, 800) : 240;
                AutomationProperties.SetName(image, alt);
                return image;
            }
            catch (Exception)
            {
                // Falls through to the alternative text.
            }
        }
        return new TextBlock { Text = "🖼 " + alt, Foreground = TranscriptColors.Muted, TextWrapping = TextWrapping.Wrap, ToolTip = src.StartsWith("https://", StringComparison.Ordinal) ? src : null };
    }

    private static UIElement Table(JsonElement node)
    {
        var grid = new Grid { HorizontalAlignment = HorizontalAlignment.Left };
        var columns = node.Value("columns").Elements().ToArray();
        for (var c = 0; c < columns.Length; c++) grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var r = 0;
        grid.RowDefinitions.Add(new RowDefinition());
        for (var c = 0; c < columns.Length; c++)
        {
            var head = Label(columns[c].Text(), semiBold: true);
            head.Margin = new Thickness(0, 0, 14, 4);
            Grid.SetColumn(head, c);
            grid.Children.Add(head);
        }
        foreach (var row in node.Value("rows").Elements())
        {
            r++;
            grid.RowDefinitions.Add(new RowDefinition());
            var c = 0;
            foreach (var cell in row.Elements())
            {
                if (c >= columns.Length) break;
                var text = Label(cell.Text());
                text.Margin = new Thickness(0, 0, 14, 2);
                Grid.SetRow(text, r);
                Grid.SetColumn(text, c++);
                grid.Children.Add(text);
            }
        }
        return grid;
    }
}
