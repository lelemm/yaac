using System.IO;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class BrowserView
{
    private ContextMenu? pageMenu;
    private Window? pageDialog;
    private Action? dismissHostMenu;
    private Action? dismissHostDialog;
    private Rect? popupBounds;
    private Rect? compositionCaret;

    internal void Paint(BrowserFrame frame)
    {
        var image = frame.Popup ? PopupImage : Page;
        if (!windowShown || active != frame.Tab || !Surface.IsVisible ||
            (frame.Popup && popupBounds is null) || frame.Patches.Count == 0)
        {
            SendFor(frame.Tab, "frameAck", new { id = frame.Id, retry = frame.ReadFailed });
            return;
        }

        var bitmap = image.Source as WriteableBitmap;
        if (bitmap is null || bitmap.PixelWidth != frame.Width || bitmap.PixelHeight != frame.Height)
        {
            bitmap = new WriteableBitmap(frame.Width, frame.Height, 96, 96, PixelFormats.Pbgra32, null);
            image.Source = bitmap;
        }
        if (!bitmap.TryLock(new Duration(TimeSpan.Zero)))
        {
            // Wait for the render thread asynchronously while the controller retains the lease.
            var retry = new DispatcherTimer(DispatcherPriority.Background, Dispatcher)
            {
                Interval = TimeSpan.FromMilliseconds(16),
            };
            retry.Tick += (_, _) => { retry.Stop(); Paint(frame); };
            retry.Start();
            return;
        }
        try
        {
            foreach (var patch in frame.Patches)
                bitmap.WritePixels(new Int32Rect(0, 0, patch.Rect.Width, patch.Rect.Height),
                    patch.Bytes, patch.Rect.Width * 4, patch.Rect.X, patch.Rect.Y);
        }
        finally
        {
            bitmap.Unlock();
            SendFor(frame.Tab, "frameAck", new { id = frame.Id });
        }
    }

    internal void ReceiveEvent(JsonElement envelope)
    {
        var tab = envelope.GetProperty("tab").GetUInt64();
        var notice = envelope.GetProperty("event");
        if (active != tab) return;
        if (notice.ValueKind == JsonValueKind.String)
        {
            switch (notice.GetString())
            {
                case "DismissMenu": dismissHostMenu?.Invoke(); break;
                case "DismissDialog": dismissHostDialog?.Invoke(); break;
            }
            return;
        }
        if (notice.TryGetProperty("Favicon", out var favicon))
        {
            try
            {
                var bytes = favicon.EnumerateArray().Select(value => value.GetByte()).ToArray();
                using var stream = new MemoryStream(bytes);
                var icon = new BitmapImage();
                icon.BeginInit();
                icon.CacheOption = BitmapCacheOption.OnLoad;
                icon.DecodePixelWidth = 16;
                icon.StreamSource = stream;
                icon.EndInit();
                icon.Freeze();
                icons[tab] = icon;
                if (tabState is JsonElement tabs)
                {
                    tabSignature = "";
                    RenderTabs(tabs, tabReason, false);
                }
            }
            catch (Exception error) when (error is IOException or NotSupportedException or ArgumentException)
            {
                // Chromium can decline or return an undecodable icon. Keep the title.
            }
            return;
        }

        if (!windowShown || !Surface.IsVisible)
        {
            if (notice.TryGetProperty("Menu", out _)) SendInput(new { Menu = (int?)null });
            if (notice.TryGetProperty("Dialog", out _)) SendInput(new { Dialog = new { accept = false, text = "" } });
            if (notice.TryGetProperty("FileDialog", out _)) SendInput(new { FileDialog = (string[]?)null });
            return;
        }
        if (notice.TryGetProperty("Popup", out var popup))
        {
            popupBounds = popup.ValueKind == JsonValueKind.Array
                ? new Rect(popup[0].GetInt32(), popup[1].GetInt32(), Math.Max(0, popup[2].GetInt32()), Math.Max(0, popup[3].GetInt32()))
                : null;
            ArrangePopup();
        }
        if (notice.TryGetProperty("Cursor", out var cursor))
            Surface.Cursor = BrowserCursor(cursor.GetInt32());
        if (notice.TryGetProperty("CompositionCaret", out var caret))
        {
            compositionCaret = new Rect(caret[0].GetInt32(), caret[1].GetInt32(),
                Math.Max(0, caret[2].GetInt32()), Math.Max(0, caret[3].GetInt32()));
            PositionImeInput();
        }
        if (notice.TryGetProperty("Menu", out var menu)) OpenMenu(tab, menu);
        if (notice.TryGetProperty("Dialog", out var dialog)) OpenDialog(tab, dialog);
        if (notice.TryGetProperty("FileDialog", out var file))
        {
            // The owner's modal file picker runs a nested dispatcher. Return the pipe
            // receive callback first so unrelated windows and browser updates can continue.
            var request = file.Clone();
            var sentThread = thread;
            Dispatcher.BeginInvoke(DispatcherPriority.Input, () => ChooseFiles(sentThread, tab, request));
        }
    }

    private JsonElement? tabState;
    private string tabReason = "Open a thread to use the browser.";

    private void ArrangePopup()
    {
        if (popupBounds is not Rect rect)
        {
            PopupImage.Visibility = Visibility.Collapsed;
            PopupImage.Source = null;
            return;
        }
        Canvas.SetLeft(PopupImage, rect.X / LayoutScale);
        Canvas.SetTop(PopupImage, rect.Y / LayoutScale);
        PopupImage.Width = rect.Width / LayoutScale;
        PopupImage.Height = rect.Height / LayoutScale;
        PopupImage.Visibility = Visibility.Visible;
    }

    private void PositionImeInput()
    {
        var caret = compositionCaret.GetValueOrDefault();
        var height = Math.Clamp(caret.Height / LayoutScale, 1, Math.Max(1, Surface.ActualHeight));
        var left = Math.Clamp(caret.X / LayoutScale, 0, Math.Max(0, Surface.ActualWidth - PageInput.Width));
        var top = Math.Clamp(caret.Y / LayoutScale, 0, Math.Max(0, Surface.ActualHeight - height));
        PageInput.Height = height;
        PageInput.Margin = new Thickness(left, top, 0, 0);
    }

    private static Cursor BrowserCursor(int cef) => cef switch
    {
        1 or 31 => Cursors.Cross,
        2 or 41 or 42 => Cursors.Hand,
        3 or 30 => Cursors.IBeam,
        4 => Cursors.Wait,
        5 => Cursors.Help,
        6 or 13 or 15 or 18 => Cursors.SizeWE,
        7 or 10 or 14 or 19 => Cursors.SizeNS,
        8 or 12 or 16 => Cursors.SizeNESW,
        9 or 11 or 17 => Cursors.SizeNWSE,
        >= 20 and <= 29 => Cursors.SizeAll,
        34 => Cursors.AppStarting,
        35 or 38 or 46 => Cursors.No,
        37 => Cursors.None,
        _ => Cursors.Arrow,
    };

    private void OpenMenu(ulong tab, JsonElement notice)
    {
        dismissHostMenu?.Invoke();
        var at = notice.GetProperty("at");
        var point = new[] { at[0].GetInt32(), at[1].GetInt32() };
        var menuThread = thread;
        void Complete(int? command) => Command?.Invoke("browser.input", new
        {
            thread = menuThread, tab, data = new { Menu = command },
        });
        var menu = new ContextMenu
        {
            Placement = System.Windows.Controls.Primitives.PlacementMode.RelativePoint,
            PlacementTarget = Surface,
            HorizontalOffset = point[0] / LayoutScale,
            VerticalOffset = point[1] / LayoutScale,
        };
        bool answered = false;
        foreach (var row in notice.GetProperty("items").EnumerateArray())
        {
            int id = row[0].GetInt32();
            var item = new MenuItem { Header = row[1].GetString(), IsEnabled = row[2].GetBoolean() };
            item.Click += (_, _) =>
            {
                answered = true;
                Complete(id);
            };
            menu.Items.Add(item);
        }
        if (menu.HasItems) menu.Items.Add(new Separator());
        var inspect = new MenuItem { Header = "Inspect element" };
        inspect.Click += (_, _) =>
        {
            answered = true;
            Complete(null);
            SendFor(tab, "inspect", new { at = point });
        };
        menu.Items.Add(inspect);
        menu.Closed += (_, _) =>
        {
            if (!answered) Complete(null);
            if (pageMenu != menu) return;
            pageMenu = null;
            dismissHostMenu = null;
        };
        pageMenu = menu;
        dismissHostMenu = () => { answered = true; menu.IsOpen = false; };
        menu.IsOpen = true;
    }

    private void OpenDialog(ulong tab, JsonElement notice)
    {
        if (Window.GetWindow(this) is not Window owner) return;
        dismissHostDialog?.Invoke();
        var kind = notice.GetProperty("kind").GetUInt32();
        var panel = new StackPanel { Margin = new Thickness(16) };
        panel.Children.Add(new TextBlock
        {
            Text = Text(notice, "message"), TextWrapping = TextWrapping.Wrap, MaxHeight = 280,
        });
        TextBox? field = null;
        if (kind == 2)
        {
            field = new TextBox { Text = Text(notice, "text"), Margin = new Thickness(0, 8, 0, 0) };
            panel.Children.Add(field);
        }

        var buttons = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right,
            Margin = new Thickness(0, 12, 0, 0) };
        panel.Children.Add(buttons);
        var confirm = new Button { Content = "OK", IsDefault = true, MinWidth = 65 };
        buttons.Children.Add(confirm);
        var dialog = new Window
        {
            Owner = owner, Title = "Browser dialog", Width = 430,
            WindowStartupLocation = WindowStartupLocation.CenterOwner,
            ShowInTaskbar = false, SizeToContent = SizeToContent.Height, ResizeMode = ResizeMode.NoResize,
            Content = panel, FontFamily = FontFamily, FontSize = FontSize,
        };
        dialog.SetResourceReference(BackgroundProperty, "YaacCanvas");
        dialog.SetResourceReference(ForegroundProperty, "YaacText");
        bool accepted = false, canceledByHost = false;
        confirm.Click += (_, _) => { accepted = true; dialog.Close(); };
        if (kind != 0)
        {
            var cancel = new Button { Content = "Cancel", MinWidth = 65, Margin = new Thickness(6, 0, 0, 0) };
            cancel.Click += (_, _) => dialog.Close();
            buttons.Children.Add(cancel);
            dialog.PreviewKeyDown += (_, e) =>
            {
                if (e.Key != Key.Escape) return;
                e.Handled = true;
                dialog.Close();
            };
        }
        var sentThread = thread;
        dialog.Closed += (_, _) =>
        {
            if (!canceledByHost)
                Command?.Invoke("browser.input", new
                {
                    thread = sentThread, tab, data = new { Dialog = new { accept = accepted || kind == 0, text = field?.Text ?? "" } },
                });
            if (pageDialog != dialog) return;
            pageDialog = null;
            dismissHostDialog = null;
        };
        pageDialog = dialog;
        dismissHostDialog = () => { canceledByHost = true; dialog.Close(); };
        dialog.Show();
        if (field is not null) { field.SelectAll(); field.Focus(); }
        else confirm.Focus();
    }

    private void ChooseFiles(string sentThread, ulong tab, JsonElement request)
    {
        if (Window.GetWindow(this) is not Window owner || !windowShown || active != tab || !Surface.IsVisible)
        {
            Command?.Invoke("browser.input", new { thread = sentThread, tab, data = new { FileDialog = (string[]?)null } });
            return;
        }
        string[]? paths = null;
        try
        {
            var mode = request.GetProperty("mode").GetUInt32();
            var title = Text(request, "title");
            if (mode == 2)
            {
                var folder = new Microsoft.Win32.OpenFolderDialog { Title = title };
                if (folder.ShowDialog(owner) == true) paths = folder.FolderNames;
            }
            else
            {
                Microsoft.Win32.FileDialog picker = mode == 3
                    ? new Microsoft.Win32.SaveFileDialog()
                    : new Microsoft.Win32.OpenFileDialog { Multiselect = mode == 1 };
                picker.Title = title;
                var extensions = request.GetProperty("filters").EnumerateArray()
                    .Select(item => item.GetString())
                    .OfType<string>()
                    .Where(filter => filter.StartsWith('.') && filter.Skip(1).All(c => char.IsAsciiLetterOrDigit(c) || c == '.'))
                    .Select(filter => "*" + filter)
                    .Distinct()
                    .ToArray();
                if (extensions.Length > 0) picker.Filter = $"Accepted files|{string.Join(";", extensions)}|All files|*.*";
                // Use the page's suggested basename only. The chooser opens in the user's
                // normal folder and the page learns only the files that the user selects.
                if (mode == 3) picker.FileName = System.IO.Path.GetFileName(Text(request, "default_path"));
                if (picker.ShowDialog(owner) == true) paths = picker.FileNames;
            }
        }
        catch (Exception error) when (error is ArgumentException or InvalidOperationException or System.Runtime.InteropServices.COMException)
        {
            // An invalid native picker request cancels the pending browser dialog.
        }
        finally
        {
            Command?.Invoke("browser.input", new { thread = sentThread, tab, data = new { FileDialog = paths } });
        }
    }

    private void DismissPageUi()
    {
        if (composing) SendInput("CancelComposition");
        composing = false;
        if (Surface.IsMouseCaptured) Surface.ReleaseMouseCapture();
        pageMenu?.SetCurrentValue(ContextMenu.IsOpenProperty, false);
        pageDialog?.Close();
        popupBounds = null;
        compositionCaret = null;
        ArrangePopup();
        PositionImeInput();
    }

}
