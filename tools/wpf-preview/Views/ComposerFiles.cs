using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class ComposerView
{
    private sealed record FileMatch(string Name, string Path, string Reference);
    private readonly DispatcherTimer fileSearchTimer = new() { Interval = TimeSpan.FromMilliseconds(120) };
    private JsonElement fileSearch;
    private (int Start, string Query)? fileToken;
    private bool fileDismissed;
    private string fileScope = "";
    private string? shownFileSearch;

    private void RenderFileSearch(JsonElement search, string scope)
    {
        if (scope != fileScope)
            CloseFiles();
        fileScope = scope;
        fileSearch = search.ValueKind == JsonValueKind.Undefined ? default : search.Clone();
        RefreshFileToken();
    }

    private void EditorFileSelectionChanged(object sender, RoutedEventArgs e) => RefreshFileToken();

    private (int Start, string Query)? GetFileToken()
    {
        if (fileScope.Length == 0 || !editingEnabled || !Editor.IsKeyboardFocusWithin || imeComposing || Editor.SelectionLength != 0)
            return null;

        var text = Editor.Text;
        var caret = Editor.CaretIndex;
        var start = caret;
        while (start > 0 && !char.IsWhiteSpace(text[start - 1]))
            start--;
        if (start == caret || text[start] != '@' || (start + 1 < caret && text[start + 1] == '"'))
            return null;
        return (start, text[(start + 1)..caret]);
    }

    private void RefreshFileToken()
    {
        if (FilePopup is null || applyingText)
            return;
        var token = GetFileToken();
        if (token is null)
        {
            CloseFiles();
            return;
        }

        if (token != fileToken)
        {
            fileToken = token;
            fileDismissed = false;
            fileSearchTimer.Stop();
            fileSearchTimer.Start();
        }
        if (fileDismissed)
            return;

        var query = token.Value.Query;
        var matches = Field(fileSearch, "query").ValueKind == JsonValueKind.String
            && Field(fileSearch, "query").String() == query;
        var search = matches ? fileSearch.GetRawText() : null;
        if (search != shownFileSearch || !FilePopup.IsOpen)
        {
            shownFileSearch = search;
            DisplayText(FileSearchHint, matches ? Field(fileSearch, "hint").String() : "Finding files…");
            var entries = Field(fileSearch, "entries");
            FileMatches.ItemsSource = matches && entries.ValueKind == JsonValueKind.Array
                ? entries.EnumerateArray().Select(entry => new FileMatch(
                    Field(entry, "name").String(), Field(entry, "path").String(), Field(entry, "reference").String())).ToArray()
                : Array.Empty<FileMatch>();
            FileMatches.SelectedIndex = FileMatches.Items.Count == 0 ? -1 : 0;
            FileMatches.Visibility = VisibleWhen(FileMatches.Items.Count > 0);
        }
        FilePopup.IsOpen = true;
    }

    private void RequestFiles()
    {
        fileSearchTimer.Stop();
        if (!fileDismissed && fileToken is { } token && token == GetFileToken())
            Command?.Invoke("composer.fileQuery", new { key, query = token.Query });
    }

    private void CloseFiles()
    {
        fileSearchTimer.Stop();
        fileToken = null;
        fileDismissed = false;
        if (FilePopup is not null)
            FilePopup.IsOpen = false;
    }

    private bool HandleFileKey(Key key)
    {
        if (!FilePopup.IsOpen)
            return false;
        switch (key)
        {
            case Key.Escape:
                fileDismissed = true;
                FilePopup.IsOpen = false;
                return true;
            case Key.Up:
            case Key.Down:
                if (FileMatches.Items.Count > 0)
                {
                    var change = key == Key.Up ? -1 : 1;
                    FileMatches.SelectedIndex = Math.Clamp(FileMatches.SelectedIndex + change, 0, FileMatches.Items.Count - 1);
                    FileMatches.ScrollIntoView(FileMatches.SelectedItem);
                }
                return true;
            case Key.Tab:
            case Key.Enter:
                if (FileMatches.SelectedItem is FileMatch match)
                    InsertFile(match);
                return true;
            default:
                return false;
        }
    }

    private void FileMatchClicked(object sender, MouseButtonEventArgs e)
    {
        if (e.OriginalSource is DependencyObject source
            && ItemsControl.ContainerFromElement(FileMatches, source) is ListBoxItem { DataContext: FileMatch match })
        {
            e.Handled = true;
            InsertFile(match);
        }
    }

    private void InsertFile(FileMatch match)
    {
        if (GetFileToken() is not { } token || token != fileToken)
            return;
        var text = Editor.Text;
        var end = Editor.CaretIndex;
        while (end < text.Length && !char.IsWhiteSpace(text[end]))
            end++;
        var reference = end == text.Length ? match.Reference + " " : match.Reference;
        CloseFiles();
        Editor.Select(token.Start, end - token.Start);
        InsertText(reference);
    }
}
