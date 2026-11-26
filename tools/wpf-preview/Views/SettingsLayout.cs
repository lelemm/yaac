using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace Yaac.WpfPreview.Views;

public partial class SettingsView
{
    private static TextBlock Text(double size = 13)
    {
        var block = new TextBlock { TextWrapping = TextWrapping.Wrap };
        SetTextSize(block, size);
        return block;
    }

    private static TextBlock Heading(double size)
    {
        var heading = Text(size);
        heading.FontWeight = FontWeights.SemiBold;
        return heading;
    }

    private static void SayRich(TextBlock block, string text)
    {
        if (!Equals(block.Tag, text))
        {
            block.Tag = text;
            var ticks = text.Count(ch => ch == '`');
            if (ticks > 0 && ticks % 2 == 0)
            {
                block.Inlines.Clear();
                var spans = text.Split('`');
                for (var i = 0; i < spans.Length; i++)
                {
                    var run = new Run(spans[i]);
                    if (i % 2 != 0)
                        run.SetResourceReference(TextElement.FontFamilyProperty, YaacFonts.CodeResource);
                    block.Inlines.Add(run);
                }
            }
            else
            {
                block.Text = text;
            }
        }
        block.Visibility = text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void SayDetail(TextBlock block, string text)
    {
        SayRich(block, text);
        block.Foreground = Tone("");
    }

    private sealed class Surface
    {
        private readonly SettingsView owner;
        private readonly TextBlock title = Heading(28);
        private readonly TextBlock detail = Text(14);
        private readonly TextBlock footer = Text(13);
        private readonly Panel sectionHost;
        private readonly Dictionary<string, Section> sections = new(StringComparer.Ordinal);
        private readonly Row header;

        public StackPanel Root { get; }
        public string Mode { get; }
        public double Offset { get; set; }

        public Surface(SettingsView owner, string mode)
        {
            this.owner = owner;
            Mode = mode;
            var settings = mode == "settings";
            Root = new StackPanel
            {
                MaxWidth = settings ? 1012 : 1160,
                Margin = settings ? new Thickness(36, 22, 36, 30) : new Thickness(28, 54, 28, 24)
            };
            var intro = new StackPanel { Margin = new Thickness(0, 0, 0, settings ? 2 : 24) };
            if (!settings)
            {
                title.TextAlignment = TextAlignment.Center;
                detail.TextAlignment = TextAlignment.Center;
                footer.TextAlignment = TextAlignment.Center;
                SetTextSize(title, 32);
            }

            title.Margin = new Thickness(0, 0, 0, 6);
            detail.MaxWidth = settings ? double.PositiveInfinity : 640;
            intro.Children.Add(title);
            intro.Children.Add(detail);

            header = new Row(owner);
            if (settings)
            {
                var top = new Grid();
                top.ColumnDefinitions.Add(new ColumnDefinition());
                top.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                Grid.SetColumn(header.Root, 1);
                header.Root.VerticalAlignment = VerticalAlignment.Center;
                top.Children.Add(intro);
                top.Children.Add(header.Root);
                Root.Children.Add(top);
            }
            else
            {
                Root.Children.Add(intro);
                header.Root.HorizontalAlignment = HorizontalAlignment.Center;
                Root.Children.Add(header.Root);
            }

            sectionHost = mode == "firstRun"
                ? new WrapPanel { HorizontalAlignment = HorizontalAlignment.Center }
                : new StackPanel();
            Root.Children.Add(sectionHost);
            footer.Margin = new Thickness(0, settings ? 12 : 20, 0, 0);
            Root.Children.Add(footer);

            if (mode == "firstRun")
                Root.SizeChanged += (_, _) => SizeCards();
        }

        public void Apply(JsonElement view, string defaultTitle)
        {
            Say(title, Str(view, "title", defaultTitle));
            owner.SayDetail(detail, Str(view, "detail"));
            owner.SayDetail(footer, Str(view, "footer"));
            header.Apply(view, actionsOnly: true, reverseActions: Mode == "settings");
            Arrange(sectionHost, sections, Items(view, "sections"), item => Str(item, "id"),
                _ => new Section(owner, Mode == "firstRun"), (section, item) => section.Apply(item),
                section => section.Root, section => section.Detach());

            if (Mode == "firstRun")
                SizeCards();
        }

        private void SizeCards()
        {
            var width = Root.ActualWidth;
            if (width <= 0)
                return;

            var columns = Math.Min(Math.Clamp((int)(width / 266), 1, 3), Math.Max(1, sections.Count));
            var cardWidth = Math.Min((width - 1) / columns, 350) - 12;
            if (cardWidth <= 0)
                return;

            foreach (var section in sections.Values)
                section.Root.Width = cardWidth;
        }

        public Row? FindRow(DependencyObject? origin)
        {
            foreach (var section in sections.Values)
                foreach (var row in section.Rows)
                    if (row.Contains(origin))
                        return row;

            return null;
        }

        public ActionControl? FindAction(bool isCancel)
        {
            var panelAction = sections.Values.Where(section => section.IsPanel)
                .SelectMany(section => section.Rows)
                .Select(row => row.FindAction(isCancel)).FirstOrDefault(action => action is not null);
            return panelAction ?? header.FindAction(isCancel) ??
                sections.Values.SelectMany(section => section.Rows)
                    .Select(row => row.FindAction(isCancel)).FirstOrDefault(action => action is not null);
        }

        public void ClearSecrets()
        {
            header.ClearSecrets();
            foreach (var section in sections.Values)
                section.ClearSecrets();
        }
    }

    private sealed class Section
    {
        private readonly SettingsView owner;
        private readonly bool card;
        private readonly TextBlock title = Heading(17);
        private readonly TextBlock detail = Text(13);
        private readonly StackPanel titleRow = new();
        private readonly StackPanel sectionActions = new();
        private readonly StackPanel rowHost = new();
        private readonly WrapPanel previews = new();
        private string previewState = "";
        private readonly Dictionary<string, Row> rows = new(StringComparer.Ordinal);
        private readonly Row header;

        public Border Root { get; }
        public bool IsPanel { get; private set; }
        public IEnumerable<Row> Rows => rows.Values.Prepend(header);

        public Section(SettingsView owner, bool card)
        {
            this.owner = owner;
            this.card = card;
            var stack = new StackPanel();
            Root = new Border { Child = stack };
            if (card)
                Root.MinHeight = 206;

            title.Margin = new Thickness(0, 0, 0, 5);
            detail.Margin = new Thickness(0, 0, 0, 6);
            titleRow.Children.Add(title);
            stack.Children.Add(titleRow);
            stack.Children.Add(detail);
            stack.Children.Add(rowHost);
            stack.Children.Add(previews);
            header = new Row(owner);
            stack.Children.Add(sectionActions);
            sectionActions.Children.Add(header.Root);
        }

        public void Apply(JsonElement section)
        {
            IsPanel = Str(section, "kind") == "panel";
            var framed = card || IsPanel;
            Root.Margin = card ? new Thickness(6, 0, 6, 12) : new Thickness(0, 15, 0, 4);
            Root.BorderThickness = framed ? new Thickness(1) : new Thickness();
            Root.BorderBrush = framed ? owner.BrushFor("YaacRule") : null;
            Root.Background = framed ? owner.BrushFor("YaacCard") : null;
            Root.Padding = framed ? new Thickness(16, 14, 16, 13) : new Thickness();
            Root.CornerRadius = framed ? new CornerRadius(7) : new CornerRadius();
            Say(title, Str(section, "title"));
            Say(detail, Str(section, "detail"));
            detail.Foreground = owner.Tone(Str(section, "tone"));
            var inlineActions = Flag(section, "inlineActions");
            titleRow.Orientation = inlineActions ? Orientation.Horizontal : Orientation.Vertical;
            title.Margin = inlineActions ? new Thickness() : new Thickness(0, 0, 0, 5);
            title.VerticalAlignment = inlineActions ? VerticalAlignment.Center : VerticalAlignment.Stretch;
            var actionParent = inlineActions ? titleRow : sectionActions;
            if (!actionParent.Children.Contains(header.Root))
            {
                (inlineActions ? sectionActions : titleRow).Children.Remove(header.Root);
                actionParent.Children.Add(header.Root);
                header.Root.Margin = inlineActions ? new Thickness(12, 0, 0, 0) : new Thickness();
            }
            header.Apply(section, true);
            if (section.TryGetProperty("cards", out var cards) && cards.GetRawText() != previewState)
            {
                previewState = cards.GetRawText();
                previews.Children.Clear();
                foreach (var card in cards.EnumerateArray())
                {
                    var name = Str(card, "name");
                    var stack = new StackPanel();
                    var sample = new Grid { Width = 264, Height = 144, ClipToBounds = true };
                    var status = new TextBlock
                    {
                        Text = Str(card, "status"), HorizontalAlignment = HorizontalAlignment.Center,
                        VerticalAlignment = VerticalAlignment.Center, Foreground = owner.Tone("")
                    };
                    var image = new Image { Stretch = Stretch.Uniform };
                    sample.Children.Add(status);
                    sample.Children.Add(image);
                    stack.Children.Add(sample);
                    var path = Str(card, "imagePath");
                    if (path.Length > 0)
                        _ = LoadPreviewAsync(path, image, status);
                    else if (Str(card, "error") is { Length: > 0 } error)
                        sample.ToolTip = error;
                    stack.Children.Add(new TextBlock { Text = name, Margin = new Thickness(4) });
                    var button = new Button { Content = stack, Padding = new Thickness(1), Margin = new Thickness(0, 0, 8, 8),
                        BorderThickness = new Thickness(Flag(card, "selected") ? 2 : 1) };
                    AutomationProperties.SetName(button, name);
                    button.Click += (_, _) => owner.SendAction("appearance.selectTheme", new() { ["name"] = name });
                    previews.Children.Add(button);
                }
            }
            Arrange(rowHost, rows, Items(section, "rows"), item => Str(item, "id"),
                _ => new Row(owner), (row, item) => row.Apply(item), row => row.Root, row => row.Detach());
        }

        private static async System.Threading.Tasks.Task LoadPreviewAsync(string path, Image image, TextBlock status)
        {
            var source = await System.Threading.Tasks.Task.Run(() =>
            {
                try
                {
                    using var stream = System.IO.File.OpenRead(path);
                    var bitmap = new BitmapImage();
                    bitmap.BeginInit();
                    bitmap.StreamSource = stream;
                    bitmap.CacheOption = BitmapCacheOption.OnLoad;
                    bitmap.EndInit();
                    bitmap.Freeze();
                    return bitmap;
                }
                catch (Exception error) when (error is System.IO.IOException or NotSupportedException)
                {
                    return null;
                }
            });
            image.Source = source;
            status.Text = source is null ? "Preview unavailable" : "";
        }

        public void ClearSecrets()
        {
            foreach (var row in Rows)
                row.ClearSecrets();
        }

        public void Detach()
        {
            foreach (var row in Rows)
                row.Detach();
        }
    }

    private sealed class Row
    {
        private readonly SettingsView owner;
        private readonly TextBlock label = Heading(13);
        private readonly TextBlock status = Text(13);
        private readonly TextBlock detail = Text(12);
        private readonly StackPanel lines = new();
        private readonly WrapPanel fieldHost = new();
        private readonly WrapPanel actionHost = new();
        private readonly TextBlock fontSample = Text(14);
        private readonly Dictionary<string, Field> fields = new(StringComparer.Ordinal);
        private readonly Dictionary<string, ActionControl> actions = new(StringComparer.Ordinal);
        private readonly TextBox code = new()
        {
            IsReadOnly = true,
            IsUndoEnabled = false,
            TextWrapping = TextWrapping.Wrap,
            Padding = new Thickness(9, 6, 9, 6),
            Visibility = Visibility.Collapsed
        };
        private string resetKey = "";
        private bool applying;

        public Grid Root { get; } = new()
        {
            Margin = new Thickness(0, 2, 0, 6),
            VerticalAlignment = VerticalAlignment.Top
        };

        public Row(SettingsView owner)
        {
            this.owner = owner;
            Root.ColumnDefinitions.Add(new ColumnDefinition());
            Root.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            Root.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            for (var index = 0; index < 4; index++)
                Root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });

            detail.Margin = new Thickness(0, 2, 0, 0);
            lines.Children.Add(label);
            lines.Children.Add(status);
            lines.Children.Add(detail);
            lines.Children.Add(code);
            Root.Children.Add(lines);
            Grid.SetRow(fieldHost, 1);
            Grid.SetColumnSpan(fieldHost, 3);
            Root.Children.Add(fieldHost);
            Root.Children.Add(actionHost);
            Grid.SetRow(fontSample, 3);
            Grid.SetColumnSpan(fontSample, 3);
            fontSample.Margin = new Thickness(0, 3, 0, 5);
            Root.Children.Add(fontSample);
            code.SetResourceReference(Control.FontFamilyProperty, YaacFonts.CodeResource);
        }

        public void Apply(JsonElement row, bool actionsOnly = false, bool reverseActions = false)
        {
            var key = Str(row, "resetKey");
            var reset = resetKey != key;
            resetKey = key;
            applying = true;
            try
            {
                if (!actionsOnly)
                {
                    var layout = Str(row, "layout");
                    var rowLabel = Str(row, "label");
                    var rowDetail = Str(row, "detail");
                    lines.Opacity = Flag(row, "dimmed") ? 0.65 : 1.0;
                    Say(label, rowLabel);
                    SayStatus(Str(row, "status"), Str(row, "tone"));
                    owner.SayDetail(detail, layout == "code" ? "" : rowDetail);
                    SetCode(layout == "code" ? rowDetail : "", rowLabel);
                    Arrange(fieldHost, fields, Items(row, "fields"), item => Str(item, "id"),
                        item => new Field(owner, DraftChanged, Str(item, "id")), (field, item) => field.Apply(item, reset),
                        field => field.Root, field => field.Detach());
                    PositionActions(layout);
                    var sample = Str(row, "fontSample");
                    Say(fontSample, sample);
                    var fontKey = Flag(row, "codeFontSample") ? YaacFonts.CodeResource : YaacFonts.UiResource;
                    if (sample.Length > 0 && !Equals(fontSample.Tag, fontKey))
                    {
                        fontSample.Tag = fontKey;
                        fontSample.SetResourceReference(TextBlock.FontFamilyProperty, fontKey);
                    }
                }
                else
                {
                    // Header and section actions use the same native controls as row actions.
                    lines.Visibility = Visibility.Collapsed;
                    Grid.SetColumn(actionHost, 0);
                    Grid.SetColumnSpan(actionHost, 3);
                }

                var visibleActions = Items(row, "actions");
                Arrange(actionHost, actions, reverseActions ? visibleActions.Reverse() : visibleActions, item => Str(item, "id"),
                    _ => new ActionControl(owner, InvokeAction), (action, item) => action.Apply(item),
                    action => action.Root);
                Root.Visibility = actionsOnly && actions.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
            }
            finally
            {
                applying = false;
            }

            DraftChanged();
        }

        private void SayStatus(string state, string tone)
        {
            SayRich(status, state);
            status.Foreground = owner.Tone(tone.Length == 0 ? "accent" : tone);
        }

        private void SetCode(string text, string accessibleName)
        {
            if (code.Text != text)
                code.Text = text;
            code.Visibility = text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
            AutomationProperties.SetName(code, accessibleName);
        }

        private void PositionActions(string layout)
        {
            var showFields = fields.Count > 0;
            var shortcut = showFields && fields.Values.All(field => field.IsShortcut);
            var inline = showFields && layout == "inline";
            var form = (showFields && !shortcut && !inline) || layout == "form";
            fieldHost.Visibility = showFields ? Visibility.Visible : Visibility.Collapsed;
            // Keep key caps beside the command and give multi-field forms space to wrap.
            Grid.SetRow(fieldHost, shortcut ? 0 : 1);
            Grid.SetColumn(fieldHost, shortcut ? 1 : 0);
            Grid.SetColumnSpan(fieldHost, shortcut ? 1 : inline ? 2 : 3);
            actionHost.MaxWidth = form ? double.PositiveInfinity : 345;
            actionHost.HorizontalAlignment = form ? HorizontalAlignment.Left : HorizontalAlignment.Right;
            Grid.SetRow(actionHost, form ? 2 : inline ? 1 : 0);
            Grid.SetColumn(actionHost, form ? 0 : 2);
            Grid.SetColumnSpan(actionHost, form ? 3 : 1);
            Grid.SetColumnSpan(lines, form || inline ? 3 : shortcut ? 1 : 2);
        }

        public bool Contains(DependencyObject? element) => Root == Ancestor<Grid>(element) ||
            (element is Visual visual && Root.IsAncestorOf(visual));

        public ActionControl? FindAction(bool isCancel) =>
            actions.Values.FirstOrDefault(control => control.Root.IsEnabled &&
                (isCancel ? control.Cancel : control.Default));

        private void DraftChanged()
        {
            if (!applying)
                foreach (var action in actions.Values)
                    action.Validate(!action.Submit || FormValid());
        }

        private bool FormValid() => fields.Values.All(field => field.IsValid) &&
            (!fields.Values.Any(field => field.NeedsAny) || fields.Values.Any(field => field.NeedsAny && field.HasValue));

        private void InvokeAction(ActionControl control, bool? check, Dictionary<string, object?> values)
        {
            if (control.CopyField.Length > 0 && fields.TryGetValue(control.CopyField, out var copied))
            {
                copied.CopyText();
                return;
            }

            if (check.HasValue)
                values["checked"] = check.Value;

            if (control.Submit)
            {
                if (!FormValid())
                    return;

                foreach (var field in fields.Values)
                    if (field.IncludeInSubmit)
                        values[field.Id] = field.Value;

                // Credential widgets are scrubbed before dispatch, including if a command fails.
                ClearSecrets();
                if (control.Clear)
                    foreach (var field in fields.Values)
                        field.ClearDraft();
            }

            if (control.Cancel)
                owner.ClearSensitiveInputs();
            owner.SendAction(control.Id, values);
        }

        public void ClearSecrets()
        {
            foreach (var field in fields.Values)
                field.ClearSecret();
        }

        public void Detach()
        {
            foreach (var field in fields.Values)
                field.Detach();
        }
    }
}
