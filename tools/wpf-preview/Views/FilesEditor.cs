using System.Globalization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using ICSharpCode.AvalonEdit;
using ICSharpCode.AvalonEdit.Document;
using ICSharpCode.AvalonEdit.Editing;
using ICSharpCode.AvalonEdit.Rendering;

namespace Yaac.WpfPreview.Views;

internal sealed class FilesEditor : Grid
{
    private const double CodeSize = 13;
    private readonly TextEditor left;
    private readonly TextEditor right;
    private readonly HashSet<int> expanded = new();
    private CancellationTokenSource? pending;
    private FileDocument? document;
    private FileLayout? layout;
    private ulong revision;
    private bool syncing;
    private int? navigationRow;
    private bool rightActive;

    internal event Action<string, object>? Command;

    internal FilesEditor()
    {
        ColumnDefinitions.Add(new ColumnDefinition());
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(0) });
        ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(0) });
        left = CreateEditor(false);
        right = CreateEditor(true);
        var divider = new Border();
        divider.SetResourceReference(BackgroundProperty, "YaacRule");
        SetColumn(divider, 1);
        SetColumn(right, 2);
        Children.Add(left);
        Children.Add(divider);
        Children.Add(right);
        left.TextArea.TextView.ScrollOffsetChanged += (_, _) => Sync(left, right);
        right.TextArea.TextView.ScrollOffsetChanged += (_, _) => Sync(right, left);
    }

    internal void Clear()
    {
        pending?.Cancel();
        expanded.Clear();
        document = null;
        layout = null;
        left.Document = null;
        right.Document = null;
    }

    internal void Render(IncomingFileDocument value)
    {
        pending?.Cancel();
        expanded.Clear();
        revision = value.Id;
        navigationRow = null;
        rightActive = false;
        document = value.Data;
        Apply(value.Layout, null);
    }

    internal void Navigate(bool forward)
    {
        if (layout?.Left.Rows is not { Count: > 0 } rows) return;
        var editor = rightActive ? right : left;
        var anchor = editor.TextArea.TextView.GetPosition(new Point(0, editor.VerticalOffset))?.Line;
        int? row = navigationRow ?? (anchor is > 1 && anchor <= rows.Count ? rows[anchor.Value - 1].Row : null);
        Command?.Invoke("change", new { document = revision, row, forward });
    }

    internal void GoTo(ulong id, int row)
    {
        if (id != revision || layout?.Left.Rows is not { } rows) return;
        var line = rows.FindIndex(item => item.Row == row) + 1;
        if (line == 0) return;
        var editor = rightActive ? right : left;
        editor.TextArea.Caret.Line = line;
        editor.TextArea.Caret.Column = 1;
        editor.SelectionLength = 0;
        editor.ScrollToLine(line);
        navigationRow = row;
    }

    private TextEditor CreateEditor(bool second)
    {
        var editor = new TextEditor
        {
            IsReadOnly = true, FontFamily = YaacFonts.Mono, FontSize = CodeSize,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
        };
        editor.SetResourceReference(BackgroundProperty, "YaacCanvas");
        editor.SetResourceReference(Control.ForegroundProperty, "YaacText");
        editor.SetResourceReference(TextEditor.LineNumbersForegroundProperty, "YaacMuted");
        editor.TextArea.SelectionBrush = Palette.Brush("YaacSelected");
        editor.TextArea.SelectionBorder = null;
        editor.Options.EnableHyperlinks = false;
        editor.Options.EnableEmailHyperlinks = false;
        editor.Options.HighlightCurrentLine = false;
        // Matching viewport heights keep vertical offsets aligned even when only one side has long lines.
        editor.Tag = new DiffMargin(this, second);
        editor.TextArea.TextView.LineTransformers.Add(new SyntaxColorizer(() => View(second)));
        editor.TextArea.TextView.BackgroundRenderers.Add(new DiffBackground(() => View(second)));
        AutomationProperties.SetName(editor, second ? "Working copy" : "File or diff contents");

        var menu = new ContextMenu();
        var add = new MenuItem { Header = "Add to composer", InputGestureText = "Ctrl+Shift+A" };
        add.Click += (_, _) => AddSelection(editor, second);
        menu.Items.Add(add);
        menu.Items.Add(new MenuItem { Header = "Copy", Command = ApplicationCommands.Copy, CommandTarget = editor.TextArea });
        menu.Opened += (_, _) => add.IsEnabled = editor.SelectionLength > 0;
        editor.ContextMenu = menu;
        editor.GotKeyboardFocus += (_, _) => rightActive = second;
        editor.PreviewMouseWheel += (_, _) => navigationRow = null;
        editor.PreviewKeyDown += (_, e) =>
        {
            navigationRow = null;
            if (e.Key != Key.A || Keyboard.Modifiers != (ModifierKeys.Control | ModifierKeys.Shift)) return;
            if (!e.IsRepeat) AddSelection(editor, second);
            e.Handled = true;
        };
        editor.PreviewMouseLeftButtonDown += (_, e) =>
        {
            navigationRow = null;
            if (editor.GetPositionFromPoint(e.GetPosition(editor)) is not { } position
                || View(second)?.Rows is not { } rows || position.Line > rows.Count
                || rows[position.Line - 1] is not { FoldCount: > 0 } row) return;
            ToggleFold(row.Row);
            e.Handled = true;
        };
        return editor;
    }

    private EditorDocument? View(bool second) => second ? layout?.Right : layout?.Left;

    private async void ToggleFold(int row)
    {
        if (document is not { } source) return;
        if (!expanded.Add(row)) expanded.Remove(row);
        pending?.Cancel();
        using var next = new CancellationTokenSource();
        pending = next;
        try
        {
            var folds = new HashSet<int>(expanded);
            var prepared = await Task.Run(() => source.Layout(folds, next.Token), next.Token);
            if (pending == next) Apply(prepared, row);
        }
        catch (OperationCanceledException)
        {
        }
        finally
        {
            if (pending == next) pending = null;
        }
    }

    private void Apply(FileLayout value, int? anchor)
    {
        layout = value;
        ColumnDefinitions[1].Width = new GridLength(value.Right is null ? 0 : 1);
        ColumnDefinitions[2].Width = value.Right is null ? new GridLength(0) : new GridLength(1, GridUnitType.Star);
        right.Visibility = value.Right is null ? Visibility.Collapsed : Visibility.Visible;
        Attach(left, value.Left, value.Right is not null);
        if (value.Right is { } workingCopy) Attach(right, workingCopy, true);
        else right.Document = null;

        if (anchor is int row) GoTo(revision, row);
        else
        {
            left.ScrollToHome();
            right.ScrollToHome();
        }
    }

    private static void Attach(TextEditor editor, EditorDocument view, bool paired)
    {
        var margin = (DiffMargin)editor.Tag;
        editor.ShowLineNumbers = view.Rows is null;
        if (view.Rows is not null && !editor.TextArea.LeftMargins.Contains(margin))
            editor.TextArea.LeftMargins.Add(margin);
        else if (view.Rows is null)
            editor.TextArea.LeftMargins.Remove(margin);
        editor.HorizontalScrollBarVisibility = paired ? ScrollBarVisibility.Visible : ScrollBarVisibility.Auto;
        editor.VerticalScrollBarVisibility = paired ? ScrollBarVisibility.Visible : ScrollBarVisibility.Auto;
        view.Text.SetOwnerThread(Thread.CurrentThread);
        editor.Document = view.Text;
        margin.InvalidateMeasure();
        margin.InvalidateVisual();
    }

    private void Sync(TextEditor source, TextEditor target)
    {
        if (syncing || layout?.Right is null || Math.Abs(source.VerticalOffset - target.VerticalOffset) < 0.01) return;
        syncing = true;
        try { target.ScrollToVerticalOffset(source.VerticalOffset); }
        finally { syncing = false; }
    }

    private void AddSelection(TextEditor editor, bool second)
    {
        if (editor.SelectionLength == 0 || View(second) is not { } view) return;
        var start = editor.Document.GetLineByOffset(editor.SelectionStart);
        var end = editor.SelectionStart + editor.SelectionLength;
        var finish = editor.Document.GetLineByOffset(end - 1);
        var first = start.LineNumber - 1;
        var last = finish.LineNumber - 1;
        int? firstColumn = Math.Min(start.Length, editor.SelectionStart - start.Offset);
        int? lastColumn = Math.Min(finish.Length, end - finish.Offset);
        if (view.Rows is not { } rows)
        {
            Command?.Invoke("add", new { document = revision, first, last, firstColumn, lastColumn });
            return;
        }

        var side = layout!.Right is null ? "unified" : second ? "new" : "old";
        while (first <= last && IsPadding(rows[first])) first++;
        while (last >= first && IsPadding(rows[last])) last--;
        if (first > last) return;
        if (first != start.LineNumber - 1) firstColumn = null;
        if (last != finish.LineNumber - 1) lastColumn = null;
        List<int> folded = [];
        for (var i = first; i <= last; i++)
            if (rows[i].FoldCount > 0) folded.Add(rows[i].Row);
        Command?.Invoke("add", new
        {
            document = revision, side,
            first = rows[first].Row, firstOffset = rows[first].Offset,
            last = rows[last].Row, lastOffset = rows[last].Offset,
            firstColumn, lastColumn, folded,
        });
    }

    private static bool IsPadding(DiffLine line) => line.Old < 0 && line.New < 0;

    private sealed class SyntaxColorizer(Func<EditorDocument?> state) : DocumentColorizingTransformer
    {
        protected override void ColorizeLine(DocumentLine line)
        {
            var view = state();
            var index = line.LineNumber - 1;
            if (view is null) return;
            if (view.Rows is { } rows && index < rows.Count && rows[index].FoldCount > 0)
                ChangeLinePart(line.Offset, line.EndOffset,
                    part => part.TextRunProperties.SetForegroundBrush(Palette.Brush("YaacMuted")));
            if (!view.Styles.TryGetValue(index, out var style)) return;
            foreach (var span in style.Spans)
            {
                var color = span.Scope switch
                {
                    "keyword" => "YaacCodeKeyword",
                    "string" => "YaacCodeString",
                    "comment" => "YaacCodeComment",
                    "number" or "constant" => "YaacCodeNumber",
                    "type" or "function" or "tag" => "YaacCodeType",
                    "attribute" or "heading" or "emphasis" => "YaacCodeKeyword",
                    "link" => "YaacUnread",
                    "operator" or "punctuation" => "YaacMuted",
                    _ => "YaacText",
                };
                Color(span.Start, span.End, part => part.TextRunProperties.SetForegroundBrush(Palette.Brush(color)));
            }

            if (view.Rows is not { } diff || index >= diff.Count || diff[index].Sign.Length == 0) return;
            var word = Palette.Brush(diff[index].Sign == "+" ? "YaacDiffAddWord" : "YaacDiffDelWord");
            foreach (var range in style.Words ?? [])
                Color(range[0], range[1], part => part.TextRunProperties.SetBackgroundBrush(word));

            void Color(int start, int end, Action<VisualLineElement> apply)
            {
                if (0 <= start && start < end && end <= line.Length)
                    ChangeLinePart(line.Offset + start, line.Offset + end, apply);
            }
        }
    }

    private sealed class DiffBackground(Func<EditorDocument?> state) : IBackgroundRenderer
    {
        public KnownLayer Layer => KnownLayer.Background;

        public void Draw(TextView view, DrawingContext context)
        {
            if (state()?.Rows is not { } rows || !view.VisualLinesValid) return;
            foreach (var line in view.VisualLines)
            {
                var index = line.FirstDocumentLine.LineNumber - 1;
                if (index >= rows.Count) continue;
                var resource = rows[index].Sign switch
                {
                    "+" => "YaacDiffAdded",
                    "−" => "YaacDiffRemoved",
                    _ when rows[index].FoldCount > 0 => "YaacPanel",
                    _ => null,
                };
                if (resource is null) continue;
                context.DrawRectangle(Palette.Brush(resource), null,
                    new Rect(0, line.VisualTop - view.VerticalOffset, view.ActualWidth, line.Height));
            }
        }
    }

    private sealed class DiffMargin(FilesEditor owner, bool second) : AbstractMargin
    {
        protected override Size MeasureOverride(Size availableSize) =>
            new(second || owner.layout?.Right is not null ? 62 : 108, 0);

        protected override void OnTextViewChanged(TextView oldView, TextView newView)
        {
            if (oldView is not null) oldView.VisualLinesChanged -= Redraw;
            base.OnTextViewChanged(oldView, newView);
            if (newView is not null) newView.VisualLinesChanged += Redraw;
        }

        private void Redraw(object? sender, EventArgs e) => InvalidateVisual();

        protected override void OnRender(DrawingContext context)
        {
            if (owner.View(second)?.Rows is not { } rows || TextView is not { VisualLinesValid: true } view) return;
            context.DrawRectangle(Palette.Brush("YaacPanel"), null, new Rect(RenderSize));
            context.DrawRectangle(Palette.Brush("YaacRule"), null, new Rect(ActualWidth - 1, 0, 1, ActualHeight));
            var paired = owner.layout?.Right is not null;
            var dpi = VisualTreeHelper.GetDpi(this).PixelsPerDip;
            foreach (var line in view.VisualLines)
            {
                var index = line.FirstDocumentLine.LineNumber - 1;
                if (index >= rows.Count) continue;
                var row = rows[index];
                var y = line.GetTextLineVisualYPosition(line.TextLines[0], VisualYPosition.TextTop) - view.VerticalOffset;
                if (row.Sign is "+" or "−")
                    context.DrawRectangle(Palette.Brush(row.Sign == "+" ? "YaacDiffAdded" : "YaacDiffRemoved"), null,
                        new Rect(0, line.VisualTop - view.VerticalOffset, Math.Max(0, ActualWidth - 1), line.Height));
                if (!paired || !second) Number(row.Old, paired ? ActualWidth - 17 : 45);
                if (!paired || second) Number(row.New, ActualWidth - 17);
                if (row.Sign.Length > 0) Draw(row.Sign, ActualWidth - 4, row.Sign == "+" ? "YaacSuccess" : "YaacFailure");
                else if (row.FoldButton) Draw(row.FoldCount > 0 ? "▸" : "▾", ActualWidth - 4, "YaacMuted");

                void Number(int number, double edge)
                {
                    if (number >= 0) Draw((number + 1).ToString(CultureInfo.CurrentCulture), edge, "YaacMuted");
                }

                void Draw(string label, double edge, string brush)
                {
                    var text = new FormattedText(label, CultureInfo.CurrentCulture, FlowDirection.LeftToRight,
                        new Typeface(YaacFonts.Mono, FontStyles.Normal, FontWeights.Normal, FontStretches.Normal),
                        CodeSize, Palette.Brush(brush), dpi);
                    context.DrawText(text, new Point(edge - text.Width, y));
                }
            }
        }

        protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
        {
            if (TextView is not { } view || owner.View(second)?.Rows is not { } rows) return;
            var y = e.GetPosition(this).Y;
            var position = view.GetPosition(new Point(0, y + view.VerticalOffset));
            if (position is not { } line || line.Line > rows.Count || !rows[line.Line - 1].FoldButton) return;
            owner.ToggleFold(rows[line.Line - 1].Row);
            e.Handled = true;
        }
    }
}
