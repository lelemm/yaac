using System.Windows;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

internal sealed partial class TerminalGrid
{
    // Two bits each for up, down, left, right: absent, light, heavy, double.
    private static readonly byte[] Arms = [
        80, 160, 5, 10, 0, 0, 0, 0, 0, 0, 0, 0, 68, 132, 72, 136,
        20, 36, 24, 40, 65, 129, 66, 130, 17, 33, 18, 34, 69, 133, 70, 73,
        74, 134, 137, 138, 21, 37, 22, 25, 26, 38, 41, 42, 84, 100, 148, 164,
        88, 104, 152, 168, 81, 97, 145, 161, 82, 98, 146, 162, 85, 101, 149, 165,
        86, 89, 90, 102, 150, 105, 153, 166, 169, 106, 154, 170, 0, 0, 0, 0,
        240, 15, 196, 76, 204, 52, 28, 60, 193, 67, 195, 49, 19, 51, 197, 79,
        207, 53, 31, 63, 244, 92, 252, 241, 83, 243, 245, 95, 255, 68, 20, 17,
        65, 0, 0, 0, 16, 1, 64, 4, 32, 2, 128, 8, 144, 9, 96, 6,
    ];
    private static bool DrawShape(DrawingContext dc, int cp, double x, double y, double w, double h, Brush ink)
    {
        void Rect(double a, double b, double c, double d) =>
            dc.DrawRectangle(ink, null, new Rect(x+a, y+b, c, d));
        if (cp < 0x2580)
        {
            int arms = Arms[cp - 0x2500];
            if (cp is >= 0x2571 and <= 0x2573)
            {
                var pen = new Pen(ink, 1);
                if (cp != 0x2572) dc.DrawLine(pen, new Point(x,y+h), new Point(x+w,y));
                if (cp != 0x2571) dc.DrawLine(pen, new Point(x,y), new Point(x+w,y+h));
                return true;
            }
            if (arms == 0)
            {
                // Double, triple and quadruple dashed horizontal / vertical lines.
                int n = cp >= 0x254c ? 2 : cp < 0x2508 ? 3 : 4;
                bool vertical = (cp & 2) != 0;
                double thick = (cp & 1) == 0 ? 1 : 2;
                double step = (vertical ? h : w) / n;
                for (int k = 0; k < n; k++)
                    if (vertical) Rect(w/2-thick/2, k*step, thick, step*.72);
                    else Rect(k*step, h/2-thick/2, step*.72, thick);
                return true;
            }
            for (int i = 0; i < 4; i++, arms >>= 2)
            {
                int weight = arms & 3;
                if (weight == 0) continue;
                double t = weight == 2 ? 2 : 1;
                void Stroke(double offset)
                {
                    if (i < 2) Rect(w/2 + offset - t/2, i == 0 ? 0 : h/2 - .5, t, h/2 + .5);
                    else Rect(i == 2 ? 0 : w/2 - .5, h/2 + offset - t/2, w/2 + .5, t);
                }
                if (weight == 3) { Stroke(-1); Stroke(1); } else Stroke(0);
            }
        }
        else if (cp == 0x2580) Rect(0, 0, w, h/2);
        else if (cp <= 0x2588) { var dh = h*(cp-0x2580)/8; Rect(0, h-dh, w, dh); }
        else if (cp <= 0x258f) Rect(0, 0, w*(0x2590-cp)/8, h);
        else if (cp == 0x2590) Rect(w/2, 0, w/2, h);
        else if (cp <= 0x2593) { dc.PushOpacity((cp-0x2590)*.25); Rect(0, 0, w, h); dc.Pop(); }
        else if (cp == 0x2594) Rect(0, 0, w, h/8);
        else if (cp == 0x2595) Rect(w*7/8, 0, w/8, h);
        else
        {
            ReadOnlySpan<byte> quadrants = [4,8,1,13,9,7,11,2,6,14];
            int mask = quadrants[cp-0x2596];
            for (int i = 0; i < 4; i++)
                if ((mask & (1<<i)) != 0) Rect((i%2)*w/2, (i/2)*h/2, w/2, h/2);
        }
        return true;
    }
}
