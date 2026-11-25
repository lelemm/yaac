using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

// `!command` mode (P344, P346). The draft keeps its leading `!`; the field shows the rest, in
// monospace, under a prompt row. The mode is a function of the draft text and whether the thread
// can run commands, so a local edit and the controller's echo always agree on it.
public partial class ComposerView
{
    private bool shellCapable;
    private bool shellMode;
    private string[] shellHistory = [];
    private int shellHistoryIndex = -1;
    private string shellHistoryDraft = "";
    private bool browsingHistory;
    private string normalPlaceholder = "Message…";
    private string normalHint = "";
    private string shellHint = "";
    private ComposerAction? normalSend;
    private ComposerAction? shellRun;
    private ComposerAction? sendNowAction;
    private string sendKeyLabel = "Enter";
    private string nowKeyLabel = "Ctrl+Enter";
    private const string ShellPlaceholder = "A command to run";

    /// The `!` mode this full draft text puts the field in.
    internal static bool IsShellText(bool capable, string text) => capable && text.StartsWith('!');

    /// The field text for a full draft text.
    internal static string ShellField(bool shell, string text) => shell && text.StartsWith('!') ? text[1..] : text;

    /// The full draft text for what the field shows.
    private string DraftText(string field) => shellMode ? "!" + field : field;

    private int DraftCaret => Editor.CaretIndex + (shellMode ? 1 : 0);

    private void RenderShell(JsonElement view)
    {
        shellCapable = Field(view, "shellCapable").ComposerFlag();
        var shell = Field(view, "shell");
        shellRun = ReadAction(Field(shell, "run"));
        shellHint = Field(shell, "hint").String();
        ShellPrompt.Text = Field(shell, "prompt").String();
        ShellPrompt.ToolTip = Field(shell, "cwd").String() is { Length: > 0 } cwd ? cwd : null;
        shellHistory = Items(shell, "history").Select(item => item.ValueKind == JsonValueKind.String ? item.GetString() ?? "" : "")
            .Where(command => command.Length > 0).ToArray();
        if (ShellIcon.Content is null)
            ShellIcon.Content = PaneIcon.Draw("Terminal", 14);
    }

    private static IEnumerable<JsonElement> Items(JsonElement value, string name) =>
        value.ValueKind == JsonValueKind.Object && value.TryGetProperty(name, out var items) && items.ValueKind == JsonValueKind.Array
            ? items.EnumerateArray() : [];

    /// Frame, prompt row, font and buttons follow the mode.
    private void ApplyShellLook()
    {
        var on = shellMode;
        ShellRow.Visibility = VisibleWhen(on);
        ShellBar.Visibility = VisibleWhen(on);
        ShellTint.Visibility = VisibleWhen(on);
        Markdown.Visibility = VisibleWhen(!on);
        var font = on ? YaacFonts.CodeResource : YaacFonts.UiResource;
        var size = on ? 13.5 : 14.5;
        if (!Equals(Editor.FontFamily, Application.Current.Resources[font]))
            Editor.SetResourceReference(Control.FontFamilyProperty, font);
        if (Editor.FontSize != size) Editor.FontSize = size;
        if (on)
            EditorPlate.SetResourceReference(Border.BorderBrushProperty, "YaacShell");
        else
            EditorPlate.ClearValue(Border.BorderBrushProperty);
        Placeholder.Text = on ? ShellPlaceholder : normalPlaceholder;
        System.Windows.Automation.AutomationProperties.SetName(Editor, Placeholder.Text);
        ShowHint();
        ShowSubmitButtons();
    }

    private void ShowHint() => DisplayText(Hint, shellMode && shellHint.Length > 0 ? shellHint : normalHint);

    /// The buttons beside the field. In `!` mode: Run (or Queue run), no attach, no Send now. The
    /// view carries both sets, so leaving the mode restores the message buttons at once.
    private void ShowSubmitButtons()
    {
        var run = shellMode && shellRun is not null;
        var send = run ? shellRun : normalSend;
        SetButton(SendButton, send, editingEnabled, sendKeyLabel, showShortcut: true);
        DecorateRunButton(run ? send?.Label : null, sendKeyLabel);
        SetButton(AttachButton, shellMode ? null : attachAction, editingEnabled);
        if (!shellMode && attachAction is not null)
            AttachButton.Content = "\uF0C6";
        SetButton(SendNowButton, shellMode ? null : sendNowAction, editingEnabled, nowKeyLabel);
        UpdateSendButtons();
    }

    /// The primary button in `!` mode: the terminal icon and "Run ↵" / "Queue run ↵".
    private void DecorateRunButton(string? label, string shortcut)
    {
        if (label is null)
            return;
        var row = new StackPanel { Orientation = Orientation.Horizontal };
        var icon = PaneIcon.Draw("Terminal", 13);
        icon.Margin = new Thickness(0, 0, 5, 0);
        if (icon is Viewbox { Child: Canvas { Children: [System.Windows.Shapes.Path path] } })
            path.SetBinding(System.Windows.Shapes.Shape.StrokeProperty,
                new System.Windows.Data.Binding(nameof(Foreground)) { Source = SendButton });
        row.Children.Add(icon);
        row.Children.Add(new TextBlock { Text = $"{label}  {(shortcut == "Enter" ? "↵" : shortcut)}", VerticalAlignment = VerticalAlignment.Center });
        SendButton.Content = row;
    }

    /// A user edit of the field: a `!` typed at the very start enters the mode.
    private void EnterShellIfTyped()
    {
        if (shellMode || !shellCapable || !Editor.Text.StartsWith('!'))
            return;
        var caret = Math.Max(0, Editor.CaretIndex - 1);
        applyingText = true;
        try
        {
            Editor.Text = Editor.Text[1..];
            Editor.Select(Math.Min(caret, Editor.Text.Length), 0);
        }
        finally
        {
            applyingText = false;
        }
        shellMode = true;
        ApplyShellLook();
    }

    /// Back to the normal composer at once, keeping `field` (P346).
    private void LeaveShell(string field)
    {
        if (!shellMode)
            return;
        shellMode = false;
        shellHistoryIndex = -1;
        applyingText = true;
        try
        {
            if (Editor.Text != field)
                Editor.Text = field;
            Editor.Select(0, 0);
        }
        finally
        {
            applyingText = false;
        }
        ApplyShellLook();
        UpdatePlaceholder();
        ReportEditorEdit();
    }

    private void ShellLeaveClicked(object sender, RoutedEventArgs e)
    {
        if (!editingEnabled)
            return;
        LeaveShell(Editor.Text);
        Editor.Focus();
    }

    /// Keys that only mean something in `!` mode. True when handled.
    private bool ShellKeyDown(KeyEventArgs e)
    {
        if (!shellMode || !editingEnabled || Keyboard.Modifiers != ModifierKeys.None)
            return false;
        switch (e.Key)
        {
            case Key.Back when Editor.SelectionLength == 0 && Editor.CaretIndex == 0:
                LeaveShell(Editor.Text);
                return true;
            case Key.Back or Key.Delete when Editor.Text.Length > 0 && Editor.SelectionLength == Editor.Text.Length:
                // Select-all and delete removes the `!` with the rest.
                LeaveShell("");
                return true;
            case Key.Up when OnFirstLine() && shellHistory.Length > 0:
                if (shellHistoryIndex < 0)
                    shellHistoryDraft = Editor.Text;
                if (shellHistoryIndex + 1 >= shellHistory.Length)
                    return true;
                ShowHistory(shellHistoryIndex + 1);
                return true;
            case Key.Down when shellHistoryIndex >= 0 && OnLastLine():
                ShowHistory(shellHistoryIndex - 1);
                return true;
        }
        return false;
    }

    private bool OnFirstLine() => Editor.GetLineIndexFromCharacterIndex(Editor.CaretIndex) <= 0;

    private bool OnLastLine()
    {
        var line = Editor.GetLineIndexFromCharacterIndex(Editor.CaretIndex);
        return line < 0 || line >= Editor.LineCount - 1;
    }

    /// ↑/↓ through the thread's earlier commands, newest first; past the newest, the draft.
    private void ShowHistory(int index)
    {
        shellHistoryIndex = index;
        var text = index < 0 ? shellHistoryDraft : shellHistory[index];
        browsingHistory = true;
        try
        {
            Editor.Text = text;
            Editor.Select(text.Length, 0);
        }
        finally
        {
            browsingHistory = false;
        }
    }

    /// Any edit that is not a history step ends the walk.
    private void ShellFieldEdited()
    {
        if (!browsingHistory)
            shellHistoryIndex = -1;
    }
}
