using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public partial class BrowserView
{
    private sealed record TerminalTitle(ulong Tab, bool Selected, Border Row, Grid Slot, Button Pick, TextBlock Label);
    private sealed record TitleEdit(string Thread, TerminalTitle Title, TextBox Editor, Window? Window);
    private readonly Dictionary<ulong, TerminalTitle> renameTabs = new();
    private TitleEdit? rename;
    internal bool IsRenamingTerminal => rename != null;

    private static string TerminalTitleTip(JsonElement tab, string title) =>
        $"{title}\n{Text(tab, "shell")} in {Text(tab, "cwd")}";

    internal bool RenameFocusedTab()
    {
        if (!TabStrip.IsKeyboardFocusWithin || Keyboard.FocusedElement is not Button { Tag: ValueTuple<ulong?, bool> tag }
            || tag.Item1 is not ulong id || !renameTabs.TryGetValue(id, out var title)) return false;
        BeginRename(title);
        return true;
    }

    private void BeginRename(TerminalTitle title)
    {
        if (!title.Row.IsVisible) return;
        EndRename(false, false);
        var editor = new TextBox { Text = title.Label.Text, Margin = new Thickness(24, 3, 1, 3),
            MinWidth = 0, MinHeight = 0, Padding = new Thickness(3, 0, 3, 0),
            Foreground = Palette.Brush("YaacText"), Template = (ControlTemplate)Resources["TerminalTitleField"] };
        AutomationProperties.SetName(editor, "Terminal tab name");
        var window = Window.GetWindow(this);
        rename = new(thread, title, editor, window);
        title.Label.Visibility = Visibility.Hidden;
        title.Slot.Children.Add(editor);
        editor.PreviewKeyDown += (_, e) =>
        {
            if (e.Key is not (Key.Enter or Key.Escape)) return;
            e.Handled = true;
            EndRename(e.Key == Key.Enter, true);
        };
        editor.LostKeyboardFocus += (_, _) => { if (rename?.Editor == editor) EndRename(false, false); };
        editor.IsVisibleChanged += (_, _) => { if (!editor.IsVisible && rename?.Editor == editor) EndRename(false, false); };
        if (window != null)
        {
            window.PreviewMouseDown += RenameMouseDown;
            window.Deactivated += RenameWindowDeactivated;
        }
        editor.Focus();
        editor.SelectAll();
        editor.BringIntoView();
    }

    private void RenameMouseDown(object sender, MouseButtonEventArgs e)
    {
        if (rename is not { } edit) return;
        if (e.OriginalSource is Visual source && (source == edit.Editor || edit.Editor.IsAncestorOf(source))) return;
        EndRename(false, false);
    }

    private void RenameWindowDeactivated(object? sender, EventArgs e) => EndRename(false, false);

    private void EndRename(bool commit, bool focusTab)
    {
        if (rename is not { } edit) return;
        rename = null;
        if (edit.Window != null)
        {
            edit.Window.PreviewMouseDown -= RenameMouseDown;
            edit.Window.Deactivated -= RenameWindowDeactivated;
        }
        edit.Title.Label.Visibility = Visibility.Visible;
        edit.Title.Slot.Children.Remove(edit.Editor);
        if (focusTab) edit.Title.Pick.Focus();
        if (commit)
            Command?.Invoke("terminal.rename", new { thread = edit.Thread, tab = edit.Title.Tab,
                panel = IsTerminalPanel, data = new { name = edit.Editor.Text } });
    }
}
