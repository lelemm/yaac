using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;

namespace Yaac.WpfPreview;

internal static class WindowInsets
{
    // WindowChrome extends the client area over the invisible resize frame when maximized.
    // Use monitor and client coordinates so the inset also follows per-monitor DPI and taskbar placement.
    internal static Thickness ForMaximized(Window window)
    {
        if (PresentationSource.FromVisual(window) is not HwndSource source || source.CompositionTarget is null)
            return new Thickness();
        var monitor = MonitorFromWindow(source.Handle, 2);
        var info = new MonitorInfo { Size = Marshal.SizeOf<MonitorInfo>() };
        var origin = new NativePoint();
        if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref info)
            || !GetClientRect(source.Handle, out var client) || !ClientToScreen(source.Handle, ref origin))
            return new Thickness();

        var transform = source.CompositionTarget.TransformFromDevice;
        return new Thickness(
            Math.Max(0, info.Work.Left - origin.X) * transform.M11,
            Math.Max(0, info.Work.Top - origin.Y) * transform.M22,
            Math.Max(0, origin.X + client.Right - info.Work.Right) * transform.M11,
            Math.Max(0, origin.Y + client.Bottom - info.Work.Bottom) * transform.M22);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativePoint { public int X, Y; }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    private struct MonitorInfo { public int Size; public NativeRect Screen, Work; public uint Flags; }

    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);

    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo info);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetClientRect(IntPtr window, out NativeRect rect);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ClientToScreen(IntPtr window, ref NativePoint point);
}
