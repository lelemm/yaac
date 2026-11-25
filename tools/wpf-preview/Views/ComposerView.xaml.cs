using System.IO;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class ComposerView : UserControl
{
    private const int MaxPendingEdits = 16;

    private readonly List<string> pendingEdits = new();
    private readonly Dictionary<(string Key, string TextKey), string> lastInserts = new();
    private string key = "";
    private string textKey = "";
    private string serverText = "";
    private string focusToken = "";
    private bool focusRequested;
    private bool applyingText;
    private bool imeComposing;
    private bool ctrlEnter;
    private bool sendNowOffered;
    private bool editingEnabled;
    private bool? submitReady;
    private bool submitWithoutText;
    private bool caretScrollQueued;
    private string[] messageHistory = [];
    private int messageHistoryIndex = -1;
    private Window? dropHost;
    private bool wasAllowDrop;
    private PasteSelection? pasteSelection;
    private ComposerAction? attachAction;
    private ComposerAction? cancelEditAction;
    private ComposerAction? pasteAction;
    private ComposerAction? dropAction;

    private sealed record PasteSelection(string Text, int Start, int Length);

    public ComposerView()
    {
        InitializeComponent();
        Markdown.Attach(Editor);
        IsVisibleChanged += (_, _) =>
        {
            if (!IsVisible)
                DropOverlayChanged?.Invoke(false);
            else
                ApplyFocus();
        };
        fileSearchTimer.Tick += (_, _) => RequestFiles();
        TextCompositionManager.AddPreviewTextInputStartHandler(Editor, (_, _) => SetComposing(true));
        TextCompositionManager.AddPreviewTextInputUpdateHandler(Editor, (_, _) => SetComposing(true));
        TextCompositionManager.AddPreviewTextInputHandler(Editor, (_, _) => SetComposing(false));
        CommandManager.AddPreviewCanExecuteHandler(Editor, EditorCanPaste);
        CommandManager.AddPreviewExecutedHandler(Editor, EditorPaste);
    }

    public event Action<string, object?>? Command;
    internal event Action<bool>? DropOverlayChanged;
    internal int CaretIndex => Editor.CaretIndex;

    // Each render describes the complete composer for a selection. Action ids are opaque to
    // WPF. Picker and clipboard work run in the controller, which returns a single-use insert
    // token for a text paste and row descriptors for upload and queue state.
    public void Render(JsonElement view)
    {
        var nextKey = Field(view, "key").Identifier();
        if (nextKey.Length == 0)
        {
            messageHistoryIndex = -1;
            Visibility = Visibility.Collapsed;
            editingEnabled = false;
            CloseFiles();
            focusRequested = false;
            DropOverlayChanged?.Invoke(false);
            return;
        }

        var nextTextKey = Field(view, "textKey").Identifier();
        var keyChanged = nextKey != key;
        var editorChanged = keyChanged || nextTextKey != textKey;
        var enteringQueueEdit = !keyChanged && editorChanged;
        key = nextKey;
        textKey = nextTextKey;
        if (editorChanged)
        {
            messageHistoryIndex = -1;
            CloseFiles();
            pasteSelection = null;
            SetComposing(false);
            focusRequested = false;
        }

        var showEditor = Field(view, "showEditor").ComposerFlag(true);
        editingEnabled = showEditor && Field(view, "editable").ComposerFlag(true);
        EditorPlate.Visibility = VisibleWhen(showEditor);
        Editor.IsReadOnly = !editingEnabled;
        Editor.IsTabStop = editingEnabled;
        EditorPlate.Opacity = editingEnabled ? 1 : 0.72;
        Visibility = Visibility.Visible;

        var placeholder = Field(view, "placeholder").String("Message…");
        Placeholder.Text = placeholder;
        System.Windows.Automation.AutomationProperties.SetName(Editor, placeholder);
        var text = Field(view, "text").String();
        normalPlaceholder = placeholder;
        RenderShell(view);
        UpdateText(text, editorChanged);
        if (shellMode)
            ApplyShellLook();
        Markdown.SetSpans(Normalize(text), Field(view, "markdown"), Field(view, "markdownBudget").ComposerInt());
        UpdatePlaceholder();
        RenderFileSearch(Field(view, "filePicker"), Field(view, "fileScope").String());

        normalHint = Field(view, "hint").String();
        ShowHint();
        DisplayText(UltrafastWarning, Field(view, "ultrafastWarning").String());
        RenderStatus(Field(view, "status"));
        RenderRows(Notices, Field(view, "notices"), RowKind.Notice);
        RenderRunningCommands(Field(view, "runningCommands"), keyChanged);
        RenderRows(QueueRows, Field(view, "queue"), RowKind.Queue);
        if (keyChanged)
            QueueScroller.ScrollToTop();
        RenderRows(AttachmentRows, Field(view, "attachments"), RowKind.Attachment);
        AttachmentRows.Visibility = VisibleWhen(AttachmentRows.Children.Count > 0);
        QueueArea.Visibility = VisibleWhen(QueueRows.Children.Count > 0);
        DisplayText(QueueTitle, Field(view, "queueLabel").String());

        RenderProject(showEditor ? view : default);
        RenderControls(view);
        RenderEditorActions(view);
        RenderSkills(Field(view, "skills"));
        if (!CanDrop)
            DropOverlayChanged?.Invoke(false);

        var nextFocus = Field(view, "focus");
        var token = nextFocus.ValueKind == JsonValueKind.True ? "true" : nextFocus.Identifier();
        if (editingEnabled && (enteringQueueEdit || token.Length > 0 && (token != focusToken || editorChanged)))
        {
            focusRequested = true;
            Dispatcher.BeginInvoke(DispatcherPriority.Input, new Action(ApplyFocus));
        }
        focusToken = token;

        var insert = Field(view, "insert");
        var insertId = Field(insert, "id").Identifier();
        var insertText = Field(insert, "text").String();
        if (editingEnabled && insertId.Length > 0 && insertText.Length > 0)
            ScheduleInsert(insertId, insertText);
    }

    private void RenderEditorActions(JsonElement view)
    {
        ctrlEnter = Field(view, "sendKey").String() == "ctrlEnter";
        var sendKey = ctrlEnter ? "Ctrl+Enter" : "Enter";
        var nowKey = ctrlEnter ? "Ctrl+Shift+Enter" : "Ctrl+Enter";
        var newlineKey = ctrlEnter ? "Enter" : "Shift+Enter";
        System.Windows.Automation.AutomationProperties.SetHelpText(Editor,
            $"New line with {newlineKey}. Send or queue with {sendKey}. Send now with {nowKey}.");

        sendNowAction = ReadAction(Field(view, "sendNow"));
        sendNowOffered = sendNowAction is not null;
        attachAction = ReadAction(Field(view, "attach"));
        cancelEditAction = ReadAction(Field(view, "cancelEdit"));
        pasteAction = ReadAction(Field(view, "paste"));
        dropAction = ReadAction(Field(view, "drop"));

        normalSend = ReadAction(Field(view, "send"));
        sendKeyLabel = sendKey;
        nowKeyLabel = nowKey;
        ShowSubmitButtons();
        SetButton(StopButton, ReadAction(Field(view, "stop")), editingEnabled, "Esc");
        SetButton(CancelEditButton, cancelEditAction, editingEnabled, "Esc");
        var ready = Field(view, "submitReady");
        submitReady = ready.ValueKind is JsonValueKind.True or JsonValueKind.False ? ready.GetBoolean() : null;
        submitWithoutText = Field(view, "submitWithoutText").ComposerFlag();
        UpdateSendButtons();
    }

    private void UpdateText(string value, bool reset)
    {
        value = Normalize(value);
        var shell = IsShellText(shellCapable, value);
        if (!reset && shell == shellMode)
        {
            // A transcript redraw or an echo of an earlier keystroke must not replace the
            // local buffer, selection, scroll position, or undo history.
            if (value == serverText)
                return;
            var index = pendingEdits.IndexOf(value);
            if (index >= 0)
            {
                pendingEdits.RemoveRange(0, index + 1);
                serverText = value;
                return;
            }
        }
        serverText = value;
        pendingEdits.Clear();
        if (shell != shellMode || reset)
        {
            shellMode = shell;
            shellHistoryIndex = -1;
            ApplyShellLook();
        }
        value = ShellField(shell, value);

        var before = Editor.Text;
        var start = Editor.SelectionStart;
        var end = start + Editor.SelectionLength;
        var verticalOffset = Editor.VerticalOffset;
        applyingText = true;
        try
        {
            if (reset)
                Editor.IsUndoEnabled = false;
            var replaced = Normalize(before) != value;
            if (replaced)
            {
                messageHistoryIndex = -1;
                Editor.Text = value;
            }
            if (reset)
            {
                Editor.Select(0, 0);
                Editor.ScrollToHome();
            }
            else if (replaced)
            {
                // The controller changed the text: the caret and selection keep their place in
                // the text around them (after a paste stays after it), not their old index.
                var from = MapThroughChange(before, Editor.Text, start);
                var to = Math.Max(from, MapThroughChange(before, Editor.Text, end));
                Editor.Select(from, to - from);
                if (Editor.Text.Length > 0 && double.IsFinite(verticalOffset))
                    Editor.ScrollToVerticalOffset(verticalOffset);
            }
        }
        finally
        {
            if (reset)
                Editor.IsUndoEnabled = true;
            applyingText = false;
        }
    }

    private static string Normalize(string value) => value.Replace("\r\n", "\n");

    // Where index `at` of `before` lies in `after`: unchanged in the common prefix, the same
    // distance from the end in the common suffix, and the end of the changed span otherwise.
    internal static int MapThroughChange(string before, string after, int at)
    {
        at = Math.Clamp(at, 0, before.Length);
        var prefix = 0;
        var most = Math.Min(before.Length, after.Length);
        while (prefix < most && before[prefix] == after[prefix])
            prefix++;
        var suffix = 0;
        while (suffix < most - prefix && before[before.Length - 1 - suffix] == after[after.Length - 1 - suffix])
            suffix++;
        if (at <= prefix)
            return at;
        if (at >= before.Length - suffix)
            return after.Length - (before.Length - at);
        return after.Length - suffix;
    }

    private void EditorTextChanged(object sender, TextChangedEventArgs e)
    {
        if (Placeholder is not null)
            UpdatePlaceholder();
        if (applyingText || !editingEnabled)
            return;

        messageHistoryIndex = -1;
        EnterShellIfTyped();
        ShellFieldEdited();
        ReportEditorEdit();
    }

    private void ReportEditorEdit()
    {
        var text = DraftText(Normalize(Editor.Text));
        if (pendingEdits.Count == MaxPendingEdits)
            pendingEdits.RemoveAt(0);
        pendingEdits.Add(text);
        lastCaret = Editor.CaretIndex;
        Command?.Invoke("composer.edit", new { key, text, caret = DraftCaret });
        RefreshFileToken();
        UpdateSendButtons();
    }

    private void UpdateSendButtons()
    {
        if (submitReady is not { } ready)
            return;
        var enabled = editingEnabled && ready && (submitWithoutText || !string.IsNullOrWhiteSpace(Editor.Text));
        SendButton.IsEnabled = enabled;
        SendNowButton.IsEnabled = sendNowOffered && enabled;
    }

    private void UpdatePlaceholder() => Placeholder.Visibility = VisibleWhen(Editor.Text.Length == 0);

    private void EditorRowSizeChanged(object sender, SizeChangedEventArgs e)
    {
        if (EditorActions is not null)
            EditorActions.MaxWidth = Math.Clamp(e.NewSize.Width - 135, 110, 420);
    }

    private void EditorKeyDown(object sender, KeyEventArgs e)
    {
        if (imeComposing || e.Key is Key.ImeProcessed or Key.DeadCharProcessed)
            return;

        if (Keyboard.Modifiers == ModifierKeys.None && MessageHistoryKey(e.Key))
        {
            e.Handled = true;
            return;
        }

        if (SkillKeyDown(e))
        {
            e.Handled = true;
            return;
        }

        var modifiers = Keyboard.Modifiers;
        if (modifiers == ModifierKeys.None && HandleFileKey(e.Key))
        {
            e.Handled = true;
            return;
        }
        if (ShellKeyDown(e))
        {
            e.Handled = true;
            return;
        }
        if (e.Key == Key.Escape && modifiers == ModifierKeys.None)
        {
            if (CancelEditButton.IsEnabled && CancelEditButton.Visibility == Visibility.Visible)
            {
                e.Handled = true;
                if (!e.IsRepeat)
                    InvokeAction(cancelEditAction);
            }
            else if (StopButton.IsEnabled && StopButton.Visibility == Visibility.Visible)
            {
                e.Handled = true;
                if (!e.IsRepeat)
                    Submit(StopButton, "composer.stop");
            }
            return;
        }
        if (e.Key != Key.Enter || (modifiers & (ModifierKeys.Alt | ModifierKeys.Windows)) != 0)
            return;

        e.Handled = true;
        if (!editingEnabled)
            return;

        var ctrl = (modifiers & ModifierKeys.Control) != 0;
        var shift = (modifiers & ModifierKeys.Shift) != 0;
        if (ctrlEnter ? !ctrl : shift)
            InsertText("\r\n");
        else if (!e.IsRepeat)
            SubmitMessage(ctrlEnter ? shift : ctrl);
    }

    private void EditorComposerSelectionChanged(object sender, RoutedEventArgs e)
    {
        EditorFileSelectionChanged(sender, e);
        EditorSelectionChanged(sender, e);
    }

    private void EditorLostFocus(object sender, KeyboardFocusChangedEventArgs e)
    {
        SetComposing(false);
        DismissPickers();
    }

    private void SetComposing(bool composing)
    {
        imeComposing = composing;
        Markdown.SetComposing(composing);
    }

    internal void DismissPickers()
    {
        CloseFiles();
        CloseSkills();
    }

    private void ApplyFocus()
    {
        if (focusRequested && editingEnabled && IsVisible)
        {
            focusRequested = false;
            Editor.Focus();
        }
    }

    private void SubmitMessage(bool now)
    {
        messageHistoryIndex = -1;
        if (now && sendNowOffered && !shellMode)
            Submit(SendNowButton, "composer.sendNow");
        else
            Submit(SendButton, "composer.send");
    }

    internal void SetMessageHistory(JsonElement history)
    {
        messageHistory = history.EnumerateArray().Select(item => item.GetString() ?? "").ToArray();
        messageHistoryIndex = -1;
    }

    private bool MessageHistoryKey(Key key)
    {
        if (!editingEnabled || shellMode || textKey.Length != 0 || messageHistory.Length == 0)
            return false;
        if (key == Key.Up && (messageHistoryIndex >= 0 || Editor.Text.Length == 0))
            messageHistoryIndex = Math.Min(messageHistoryIndex + 1, messageHistory.Length - 1);
        else if (key == Key.Down && messageHistoryIndex >= 0)
            messageHistoryIndex--;
        else
            return false;

        var text = messageHistoryIndex < 0 ? "" : messageHistory[messageHistoryIndex];
        if (Normalize(Editor.Text) != text)
        {
            applyingText = true;
            try
            {
                Editor.Text = text;
                Editor.Select(Editor.Text.Length, 0);
                Editor.ScrollToEnd();
            }
            finally
            {
                applyingText = false;
            }
            ReportEditorEdit();
        }
        return true;
    }

    private void Submit(Button button, string command)
    {
        if (editingEnabled && button.IsEnabled && button.Visibility == Visibility.Visible)
            Command?.Invoke(command, new { key });
    }

    private void SendClicked(object sender, RoutedEventArgs e) => SubmitMessage(now: false);
    private void SendNowClicked(object sender, RoutedEventArgs e) => SubmitMessage(now: true);
    private void StopClicked(object sender, RoutedEventArgs e) => Submit(StopButton, "composer.stop");
    private void AttachClicked(object sender, RoutedEventArgs e) => InvokeAction(attachAction);
    private void CancelEditClicked(object sender, RoutedEventArgs e) => InvokeAction(cancelEditAction);

    private void EditorCanPaste(object sender, CanExecuteRoutedEventArgs e)
    {
        if (e.Command != ApplicationCommands.Paste || pasteAction is null)
            return;
        e.CanExecute = editingEnabled && pasteAction.Enabled;
        e.Handled = true;
    }

    private void EditorPaste(object sender, ExecutedRoutedEventArgs e)
    {
        if (e.Command != ApplicationCommands.Paste || pasteAction is null)
            return;
        e.Handled = true;
        if (!editingEnabled || !pasteAction.Enabled)
            return;

        pasteSelection = new PasteSelection(Editor.Text, Editor.SelectionStart, Editor.SelectionLength);
        InvokeAction(pasteAction);
    }

    private void ScheduleInsert(string id, string text)
    {
        var identity = (key, textKey);
        if (lastInserts.TryGetValue(identity, out var last) && last == id)
            return;
        Dispatcher.BeginInvoke(DispatcherPriority.Input, new Action(() =>
        {
            if ((key, textKey) != identity || !editingEnabled ||
                lastInserts.TryGetValue(identity, out var consumed) && consumed == id)
                return;
            lastInserts[identity] = id;
            if (pasteSelection is { } selection && selection.Text == Editor.Text)
                Editor.Select(selection.Start, selection.Length);
            pasteSelection = null;
            InsertText(text);
            if (IsKeyboardFocusWithin)
                Editor.Focus();
        }));
    }

    private void InsertText(string text)
    {
        var before = Editor.Text;
        var start = Editor.SelectionStart;
        var kept = before.Length - Editor.SelectionLength;
        applyingText = true;
        try
        {
            Editor.SelectedText = text;
            // Read the text after WPF commits the replacement so CRLF and pasted text use
            // the editor's character offsets. Report the edit after placing the caret.
            var end = Math.Clamp(start + Editor.Text.Length - kept, 0, Editor.Text.Length);
            Editor.Select(end, 0);
        }
        finally
        {
            applyingText = false;
        }
        if (editingEnabled && Editor.Text != before)
            ReportEditorEdit();
        QueueCaretScroll();
    }

    private void QueueCaretScroll()
    {
        if (caretScrollQueued) return;
        caretScrollQueued = true;
        var identity = (key, textKey);
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, () =>
        {
            caretScrollQueued = false;
            if (identity != (key, textKey) || !Editor.IsKeyboardFocusWithin || Editor.ExtentHeight <= Editor.ViewportHeight)
                return;
            var caret = Editor.GetRectFromCharacterIndex(Editor.CaretIndex);
            if (caret.IsEmpty || TranscriptView.FindVisual<ScrollContentPresenter>(Editor) is not { } viewport)
                return;
            var top = viewport.TranslatePoint(new Point(), Editor).Y;
            var bottom = top + viewport.ActualHeight;
            if (caret.Top < top)
                Editor.ScrollToVerticalOffset(Editor.VerticalOffset + caret.Top - top);
            else if (caret.Bottom > bottom)
                Editor.ScrollToVerticalOffset(Editor.VerticalOffset + caret.Bottom - bottom);
        });
    }

    private bool CanDrop => IsVisible && editingEnabled && !shellMode && dropAction?.Enabled == true;

    private void ComposerLoaded(object sender, RoutedEventArgs e)
    {
        ApplyFocus();
        dropHost = Window.GetWindow(this);
        if (dropHost is null)
            return;
        wasAllowDrop = dropHost.AllowDrop;
        dropHost.SetCurrentValue(AllowDropProperty, true);
        dropHost.PreviewDragOver += FileDragOver;
        dropHost.PreviewDragLeave += FileDragLeave;
        dropHost.PreviewDrop += FileDrop;
    }

    private void ComposerUnloaded(object sender, RoutedEventArgs e)
    {
        DropOverlayChanged?.Invoke(false);
        if (dropHost is null)
            return;
        dropHost.PreviewDragOver -= FileDragOver;
        dropHost.PreviewDragLeave -= FileDragLeave;
        dropHost.PreviewDrop -= FileDrop;
        dropHost.SetCurrentValue(AllowDropProperty, wasAllowDrop);
        dropHost = null;
    }

    private void FileDragOver(object sender, DragEventArgs e)
    {
        if (!CanDrop || !e.Data.GetDataPresent(DataFormats.FileDrop) || (e.AllowedEffects & DragDropEffects.Copy) == 0)
        {
            DropOverlayChanged?.Invoke(false);
            return;
        }
        DropOverlayChanged?.Invoke(true);
        e.Effects = DragDropEffects.Copy;
        e.Handled = true;
    }

    private void FileDragLeave(object sender, DragEventArgs e) => DropOverlayChanged?.Invoke(false);

    private void FileDrop(object sender, DragEventArgs e)
    {
        DropOverlayChanged?.Invoke(false);
        if (!CanDrop || (e.AllowedEffects & DragDropEffects.Copy) == 0 ||
            !e.Data.GetDataPresent(DataFormats.FileDrop) || e.Data.GetData(DataFormats.FileDrop) is not string[] files)
            return;
        var paths = files.Where(Path.IsPathFullyQualified).ToArray();
        if (paths.Length == 0 || dropAction is not { } action)
            return;

        e.Effects = DragDropEffects.Copy;
        e.Handled = true;
        Command?.Invoke("composer.action", new { key, id = action.Id, paths });
    }
}
