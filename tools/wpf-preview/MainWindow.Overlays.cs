using System.Globalization;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private UIElement? commanderPreviousFocus;
    private bool renderingCommander;
    private string? commanderStep;
    private string commanderRenderedQuery = "";
    private TextBlock? dialogBody;
    private TextBlock? dialogHint;
    private Button? dialogSubmit;
    private Button? dialogCancel;
    private StackPanel? dialogFields;
    private StackPanel? dialogExtras;
    private PullRequestForm? pullRequestForm;
    private string? fieldSignature;
    private string? dialogFieldRevision;
    private string? dialogExtrasSignature;
    private readonly Dictionary<string, Func<object?>> fieldValues = new();

    private const double DialogWidth = 560;

    private Window Overlay(string title, double maxHeight) => new()
    {
        Owner = this,
        Title = title,
        Width = DialogWidth * fontScale,
        MaxHeight = maxHeight,
        SizeToContent = SizeToContent.Height,
        WindowStartupLocation = WindowStartupLocation.CenterOwner,
        ShowInTaskbar = false,
        FontFamily = FontFamily,
        FontSize = FontSize,
        Background = Palette.Brush("YaacCanvas"),
        Foreground = Palette.Brush("YaacText"),
    };

    private void UpdateDialogWidth()
    {
        if (dialog is not null)
            dialog.Width = DialogWidth * fontScale;
    }

    private void OpenCommander()
    {
        if (CommanderOverlay.IsVisible)
        {
            Send("commander.close");
            return;
        }
        var approval = TranscriptPane.FocusedApproval;
        Send("commander.open", new { card = approval?.ActionId, threadId = approval?.ThreadId });
    }

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
        var instance = Text(data, "instance");
        var key = instance.Length == 0 ? id : instance;
        if (activeDialog != key)
        {
            var previous = dialog;
            dialog = null;
            pullRequestForm = null;
            fieldValues.Clear();
            previous?.Close();
        }
        activeDialog = key;
        if (id.Length == 0) return;
        if (dialog == null)
        {
            fieldSignature = null;
            dialogFieldRevision = null;
            dialogExtrasSignature = null;
            dialog = Overlay(Text(data, "title"), Math.Max(360, ActualHeight - 50));
            var content = new DockPanel();
            var surface = new Border { Padding = new Thickness(16), Child = content };
            surface.SetResourceReference(LayoutTransformProperty, "YaacUiScale");
            var buttons = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };
            DockPanel.SetDock(buttons, Dock.Bottom);
            dialogSubmit = new Button
            {
                Style = (Style)FindResource("YaacPrimaryButton"),
                IsDefault = id is not ("mode" or "ultrafast"), MinWidth = 90, Margin = new Thickness(0, 10, 8, 0),
            };
            dialogSubmit.Click += (_, _) => Send("dialog.submit", new
            {
                id,
                instance,
                values = pullRequestForm is { } pullRequest
                    ? new Dictionary<string, object?> { ["draft"] = pullRequest.Draft }
                    : fieldValues.ToDictionary(field => field.Key, field => field.Value()),
            });
            var cancel = new Button { Content = id is "mode" or "ultrafast" or "pullRequest" or "project" or "mute" or "fork" or "quit" ? "Cancel" : "Close",
                IsCancel = id != "updateNotice", Visibility = id == "updateNotice" ? Visibility.Collapsed : Visibility.Visible,
                MinWidth = 80, Margin = new Thickness(0, 10, 0, 0) };
            dialogCancel = cancel;
            cancel.Click += (_, _) => Send("dialog.cancel", new { id, instance });
            buttons.Children.Add(dialogSubmit);
            buttons.Children.Add(cancel);
            content.Children.Add(buttons);
            dialogBody = new TextBlock { TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 0, 0, 8) };
            dialogHint = new TextBlock { TextWrapping = TextWrapping.Wrap, Margin = new Thickness(0, 6, 0, 0) };
            dialogFields = new StackPanel();
            dialogExtras = new StackPanel();
            var form = new StackPanel();
            form.Children.Add(dialogBody);
            if (id == "pullRequest")
            {
                pullRequestForm = new PullRequestForm(this, instance);
                form.Children.Add(pullRequestForm);
            }
            else
            {
                form.Children.Add(dialogFields);
                form.Children.Add(dialogExtras);
                form.Children.Add(dialogHint);
            }
            content.Children.Add(new ScrollViewer { Content = form, VerticalScrollBarVisibility = ScrollBarVisibility.Auto });
            dialog.Content = surface;
            var window = dialog;
            window.PreviewKeyDown += (_, e) =>
            {
                if (id is "mode" or "ultrafast" && e.Key == Key.Enter)
                    e.Handled = true;
                if (id == "updateNotice" && e.Key == Key.Escape)
                {
                    Send("dialog.cancel", new { id, instance });
                    e.Handled = true;
                }
            };
            window.Closing += (_, e) =>
            {
                if (ReferenceEquals(dialog, window) && !closed)
                {
                    e.Cancel = true;
                    if (cancel.IsEnabled)
                        Send("dialog.cancel", new { id, instance });
                }
            };
            dialog.Show();
            if (id is "mode" or "ultrafast")
                cancel.Focus();
        }
        dialogBody!.Text = Text(data, "body");
        var submit = Text(data, "submit");
        var blocked = Text(data, "submitBlocked");
        dialogSubmit!.Content = submit;
        dialogSubmit.Visibility = submit.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        dialogSubmit.IsEnabled = blocked.Length == 0;
        dialogSubmit.ToolTip = blocked.Length == 0 ? null : blocked;
        ToolTipService.SetShowOnDisabled(dialogSubmit, true);
        if (pullRequestForm is { } pr)
        {
            var canCancel = !Flag(data, "creating");
            dialogCancel!.IsEnabled = canCancel;
            dialogCancel.IsCancel = canCancel;
            pr.Render(data);
            return;
        }
        var hint = Text(data, "hint");
        dialogHint!.Text = hint.Length == 0 ? blocked : blocked.Length == 0 ? hint : $"{hint}\n{blocked}";
        dialogHint.Visibility = dialogHint.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        dialogHint.SetResourceReference(TextBlock.ForegroundProperty, blocked.Length == 0 ? "YaacMuted" : "YaacFailure");
        RenderDialogExtras(data, id, instance);
        if (!data.TryGetProperty("fields", out var fields)) return;
        var signature = string.Join("\n", fields.EnumerateArray().Select(field => Flag(field, "live")
            ? $"{Text(field, "id")} · {Text(field, "type")} · {Text(field, "minDate")}"
            : field.GetRawText()));
        var revision = data.TryGetProperty("fieldRevision", out var revisionValue) ? revisionValue.GetRawText() : "";
        var replace = dialogFieldRevision != revision;
        if (!replace && fieldSignature == signature) return;
        fieldSignature = signature;
        dialogFieldRevision = revision;
        var drafts = replace ? new Dictionary<string, object?>() : fieldValues.ToDictionary(field => field.Key, field => field.Value());
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
                    var reason = Text(option, "detail");
                    var item = new ComboBoxItem { Content = Text(option, "label"), Tag = Text(option, "id"),
                        IsEnabled = !Flag(option, "disabled"), ToolTip = reason.Length == 0 ? null : reason };
                    ToolTipService.SetShowOnDisabled(item, true);
                    choice.Items.Add(item);
                    if (Equals(item.Tag, selected)) choice.SelectedItem = item;
                }
                fieldValues[fieldId] = () => (choice.SelectedItem as ComboBoxItem)?.Tag;
                choice.SelectionChanged += (_, _) => Send("dialog.change", new
                {
                    id, instance,
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
                AutomationProperties.SetName(box, label);
                fieldValues[fieldId] = () => box.Text;
                if (Flag(field, "live"))
                    box.TextChanged += (_, _) => Send("dialog.change", new
                    {
                        id, instance, values = new Dictionary<string, object?> { [fieldId] = box.Text },
                    });
                dialogFields.Children.Add(type == "date" ? DialogDateField(box, Text(field, "minDate")) : box);
            }
        }
    }

    private void RenderDialogExtras(JsonElement data, string id, string instance)
    {
        bool hasActions = data.TryGetProperty("auxiliaryActions", out var actions);
        bool hasFolders = data.TryGetProperty("folders", out var folders);
        var signature = (hasActions ? actions.GetRawText() : "") + (hasFolders ? folders.GetRawText() : "");
        if (signature == dialogExtrasSignature) return;
        dialogExtrasSignature = signature;
        dialogExtras!.Children.Clear();

        void AddActions(Panel host, JsonElement descriptors)
        {
            foreach (var item in descriptors.EnumerateArray())
            {
                var action = Text(item, "id");
                var path = Text(item, "path");
                var button = new Button
                {
                    Content = Text(item, "label"), IsEnabled = Flag(item, "enabled"),
                    HorizontalAlignment = HorizontalAlignment.Left, Margin = new Thickness(0, 3, 6, 0),
                };
                button.Click += (_, _) => Send("dialog.action", new
                {
                    id, instance, action, path, values = fieldValues.ToDictionary(field => field.Key, field => field.Value()),
                });
                host.Children.Add(button);
            }
        }

        if (hasActions)
        {
            var links = new WrapPanel();
            AddActions(links, actions);
            dialogExtras.Children.Add(links);
        }
        if (hasFolders)
        {
            var list = new StackPanel { Margin = new Thickness(0, 6, 0, 0) };
            list.Children.Add(new TextBlock { Text = Text(folders, "text"), TextWrapping = TextWrapping.Wrap });
            if (folders.TryGetProperty("entries", out var entries))
            {
                var rows = new StackPanel();
                AddActions(rows, entries);
                list.Children.Add(new ScrollViewer { Content = rows, MaxHeight = 148,
                    VerticalScrollBarVisibility = ScrollBarVisibility.Auto });
            }
            var hint = Text(folders, "hint");
            if (hint.Length > 0)
                list.Children.Add(new TextBlock { Text = hint, TextWrapping = TextWrapping.Wrap });
            dialogExtras.Children.Add(list);
        }
    }

    private static DockPanel DialogDateField(TextBox text, string earliest)
    {
        var row = new DockPanel();
        var pick = new Button { Content = "Choose date", Margin = new Thickness(6, 2, 0, 7) };
        var calendar = new System.Windows.Controls.Calendar { SelectionMode = CalendarSelectionMode.SingleDate };
        if (DateTime.TryParseExact(earliest, "yyyy-MM-dd", CultureInfo.InvariantCulture, DateTimeStyles.None, out var start))
            calendar.DisplayDateStart = start;
        var frame = new Border { Child = calendar, Padding = new Thickness(5), CornerRadius = new CornerRadius(5) };
        frame.SetResourceReference(Border.BackgroundProperty, "YaacRaised");
        var popup = new Popup { Child = frame, PlacementTarget = pick, Placement = PlacementMode.Custom,
            CustomPopupPlacementCallback = DropdownPlacement.Below, AllowsTransparency = true, StaysOpen = false };
        bool showing = false;
        pick.Click += (_, _) =>
        {
            showing = true;
            if (DateTime.TryParseExact(text.Text, "yyyy-MM-dd", CultureInfo.InvariantCulture, DateTimeStyles.None, out var date)
                && (calendar.DisplayDateStart is null || date >= calendar.DisplayDateStart))
            {
                calendar.DisplayDate = date;
                calendar.SelectedDate = date;
            }
            showing = false;
            popup.IsOpen = true;
        };
        calendar.SelectedDatesChanged += (_, _) =>
        {
            if (showing || calendar.SelectedDate is not { } selected)
                return;
            text.Text = selected.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture);
            popup.IsOpen = false;
            text.Focus();
        };
        row.Unloaded += (_, _) => popup.IsOpen = false;
        DockPanel.SetDock(pick, Dock.Right);
        row.Children.Add(pick);
        row.Children.Add(text);
        return row;
    }
}
