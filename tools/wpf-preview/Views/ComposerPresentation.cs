using System.Collections;
using System.Globalization;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public partial class ComposerView
{
    private enum RowKind { Notice, Queue, Command, Attachment }

    private sealed record ComposerAction(
        string Id, string Label, bool Enabled, bool Checked, string Detail, string Tone, ComposerAction[] Children);

    private sealed class OptionButton : Button
    {
        public ComposerAction? Option { get; set; }
    }

    private sealed class OptionMenuItem : MenuItem
    {
        public ComposerAction? Option { get; set; }
    }

    private static JsonElement Field(JsonElement owner, string name) =>
        owner.ValueKind == JsonValueKind.Object && owner.TryGetProperty(name, out var value) ? value : default;

    private static JsonElement[] Items(JsonElement value) =>
        value.ValueKind == JsonValueKind.Array ? value.EnumerateArray().ToArray() : [];

    private static Visibility VisibleWhen(bool visible) => visible ? Visibility.Visible : Visibility.Collapsed;

    private static ComposerAction? ReadAction(JsonElement value)
    {
        var id = Field(value, "id").Identifier();
        var label = Field(value, "label").String();
        if (id.Length == 0 || label.Length == 0)
            return null;

        return new ComposerAction(
            id,
            label,
            Field(value, "enabled").ComposerFlag(),
            Field(value, "checked").ComposerFlag(),
            Field(value, "detail").String(),
            Field(value, "tone").String(),
            ReadActions(Field(value, "children")));
    }

    private static ComposerAction[] ReadActions(JsonElement value) =>
        Items(value).Select(ReadAction).OfType<ComposerAction>().ToArray();

    private void InvokeAction(ComposerAction? action)
    {
        if (action is { Enabled: true })
            Command?.Invoke("composer.action", new { key, id = action.Id });
    }

    private static void DisplayText(TextBlock block, string text)
    {
        block.Text = text;
        block.Visibility = VisibleWhen(text.Length > 0);
    }

    private static void SetButton(Button button, ComposerAction? action, bool available = true,
        string? shortcut = null, bool showShortcut = false)
    {
        button.Visibility = VisibleWhen(action is not null);
        button.IsEnabled = available && action?.Enabled == true;
        if (action is null)
            return;

        button.Content = showShortcut
            ? $"{action.Label}  {(shortcut == "Enter" ? "↵" : shortcut)}"
            : action.Children.Length > 0 ? action.Label + "  ▾" : action.Label;
        AutomationProperties.SetName(button, action.Label);
        AutomationProperties.SetAcceleratorKey(button, shortcut ?? "");
        var tip = action.Detail.Length == 0 ? action.Label : action.Detail;
        button.ToolTip = shortcut is null ? tip : $"{tip} ({shortcut})";
    }

    private void RenderStatus(JsonElement value)
    {
        DisplayText(StatusText, Field(value, "text").String());
        RenderActions(StatusActions, ReadActions(Field(value, "actions")));
        StatusActions.Visibility = VisibleWhen(StatusActions.Children.Count > 0);
        StatusPlate.Visibility = VisibleWhen(StatusText.Visibility == Visibility.Visible || StatusActions.Children.Count > 0);
    }

    private void RenderProject(ComposerAction? project)
    {
        RenderActions(ProjectActions, project is null ? [] : [project]);
        ProjectActions.Visibility = VisibleWhen(ProjectActions.Children.Count > 0);
    }

    private void RenderControls(JsonElement view)
    {
        var actions = ReadActions(Field(view, "controls"));
        var selectors = actions.Where(a => a.Id != "fork").OrderBy(a => a.Id switch
        {
            "model" => 0, "effort" => 1, "fast" or "speed" => 2, "mode" => 3, "agents" => 4, _ => 5,
        }).Select(a => a.Id == "fast" ? a with { Label = "ϟ", Detail = "Fast mode" } : a).ToArray();
        RenderActions(ControlActions, selectors);
        RenderActions(ForkActions, actions.Where(a => a.Id == "fork").ToArray());

        var percent = Field(view, "contextPercent");
        var hasPercent = percent.ValueKind == JsonValueKind.Number && percent.TryGetDouble(out _);
        ContextMeter.Visibility = VisibleWhen(hasPercent);
        if (hasPercent)
        {
            ContextProgress.Value = Math.Clamp(percent.GetDouble(), 0, 100);
            ContextLabel.Text = ContextProgress.Value.ToString("0", CultureInfo.CurrentCulture) + "%";
        }
        DisplayText(ControlNote, Field(view, "note").String());
        ControlActions.Visibility = VisibleWhen(actions.Length > 0 || hasPercent || ControlNote.Visibility == Visibility.Visible);
    }

    private void RenderActions(Panel panel, ComposerAction[] actions, bool available = true)
    {
        Reconcile<OptionButton, ComposerAction>(
            panel.Children, actions, action => action.Id, CreateAction,
            (button, action) =>
            {
                button.Option = action;
                SetButton(button, action, available);
                if (action.Id == "fast") AutomationProperties.SetName(button, "Fast mode");
                if (action.Id == "speed") AutomationProperties.SetName(button, action.Detail);
                AutomationProperties.SetItemStatus(button, action.Checked ? "On" : "");
                button.FontWeight = action.Checked ? FontWeights.SemiBold : FontWeights.Normal;
                if (action.Tone == "ultrafast")
                    button.Foreground = (Brush)FindResource("YaacUltrafast");
                else if (action.Tone == "fast" || action.Id == "fast" && action.Checked)
                    button.Foreground = (Brush)FindResource("YaacFast");
                else
                    button.ClearValue(ForegroundProperty);

                if (button.ContextMenu is { IsOpen: true } menu)
                {
                    if (button.IsEnabled && action.Children.Length > 0)
                        RenderMenu(menu, action.Children);
                    else
                        menu.IsOpen = false;
                }
            });
    }

    private OptionButton CreateAction(string id)
    {
        var button = new OptionButton
        {
            Tag = id,
            Style = (Style)FindResource("ComposerButton"),
        };
        button.Unloaded += (_, _) =>
        {
            if (button.ContextMenu is { } menu)
                menu.IsOpen = false;
        };
        button.Click += (_, _) =>
        {
            if (button.Option is not { Enabled: true } action)
                return;
            if (action.Children.Length == 0)
            {
                InvokeAction(action);
                return;
            }
            var menu = button.ContextMenu ??= new ContextMenu();
            menu.PlacementTarget = button;
            menu.Placement = System.Windows.Controls.Primitives.PlacementMode.Top;
            RenderMenu(menu, action.Children);
            menu.IsOpen = true;
        };
        return button;
    }

    private void RenderMenu(ItemsControl owner, ComposerAction[] actions)
    {
        Reconcile<OptionMenuItem, ComposerAction>(
            owner.Items, actions, action => action.Id, CreateMenuItem,
            (item, action) =>
            {
                item.Header = action.Label;
                item.IsEnabled = action.Enabled;
                item.IsCheckable = action.Checked;
                item.IsChecked = action.Checked;
                item.ToolTip = action.Detail.Length == 0 ? null : action.Detail;
                AutomationProperties.SetHelpText(item, action.Detail);
                item.Option = action;
                RenderMenu(item, action.Children);
            });
    }

    private OptionMenuItem CreateMenuItem(string id)
    {
        var item = new OptionMenuItem { Tag = id, Style = TryFindResource(typeof(MenuItem)) as Style };
        ToolTipService.SetShowOnDisabled(item, true);
        item.Click += (_, e) =>
        {
            if (ReferenceEquals(e.OriginalSource, item) && item.Option is { } action && action.Children.Length == 0)
            {
                item.IsChecked = action.Checked;
                InvokeAction(action);
            }
        };
        return item;
    }

    private ComposerAction? closeCommandLogs;

    private void RenderRunningCommands(JsonElement view, bool keyChanged)
    {
        var title = Field(view, "title").String();
        CommandArea.Visibility = VisibleWhen(title.Length > 0);
        if (title.Length == 0) return;

        CommandTitle.Content = title;
        var expanded = Field(view, "expanded").ComposerFlag();
        AutomationProperties.SetName(CommandTitle, title);
        RenderRows(CommandRows, Field(view, "rows"), RowKind.Command);
        CommandScroller.Visibility = VisibleWhen(expanded && CommandRows.Children.Count > 0);
        if (keyChanged) CommandScroller.ScrollToTop();

        var log = Field(view, "log");
        closeCommandLogs = ReadAction(Field(log, "close"));
        CommandLogArea.Visibility = VisibleWhen(expanded && closeCommandLogs is not null);
        SetButton(CommandLogClose, closeCommandLogs);
        DisplayText(CommandLogTitle, Field(log, "title").String());
        DisplayText(CommandLogStatus, Field(log, "status").String());
        var output = Field(log, "output").String();
        CommandOutput.Visibility = VisibleWhen(output.Length > 0);
        if (CommandOutput.Text != output)
        {
            var follow = keyChanged || CommandOutput.Text.Length == 0
                || CommandOutput.VerticalOffset + CommandOutput.ViewportHeight >= CommandOutput.ExtentHeight - 2;
            CommandOutput.Text = output;
            if (follow) CommandOutput.ScrollToEnd();
        }
    }

    private void ToggleCommandsClicked(object sender, RoutedEventArgs e) =>
        Command?.Invoke("composer.action", new { key, id = "toggleCommands" });

    private void CloseCommandLogsClicked(object sender, RoutedEventArgs e) => InvokeAction(closeCommandLogs);

    private void RenderRows(Panel panel, JsonElement value, RowKind kind)
    {
        Reconcile<ComposerRow, JsonElement>(
            panel.Children, Items(value), row => Field(row, "id").Identifier(),
            id => new ComposerRow(this, id, kind),
            (row, model) => row.Update(model));
    }

    private sealed class ComposerRow : DockPanel
    {
        private readonly ComposerView owner;
        private readonly RowKind kind;
        private readonly TextBlock label = new() { VerticalAlignment = VerticalAlignment.Center };
        private readonly TextBlock detail = new() { VerticalAlignment = VerticalAlignment.Center };
        private readonly Panel actions;
        private readonly ProgressBar? progress;
        private readonly TextBlock? imageMark;

        public ComposerRow(ComposerView owner, string id, RowKind kind)
        {
            this.owner = owner;
            this.kind = kind;
            Tag = id;
            detail.Style = (Style)owner.FindResource("ComposerQuietText");
            detail.Margin = new Thickness(5, 0, 5, 0);
            actions = kind is RowKind.Queue or RowKind.Command
                ? new StackPanel { Orientation = Orientation.Horizontal }
                : new WrapPanel();

            switch (kind)
            {
                case RowKind.Command:
                    label.SetResourceReference(TextBlock.ForegroundProperty, "YaacMuted");
                    label.FontSize = 11;
                    label.FontFamily = YaacFonts.Mono;
                    detail.FontSize = 11;
                    goto case RowKind.Queue;
                case RowKind.Queue:
                    label.TextTrimming = TextTrimming.CharacterEllipsis;
                    detail.TextTrimming = TextTrimming.CharacterEllipsis;
                    AddDocked(actions, Dock.Right);
                    AddDocked(detail, Dock.Right);
                    Margin = new Thickness(0, 0, 0, 1);
                    break;
                case RowKind.Attachment:
                    label.TextTrimming = TextTrimming.CharacterEllipsis;
                    MaxWidth = 390;
                    Background = (Brush)owner.FindResource("YaacRaised");
                    Margin = new Thickness(0, 0, 5, 4);
                    imageMark = new TextBlock { Margin = new Thickness(6, 0, 5, 0), VerticalAlignment = VerticalAlignment.Center };
                    progress = new ProgressBar { Width = 44, Height = 4, Maximum = 1, Margin = new Thickness(5, 0, 0, 0) };
                    AutomationProperties.SetName(progress, "Upload progress");
                    AddDocked(imageMark, Dock.Left);
                    AddDocked(actions, Dock.Bottom);
                    AddDocked(progress, Dock.Right);
                    AddDocked(detail, Dock.Right);
                    break;
                case RowKind.Notice:
                    label.TextWrapping = TextWrapping.Wrap;
                    label.Style = (Style)owner.FindResource("ComposerQuietText");
                    Margin = new Thickness(0, 0, 0, 5);
                    AddDocked(actions, Dock.Bottom);
                    AddDocked(detail, Dock.Bottom);
                    break;
            }

            Children.Add(label);
        }

        public void Update(JsonElement model)
        {
            var title = Field(model, kind == RowKind.Notice ? "text" : "label").String();
            label.Text = kind is RowKind.Queue or RowKind.Command ? title.Split(['\r', '\n'], 2)[0] : title;
            label.ToolTip = title.Length > 0 ? title : null;
            var busy = Field(model, "busy").ComposerFlag();
            Opacity = busy ? 0.65 : 1;
            var buttons = ReadActions(Field(model, "actions"));
            owner.RenderActions(actions, buttons, !busy);
            actions.Visibility = VisibleWhen(buttons.Length > 0);

            var detailText = Field(model, "detail").String();
            if (progress is not null && imageMark is not null)
            {
                var fraction = Field(model, "progress");
                var hasProgress = fraction.ValueKind == JsonValueKind.Number && fraction.TryGetDouble(out _);
                progress.Visibility = VisibleWhen(hasProgress);
                if (hasProgress)
                {
                    progress.Value = Math.Clamp(fraction.GetDouble(), 0, 1);
                    if (detailText.Length == 0)
                        detailText = progress.Value.ToString("P0", CultureInfo.CurrentCulture);
                }
                var image = Field(model, "image").ComposerFlag();
                imageMark.Text = image ? "▧" : "▤";
                imageMark.ToolTip = image ? "Image" : "File";
            }
            DisplayText(detail, detailText);
            detail.ToolTip = detailText.Length > 0 ? detailText : null;
        }

        private void AddDocked(UIElement element, Dock position)
        {
            SetDock(element, position);
            Children.Add(element);
        }
    }

    // Remove departed keys first. A surviving row following them can then stay parented and
    // retain its focus even when the first item in a queue was delivered.
    private static void Reconcile<TControl, TData>(IList children, IEnumerable<TData> source,
        Func<TData, string> getId, Func<string, TControl> create, Action<TControl, TData> update)
        where TControl : FrameworkElement
    {
        var entries = source.Select(value => (Id: getId(value), Value: value))
            .Where(entry => entry.Id.Length > 0).DistinctBy(entry => entry.Id).ToArray();
        var present = entries.Select(entry => entry.Id).ToHashSet(StringComparer.Ordinal);
        for (var index = children.Count - 1; index >= 0; index--)
        {
            if (children[index] is TControl control && control.Tag is string id && !present.Contains(id))
                children.RemoveAt(index);
        }

        for (var index = 0; index < entries.Length; index++)
        {
            var entry = entries[index];
            if (index < children.Count && children[index] is TControl current && Equals(current.Tag, entry.Id))
            {
                update(current, entry.Value);
                continue;
            }

            var existing = children.OfType<TControl>().FirstOrDefault(control => Equals(control.Tag, entry.Id));
            if (existing is not null)
                children.Remove(existing);
            var next = existing ?? create(entry.Id);
            children.Insert(index, next);
            update(next, entry.Value);
        }
    }
}

internal static class ComposerJson
{
    public static string String(this JsonElement value, string fallback = "") =>
        value.ValueKind == JsonValueKind.String ? value.GetString() ?? fallback : fallback;

    public static string Identifier(this JsonElement value) =>
        value.ValueKind == JsonValueKind.Number ? value.GetRawText() : value.String();

    public static int ComposerInt(this JsonElement value, int fallback = 0) =>
        value.ValueKind == JsonValueKind.Number && value.TryGetInt32(out var n) ? n : fallback;

    public static bool ComposerFlag(this JsonElement value, bool fallback = false) => value.ValueKind switch
    {
        JsonValueKind.True => true,
        JsonValueKind.False => false,
        _ => fallback,
    };
}
