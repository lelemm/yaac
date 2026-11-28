using System.Globalization;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public sealed class TranscriptRowView : UserControl
{
    private const double UserBubbleShare = 0.8;
    private readonly Border plate = new() { CornerRadius = new CornerRadius(7) };
    private readonly StackPanel content = new();
    private readonly Grid heading = new();
    private readonly TextBlock title = Label(semiBold: true);
    private readonly TextBlock publicationTime = Label(color: TranscriptColors.Muted);
    private readonly TextBlock cursor = Label("▌", color: TranscriptColors.Muted);
    private readonly TextBlock status = Label(color: TranscriptColors.Muted);
    private readonly TextBlock stamp = Label(color: TranscriptColors.Muted);
    private readonly WrapPanel taskHeader = new() { Margin = new Thickness(0, 0, 0, 5) };
    private readonly TextBlock handoffDivider = Label(color: TranscriptColors.Muted);
    private readonly Grid handoffRule = new() { Margin = new Thickness(0, 10, 0, 3) };
    private readonly WrapPanel attachments = new();
    private readonly WrapPanel actions = new();
    private readonly StackPanel details = new();
    private readonly StackPanel detailContent = new();
    private readonly StackPanel questions = new();
    private readonly WrapPanel fields = new();
    private readonly StackPanel dialogHost = new();
    private DialogLayoutView? dialog;
    private readonly List<TranscriptPart> parts = new();
    private readonly Dictionary<string, Button> buttons = new();
    private TranscriptRow? row;
    private string? boundKind;
    private TranscriptRowState? state;
    private TranscriptText? body;
    private TranscriptText? detail;
    private TranscriptActivity? activityRow;
    private TranscriptShell? shellBlock;
    private Expander? detailExpander;
    private MermaidDiagram? diagram;
    private JsonElement previous;
    private JsonElement escape;
    private bool card;
    private bool reserveStamp;
    private bool showStatusStamp;

    public TranscriptRowView()
    {
        Focusable = false;
        HorizontalAlignment = HorizontalAlignment.Stretch;
        Margin = new Thickness(0, 0, 0, 9);
        var root = new StackPanel { Background = Brushes.Transparent };
        plate.Child = content;
        root.Children.Add(plate);
        root.Children.Add(stamp);
        Content = root;
        stamp.FontSize = 11;
        stamp.Margin = new Thickness(1, 2, 1, 0);
        status.FontSize = 12;
        status.Margin = new Thickness(0, 2, 0, 1);
        publicationTime.FontSize = 11;
        publicationTime.Margin = new Thickness(8, 0, 0, 0);
        publicationTime.TextWrapping = TextWrapping.NoWrap;
        heading.ColumnDefinitions.Add(new ColumnDefinition());
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        heading.Children.Add(title);
        Grid.SetColumn(publicationTime, 1);
        heading.Children.Add(publicationTime);
        handoffDivider.HorizontalAlignment = HorizontalAlignment.Center;
        handoffDivider.Margin = new Thickness(9, 0, 9, 0);
        handoffRule.ColumnDefinitions.Add(new ColumnDefinition());
        handoffRule.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto, MaxWidth = 640 });
        handoffRule.ColumnDefinitions.Add(new ColumnDefinition());
        handoffRule.Children.Add(handoffDivider);
        Grid.SetColumn(handoffDivider, 1);
        foreach (var column in new[] { 0, 2 })
        {
            var line = new Border { Height = 1, VerticalAlignment = VerticalAlignment.Center };
            line.SetResourceReference(Border.BackgroundProperty, "YaacRule");
            Grid.SetColumn(line, column);
            handoffRule.Children.Add(line);
        }
        KeyboardNavigation.SetTabNavigation(this, KeyboardNavigationMode.Continue);

        var menu = new ContextMenu();
        // Copy the transcript selection (handled by TranscriptSelection), shown while there is one.
        var selection = new MenuItem { Header = "Copy", Command = ApplicationCommands.Copy, CommandTarget = plate };
        var copy = new MenuItem { Header = "Copy message" };
        copy.Click += (_, _) => TranscriptText.Copy(CopyMessage());
        menu.Items.Add(selection);
        menu.Items.Add(copy);
        menu.Opened += (_, _) =>
        {
            selection.Visibility = ApplicationCommands.Copy.CanExecute(null, plate) ? Visibility.Visible : Visibility.Collapsed;
            copy.IsEnabled = CopyMessage() is not null;
        };
        plate.ContextMenu = menu;

        MouseEnter += (_, _) => ShowStamp();
        MouseLeave += (_, _) => ShowStamp();
        DataContextChanged += (_, _) => Bind();
        Loaded += (_, _) => Bind();
        Unloaded += (_, _) => Unbind();
        PreviewKeyDown += CardKeyDown;
    }

    protected override Size MeasureOverride(Size constraint)
    {
        plate.MaxWidth = boundKind == "user" ? constraint.Width * UserBubbleShare : double.PositiveInfinity;
        return base.MeasureOverride(constraint);
    }

    private void Bind()
    {
        var next = DataContext as TranscriptRow;
        if (ReferenceEquals(row, next) && boundKind == next?.Kind)
            return;
        Unbind();
        if (next is null)
            return;
        row = next;
        boundKind = next.Kind;
        state = row.State;
        row.Changed += Refresh;
        body = CreateText("body");
        detail = CreateText("detail");
        if (row.Kind == "shell")
        {
            shellBlock = new TranscriptShell(state, body, descriptor => Submit(descriptor, false));
            content.Children.Add(shellBlock);
            content.Children.Add(actions);
            Refresh();
            return;
        }
        content.Children.Add(taskHeader);
        content.Children.Add(heading);
        content.Children.Add(body);
        content.Children.Add(handoffRule);
        content.Children.Add(cursor);
        if (row.Kind is "call" or "tool" or "activity" or "worked" or "changes")
        {
            activityRow = new TranscriptActivity(state, CopyMessage, descriptor => Submit(descriptor, false),
                details, () => UpdateDetails(next.Data, force: true), row.Kind == "call");
            content.Children.Add(activityRow);
        }
        else
            content.Children.Add(details);
        content.Children.Add(attachments);
        content.Children.Add(questions);
        content.Children.Add(dialogHost);
        content.Children.Add(fields);
        content.Children.Add(status);
        content.Children.Add(actions);
        Refresh();
    }

    private void Unbind()
    {
        if (row is null)
            return;
        if (plate.ContextMenu is { } menu)
            menu.IsOpen = false;
        CloseMenus(actions);
        CloseMenus(attachments);
        row.Changed -= Refresh;
        row = null;
        boundKind = null;
        state = null;
        previous = default;
        escape = default;
        content.Children.Clear();
        diagram = null;
        activityRow?.DetachDetails(details);
        activityRow = null;
        shellBlock = null;
        details.Children.Clear();
        taskHeader.Children.Clear();
        if (detailExpander is not null)
            detailExpander.Content = null;
        detailExpander = null;
        detailContent.Children.Clear();
        parts.Clear();
        buttons.Clear();
        attachments.Children.Clear();
        questions.Children.Clear();
        dialogHost.Children.Clear();
        dialog = null;
        fields.Children.Clear();
        actions.Children.Clear();
        body = null;
        detail = null;
    }

    private void Refresh()
    {
        if (row is null || state is null || body is null)
            return;
        if (boundKind != row.Kind)
        {
            Bind();
            return;
        }
        var data = row.Data;
        var kind = row.Kind;
        if (shellBlock is not null)
        {
            RefreshShell(data);
            return;
        }
        var user = kind == "user";
        card = kind is "artifact" or "request" or "approval" or "permission" or "question" or "form" or "budget" or "dialog";
        var activity = kind is "activity" or "tool" or "file" or "call" or "notice" or "worked" or "changes";
        var tone = TranscriptColors.Tone(data.Value("tone").Text(), activity ? TranscriptColors.Muted : TranscriptColors.Text);

        plate.HorizontalAlignment = user ? HorizontalAlignment.Right : HorizontalAlignment.Stretch;
        plate.Margin = user ? new Thickness(42, 0, 0, 0) : new Thickness(0);
        plate.Background = user ? TranscriptColors.User : card || kind == "task" ? TranscriptColors.Panel : Brushes.Transparent;
        plate.BorderBrush = kind == "artifact" ? TranscriptColors.Rule : card ? TranscriptColors.Tone(data.Value("tone").Text(), TranscriptColors.Warning) : TranscriptColors.Rule;
        plate.BorderThickness = card ? new Thickness(3, 0, 0, 0) : user || kind == "task" ? new Thickness(1) : new Thickness(0);
        plate.Padding = user || card || kind == "task" ? new Thickness(12, 8, 12, 8) : new Thickness(2, activity ? 2 : 3, 2, 0);
        Margin = new Thickness(0, 0, 0, activityRow is null ? 9 : 0);

        SetLabel(title, user || kind == "task" ? "" : data.Value("title").Text(), tone);
        SetLabel(publicationTime, kind == "artifact" ? data.Value("publishedAt").Text() : "");
        SetLabel(handoffDivider, data.Value("divider").Text(), TranscriptColors.Muted);
        handoffRule.Visibility = kind == "handoff" ? Visibility.Visible : Visibility.Collapsed;
        if (!Same("taskPeer", data))
            UpdateTaskHeader(data.Value("taskPeer"));
        taskHeader.Visibility = data.Value("taskPeer").ValueKind == JsonValueKind.Object ? Visibility.Visible : Visibility.Collapsed;
        var activityWasOpen = activityRow?.IsExpanded == true;
        activityRow?.Render(data);
        var format = data.Value("markdown").Flag(kind is "agent" or "task" or "handoff") ? "markdown" : "text";
        body.Foreground = kind == "handoff" ? TranscriptColors.Muted : tone;
        if (kind is "agent" or "task") body.FontSize = 15;
        body.Show(data.Value("text").Text(), format, data.Value("textBlocks"));
        cursor.Visibility = data.Value("streaming").Flag() ? Visibility.Visible : Visibility.Collapsed;

        if (kind == "artifact" && !Same("mermaidSource", data))
        {
            if (diagram is not null) content.Children.Remove(diagram);
            diagram = data.Value("mermaidSource").OptionalText() is { } source ? new MermaidDiagram(source) : null;
            if (diagram is not null) content.Children.Insert(content.Children.IndexOf(body) + 1, diagram);
        }
        var statusText = data.Value("status").OptionalText();
        var statusTone = TranscriptColors.Tone(data.Value("tone").Text(), TranscriptColors.Muted);
        showStatusStamp = user && statusText is not null && !data.Value("actions").Elements().Any();
        reserveStamp = user || kind is "agent" or "task";
        stamp.MinHeight = reserveStamp ? 14 : 0;
        SetLabel(status, showStatusStamp ? "" : statusText ?? "", statusTone);
        SetLabel(stamp, showStatusStamp ? statusText! : data.Value("stamp").Text(),
            showStatusStamp ? statusTone : TranscriptColors.Muted);
        stamp.HorizontalAlignment = user ? HorizontalAlignment.Right : HorizontalAlignment.Left;
        ShowStamp();
        actions.HorizontalAlignment = user ? HorizontalAlignment.Right : HorizontalAlignment.Left;

        if (activityRow?.IsExpanded != false)
            UpdateDetails(data, force: activityRow is not null && !activityWasOpen);
        if (!Same("attachments", data))
            UpdateAttachments(data.Value("attachments"));
        if (!Same("questions", data))
            UpdateQuestions(data.Value("questions"));
        if (!Same("fields", data))
            UpdateFields(data.Value("fields"));
        UpdateDialog(data.Value("dialog"));
        if (!Same("actions", data))
            UpdateActions(data.Value("actions"));
        previous = data;
    }

    // A `!command` block (P346): full width, no bubble, on the transcript's own background.
    private void RefreshShell(JsonElement data)
    {
        plate.HorizontalAlignment = HorizontalAlignment.Stretch;
        plate.Margin = new Thickness(0);
        plate.Background = Brushes.Transparent;
        plate.BorderThickness = new Thickness(0);
        plate.Padding = new Thickness(2, 3, 2, 0);
        Margin = new Thickness(0, 0, 0, 9);
        reserveStamp = false;
        showStatusStamp = false;
        SetLabel(stamp, "");
        ShowStamp();
        shellBlock!.Render(data);
        actions.HorizontalAlignment = HorizontalAlignment.Left;
        if (!Same("actions", data))
            UpdateActions(data.Value("actions"));
        previous = data;
    }

    private bool Same(string name, JsonElement data) =>
        previous.ValueKind != JsonValueKind.Undefined && TranscriptJson.Same(previous.Value(name), data.Value(name));

    private void ShowStamp() => stamp.Visibility = showStatusStamp || stamp.Text.Length > 0 && IsMouseOver
        ? Visibility.Visible
        : reserveStamp || stamp.Text.Length > 0 ? Visibility.Hidden : Visibility.Collapsed;

    private void UpdateTaskHeader(JsonElement peer)
    {
        taskHeader.Children.Clear();
        if (peer.ValueKind != JsonValueKind.Object)
            return;
        var from = Label(peer.Value("heading").OptionalText() ?? "From", color: TranscriptColors.Muted);
        from.Margin = new Thickness(0, 0, 5, 0);
        taskHeader.Children.Add(from);
        if (peer.Value("enabled").Flag())
            taskHeader.Children.Add(ActionButton(peer, includeDraft: false));
        else
            taskHeader.Children.Add(Label(peer.Value("label").Text(), semiBold: true));
        if (peer.Value("machine").OptionalText() is { } machine)
        {
            var on = Label($"on {machine}", color: TranscriptColors.Muted);
            on.Margin = new Thickness(5, 0, 0, 0);
            taskHeader.Children.Add(on);
        }
    }

    private string? CopyMessage()
    {
        if (row is null)
            return null;
        var data = row.Data;
        if (data.Value("copyText").OptionalText() is { } copy)
            return copy;
        if (row.Kind is not ("user" or "agent" or "task"))
            return null;
        var text = data.Value("text").Text();
        var extra = data.Value("parts").Elements()
            .Where(part => part.Value("kind").Text() is "text" or "markdown" or "code" or "command" or "pasted")
            .Select(part => part.Value("text").Text());
        return string.Join("\n", new[] { text }.Concat(extra).Where(value => value.Length > 0));
    }

    private TranscriptText CreateText(string key)
    {
        var bound = row;
        return new TranscriptText(key, state!, () => ReferenceEquals(bound, row) ? CopyMessage() : null);
    }

    private void UpdateDetails(JsonElement data, bool force = false)
    {
        if (state is null || detail is null
            || !force && Same("detail", data) && Same("detailBlocks", data) && Same("parts", data)
                && Same("disclosureLabel", data) && Same("expanded", data))
            return;

        var label = data.Value("disclosureLabel").OptionalText();
        if (label is null && activityRow is null && data.Value("detail").OptionalText() is not null
            && row?.Kind is "file" or "mesh")
            label = "Details";

        var hasContent = data.Value("detail").OptionalText() is not null || data.Value("parts").Elements().Any();
        details.Visibility = hasContent ? Visibility.Visible : Visibility.Collapsed;
        if (label is null)
        {
            if (detailExpander is not null)
                detailExpander.Content = null;
            if (!details.Children.Contains(detailContent))
            {
                details.Children.Clear();
                details.Children.Add(detailContent);
            }
            detailExpander = null;
        }
        else if (detailExpander is null)
        {
            details.Children.Clear();
            detailExpander = Disclosure(label, "details", data.Value("expanded").Flag(), detailContent, RebuildDetails);
            details.Children.Add(detailExpander);
        }
        else
        {
            detailExpander.Header = label;
            if (!Same("expanded", data) && data.Value("expanded").ValueKind is JsonValueKind.True or JsonValueKind.False)
                detailExpander.IsExpanded = data.Value("expanded").Flag();
        }

        if (detailExpander is null || detailExpander.IsExpanded)
            RebuildDetails();
    }

    private void RebuildDetails()
    {
        if (row is null || detail is null || state is null)
            return;
        var data = row.Data;
        if (detailContent.Children.Count == 0)
            detailContent.Children.Add(detail);
        detail.Foreground = TranscriptColors.Muted;
        if (activityRow is not null) detail.FontSize = 12;
        detail.Show(data.Value("detail").Text(), row.Kind == "handoff" ? "markdown" : "text", data.Value("detailBlocks"));

        var count = 0;
        foreach (var part in data.Value("parts").Elements())
        {
            var id = part.Value("id").OptionalText() ?? count.ToString(CultureInfo.InvariantCulture);
            if (count < parts.Count && parts[count].Id != id)
                RemovePartsFrom(count);
            if (count == parts.Count)
            {
                var slot = new TranscriptPart(id);
                parts.Add(slot);
                detailContent.Children.Add(slot.Host);
            }
            UpdatePart(parts[count], part);
            count++;
        }
        RemovePartsFrom(count);
    }

    private sealed class TranscriptPart(string id)
    {
        public string Id { get; } = id;
        public ContentControl Host { get; } = new() { Margin = new Thickness(0, 2, 0, 1) };
        public TranscriptText? Text { get; set; }
        public string Value { get; set; } = "";
        public string Format { get; set; } = "";
        public JsonElement Blocks { get; set; }
    }

    private void RemovePartsFrom(int start)
    {
        for (var i = parts.Count - 1; i >= start; i--)
        {
            detailContent.Children.Remove(parts[i].Host);
            parts.RemoveAt(i);
        }
    }

    private void UpdatePart(TranscriptPart slot, JsonElement part)
    {
        if (state is null)
            return;
        var kind = part.Value("kind").Text();
        if (kind == "image")
        {
            if (slot.Host.Content is not TranscriptImage image)
            {
                if (slot.Host.Content is Expander old)
                    old.Content = null;
                var bound = row;
                image = new TranscriptImage(descriptor =>
                {
                    if (ReferenceEquals(bound, row))
                        Submit(descriptor, includeDraft: false);
                });
                slot.Host.Content = image;
                slot.Text = null;
            }
            image.Render(part);
            return;
        }
        if (slot.Host.Content is TranscriptImage)
            slot.Host.Content = null;
        var value = part.Value("text").Text();
        var label = part.Value("label").OptionalText();
        slot.Value = value;
        slot.Blocks = part.Value("blocks");
        slot.Format = kind switch { "markdown" or "diff" or "code" or "command" => kind, _ => "text" };
        if (kind == "pasted")
        {
            var lines = value.Count(c => c == '\n') + 1;
            label ??= $"Pasted text · {lines} {(lines == 1 ? "line" : "lines")}";
            if (slot.Blocks.ValueKind == JsonValueKind.Array)
                slot.Format = "markdown";
        }
        if (kind == "detail")
            label ??= "Details";
        if (kind == "code")
            label ??= part.Value("language").OptionalText();

        var text = slot.Text ??= CreateText("part:" + slot.Id);
        if (row?.Kind == "user" && kind == "markdown") text.FontSize = 15;
        text.Foreground = TranscriptColors.Tone(part.Value("tone").Text(),
            kind == "detail" ? TranscriptColors.Muted : TranscriptColors.Text);
        void Show() => text.Show(slot.Value, slot.Format, slot.Blocks);
        if (label is null || kind is "text" or "markdown" or "command")
        {
            if (slot.Host.Content is Expander old)
            {
                old.Content = null;
                slot.Host.Content = null;
            }
            if (label is not null && kind == "command")
            {
                if (slot.Host.Content is not StackPanel layout)
                {
                    slot.Host.Content = null;
                    layout = new StackPanel();
                    layout.Children.Add(text);
                    layout.Children.Add(Label(color: TranscriptColors.Muted));
                    slot.Host.Content = layout;
                }
                SetLabel((TextBlock)layout.Children[1], label);
            }
            else if (slot.Host.Content is not TranscriptText)
            {
                if (slot.Host.Content is StackPanel layout)
                    layout.Children.Remove(text);
                slot.Host.Content = text;
            }
            Show();
            return;
        }
        if (slot.Host.Content is not Expander expander)
        {
            if (slot.Host.Content is StackPanel layout)
                layout.Children.Remove(text);
            slot.Host.Content = null;
            expander = Disclosure(label, "part:" + slot.Id, part.Value("expanded").Flag(kind == "code"), text, Show, part.Value("preview").OptionalText());
            slot.Host.Content = expander;
        }
        else
        {
            expander.Header = expander.IsExpanded ? label : PreviewHeader(label, part.Value("preview").OptionalText());
        }
        if (expander.IsExpanded)
            Show();
    }

    private static string PreviewHeader(string label, string? preview) => preview is null ? label : $"{label}   {preview}";

    private Expander Disclosure(string label, string key, bool expanded, FrameworkElement element, Action show, string? preview = null)
    {
        var saved = state!;
        var expander = new Expander
        {
            Header = label,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Content = element,
            IsExpanded = saved.Expanded.GetValueOrDefault(key, expanded),
        };
        if (!expander.IsExpanded) expander.Header = PreviewHeader(label, preview);
        expander.Expanded += (_, e) =>
        {
            if (e.OriginalSource != expander)
                return;
            if (preview is not null) expander.Header = "Hide command";
            saved.Expanded[key] = true;
            show();
        };
        expander.Collapsed += (_, e) =>
        {
            if (e.OriginalSource == expander)
            {
                if (preview is not null) expander.Header = PreviewHeader(label, preview);
                saved.Expanded[key] = false;
            }
        };
        return expander;
    }

    private void UpdateAttachments(JsonElement data)
    {
        CloseMenus(attachments);
        attachments.Children.Clear();
        foreach (var attachment in data.Elements())
        {
            var label = attachment.Value("label").Text();
            if (label.Length == 0)
                continue;
            var icon = attachment.Value("image").Flag() ? "▧" : "▤";
            var missing = attachment.Value("missing").Flag();
            var full = missing ? $"{icon}  {label} · no longer on the target" : $"{icon}  {label}";
            var descriptor = attachment.Value("action");
            if (descriptor.ValueKind == JsonValueKind.Object)
            {
                attachments.Children.Add(ActionButton(descriptor, full, includeDraft: false));
                continue;
            }
            attachments.Children.Add(new Border
            {
                Child = Label(full, color: missing ? TranscriptColors.Muted : TranscriptColors.Text),
                Background = TranscriptColors.Raised,
                CornerRadius = new CornerRadius(4),
                Margin = new Thickness(0, 3, 4, 1),
                Padding = new Thickness(7, 3, 7, 3),
            });
        }
        attachments.Visibility = attachments.Children.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void UpdateQuestions(JsonElement data)
    {
        var focus = RememberFocus(questions);
        questions.Children.Clear();
        if (state is null)
            return;
        var active = new HashSet<string>();
        foreach (var question in data.Elements())
        {
            var id = question.Value("id").OptionalText();
            if (id is null || !active.Add(id))
                continue;
            if (!state.Answers.TryGetValue(id, out var draft))
                state.Answers.Add(id, draft = new TranscriptAnswerDraft());

            var secret = question.Value("secret").Flag();
            if (secret != draft.Secret)
                draft.Other = "";
            draft.Secret = secret;
            var header = question.Value("header").OptionalText();
            if (header is not null)
                questions.Children.Add(Label(header, semiBold: true, color: TranscriptColors.Muted));
            var prompt = question.Value("text").Text();
            var promptLabel = Label(prompt);
            promptLabel.Margin = new Thickness(0, 3, 0, 4);
            questions.Children.Add(promptLabel);
            var options = question.Value("options").Elements().ToArray();
            var multi = question.Value("multiSelect").Flag();
            var valid = new HashSet<string>();
            var group = Guid.NewGuid().ToString("N");
            foreach (var option in options)
            {
                var label = option.Value("label").OptionalText() ?? option.OptionalText();
                var optionId = option.Value("id").OptionalText() ?? label;
                if (label is null || optionId is null || !valid.Add(optionId))
                    continue;

                ToggleButton toggle = multi ? new CheckBox() : new RadioButton { GroupName = group };
                toggle.Tag = $"option:{id}:{optionId}";
                toggle.Content = Label(label);
                toggle.Margin = new Thickness(0, 0, 0, 3);
                toggle.IsEnabled = option.Value("enabled").Flag(true);
                toggle.ToolTip = option.Value("detail").OptionalText();
                toggle.IsChecked = toggle.IsEnabled && draft.Selected.Contains(optionId);
                if (!toggle.IsEnabled)
                    draft.Selected.Remove(optionId);
                toggle.Checked += (_, _) =>
                {
                    if (!multi)
                        draft.Selected.Clear();
                    draft.Selected.Add(optionId);
                };
                toggle.Unchecked += (_, _) => draft.Selected.Remove(optionId);
                questions.Children.Add(toggle);
            }
            draft.Selected.IntersectWith(valid);
            if (question.Value("allowOther").Flag() || options.Length == 0)
            {
                var hint = options.Length == 0 ? "Your answer" : "Something else";
                questions.Children.Add(Label(hint, color: TranscriptColors.Muted));
                Control editor;
                if (secret)
                {
                    var password = new PasswordBox { Password = draft.Other };
                    password.PasswordChanged += (_, _) => draft.Other = password.Password;
                    editor = password;
                }
                else
                {
                    var input = new TextBox { Text = draft.Other };
                    input.TextChanged += (_, _) => draft.Other = input.Text;
                    editor = input;
                }
                AutomationProperties.SetName(editor, prompt.Length > 0 ? $"{hint} · {prompt}" : hint);
                editor.Tag = $"question:{id}";
                editor.Margin = new Thickness(0, 3, 0, 9);
                questions.Children.Add(editor);
            }
        }
        foreach (var (id, draft) in state.Answers)
        {
            if (draft.Secret && !active.Contains(id))
                draft.Other = "";
        }
        questions.Visibility = questions.Children.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
        RestoreFocus(questions, focus);
    }

    // An external dialog (P418): the source line, a gone integrator, then the layout built at
    // runtime. Its buttons send `dialog:<id>` with the values entered.
    private void UpdateDialog(JsonElement data)
    {
        if (state is null || data.ValueKind != JsonValueKind.Object)
        {
            dialogHost.Children.Clear();
            dialog = null;
            dialogHost.Visibility = Visibility.Collapsed;
            return;
        }
        if (dialog is null)
        {
            var bound = row;
            dialogHost.Children.Clear();
            dialogHost.Children.Add(Label(color: TranscriptColors.Muted));
            dialogHost.Children.Add(Label(color: TranscriptColors.Warning));
            dialog = new DialogLayoutView(state.Dialog);
            dialog.Pressed += button =>
            {
                if (row is null || !ReferenceEquals(bound, row) || dialog is null)
                    return;
                row.Send(new { id = row.ActionId, action = $"dialog:{button}", threadId = row.ThreadId, values = dialog.Values });
            };
            dialogHost.Children.Add(dialog);
        }
        SetLabel((TextBlock)dialogHost.Children[0], data.Value("source").Text(), TranscriptColors.Muted);
        SetLabel((TextBlock)dialogHost.Children[1], data.Value("gone").Text(), TranscriptColors.Warning);
        dialog.Render(data);
        dialogHost.Visibility = Visibility.Visible;
    }

    private void UpdateFields(JsonElement data)
    {
        var focus = RememberFocus(fields);
        fields.Children.Clear();
        if (state is null)
            return;
        var saved = state;
        foreach (var field in data.Elements())
        {
            var id = field.Value("id").OptionalText();
            if (id is null)
                continue;
            var label = field.Value("label").OptionalText() ?? id;
            var group = new StackPanel { Margin = new Thickness(0, 4, 8, 4) };
            group.Children.Add(Label(label, color: TranscriptColors.Muted));
            var input = new TextBox
            {
                MinWidth = 83,
                Tag = $"field:{id}",
                Text = saved.Fields.GetValueOrDefault(id, field.Value("value").Scalar() ?? ""),
                Margin = new Thickness(0, 3, 0, 0),
            };
            AutomationProperties.SetName(input, label);
            input.TextChanged += (_, _) => saved.Fields[id] = input.Text;
            group.Children.Add(input);
            fields.Children.Add(group);
        }
        fields.Visibility = fields.Children.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
        RestoreFocus(fields, focus);
    }

    private sealed record EditorFocus(string Id, int Start, int Length);

    private static EditorFocus? RememberFocus(Panel scope)
    {
        if (!scope.IsKeyboardFocusWithin || Keyboard.FocusedElement is not Control { Tag: string id } control)
            return null;
        return control is TextBox text ? new EditorFocus(id, text.SelectionStart, text.SelectionLength) : new EditorFocus(id, 0, 0);
    }

    private static void RestoreFocus(Panel scope, EditorFocus? focus)
    {
        if (focus is null)
            return;
        foreach (FrameworkElement element in scope.Children)
        {
            if (element is Panel inner)
            {
                RestoreFocus(inner, focus);
                continue;
            }
            if (!Equals(element.Tag, focus.Id) || !element.IsEnabled)
                continue;
            element.Focus();
            if (element is TextBox box)
                box.Select(Math.Min(focus.Start, box.Text.Length), focus.Length);
            break;
        }
    }

    private static void CloseMenus(Panel scope)
    {
        foreach (FrameworkElement element in scope.Children)
        {
            if (element.ContextMenu is { } menu)
                menu.IsOpen = false;
        }
    }

    private void UpdateActions(JsonElement data)
    {
        var previousFocus = actions.IsKeyboardFocusWithin && Keyboard.FocusedElement is Button focused && focused.Tag is string id ? id : null;
        CloseMenus(actions);
        actions.Children.Clear();
        buttons.Clear();
        escape = default;
        foreach (var descriptor in data.Elements())
        {
            if (descriptor.Value("id").OptionalText() is not { } actionId)
                continue;
            var button = ActionButton(descriptor);
            actions.Children.Add(button);
            buttons.TryAdd(actionId, button);
            if (descriptor.Value("key").Text() is "Escape" or "Esc" && descriptor.Value("enabled").Flag())
                escape = descriptor;
        }
        actions.Visibility = actions.Children.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
        if (previousFocus is not null && buttons.TryGetValue(previousFocus, out var buttonToFocus) && buttonToFocus.IsEnabled)
            buttonToFocus.Focus();
    }

    private Button ActionButton(JsonElement descriptor, string? label = null, bool includeDraft = true)
    {
        var bound = row;
        var id = descriptor.Value("id").Text();
        var entries = descriptor.Value("children").Elements().ToArray();
        var checkedMark = descriptor.Value("checked").Flag() ? "✓  " : "";
        var caption = label ?? descriptor.Value("label").OptionalText() ?? id;
        var button = new Button
        {
            Tag = id,
            Content = checkedMark + caption + (entries.Length > 0 ? "  ▾" : ""),
            IsEnabled = id.Length > 0 && descriptor.Value("enabled").Flag(),
            ToolTip = descriptor.Value("detail").OptionalText(),
            Padding = new Thickness(10, 4, 10, 4),
            Margin = new Thickness(0, 3, 5, 2),
            FontWeight = descriptor.Value("primary").Flag() ? FontWeights.SemiBold : FontWeights.Normal,
        };
        if (descriptor.Value("primary").Flag())
            button.Style = (Style)FindResource("YaacPrimaryButton");
        ToolTipService.SetShowOnDisabled(button, true);
        if (entries.Length == 0)
        {
            button.Click += (_, _) =>
            {
                if (ReferenceEquals(bound, row))
                    Submit(descriptor, includeDraft);
            };
        }
        else
        {
            var menu = new ContextMenu { PlacementTarget = button, Placement = PlacementMode.Bottom };
            foreach (var entry in entries)
                menu.Items.Add(MenuEntry(entry, includeDraft, bound));
            button.ContextMenu = menu;
            button.Click += (_, _) => menu.IsOpen = true;
        }
        return button;
    }

    private MenuItem MenuEntry(JsonElement descriptor, bool includeDraft, TranscriptRow? bound)
    {
        var id = descriptor.Value("id").Text();
        var entry = new MenuItem
        {
            Header = descriptor.Value("label").OptionalText() ?? id,
            IsEnabled = id.Length > 0 && descriptor.Value("enabled").Flag(),
            ToolTip = descriptor.Value("detail").OptionalText(),
            IsCheckable = descriptor.Value("checked").ValueKind is JsonValueKind.True or JsonValueKind.False,
            IsChecked = descriptor.Value("checked").Flag(),
        };
        ToolTipService.SetShowOnDisabled(entry, true);
        foreach (var child in descriptor.Value("children").Elements())
            entry.Items.Add(MenuEntry(child, includeDraft, bound));
        if (entry.Items.Count == 0)
        {
            entry.Click += (_, e) =>
            {
                e.Handled = true;
                entry.IsChecked = descriptor.Value("checked").Flag();
                if (ReferenceEquals(bound, row))
                    Submit(descriptor, includeDraft);
            };
        }
        return entry;
    }

    private void Submit(JsonElement descriptor, bool includeDraft)
    {
        if (row is null || state is null || !descriptor.Value("enabled").Flag() || descriptor.Value("id").OptionalText() is not { } action)
            return;

        var data = row.Data;
        var id = row.ActionId;
        var threadId = row.ThreadId;
        var answers = new Dictionary<string, object>();
        var values = new Dictionary<string, long>();
        if (includeDraft && descriptor.Value("submit").Flag(true))
        {
            foreach (var question in data.Value("questions").Elements())
            {
                var questionId = question.Value("id").OptionalText();
                if (questionId is null || !state.Answers.TryGetValue(questionId, out var draft))
                    continue;
                var selected = draft.Selected.OrderBy(option => option, StringComparer.Ordinal).ToArray();
                var hasOther = question.Value("allowOther").Flag() || !question.Value("options").Elements().Any();
                var other = hasOther && draft.Other.Length > 0 ? draft.Other : null;
                answers[questionId] = new { selected, other };
            }
            foreach (var field in data.Value("fields").Elements())
            {
                var fieldId = field.Value("id").OptionalText();
                if (fieldId is null)
                    continue;
                var draft = state.Fields.GetValueOrDefault(fieldId, field.Value("value").Scalar() ?? "");
                if (!long.TryParse(draft, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value))
                    continue;
                if (long.TryParse(field.Value("min").Scalar(), out var min))
                    value = Math.Max(value, min);
                if (long.TryParse(field.Value("max").Scalar(), out var max))
                    value = Math.Min(value, max);
                values[fieldId] = value;
            }
        }

        if (answers.Count > 0 && values.Count > 0)
            row.Send(new { id, action, threadId, answers, fields = values });
        else if (answers.Count > 0)
            row.Send(new { id, action, threadId, answers });
        else if (values.Count > 0)
            row.Send(new { id, action, threadId, fields = values });
        else
            row.Send(new { id, action, threadId });
    }

    private Button? FocusedAction => Keyboard.FocusedElement is Button { Tag: string id } button
        && buttons.TryGetValue(id, out var action) && action == button ? button : null;

    internal TranscriptRow? FocusedApproval => row?.Kind == "approval"
        && FocusedAction is { IsEnabled: true, Tag: string id }
        && id.StartsWith("approval:", StringComparison.Ordinal) ? row : null;

    private void CardKeyDown(object sender, KeyEventArgs e)
    {
        if (!card || Keyboard.Modifiers != ModifierKeys.None || FocusedAction is not { } button)
            return;
        if (e.Key == Key.Escape && escape.ValueKind != JsonValueKind.Undefined)
        {
            e.Handled = true;
            if (!e.IsRepeat)
                Submit(escape, includeDraft: true);
        }
        else if (e.Key == Key.Enter && button.IsEnabled)
        {
            e.Handled = true;
            if (!e.IsRepeat)
                button.RaiseEvent(new RoutedEventArgs(ButtonBase.ClickEvent, button));
        }
    }

    internal void FocusAction()
    {
        if (!card)
            return;
        var first = buttons.Values.FirstOrDefault(button => button.IsEnabled);
        first?.Focus();
    }

    private static TextBlock Label(string text = "", bool semiBold = false, Brush? color = null)
    {
        var label = new TextBlock
        {
            Text = text,
            TextWrapping = TextWrapping.Wrap,
            FontWeight = semiBold ? FontWeights.SemiBold : FontWeights.Normal,
        };
        if (color is not null)
            label.Foreground = color;
        return label;
    }

    private static void SetLabel(TextBlock label, string value, Brush? color = null)
    {
        label.Text = value;
        label.Visibility = value.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        if (color is not null)
            label.Foreground = color;
    }
}
