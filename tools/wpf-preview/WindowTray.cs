using System.Drawing;
using System.Runtime.InteropServices;
using System.Windows;
using Forms = System.Windows.Forms;

namespace Yaac.WpfPreview;

internal sealed class WindowTray : IDisposable
{
    private readonly Icon plain;
    private readonly Icon badged;
    private readonly Forms.NotifyIcon icon;
    private readonly Forms.ContextMenuStrip menu;
    private bool showingBadge;

    private WindowTray(Action show, Action quit)
    {
        var source = Application.GetResourceStream(new Uri("Assets/yaac.ico", UriKind.Relative))!;
        using (source.Stream)
        using (var ico = new Icon(source.Stream))
            plain = (Icon)ico.Clone();
        badged = Badged(plain);
        menu = new Forms.ContextMenuStrip();
        menu.Items.Add("Open YAAC", null, (_, _) => show());
        menu.Items.Add(new Forms.ToolStripSeparator());
        menu.Items.Add("Quit", null, (_, _) => quit());
        icon = new Forms.NotifyIcon { Icon = plain, Text = "YAAC", ContextMenuStrip = menu, Visible = true };
        icon.MouseClick += (_, e) => { if (e.Button == Forms.MouseButtons.Left) show(); };
    }

    internal static WindowTray? TryCreate(Action show, Action quit)
    {
        if (FindWindow("Shell_TrayWnd", null) == IntPtr.Zero) return null;
        try { return new WindowTray(show, quit); }
        catch (Exception error) when (error is ExternalException or ArgumentException or System.IO.IOException)
        {
            System.Diagnostics.Debug.WriteLine($"YAAC tray: {error.Message}");
            return null;
        }
    }

    internal void Update(int needs, string tooltip)
    {
        if (showingBadge != (needs != 0))
        {
            showingBadge = needs != 0;
            icon.Icon = showingBadge ? badged : plain;
        }
        if (icon.Text != tooltip) icon.Text = tooltip;
    }

    private static Icon Badged(Icon source)
    {
        using var image = new Bitmap(32, 32);
        using var graphics = Graphics.FromImage(image);
        graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
        graphics.DrawIcon(source, new Rectangle(0, 0, 32, 32));
        using var outline = new SolidBrush(Color.FromArgb(30, 33, 40));
        using var badge = new SolidBrush(Color.FromArgb(255, 124, 135));
        graphics.FillEllipse(outline, 19, 19, 13, 13);
        graphics.FillEllipse(badge, 21, 21, 9, 9);
        var handle = image.GetHicon();
        try { return (Icon)Icon.FromHandle(handle).Clone(); }
        finally { DestroyIcon(handle); }
    }

    public void Dispose()
    {
        icon.Visible = false;
        icon.Dispose();
        menu.Dispose();
        plain.Dispose();
        badged.Dispose();
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr FindWindow(string className, string? windowName);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyIcon(IntPtr icon);
}
