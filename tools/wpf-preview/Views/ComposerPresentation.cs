using System.Collections;
using System.Globalization;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Path = System.Windows.Shapes.Path;

namespace Yaac.WpfPreview.Views;

public partial class ComposerView
{
    private enum RowKind { Notice, Queue, Command, Attachment }

    private sealed record ComposerAction(
        string Id, string Label, bool Enabled, bool Checked, string Detail, string Tone, ComposerAction[] Children);

    private sealed class OptionButton : Button
    {
        public ComposerAction? Option { get; set; }
        public Path? SpeedBolt { get; set; }
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

        if (button is not OptionButton { SpeedBolt: not null })
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

    private void RenderProject(JsonElement view)
    {
        var project = ReadAction(Field(view, "project"));
        var worktree = ReadAction(Field(view, "worktree"));
        RenderActions(ProjectActions, project is null ? [] : worktree is null ? [project] : [project, worktree]);
        ProjectActions.Visibility = VisibleWhen(ProjectActions.Children.Count > 0);
    }

    private void RenderControls(JsonElement view)
    {
        var actions = ReadActions(Field(view, "controls"));
        var selectors = actions.Where(a => a.Id != "fork").OrderBy(a => a.Id switch
        {
            "model" => 0, "effort" => 1, "fast" or "speed" => 2, "mode" => 3, "agents" => 4, _ => 5,
        }).ToArray();
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
                AutomationProperties.SetItemStatus(button, action.Checked ? "On" : "");
                button.FontWeight = action.Checked ? FontWeights.SemiBold : FontWeights.Normal;
                if (button.SpeedBolt is { } bolt)
                {
                    var color = action.Tone switch
                    {
                        "ultrafast" => "YaacUltrafast",
                        "fast" => "YaacFast",
                        _ when action.Checked => "YaacFast",
                        _ => "YaacMuted",
                    };
                    var active = color != "YaacMuted";
                    var ink = Palette.Brush(color);
                    bolt.Stroke = ink;
                    bolt.Fill = active ? ink : null;
                    if (active)
                        button.Background = Palette.Brush("YaacSelected");
                    else
                        button.ClearValue(BackgroundProperty);
                    if (action.Id == "speed")
                        AutomationProperties.SetName(button, action.Detail);
                }

                if (button.ContextMenu is { IsOpen: true } menu)
                {
                    if (button.IsEnabled && action.Children.Length > 0)
                        RenderMenu(menu, action.Children);
                    else
                        menu.IsOpen = false;
                }
            });
    }

    // The six corners of the egui bolt, in a 10 × 16 design space.
    private static readonly Geometry BoltOutline = Geometry.Parse("M8,0 L0,8.96 4.8,8.96 2,16 10,7.04 5.2,7.04 Z");
    private static readonly Geometry SpeedChevron = Geometry.Parse("M0,0 L3,3 6,0");

    private static void SetSpeedIcon(OptionButton button, bool menu)
    {
        var bolt = new Path
        {
            Data = BoltOutline, Stretch = Stretch.Uniform, Width = 12, Height = 18,
            StrokeThickness = 1.3, StrokeLineJoin = PenLineJoin.Round,
            VerticalAlignment = VerticalAlignment.Center,
        };
        var content = new StackPanel { Orientation = Orientation.Horizontal, IsHitTestVisible = false };
        content.Children.Add(bolt);
        if (menu)
            content.Children.Add(new Path
            {
                Data = SpeedChevron, Stroke = Palette.Brush("YaacMuted"), StrokeThickness = 1.2,
                StrokeLineJoin = PenLineJoin.Round, Margin = new Thickness(5, 0, 0, 0),
                VerticalAlignment = VerticalAlignment.Center,
            });
        button.SpeedBolt = bolt;
        button.Content = content;
        button.MinWidth = menu ? 40 : 28;
    }

    private OptionButton CreateAction(string id)
    {
        var button = new OptionButton
        {
            Tag = id,
            Style = (Style)FindResource("ComposerButton"),
        };
        if (id is "fast" or "speed")
            SetSpeedIcon(button, id == "speed");
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
            var menu = button.ContextMenu ??= DropdownPlacement.CreateMenu(button, preferAbove: true);
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
        var item = new OptionMenuItem { Tag = id, Style = (Style)FindResource("YaacMenuItem") };
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
        private readonly Image? thumbnail;
        private readonly Button? attachmentCaption;
        private bool imageAttachment;
        private (int Width, int Height, string Pixels) thumbnailFrame = (0, 0, "");

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
                    thumbnail = new Image
                    {
                        Height = 24, MinWidth = 16, MaxWidth = 40, Stretch = Stretch.Uniform,
                        VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(6, 0, 5, 0),
                        Visibility = Visibility.Collapsed,
                    };
                    progress = new ProgressBar { Width = 44, Height = 4, Maximum = 1, Margin = new Thickness(5, 0, 0, 0) };
                    AutomationProperties.SetName(progress, "Upload progress");
                    var caption = new DockPanel();
                    SetDock(imageMark, Dock.Left);
                    SetDock(thumbnail, Dock.Left);
                    caption.Children.Add(imageMark);
                    caption.Children.Add(thumbnail);
                    caption.Children.Add(label);
                    attachmentCaption = new Button
                    {
                        Style = (Style)owner.FindResource("YaacQuietButton"),
                        HorizontalContentAlignment = HorizontalAlignment.Stretch,
                        Padding = new Thickness(0), Content = caption,
                    };
                    attachmentCaption.Click += (_, _) =>
                    {
                        if (imageAttachment)
                            owner.Command?.Invoke("composer.action", new { key = owner.key, id = $"previewAttachment:{id}" });
                    };
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

            Children.Add(attachmentCaption is { } captionButton ? captionButton : label);
        }

        public void Update(JsonElement model)
        {
            var title = Field(model, kind == RowKind.Notice ? "text" : "label").String();
            if (kind == RowKind.Notice)
            {
                if (Field(model, "error").ComposerFlag())
                    label.SetResourceReference(TextBlock.ForegroundProperty, "YaacFailure");
                else
                    label.ClearValue(TextBlock.ForegroundProperty);
            }
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
                imageAttachment = Field(model, "image").ComposerFlag();
                imageMark.Text = imageAttachment ? "▧" : "▤";
                imageMark.ToolTip = imageAttachment ? "Image" : "File";
                if (attachmentCaption is { } preview)
                {
                    preview.IsHitTestVisible = preview.Focusable = preview.IsTabStop = imageAttachment;
                    var openLabel = imageAttachment ? $"Open {title}" : title;
                    preview.ToolTip = imageAttachment ? openLabel : null;
                    AutomationProperties.SetName(preview, openLabel);
                }
                UpdateThumbnail(Field(model, "thumbnail"));
            }
            DisplayText(detail, detailText);
            detail.ToolTip = detailText.Length > 0 ? detailText : null;
        }

        private void UpdateThumbnail(JsonElement model)
        {
            if (thumbnail is null || imageMark is null) return;
            var pixels = Field(model, "pixels").String();
            var width = Field(model, "width").ComposerInt();
            var height = Field(model, "height").ComposerInt();
            if ((width, height, pixels) == thumbnailFrame) return;
            thumbnailFrame = (width, height, pixels);
            thumbnail.Source = null;

            // The shared worker already decoded and shrank the image.
            if (width is > 0 and <= 160 && height is > 0 and <= 160)
            {
                var stride = width * 4;
                var bytes = new byte[stride * height];
                if (Convert.TryFromBase64String(pixels, bytes, out var written) && written == bytes.Length)
                {
                    var bitmap = BitmapSource.Create(width, height, 96, 96, PixelFormats.Pbgra32, null, bytes, stride);
                    bitmap.Freeze();
                    thumbnail.Source = bitmap;
                }
            }
            thumbnail.Visibility = VisibleWhen(thumbnail.Source is not null);
            imageMark.Visibility = VisibleWhen(thumbnail.Source is null);
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
