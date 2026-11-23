using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

// The skill picker over the composer (`/` at the start of a message, `$` inside one). The
// controller decides everything: when it opens, which rows, and what a pick inserts in place
// of [start, end) of the text. WPF draws the popup, keeps the highlighted row, and applies a
// pick to the editor, so the edit travels to the controller like any other keystroke.
// A trigger the provider does not support shows a tooltip and leaves the text alone.
public partial class ComposerView
{
    private sealed record SkillRow(string Label, string Hint, string Detail, string Scope, string Insert);

    private SkillRow[] skillRows = [];
    private int skillSelected;
    private int skillStart;
    private int skillEnd;
    private string skillQuery = "";
    private string skillTip = "";
    private bool skillRetry;
    private int lastCaret = -1;

    private bool SkillPickerOpen => skillRows.Length > 0 && SkillPopup.IsOpen;

    private void RenderSkills(JsonElement skills)
    {
        var rows = Items(Field(skills, "rows"))
            .Select(r => new SkillRow(
                Field(r, "label").String(), Field(r, "hint").String(), Field(r, "detail").String(),
                Field(r, "scope").String(), Field(r, "insert").String()))
            .ToArray();
        var tooltip = Field(skills, "tooltip").String();
        var status = Field(skills, "status").String();
        var message = Field(skills, "message").String();
        var query = Field(skills, "query").String();
        var shown = editingEnabled && Editor.IsKeyboardFocusWithin &&
                    (rows.Length > 0 || tooltip.Length > 0 || message.Length > 0);
        if (!shown)
        {
            skillRows = [];
            skillTip = "";
            SkillPopup.IsOpen = false;
            return;
        }

        skillStart = Field(skills, "start").ComposerInt();
        skillEnd = Field(skills, "end").ComposerInt();
        // A new query starts at the top again; the same query keeps the user's choice.
        skillSelected = query == skillQuery && rows.Length == skillRows.Length
            ? Math.Min(skillSelected, Math.Max(0, rows.Length - 1))
            : Math.Min(Field(skills, "selected").ComposerInt(), Math.Max(0, rows.Length - 1));
        skillQuery = query;
        skillRows = rows;
        skillTip = tooltip;
        skillRetry = status == "failed";
        BuildSkillPopup(tooltip.Length > 0 ? tooltip : message);
        SkillPopup.IsOpen = true;
    }

    private void BuildSkillPopup(string note)
    {
        SkillRows.Children.Clear();
        SkillNote.Text = note;
        SkillNote.Visibility = VisibleWhen(note.Length > 0);
        SkillRetryButton.Visibility = VisibleWhen(skillRetry);
        SkillScroller.Visibility = VisibleWhen(skillRows.Length > 0);
        for (var i = 0; i < skillRows.Length; i++)
        {
            var row = skillRows[i];
            var index = i;
            var name = new TextBlock
            {
                Text = row.Hint.Length > 0 ? $"{row.Label} {row.Hint}" : row.Label,
                FontWeight = FontWeights.SemiBold,
                Foreground = (Brush)FindResource("YaacText"),
                TextTrimming = TextTrimming.CharacterEllipsis,
            };
            var detail = new TextBlock
            {
                Text = row.Detail,
                Style = (Style)FindResource("ComposerQuietText"),
                TextWrapping = TextWrapping.NoWrap,
                TextTrimming = TextTrimming.CharacterEllipsis,
            };
            var scope = new TextBlock
            {
                Text = row.Scope,
                Style = (Style)FindResource("ComposerQuietText"),
                Margin = new Thickness(10, 0, 0, 0),
                VerticalAlignment = VerticalAlignment.Top,
            };
            var text = new StackPanel();
            text.Children.Add(name);
            text.Children.Add(detail);
            var line = new DockPanel { LastChildFill = true };
            DockPanel.SetDock(scope, Dock.Right);
            line.Children.Add(scope);
            line.Children.Add(text);
            var item = new Border
            {
                Child = line,
                Padding = new Thickness(8, 3, 8, 3),
                CornerRadius = new CornerRadius(4),
                Focusable = false,
                Background = index == skillSelected ? (Brush)FindResource("YaacSelected") : Brushes.Transparent,
                Cursor = Cursors.Hand,
            };
            System.Windows.Automation.AutomationProperties.SetName(item, $"{row.Label}. {row.Detail}");
            // Mouse down, not click: the editor must not lose its caret before the pick lands.
            item.MouseLeftButtonDown += (_, e) =>
            {
                e.Handled = true;
                PickSkill(index);
            };
            item.MouseEnter += (_, _) => HighlightSkill(index);
            SkillRows.Children.Add(item);
        }
    }

    private void HighlightSkill(int index)
    {
        if (index == skillSelected || index < 0 || index >= SkillRows.Children.Count)
            return;
        skillSelected = index;
        for (var i = 0; i < SkillRows.Children.Count; i++)
        {
            if (SkillRows.Children[i] is Border border)
                border.Background = i == index ? (Brush)FindResource("YaacSelected") : Brushes.Transparent;
        }
        if (SkillRows.Children[index] is FrameworkElement chosen)
            chosen.BringIntoView();
    }

    // Applies the row's insertion in place of the trigger and the name typed so far.
    private void PickSkill(int index)
    {
        if (index < 0 || index >= skillRows.Length || !editingEnabled)
            return;
        var text = Editor.Text;
        if (skillStart < 0 || skillEnd < skillStart || skillEnd > text.Length)
            return;
        var insert = skillRows[index].Insert;
        Editor.BeginChange();
        try
        {
            Editor.Select(skillStart, skillEnd - skillStart);
            Editor.SelectedText = insert;
            Editor.Select(Math.Min(skillStart + insert.Length, Editor.Text.Length), 0);
        }
        finally
        {
            Editor.EndChange();
        }
        SkillPopup.IsOpen = false;
        skillRows = [];
        Editor.Focus();
        QueueCaretScroll();
    }

    // Up, Down, Tab, Enter and Esc belong to the picker while it lists rows. Returns whether
    // the key was taken.
    private bool SkillKeyDown(KeyEventArgs e)
    {
        if (!SkillPickerOpen || Keyboard.Modifiers != ModifierKeys.None)
            return false;
        switch (e.Key)
        {
            case Key.Down:
                HighlightSkill((skillSelected + 1) % skillRows.Length);
                return true;
            case Key.Up:
                HighlightSkill((skillSelected + skillRows.Length - 1) % skillRows.Length);
                return true;
            case Key.Tab:
            case Key.Enter:
                if (!e.IsRepeat)
                    PickSkill(skillSelected);
                return true;
            case Key.Escape:
                SkillPopup.IsOpen = false;
                skillRows = [];
                Command?.Invoke("composer.skillDismiss", new { key });
                return true;
            default:
                return false;
        }
    }

    private void SkillRetryClicked(object sender, RoutedEventArgs e)
    {
        Command?.Invoke("composer.skillRetry", new { key });
        Editor.Focus();
    }

    // The caret moved without an edit (an arrow key, a click): the picker follows it.
    private void EditorSelectionChanged(object sender, RoutedEventArgs e)
    {
        if (applyingText || !editingEnabled || Editor.CaretIndex == lastCaret)
            return;
        lastCaret = Editor.CaretIndex;
        Command?.Invoke("composer.caret", new { key, caret = lastCaret });
    }

    private void CloseSkills()
    {
        SkillPopup.IsOpen = false;
        skillRows = [];
    }
}
