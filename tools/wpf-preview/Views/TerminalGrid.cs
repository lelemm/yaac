using System.Text;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

// The controller owns all terminal state. Each visual retains exactly one painted row.
internal sealed partial class TerminalGrid : FrameworkElement
{
    private const double Em = 13;
    private static readonly GlyphTypeface BundledFace = new(new Uri("pack://application:,,,/Yaac.WpfPreview;component/Assets/Fonts/CaskaydiaCoveNerdFontMono-Regular.ttf"));
    private GlyphTypeface face = BundledFace;
    private static readonly DependencyProperty CodeFontProperty = DependencyProperty.Register(
        "TerminalFont", typeof(FontFamily), typeof(TerminalGrid), new PropertyMetadata(null, (owner, args) =>
            ((TerminalGrid)owner).ApplyFont(args.NewValue as FontFamily)));
    private readonly Dictionary<int, SolidColorBrush> colors = new();
    private readonly VisualCollection lines;
    private readonly DrawingVisual cursor = new();
    private TerminalFrame? pending;
    private bool scheduled;
    private readonly DispatcherTimer blink = new() { Interval = TimeSpan.FromMilliseconds(550) };
    private long blinkUntil;
    private bool cursorOn = true;
    private int cx, cy;
    private bool wide, blinking;
    private string shape = "hidden";
    private int cursorColor;
    private TerminalRun? cursorGlyph;
    private string preedit = "";
    internal event Action<string, object?>? Command;
    internal event Action<ulong>? Painted;
    private double Cell { get; set; }
    private double Line { get; set; }
    private readonly ushort[] ascii = new ushort[128];
    private readonly Dictionary<(int Count, double Width), double[]> advanceCache = new();

    internal TerminalGrid()
    {
        MeasureFont();
        Focusable = true;
        ClipToBounds = true;
        lines = new(this) { cursor };
        SizeChanged += (_, _) => ResizeGrid();
        IsVisibleChanged += (_, _) =>
        {
            if (IsVisible) { Command?.Invoke("refresh", null); ResizeGrid(); }
            else Reset();
        };
        GotKeyboardFocus += (_, _) =>
        {
            Command?.Invoke("focus", new { focused = true });
            if (blinking && Environment.TickCount64 < blinkUntil) blink.Start();
            DrawCursor();
        };
        LostKeyboardFocus += (_, _) => { Command?.Invoke("focus", new { focused = false }); blink.Stop(); cursorOn = true; preedit = ""; DrawCursor(); };
        blink.Tick += (_, _) =>
        {
            cursorOn = !cursorOn;
            if (Environment.TickCount64 >= blinkUntil) { blink.Stop(); cursorOn = true; }
            DrawCursor();
        };
        Palette.Brush("YaacCanvas").Changed += (_, _) => { if (IsVisible) ResizeGrid(); };
        InitializeInput();
        SetResourceReference(CodeFontProperty, YaacFonts.TerminalResource);
    }

    private void ApplyFont(FontFamily? family)
    {
        var chosen = BundledFace;
        if (family is not null && new Typeface(family, FontStyles.Normal, FontWeights.Normal, FontStretches.Normal).TryGetGlyphTypeface(out var candidate)
            && Enumerable.Range(32, 95).All(candidate.CharacterToGlyphMap.ContainsKey))
            chosen = candidate;
        if (ReferenceEquals(face, chosen)) return;
        face = chosen;
        MeasureFont();
        advanceCache.Clear();
        fallback.Clear();
        Reset();
        if (IsVisible)
        {
            ResizeGrid();
            Command?.Invoke("refresh", null);
        }
    }

    private void MeasureFont()
    {
        double widest = 0;
        for (int i = 32; i < 127; i++)
        {
            ascii[i] = face.CharacterToGlyphMap[i];
            widest = Math.Max(widest, face.AdvanceWidths[ascii[i]]);
        }
        Cell = Em * widest;
        Line = Math.Ceiling(Em * face.Height);
    }

    internal void ResizeGrid()
    {
        if (ActualWidth < 2 * Cell || ActualHeight < Line) return;
        var dpi = VisualTreeHelper.GetDpi(this);
        Command?.Invoke("resize", new {
        cellWidth = Math.Max(1, (int)Math.Round(Cell * dpi.DpiScaleX)),
        cellHeight = Math.Max(1, (int)Math.Round(Line * dpi.DpiScaleY)),
        dark = ((SolidColorBrush)Palette.Brush("YaacCanvas")).Color.R < 128,
        cols = Math.Clamp((int)(ActualWidth / Cell), 2, 1000),
        rows = Math.Clamp((int)(ActualHeight / Line), 1, 500) });
    }

    internal void Reset()
    {
        if (scheduled) CompositionTarget.Rendering -= Paint;
        scheduled = false;
        if (pending is { } dropped) Painted?.Invoke(dropped.Serial);
        pending = default;
        if (lines.Count > 1) lines.RemoveRange(0, lines.Count - 1);
        shape = "hidden";
        preedit = "";
        blinkUntil = 0;
        blink.Stop();
        DrawCursor();
    }

    internal void Queue(TerminalFrame frame)
    {
        pending = frame;
        if (scheduled) return;
        scheduled = true;
        CompositionTarget.Rendering += Paint;
    }

    private void Paint(object? sender, EventArgs args)
    {
        CompositionTarget.Rendering -= Paint;
        scheduled = false;
        var frame = pending!;
        pending = default;
        int rows = frame.Height;
        while (lines.Count - 1 < rows) lines.Insert(lines.Count - 1, new DrawingVisual());
        while (lines.Count - 1 > rows) lines.RemoveAt(lines.Count - 2);
        foreach (var row in frame.Rows)
        {
            int index = row.Index;
            if ((uint)index >= rows) continue;
            using var dc = ((DrawingVisual)lines[index]).RenderOpen();
            foreach (var run in row.Runs) DrawRun(dc, index * Line, run);
        }
        cursorGlyph = frame.CursorGlyph;
        cx = frame.CursorColumn;
        cy = frame.CursorRow;
        shape = frame.Shape;
        cursorColor = frame.CursorColor;
        wide = frame.Wide;
        if (blinking != frame.Blinking)
        {
            blinking = frame.Blinking;
            if (blinking && IsKeyboardFocused && Environment.TickCount64 < blinkUntil) blink.Start();
            else { blink.Stop(); cursorOn = true; }
        }
        DrawCursor();
        Painted?.Invoke(frame.Serial);
    }

    private Brush Ink(int rgb)
    {
        if (colors.TryGetValue(rgb, out var value)) return value;
        value = new(Color.FromRgb((byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));
        value.Freeze();
        if (colors.Count >= 4096) colors.Clear();
        colors.Add(rgb, value);
        return value;
    }

    private readonly List<GlyphTypeface> fallbackFaces = new();
    private bool loadedFallbackFaces;
    private readonly Dictionary<int, GlyphTypeface> fallback = new();
    private GlyphTypeface Font(int cp)
    {
        if (cp is >= 32 and <= 126 || face.CharacterToGlyphMap.ContainsKey(cp)) return face;
        if (BundledFace.CharacterToGlyphMap.ContainsKey(cp)) return BundledFace;
        if (fallback.TryGetValue(cp, out var cached)) return cached;
        if (!loadedFallbackFaces)
        {
            foreach (var name in new[] { "Segoe UI Emoji", "Microsoft YaHei UI", "Yu Gothic UI", "Segoe UI Symbol" })
                if (new Typeface(name).TryGetGlyphTypeface(out var font)) fallbackFaces.Add(font);
            loadedFallbackFaces = true;
        }
        if (fallback.Count >= 4096) fallback.Clear();
        foreach (var font in fallbackFaces)
            if (font.CharacterToGlyphMap.ContainsKey(cp)) return fallback[cp] = font;
        return fallback[cp] = face;
    }

    private void DrawRun(DrawingContext dc, double top, TerminalRun run)
    {
        var x = run.Column * Cell;
        var width = run.Width * Cell;
        var fg = Ink(run.Ink);
        var bg = run.Paper;
        int flags = run.Flags;
        var text = run.Text;
        bool cluster = (flags & 16) != 0;
        double length = width * (cluster ? 1 : text.EnumerateRunes().Count());
        if (bg >= 0) dc.DrawRectangle(Ink(bg), null, new Rect(x, top, length, Line));
        if ((flags & 4) != 0) dc.DrawRectangle(fg, null, new Rect(x, top + Line - 2, length, 1));
        if ((flags & 32) != 0) dc.DrawRectangle(fg, null, new Rect(x, top + Line - 4, length, 1));
        if ((flags & 8) != 0) dc.DrawRectangle(fg, null, new Rect(x, top + Line * .53, length, 1));
        if (string.IsNullOrWhiteSpace(text)) return;
        if (!cluster && System.Text.Ascii.IsValid(text))
        {
            var indices = new ushort[text.Length];
            for (int i = 0; i < text.Length; i++) indices[i] = ascii[text[i]];
            if (!advanceCache.TryGetValue((indices.Length, width), out var steps))
            {
                steps = new double[indices.Length];
                Array.Fill(steps, width);
                if (advanceCache.Count >= 128) advanceCache.Clear();
                advanceCache[(indices.Length, width)] = steps;
            }
            DrawGlyphs(face, x, indices, steps);
            return;
        }
        var glyphs = new List<ushort>(text.Length);
        var advances = new List<double>(text.Length);
        GlyphTypeface current = face;
        var start = x;
        foreach (var rune in text.EnumerateRunes())
        {
            var cp = rune.Value;
            if (cp is >= 0x2500 and <= 0x259f && DrawShape(dc, cp, x, top, width, Line, fg))
            {
                Flush();
                x += width;
                start = x;
                continue;
            }
            var font = Font(cp);
            if (font != current) { Flush(); current = font; start = x; }
            glyphs.Add(font.CharacterToGlyphMap.TryGetValue(cp, out var index) ? index : (ushort)0);
            advances.Add(cluster ? 0 : width);
            if (!cluster) x += width;
        }
        Flush();

        void Flush()
        {
            if (glyphs.Count == 0) return;
            DrawGlyphs(current, start, glyphs.ToArray(), advances.ToArray());
            glyphs.Clear();
            advances.Clear();
        }
        void DrawGlyphs(GlyphTypeface font, double left, ushort[] indices, double[] steps)
        {
            var baseline = top + face.Baseline * Em;
            var run = new GlyphRun(font, 0, false, Em, (float)VisualTreeHelper.GetDpi(this).PixelsPerDip,
                indices, new Point(left, baseline), steps, null, null, null, null, null, null);
            if ((flags & 2) != 0) dc.PushTransform(new SkewTransform(-12, 0, left, baseline));
            dc.DrawGlyphRun(fg, run);
            if ((flags & 1) != 0)
            {
                dc.PushTransform(new TranslateTransform(1 / VisualTreeHelper.GetDpi(this).DpiScaleX, 0));
                dc.DrawGlyphRun(fg, run);
                dc.Pop();
            }
            if ((flags & 2) != 0) dc.Pop();
        }
    }

    private void RestartBlink()
    {
        blinkUntil = Environment.TickCount64 + 30_000;
        cursorOn = true;
        blink.Stop();
        if (IsKeyboardFocused && blinking) blink.Start();
        DrawCursor();
    }

    private void SetPreedit(string value)
    {
        if (preedit == value) return;
        preedit = value;
        DrawCursor();
    }

    private void DrawCursor()
    {
        using var dc = cursor.RenderOpen();
        var bounds = new Rect(cx * Cell, cy * Line, wide ? 2 * Cell : Cell, Line);
        if (preedit.Length > 0 && IsKeyboardFocused)
        {
            var text = new FormattedText(preedit, System.Globalization.CultureInfo.CurrentCulture,
                FlowDirection.LeftToRight, new Typeface(YaacFonts.Mono, FontStyles.Normal, FontWeights.Normal, FontStretches.Normal),
                Em, Palette.Brush("YaacText"), VisualTreeHelper.GetDpi(this).PixelsPerDip);
            text.SetTextDecorations(TextDecorations.Underline);
            dc.DrawRectangle(Palette.Brush("YaacCanvas"), null, new Rect(bounds.TopLeft,
                new Size(Math.Max(Cell, text.WidthIncludingTrailingWhitespace), Line)));
            dc.DrawText(text, bounds.TopLeft);
            return;
        }
        if (!cursorOn || shape == "hidden") return;
        var ink = Ink(cursorColor);
        if (!IsKeyboardFocused || shape == "hollow") dc.DrawRectangle(null, new Pen(ink, 1), bounds);
        else if (shape == "bar") dc.DrawRectangle(ink, null, new Rect(bounds.TopLeft, new Size(2, Line)));
        else if (shape == "underline") dc.DrawRectangle(ink, null, new Rect(bounds.X, bounds.Bottom - 2, bounds.Width, 2));
        else
        {
            dc.DrawRectangle(ink, null, bounds);
            if (cursorGlyph is { } glyph) DrawRun(dc, bounds.Y, glyph);
        }
    }

    protected override int VisualChildrenCount => lines.Count;
    protected override Visual GetVisualChild(int index) => lines[index];
    protected override void OnRender(DrawingContext dc) =>
        dc.DrawRectangle(Brushes.Transparent, null, new Rect(RenderSize));
}
