using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

// Paint the styled ranges inside the TextBox's own wrap and scroll geometry. The native
// editor still owns input, clipboard selection, accessibility, caret, and undo.
public sealed class ComposerMarkdown : FrameworkElement
{
    private readonly record struct Span(int Start, int End, string Style);
    private TextBox? editor;
    private string? parsedText;
    private string? rawText;
    private Span[] parsedSpans = [];
    private int parsedBudget;
    private Span[] visibleTextSpans = [];
    private bool composing;

    public static readonly DependencyProperty InputBrushProperty = RegisterBrush(nameof(InputBrush));
    public static readonly DependencyProperty TextBrushProperty = RegisterBrush(nameof(TextBrush));
    public static readonly DependencyProperty MarkerBrushProperty = RegisterBrush(nameof(MarkerBrush));
    public static readonly DependencyProperty CodeBrushProperty = RegisterBrush(nameof(CodeBrush));

    public Brush InputBrush { get => (Brush)GetValue(InputBrushProperty); set => SetValue(InputBrushProperty, value); }
    public Brush TextBrush { get => (Brush)GetValue(TextBrushProperty); set => SetValue(TextBrushProperty, value); }
    public Brush MarkerBrush { get => (Brush)GetValue(MarkerBrushProperty); set => SetValue(MarkerBrushProperty, value); }
    public Brush CodeBrush { get => (Brush)GetValue(CodeBrushProperty); set => SetValue(CodeBrushProperty, value); }

    private static DependencyProperty RegisterBrush(string name) =>
        DependencyProperty.Register(name, typeof(Brush), typeof(ComposerMarkdown),
            new FrameworkPropertyMetadata(Brushes.Transparent, FrameworkPropertyMetadataOptions.AffectsRender));

    internal void Attach(TextBox value)
    {
        editor = value;
        editor.TextChanged += (_, _) => InvalidateVisual();
        editor.SelectionChanged += (_, _) => InvalidateVisual();
        editor.SizeChanged += (_, _) => InvalidateVisual();
        editor.AddHandler(ScrollViewer.ScrollChangedEvent, new ScrollChangedEventHandler((_, _) => InvalidateVisual()));
    }

    internal void SetSpans(string text, JsonElement spans, int budget)
    {
        if (parsedText == text) return;
        parsedText = text;
        parsedBudget = budget;
        parsedSpans = spans.ValueKind == JsonValueKind.Array
            ? spans.EnumerateArray().Select(span => new Span(span.GetProperty("start").GetInt32(),
                span.GetProperty("end").GetInt32(), span.GetProperty("style").String())).ToArray()
            : [];
        rawText = null;
        InvalidateVisual();
    }

    // The spans arrive with the text they were computed for, which lags the editor while the
    // user types (each keystroke reaches the controller and comes back). Spans are never
    // applied to another text by position: only lines provably styled the same in `current`
    // keep theirs (P317). The styler is line by line and carries only "inside a fence" down
    // the text, so:
    // - every whole line before the first changed one keeps its spans exactly;
    // - whole unchanged lines after the change keep theirs, shifted, when neither version of
    //   the changed lines holds a fence marker (so the fence state below is the same);
    // - the changed lines themselves stay code when they were wholly fenced code before;
    // - anything else is drawn plain until the spans for this exact text arrive.
    // A keystroke at the end of the text therefore never restyles the text above it.
    internal static (int Start, int End, string Style)[] Carry(
        string parsed, IReadOnlyList<(int Start, int End, string Style)> spans, string current, int budget)
    {
        if (parsed == current) return spans.ToArray();
        var most = Math.Min(parsed.Length, current.Length);
        var prefix = 0;
        while (prefix < most && parsed[prefix] == current[prefix]) prefix++;
        var suffix = 0;
        while (suffix < most - prefix && parsed[^(suffix + 1)] == current[^(suffix + 1)]) suffix++;

        // [changed, oldEnd) in `parsed` became [changed, newEnd) in `current`; both are whole lines.
        var changed = prefix == 0 ? 0 : parsed.LastIndexOf('\n', prefix - 1) + 1;
        var oldTail = parsed.Length - suffix;
        var newTail = current.Length - suffix;
        int oldEnd;
        if ((oldTail == 0 || parsed[oldTail - 1] == '\n') && (newTail == 0 || current[newTail - 1] == '\n'))
            oldEnd = oldTail;
        else
        {
            var lineBreak = parsed.IndexOf('\n', oldTail);
            oldEnd = lineBreak < 0 ? parsed.Length : lineBreak + 1;
        }
        var delta = current.Length - parsed.Length;
        var newEnd = oldEnd + delta;

        var output = new List<(int Start, int End, string Style)>();
        foreach (var span in spans)
            if (span.Start < changed)
                output.Add((span.Start, Math.Min(span.End, changed), span.Style));

        var bounded = Encoding.UTF8.GetByteCount(parsed) <= budget && Encoding.UTF8.GetByteCount(current) <= budget;
        if (!bounded || HasFence(parsed, changed, oldEnd) || HasFence(current, changed, newEnd))
            return output.ToArray();

        if (oldEnd > changed && newEnd > changed &&
            spans.Any(span => span.Style == "block" && span.Start <= changed && span.End >= oldEnd))
            output.Add((changed, newEnd, "block"));
        foreach (var span in spans)
            if (span.End > oldEnd)
                output.Add((Math.Max(span.Start, oldEnd) + delta, span.End + delta, span.Style));
        return output.ToArray();

        // A superset of the styler's fence rule (a line starting with ``` or ~~~).
        static bool HasFence(string text, int start, int end) =>
            text.IndexOf("```", start, end - start, StringComparison.Ordinal) >= 0 ||
            text.IndexOf("~~~", start, end - start, StringComparison.Ordinal) >= 0;
    }

    internal void SetComposing(bool value)
    {
        if (composing == value) return;
        composing = value;
        InvalidateVisual();
    }

    private void UpdateOffsets(string text)
    {
        if (rawText == text) return;
        rawText = text;
        visibleTextSpans = [];
        if (parsedText is null) return;
        var current = Carry(parsedText, parsedSpans.Select(span => (span.Start, span.End, span.Style)).ToArray(),
            text.Replace("\r\n", "\n"), parsedBudget);

        // The controller normalizes line endings. Keep the TextBox's own offsets for its
        // layout APIs, including any CRs that were removed from the controller's text.
        var normalized = 0;
        var raw = 0;
        int Advance(int target)
        {
            while (normalized < target)
            {
                if (text[raw] == '\r' && raw + 1 < text.Length && text[raw + 1] == '\n') raw++;
                raw++;
                normalized++;
            }
            return raw;
        }
        visibleTextSpans = current.Select(span => new Span(Advance(span.Start), Advance(span.End), span.Style)).ToArray();
    }

    protected override void OnRender(DrawingContext drawing)
    {
        base.OnRender(drawing);
        if (editor is not { IsMeasureValid: true, IsArrangeValid: true } box || composing ||
            parsedSpans.Length == 0 || box.Text.Length == 0 ||
            TranscriptView.FindVisual<ScrollContentPresenter>(box) is not { } viewport)
            return;

        UpdateOffsets(box.Text);
        if (visibleTextSpans.Length == 0) return;
        var firstLine = box.GetFirstVisibleLineIndex();
        var lineCount = box.LineCount;
        var lastLine = Math.Min(lineCount - 1, box.GetLastVisibleLineIndex());
        if (firstLine < 0 || lastLine < firstLine) return;
        var first = box.GetCharacterIndexFromLineIndex(firstLine);
        var last = LineEnd(lastLine);
        var selectionStart = box.SelectionStart;
        var selectionEnd = selectionStart + box.SelectionLength;

        var clip = new Rect(viewport.TranslatePoint(new Point(), this), viewport.RenderSize);
        drawing.PushClip(new RectangleGeometry(clip));
        foreach (var span in visibleTextSpans)
        {
            if (span.End <= first) continue;
            if (span.Start >= last) break;
            var start = Math.Max(span.Start, first);
            var end = Math.Min(span.End, last);
            while (start < end)
            {
                var line = box.GetLineIndexFromCharacterIndex(start);
                if (line < 0) break;
                var next = Math.Min(end, LineEnd(line));
                if (next <= start) break;
                var paintEnd = next;
                while (paintEnd > start && box.Text[paintEnd - 1] is '\n' or '\r') paintEnd--;
                if (selectionStart == selectionEnd || paintEnd <= selectionStart || start >= selectionEnd)
                    Paint(drawing, box, start, paintEnd, span.Style);
                else
                {
                    Paint(drawing, box, start, selectionStart, span.Style);
                    Paint(drawing, box, selectionEnd, paintEnd, span.Style);
                }
                start = next;
            }
        }
        drawing.Pop();

        // The next line's start also steps over the line break on a fenced-code span.
        int LineEnd(int line) => line + 1 < lineCount ? box.GetCharacterIndexFromLineIndex(line + 1) : box.Text.Length;
    }

    private void Paint(DrawingContext drawing, TextBox box, int start, int end, string style)
    {
        if (end <= start) return;
        var leading = box.GetRectFromCharacterIndex(start);
        var trailing = box.GetRectFromCharacterIndex(end - 1, trailingEdge: true);
        if (leading.IsEmpty || trailing.IsEmpty || Math.Abs(leading.Top - trailing.Top) > .5) return;
        var width = Math.Abs(trailing.X - leading.X);
        if (width < .5) return;

        var code = style is "code" or "block";
        var heading = style == "heading";
        var typeface = new Typeface(code ? YaacFonts.Mono : box.FontFamily,
            style == "italic" ? FontStyles.Italic : box.FontStyle,
            heading || style == "bold" ? FontWeights.Bold : box.FontWeight, box.FontStretch);
        var origin = box.TranslatePoint(new Point(Math.Min(leading.X, trailing.X), leading.Top), this);
        var bounds = new Rect(origin, new Size(width, leading.Height));
        var text = new FormattedText(box.Text[start..end], CultureInfo.CurrentCulture,
            leading.X <= trailing.X ? FlowDirection.LeftToRight : FlowDirection.RightToLeft,
            typeface, box.FontSize + (heading ? 1 : 0), style == "marker" ? MarkerBrush : TextBrush,
            VisualTreeHelper.GetDpi(this).PixelsPerDip);
        var renderedWidth = text.WidthIncludingTrailingWhitespace;
        if (!(renderedWidth > 0)) return;
        drawing.PushClip(new RectangleGeometry(bounds));
        drawing.DrawRectangle(code ? CodeBrush : InputBrush, null, bounds);
        drawing.PushTransform(new TranslateTransform(origin.X, origin.Y + (bounds.Height - text.Height) / 2));
        drawing.PushTransform(new ScaleTransform(width / renderedWidth, 1));
        drawing.DrawText(text, new Point());
        drawing.Pop();
        drawing.Pop();
        drawing.Pop();
    }
}
