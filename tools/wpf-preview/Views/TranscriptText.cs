using System.Text;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Documents;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

internal sealed class TranscriptText : ContentControl
{
    private readonly string key;
    private readonly TranscriptRowState state;
    private readonly Func<string?> copyMessage;
    private TextBox? plain;
    private RichTextBox? rich;
    private readonly List<Piece> pieces = new();
    private string text = "";
    private string mode = "";
    private bool setting;

    private sealed record Piece(string Kind, string Text, string? Language = null, JsonElement Data = default)
    {
        public bool SameAs(Piece other) => Kind == other.Kind && Text == other.Text && Language == other.Language
            && TranscriptJson.Same(Data, other.Data);
    }

    private static readonly FontFamily Mono = YaacFonts.Mono;
    private static readonly object CodePadding = new();

    public TranscriptText(string key, TranscriptRowState state, Func<string?> copyMessage)
    {
        this.key = key;
        this.state = state;
        this.copyMessage = copyMessage;
        Visibility = Visibility.Collapsed;
    }

    private TextBox CreatePlain()
    {
        var box = new TextBox { TextWrapping = TextWrapping.Wrap, AcceptsReturn = true, IsUndoEnabled = false };
        InitializeBox(box);
        box.SelectionChanged += (_, _) =>
        {
            if (!setting && ReferenceEquals(Content, box))
                state.Selection[key] = (box.SelectionStart, box.SelectionLength);
        };
        return box;
    }

    private RichTextBox CreateRich()
    {
        var box = new RichTextBox
        {
            Document = new FlowDocument { PagePadding = new Thickness(0), TextAlignment = TextAlignment.Left },
            IsDocumentEnabled = true,
            IsUndoEnabled = false,
        };
        InitializeBox(box);
        BindingOperations.SetBinding(box.Document, TextElement.ForegroundProperty, new Binding(nameof(Foreground)) { Source = this });
        BindingOperations.SetBinding(box.Document, TextElement.FontFamilyProperty, new Binding(nameof(FontFamily)) { Source = this });
        BindingOperations.SetBinding(box.Document, TextElement.FontSizeProperty, new Binding(nameof(FontSize)) { Source = this });
        CommandManager.AddPreviewExecutedHandler(box, (_, e) =>
        {
            if (mode == "markdown" && e.Command == ApplicationCommands.Copy
                && WithoutCodePadding(box.Selection.Start, box.Selection.End) is { } selected)
            {
                Copy(selected);
                e.Handled = true;
            }
        });
        box.SelectionChanged += (_, _) =>
        {
            if (!setting && ReferenceEquals(Content, box))
                state.Selection[key] = (box.Document.ContentStart.GetOffsetToPosition(box.Selection.Start),
                    box.Selection.Start.GetOffsetToPosition(box.Selection.End));
        };
        return box;
    }

    private JsonElement renderedBlocks;

    internal int SelectionLength => Content switch
    {
        TextBox editor => editor.Text.Length,
        RichTextBox editor => editor.Document.ContentStart.GetOffsetToPosition(editor.Document.ContentEnd),
        _ => 0,
    };

    internal int SelectionOffset(Point position)
    {
        if (Content is TextBox plainEditor)
        {
            var point = TranslatePoint(position, plainEditor);
            var index = plainEditor.GetCharacterIndexFromPoint(point, snapToText: true);
            if (index < 0) return 0;
            var trailing = plainEditor.GetRectFromCharacterIndex(index, trailingEdge: true);
            var leading = plainEditor.GetRectFromCharacterIndex(index);
            return !trailing.IsEmpty && Math.Abs(point.X - trailing.X) < Math.Abs(point.X - leading.X)
                ? Math.Min(index + 1, plainEditor.Text.Length) : index;
        }
        if (Content is RichTextBox richEditor)
        {
            var pointer = richEditor.GetPositionFromPoint(TranslatePoint(position, richEditor), snapToText: true);
            if (pointer is not null) return richEditor.Document.ContentStart.GetOffsetToPosition(pointer);
        }
        return 0;
    }

    internal string SelectionText(int start, int end)
    {
        if (Content is TextBox plainEditor) return plainEditor.Text[start..end];
        if (Content is RichTextBox richEditor)
        {
            var from = PositionAt(richEditor, start);
            var to = PositionAt(richEditor, end);
            return (mode == "markdown" ? WithoutCodePadding(from, to) : null) ?? new TextRange(from, to).Text;
        }
        return "";
    }

    private static string? WithoutCodePadding(TextPointer start, TextPointer end)
    {
        StringBuilder? text = null;
        var from = start;
        for (var pointer = start; pointer is not null && pointer.CompareTo(end) < 0;
            pointer = pointer.GetNextContextPosition(LogicalDirection.Forward))
        {
            if (pointer.GetPointerContext(LogicalDirection.Forward) != TextPointerContext.Text
                || pointer.Parent is not Run run || !ReferenceEquals(run.Tag, CodePadding))
                continue;

            text ??= new StringBuilder();
            text.Append(new TextRange(from, pointer).Text);
            from = pointer.GetNextContextPosition(LogicalDirection.Forward) ?? end;
            if (from.CompareTo(end) >= 0)
                return text.ToString();
        }
        return text?.Append(new TextRange(from, end).Text).ToString();
    }

    internal void PaintSelection(int start, int end)
    {
        setting = true;
        try
        {
            if (Content is TextBox plainEditor) plainEditor.Select(start, end - start);
            if (Content is RichTextBox richEditor)
                richEditor.Selection.Select(PositionAt(richEditor, start), PositionAt(richEditor, end));
        }
        finally { setting = false; }
    }

    internal IEnumerable<Rect> SelectionRectangles(int start, int end, Rect clip)
    {
        if (Content is TextBox plainEditor)
        {
            var origin = plainEditor.TranslatePoint(new Point(), this);
            for (var index = start; index < end; index++)
            {
                var left = plainEditor.GetRectFromCharacterIndex(index);
                var right = plainEditor.GetRectFromCharacterIndex(index, trailingEdge: true);
                var rect = CharacterBounds(left, right);
                if (!rect.IsEmpty)
                {
                    rect.Offset(origin.X, origin.Y);
                    if (clip.IntersectsWith(rect)) yield return rect;
                }
            }
        }
        else if (Content is RichTextBox richEditor)
        {
            var origin = richEditor.TranslatePoint(new Point(), this);
            var last = PositionAt(richEditor, end);
            var current = PositionAt(richEditor, start);
            while (current is not null && current.CompareTo(last) < 0)
            {
                if (current.GetPointerContext(LogicalDirection.Forward) != TextPointerContext.Text)
                {
                    current = current.GetNextContextPosition(LogicalDirection.Forward);
                    continue;
                }
                var next = current.GetPositionAtOffset(1, LogicalDirection.Backward);
                if (next is null) break;
                var rect = CharacterBounds(current.GetCharacterRect(LogicalDirection.Forward),
                    next.GetCharacterRect(LogicalDirection.Backward));
                if (!rect.IsEmpty)
                {
                    rect.Offset(origin.X, origin.Y);
                    if (clip.IntersectsWith(rect)) yield return rect;
                }
                current = next;
            }
        }
    }

    internal static Rect CharacterBounds(Rect left, Rect right)
    {
        if (left.IsEmpty || right.IsEmpty || Math.Abs(left.Bottom - right.Bottom) > Math.Max(left.Height, right.Height) / 2)
            return Rect.Empty;
        var x = Math.Min(left.X, right.X);
        var y = Math.Min(left.Y, right.Y);
        return new Rect(x, y, Math.Abs(right.X - left.X), Math.Max(left.Bottom, right.Bottom) - y);
    }

    private static TextPointer PositionAt(RichTextBox editor, int offset) =>
        editor.Document.ContentStart.GetPositionAtOffset(offset) ?? editor.Document.ContentEnd;

    public void Show(string value, string format, JsonElement markdown = default)
    {
        if (value == text && format == mode && TranscriptJson.Same(markdown, renderedBlocks))
            return;
        if (value.Length == 0)
        {
            text = value;
            mode = format;
            Visibility = Visibility.Collapsed;
            return;
        }
        renderedBlocks = markdown;
        var (start, length) = state.Selection.GetValueOrDefault(key);
        setting = true;
        var previous = Content;
        try
        {
            if (format is "markdown" or "diff" or "code" or "command")
            {
                var editor = rich ??= CreateRich();
                Content = editor;
                RenderRich(editor.Document.Blocks, format switch
                {
                    "markdown" => markdown.ValueKind == JsonValueKind.Array
                        ? markdown.Elements().Select(block => new Piece(block.Value("kind").Text(), block.Value("text").Text(),
                            block.Value("language").OptionalText(), block)).ToList()
                        : [new Piece("paragraph", value)],
                    "diff" => [new Piece("diff", value, Data: markdown)],
                    _ => [new Piece("code", value)],
                });
                var begin = editor.Document.ContentStart;
                var total = begin.GetOffsetToPosition(editor.Document.ContentEnd);
                var from = begin.GetPositionAtOffset(Math.Clamp(start, 0, total)) ?? begin;
                var to = begin.GetPositionAtOffset(Math.Clamp(start + length, 0, total)) ?? from;
                editor.Selection.Select(from, to);
            }
            else
            {
                var editor = plain ??= CreatePlain();
                Content = editor;
                editor.FontFamily = FontFamily;
                if (value.StartsWith(editor.Text, StringComparison.Ordinal) && editor.Text.Length > 0)
                    editor.AppendText(value[editor.Text.Length..]);
                else
                    editor.Text = value;
                var clamped = Math.Min(start, value.Length);
                editor.Select(clamped, Math.Min(length, value.Length - clamped));
            }
            if (previous is UIElement { IsKeyboardFocusWithin: true } && !ReferenceEquals(previous, Content))
                ((UIElement)Content).Focus();
            text = value;
            mode = format;
            Visibility = Visibility.Visible;
        }
        finally
        {
            setting = false;
        }
    }

    private void InitializeBox(System.Windows.Controls.Primitives.TextBoxBase box)
    {
        BindingOperations.SetBinding(box, ForegroundProperty, new Binding(nameof(Foreground)) { Source = this });
        BindingOperations.SetBinding(box, FontSizeProperty, new Binding(nameof(FontSize)) { Source = this });
        box.IsReadOnly = true;
        box.IsInactiveSelectionHighlightEnabled = true;
        box.IsReadOnlyCaretVisible = false;
        box.Template = (ControlTemplate)FindResource("YaacTranscriptText");
        box.FocusVisualStyle = null;
        box.BorderThickness = new Thickness(0);
        box.Background = Brushes.Transparent;
        box.Padding = new Thickness(0);
        box.Margin = new Thickness(0);
        box.MinHeight = 0;
        box.VerticalScrollBarVisibility = ScrollBarVisibility.Disabled;
        box.HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled;
        var menu = new ContextMenu();
        var selection = new MenuItem { Header = "Copy", Command = ApplicationCommands.Copy, CommandTarget = box };
        var message = new MenuItem { Header = "Copy message" };
        var code = new MenuItem { Header = "Copy code" };
        string? codeText = null;
        message.Click += (_, _) => Copy(copyMessage());
        code.Click += (_, _) => Copy(codeText);
        menu.Opened += (_, _) =>
        {
            message.Visibility = copyMessage() is null ? Visibility.Collapsed : Visibility.Visible;
            codeText = ReferenceEquals(box, plain) && mode is ("code" or "command") ? text : null;
            if (box is RichTextBox editor)
            {
                var pointer = editor.GetPositionFromPoint(Mouse.GetPosition(editor), snapToText: true);
                codeText = pointer?.Paragraph?.Tag as string;
            }
            code.Header = mode == "diff" ? "Copy diff" : "Copy code";
            code.Visibility = codeText is null ? Visibility.Collapsed : Visibility.Visible;
        };
        menu.Items.Add(selection);
        menu.Items.Add(message);
        menu.Items.Add(code);
        box.ContextMenu = menu;
    }

    internal static void Copy(string? value) => TryCopy(value);

    private static bool TryCopy(string? value)
    {
        if (string.IsNullOrEmpty(value)) return false;
        try { Clipboard.SetText(value); return true; }
        catch (System.Runtime.InteropServices.ExternalException) { return false; }
    }

    private void RenderRich(BlockCollection blocks, List<Piece> next)
    {
        var common = 0;
        while (common < pieces.Count && common < next.Count && pieces[common].SameAs(next[common]))
            common++;
        while (pieces.Count > common)
        {
            blocks.Remove(blocks.LastBlock!);
            pieces.RemoveAt(pieces.Count - 1);
        }
        for (var i = common; i < next.Count; i++)
        {
            var piece = next[i];
            blocks.Add(BlockFor(piece));
            pieces.Add(piece);
        }
    }

    private Block BlockFor(Piece piece)
    {
        if (piece.Kind is "code" or "diff") return CodeBlock(piece);
        if (piece.Kind == "table") return TableFor(piece.Data);

        var paragraph = new Paragraph { Margin = new Thickness(0, 0, 0, 6), Foreground = Foreground, FontSize = FontSize };
        if (piece.Kind == "heading")
        {
            paragraph.FontWeight = FontWeights.Bold;
            paragraph.FontSize = FontSize + (piece.Data.Value("level").GetInt32() switch { 1 => 5, 2 => 3, _ => 1 });
        }
        else if (piece.Kind == "quote")
        {
            paragraph.Foreground = TranscriptColors.Muted;
            paragraph.Padding = new Thickness(12, 0, 0, 0);
        }
        else if (piece.Kind == "item")
        {
            paragraph.Margin = new Thickness(6 + 18 * piece.Data.Value("depth").GetInt32(), 0, 0, 6);
            paragraph.Inlines.Add(new Run(piece.Data.Value("marker").Text() + "  ") { Foreground = TranscriptColors.Muted });
        }
        else if (piece.Kind == "rule")
        {
            paragraph.BorderBrush = TranscriptColors.Rule;
            paragraph.BorderThickness = new Thickness(0, 1, 0, 0);
            paragraph.LineHeight = 4;
        }
        if (piece.Kind != "rule")
        {
            paragraph.LineStackingStrategy = LineStackingStrategy.BlockLineHeight;
            paragraph.LineHeight = paragraph.FontSize * 1.45;
        }
        AddSpans(paragraph, piece.Data.Value("spans"));
        if (piece.Text.Length > 0)
            paragraph.Inlines.Add(new Run(piece.Text) { Foreground = paragraph.Foreground });
        return paragraph;
    }

    private Table TableFor(JsonElement data)
    {
        var table = new Table { CellSpacing = 0, Margin = new Thickness(0, 3, 0, 6) };
        var group = new TableRowGroup();
        table.RowGroups.Add(group);
        var rows = data.Value("rows").Elements()
            .Prepend(data.Value("head")).Where(row => row.ValueKind == JsonValueKind.Array && row.GetArrayLength() > 0)
            .ToArray();
        if (rows.Length == 0) return table;
        var columns = rows.Max(row => row.GetArrayLength());
        var columnLimit = Math.Max(28, Math.Min(220, 360.0 / Math.Max(1, columns - 1)));
        for (var i = 0; i < columns - 1; i++)
        {
            var column = i;
            var longest = rows.Where(row => column < row.GetArrayLength())
                .Max(row => row[column].Elements().Sum(span => span.Value("text").Text().Length));
            var width = Math.Clamp(longest * FontSize * 0.58 + 18, 28, columnLimit);
            table.Columns.Add(new TableColumn { Width = new GridLength(width) });
        }
        table.Columns.Add(new TableColumn { Width = GridLength.Auto });
        for (var i = 0; i < rows.Length; i++)
        {
            var row = new TableRow();
            group.Rows.Add(row);
            for (var column = 0; column < columns; column++)
            {
                var paragraph = new Paragraph { Margin = new Thickness(0), Foreground = Foreground, LineHeight = FontSize * 1.45 };
                if (column < rows[i].GetArrayLength())
                    AddSpans(paragraph, rows[i][column]);
                var cell = new TableCell(paragraph) { Padding = new Thickness(0, 3, 9, 3) };
                if (i == 0)
                {
                    cell.FontWeight = FontWeights.Bold;
                    cell.BorderBrush = TranscriptColors.Rule;
                    cell.BorderThickness = new Thickness(0, 0, 0, 1);
                }
                row.Cells.Add(cell);
            }
        }
        return table;
    }

    private static Section CodeBlock(Piece piece)
    {
        var diff = piece.Kind == "diff";
        var copyText = piece.Data.Value("copyText").OptionalText() ?? piece.Text;
        var copyLabel = diff ? "Copy diff" : piece.Language is { Length: > 0 } language ? $"Copy {language} code" : "Copy code";
        var section = new Section
        {
            Background = TranscriptColors.Code,
            Margin = new Thickness(0, 0, 0, 6),
            Padding = new Thickness(10, 6, 10, 7),
        };
        var button = new Button
        {
            Content = "Copy",
            Style = (Style)Application.Current.FindResource("YaacQuietButton"),
            ToolTip = copyLabel,
            FontFamily = YaacFonts.Ui,
            FontSize = 11,
        };
        AutomationProperties.SetName(button, copyLabel);
        var reset = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1.5) };
        reset.Tick += (_, _) =>
        {
            reset.Stop();
            button.Content = "Copy";
        };
        button.Unloaded += (_, _) => reset.Stop();
        button.Click += (_, _) =>
        {
            if (!TryCopy(copyText)) return;
            button.Content = "Copied";
            reset.Stop();
            reset.Start();
        };

        var paragraph = new Paragraph
        {
            Margin = new Thickness(0),
            FontFamily = Mono,
            FontSize = 13,
            LineHeight = 20,
            LineStackingStrategy = LineStackingStrategy.BlockLineHeight,
            Tag = copyText,
        };
        var copy = new Floater { HorizontalAlignment = HorizontalAlignment.Right, Width = 58, Padding = new Thickness(0) };
        copy.Blocks.Add(new BlockUIContainer(button) { Margin = new Thickness(0) });
        paragraph.Inlines.Add(copy);
        if (diff)
        {
            var width = piece.Data.Value("prefixWidth");
            var prefix = width.ValueKind == JsonValueKind.Number && width.TryGetInt32(out var count) ? count : 0;
            var first = true;
            foreach (var line in piece.Text.Split('\n'))
            {
                if (!first) paragraph.Inlines.Add(new LineBreak());
                first = false;
                var split = Math.Clamp(prefix, 0, line.Length);
                if (split > 0)
                    paragraph.Inlines.Add(new Run(line[..split]) { Foreground = TranscriptColors.Muted });
                var body = line[split..];
                var color = body.StartsWith('+') ? TranscriptColors.Success
                    : body.StartsWith('-') ? TranscriptColors.Error
                    : split > 0 && body.StartsWith(' ') ? TranscriptColors.Text : TranscriptColors.Muted;
                paragraph.Inlines.Add(new Run(body) { Foreground = color });
            }
        }
        else
        {
            TranscriptSyntax.Append(paragraph, piece.Text,
                piece.Data.Value("kind").Text() == "code" ? CodePadding : null);
        }

        section.Blocks.Add(paragraph);
        return section;
    }

    private static void AddSpans(Paragraph paragraph, JsonElement spans)
    {
        foreach (var span in spans.Elements())
        {
            var lines = span.Value("text").Text().Split('\n');
            for (var i = 0; i < lines.Length; i++)
            {
                if (i > 0) paragraph.Inlines.Add(new LineBreak());
                if (lines[i].Length == 0) continue;
                var run = new Run(lines[i]) { Foreground = paragraph.Foreground };
                if (span.Value("bold").Flag()) run.FontWeight = FontWeights.Bold;
                if (span.Value("italic").Flag()) run.FontStyle = FontStyles.Italic;
                Inline inline = span.Value("code").Flag() ? InlineCode(run) : run;
                var target = span.Value("link").OptionalText();
                paragraph.Inlines.Add(target is null ? inline : Link(inline, target));
            }
        }
    }

    private static Span InlineCode(Run text)
    {
        var background = TranscriptColors.Code;
        text.FontFamily = Mono;
        text.Foreground = TranscriptColors.Text;
        text.Background = background;

        // Narrow no-break spaces give the background room at either edge while keeping
        // the code itself in the document as selectable, wrappable text.
        Run Padding() => new("\u202f") { FontFamily = YaacFonts.Ui, Background = background, Tag = CodePadding };
        var code = new Span();
        code.Inlines.Add(Padding());
        code.Inlines.Add(text);
        code.Inlines.Add(Padding());
        return code;
    }

    private static Inline Link(Inline label, string target)
    {
        if (!Uri.TryCreate(target, UriKind.Absolute, out var uri) || uri.Scheme is not ("https" or "http"))
            { label.ToolTip = target; return label; }
        label.Foreground = TranscriptColors.Link;
        var link = new Hyperlink(label) { Foreground = TranscriptColors.Link, ToolTip = target };
        link.Click += (_, _) =>
        {
            try
            {
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(uri.AbsoluteUri) { UseShellExecute = true });
            }
            catch (System.ComponentModel.Win32Exception) { }
        };
        return link;
    }

}
