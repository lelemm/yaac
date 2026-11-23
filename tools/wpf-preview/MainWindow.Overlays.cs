using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private UIElement? commanderPreviousFocus;
    private bool renderingCommander;
    private string? commanderStep;
    private string commanderRenderedQuery = "";
    private TextBlock? dialogBody;
    private Button? dialogSubmit;
    private StackPanel? dialogFields;
    private string? fieldSignature;
    private readonly Dictionary<string, Func<object?>> fieldValues = new();

    private Window Overlay(string title, double width, double maxHeight) => new()
    {
        Owner = this,
        Title = title,
        Width = width,
        MaxHeight = maxHeight,
        SizeToContent = SizeToContent.Height,
        WindowStartupLocation = WindowStartupLocation.CenterOwner,
        ShowInTaskbar = false,
        FontFamily = FontFamily,
        FontSize = FontSize,
        Background = Palette.Brush("YaacCanvas"),
        Foreground = Palette.Brush("YaacText"),
    };

    private void OpenCommander() => Send(CommanderOverlay.IsVisible ? "commander.close" : "commander.open");

    private void RenderCommander(JsonElement data)
    {
        if (data.ValueKind != JsonValueKind.Object)
        {
            if (CommanderOverlay.IsVisible)
            {
                CommanderOverlay.Visibility = Visibility.Collapsed;
                if (commanderPreviousFocus is { IsVisible: true, IsEnabled: true } previousFocus)
                    previousFocus.Focus();
                commanderPreviousFocus = null;
            }
            return;
        }
        var query = Text(data, "query");
        var step = Text(data, "step");
        var opening = !CommanderOverlay.IsVisible;
        renderingCommander = true;
        try
        {
            if (opening)
            {
                commanderPreviousFocus = Keyboard.FocusedElement as UIElement;
                AgentsPane.DismissFloatingCard();
                CommanderOverlay.Visibility = Visibility.Visible;
            }
            if (opening || commanderStep != step)
            {
                commanderStep = step;
                CommanderQuery.Text = query;
                CommanderQuery.CaretIndex = query.Length;
                CommanderQuery.Focus();
            }
            var previous = (CommanderRows.SelectedItem as ListBoxItem)?.Tag as string;
            commanderRenderedQuery = query;
            CommanderRows.Items.Clear();
            if (data.TryGetProperty("rows", out var rows))
            {
                foreach (var row in rows.EnumerateArray())
                {
                    var label = (Flag(row, "checked") ? "✓ " : "") + Text(row, "label");
                    var key = Text(row, "key");
                    var detail = Text(row, "detail");
                    var heading = Text(row, "heading");
                    var title = string.IsNullOrEmpty(key) ? label : $"{label}  ·  {key}";
                    if (!string.IsNullOrEmpty(heading))
                        CommanderRows.Items.Add(new ListBoxItem { Content = heading, IsEnabled = false, Opacity = .68 });
                    CommanderRows.Items.Add(new ListBoxItem
                    {
                        Content = title,
                        ToolTip = detail,
                        Tag = Text(row, "id"),
                        IsEnabled = Flag(row, "enabled"),
                    });
                }
            }
            CommanderRows.SelectedItem = CommanderRows.Items.OfType<ListBoxItem>()
                .FirstOrDefault(item => item.IsEnabled && Equals(item.Tag, previous))
                ?? CommanderRows.Items.OfType<ListBoxItem>().FirstOrDefault(item => item.IsEnabled);
        }
        finally
        {
            renderingCommander = false;
        }
    }

    private void CommanderQueryChanged(object sender, TextChangedEventArgs e)
    {
        if (!renderingCommander && CommanderOverlay.IsVisible)
            Send("commander.query", new { text = CommanderQuery.Text });
    }

    private void CommanderOutsideClicked(object sender, MouseButtonEventArgs e)
    {
        Send("commander.close");
        e.Handled = true;
    }

    private void CommanderKeyDown(object sender, KeyEventArgs e)
    {
        switch (e.Key)
        {
            case Key.Escape:
                Send("commander.close");
                break;
            case Key.Enter:
                PickCommand();
                break;
            case Key.Back when CommanderQuery.Text.Length == 0 && !string.IsNullOrEmpty(commanderStep):
                Send("commander.back");
                break;
            case Key.Down:
            case Key.Up:
                MoveCommand(e.Key == Key.Down ? 1 : -1);
                break;
            default:
                return;
        }
        e.Handled = true;
    }

    private void CommanderRowClicked(object sender, MouseButtonEventArgs e)
    {
        if (e.OriginalSource is not DependencyObject source || ItemsControl.ContainerFromElement(CommanderRows, source)
            is not ListBoxItem { IsEnabled: true } item)
            return;
        CommanderRows.SelectedItem = item;
        PickCommand();
        e.Handled = true;
    }

    private void MoveCommand(int direction)
    {
        for (var index = CommanderRows.SelectedIndex + direction;
             index >= 0 && index < CommanderRows.Items.Count; index += direction)
        {
            if (CommanderRows.Items[index] is ListBoxItem item && item.IsEnabled)
            {
                CommanderRows.SelectedItem = item;
                CommanderRows.ScrollIntoView(item);
                return;
            }
        }
    }

    private void PickCommand()
    {
        if (CommanderRows.SelectedItem is ListBoxItem { IsEnabled: true, Tag: string id }
            && CommanderQuery.Text == commanderRenderedQuery)
            Send("commander.pick", new { id, query = commanderRenderedQuery });
    }

    private void RenderDialog(JsonElement data)
    {
        var id = Text(data, "id");
        if (activeDialog != id && dialog != null)
        {
            var previous = dialog;
            dialog = null;
            previous.Close();
        }
        activeDialog = id;
        if (id.Length == 0) return;
        if (dialog == null)
        {
            fieldSignature = null;
            dialog = Overlay(Text(data, "title"), 560, Math.Max(360, ActualHeight - 50));
            var content = new DockPanel { Margin = new Thickness(16) };
            var buttons = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };
            DockPanel.SetDock(buttons, Dock.Bottom);
            dialogSubmit = new Button
            {
                Style = (Style)FindResource("YaacPrimaryButton"),
                IsDefault = true, MinWidth = 90, Margin = new Thickness(0, 10, 8, 0),
            };
            dialogSubmit.Click += (_, _) => Send("dialog.submit", new
            {
                id,
                values = fieldValues.ToDictionary(field => field.Key, field => field.Value()),
            });
            var cancel = new Button { Content = "Close", IsCancel = true, MinWidth = 80, Margin = new Thickness(0, 10, 0, 0) };
            cancel.Click += (_, _) => Send("dialog.cancel", new { id });
            buttons.Children.Add(dialogSubmit);
            buttons.Children.Add(cancel);
            content.Children.Add(buttons);
            dialogBody = new TextBlock { TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 0, 0, 8) };
            dialogFields = new StackPanel();
            var form = new StackPanel();
            form.Children.Add(dialogBody);
            form.Children.Add(dialogFields);
            content.Children.Add(new ScrollViewer { Content = form, VerticalScrollBarVisibility = ScrollBarVisibility.Auto });
            dialog.Content = content;
            var window = dialog;
            window.Closing += (_, e) =>
            {
                if (ReferenceEquals(dialog, window) && !closed)
                {
                    e.Cancel = true;
                    Send("dialog.cancel", new { id });
                }
            };
            dialog.Show();
        }
        dialogBody!.Text = Text(data, "body");
        var submit = Text(data, "submit");
        dialogSubmit!.Content = submit;
        dialogSubmit.Visibility = submit.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        if (!data.TryGetProperty("fields", out var fields)) return;
        var signature = fields.GetRawText();
        if (fieldSignature == signature) return;
        fieldSignature = signature;
        var drafts = fieldValues.ToDictionary(field => field.Key, field => field.Value());
        fieldValues.Clear();
        dialogFields!.Children.Clear();
        foreach (var field in fields.EnumerateArray())
        {
            var fieldId = Text(field, "id");
            var label = Text(field, "label");
            var type = Text(field, "type");
            if (type == "check")
            {
                var check = new CheckBox { Content = label, Margin = new Thickness(0, 4, 0, 7), IsChecked = drafts.TryGetValue(fieldId, out var lastCheck) ? lastCheck is true : Flag(field, "value") };
                dialogFields.Children.Add(check);
                fieldValues[fieldId] = () => check.IsChecked;
                continue;
            }
            dialogFields.Children.Add(new TextBlock { Text = label });
            if (type == "choice" && field.TryGetProperty("options", out var options))
            {
                var choice = new ComboBox { Margin = new Thickness(0, 2, 0, 7) };
                var selected = Text(field, "value");
                foreach (var option in options.EnumerateArray())
                {
                    var item = new ComboBoxItem { Content = Text(option, "label"), Tag = Text(option, "id") };
                    choice.Items.Add(item);
                    if (Equals(item.Tag, selected)) choice.SelectedItem = item;
                }
                fieldValues[fieldId] = () => (choice.SelectedItem as ComboBoxItem)?.Tag;
                choice.SelectionChanged += (_, _) => Send("dialog.change", new
                {
                    id,
                    values = new Dictionary<string, object?> { [fieldId] = fieldValues[fieldId]() },
                });
                dialogFields.Children.Add(choice);
            }
            else
            {
                var box = new TextBox
                {
                    Text = drafts.TryGetValue(fieldId, out var prior) && prior is string text ? text : Text(field, "value"),
                    Margin = new Thickness(0, 2, 0, 7),
                    TextWrapping = TextWrapping.Wrap,
                    AcceptsReturn = type == "multiline",
                    MinHeight = type == "multiline" ? 65 : 25,
                };
                fieldValues[fieldId] = () => box.Text;
                dialogFields.Children.Add(box);
            }
        }
    }
}
