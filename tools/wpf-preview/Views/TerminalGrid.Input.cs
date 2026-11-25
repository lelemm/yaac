using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace Yaac.WpfPreview.Views;

internal sealed partial class TerminalGrid
{
    internal event Action? Find;
    internal bool Exited { get; set; }
    internal bool Selection { get; set; }
    internal bool MouseReporting { get; set; }

    private (int Row, int Col)? hover;
    private void Hover(Point? pt)
    {
        (int Row, int Col)? cell = pt is Point pos && (Modifiers() & 4) != 0
            ? ((int)(pos.Y / Line), (int)(pos.X / Cell)) : null;
        if (hover == cell) return;
        hover = cell;
        Command?.Invoke("hover", new { row = cell?.Row, col = cell?.Col });
    }

    private static int Modifiers()
    {
        var m = Keyboard.Modifiers;
        return ((m & ModifierKeys.Shift) != 0 ? 1 : 0) | ((m & ModifierKeys.Alt) != 0 ? 2 : 0)
             | ((m & ModifierKeys.Control) != 0 ? 4 : 0) | ((m & ModifierKeys.Windows) != 0 ? 8 : 0);
    }

    private void InitializeInput()
    {
        // Intermediate IME text belongs to the local input method. Only the final
        // TextInput event sends text to the shell.
        TextCompositionManager.AddPreviewTextInputStartHandler(this, (_, e) => SetPreedit(e.TextComposition.CompositionText));
        TextCompositionManager.AddPreviewTextInputUpdateHandler(this, (_, e) => SetPreedit(e.TextComposition.CompositionText));
        TextCompositionManager.AddPreviewTextInputHandler(this, (_, _) => SetPreedit(""));
        TextInput += (_, e) =>
        {
            string text = e.Text.Length > 0 ? e.Text : e.SystemText;
            if (text.Length == 0) return;
            RestartBlink();
            Command?.Invoke("text", new { text, mods = Modifiers() });
            e.Handled = true;
        };
        PreviewKeyDown += (_, e) =>
        {
            var key = e.Key == Key.System ? e.SystemKey : e.Key;
            var mods = Modifiers();
            if (preedit.Length > 0 || key == Key.ImeProcessed) return;
            RestartBlink();
            if (mods == 5 && key == Key.F) Find?.Invoke();
            else
            {
                bool special = key is Key.Tab or Key.Return or Key.Escape or Key.Back or Key.Delete
                    or Key.Insert or Key.Home or Key.End or Key.Left or Key.Right or Key.Up or Key.Down
                    or Key.PageUp or Key.PageDown or >= Key.NumPad0 and <= Key.Divide or >= Key.F1 and <= Key.F24;
                // Leave text (including dead keys, AltGr, and IME composition) to TextInput.
                bool altGr = Keyboard.IsKeyDown(Key.RightAlt) && (mods & 4) != 0;
                if (!special && ((mods & 4) == 0 || altGr)) return;
                var name = key switch { Key.Back => "Backspace", Key.Return => "Enter",
                    Key.Prior => "PageUp", Key.Next => "PageDown", Key.Space => " ",
                    >= Key.A and <= Key.Z => (mods & 1) == 0 ? key.ToString().ToLowerInvariant() : key.ToString(),
                    >= Key.D0 and <= Key.D9 => ((int)(key-Key.D0)).ToString(),
                    Key.OemOpenBrackets => "[", Key.OemCloseBrackets => "]",
                    Key.OemPipe or Key.OemBackslash => "\\", Key.OemMinus => "-",
                    Key.OemQuestion => (mods & 1) == 0 ? "/" : "?", Key.Oem3 => (mods & 1) == 0 ? "`" : "~",
                    Key.OemPlus => (mods & 1) == 0 ? "=" : "+", Key.OemPeriod => (mods & 1) == 0 ? "." : ">",
                    Key.OemComma => (mods & 1) == 0 ? "," : "<", Key.OemQuotes => (mods & 1) == 0 ? "'" : "\"",
                    Key.OemSemicolon => (mods & 1) == 0 ? ";" : ":", _ => key.ToString() };
                Command?.Invoke("key", new { key = name, mods });
            }
            e.Handled = true;
        };
        MouseDown += (_, e) =>
        {
            Focus();
            if (e.ChangedButton == MouseButton.Right && (!MouseReporting || (Modifiers() & 1) != 0))
            {
                var menu = new ContextMenu();
                if (Selection)
                {
                    Item("Copy", () => Command?.Invoke("copy", null));
                    Item("Add to composer", () => Command?.Invoke("add", null));
                    Item("Search for this", () => Command?.Invoke("searchSelection", null));
                }
                else Item("Paste", Paste);
                Item("Clear", () => Command?.Invoke("clear", null));
                Item("Search", () => Find?.Invoke());
                if (Exited) Item("Restart", () => Command?.Invoke("restart", null));
                menu.IsOpen = true;
                void Item(string title, Action click)
                {
                    var item = new MenuItem { Header = title };
                    item.Click += (_, _) => click();
                    menu.Items.Add(item);
                }
            }
            else
            {
                CaptureMouse();
                MouseAction("down", e.GetPosition(this), e.ChangedButton.ToString(), e.ClickCount);
            }
            e.Handled = true;
        };
        MouseLeave += (_, _) => Hover(null);
        KeyUp += (_, _) => Hover(Mouse.GetPosition(this));
        MouseMove += (_, e) =>
        {
            Hover(e.GetPosition(this));
            if (IsMouseCaptured || MouseReporting)
                MouseAction("move", e.GetPosition(this), e.LeftButton == MouseButtonState.Pressed ? "Left" : e.RightButton == MouseButtonState.Pressed ? "Right" : e.MiddleButton == MouseButtonState.Pressed ? "Middle" : "None", 0);
        };
        MouseUp += (_, e) =>
        {
            if (!IsMouseCaptured) return;
            MouseAction("up", e.GetPosition(this), e.ChangedButton.ToString(), 0);
            ReleaseMouseCapture();
            e.Handled = true;
        };
        MouseWheel += (_, e) =>
        {
            MouseAction("wheel", e.GetPosition(this), e.Delta > 0 ? "Up" : "Down", 0);
            e.Handled = true;
        };
    }

    private (int Col, int Row, bool Right, string Button, int Mods)? lastMotion;
    private void MouseAction(string action, Point pt, string button, int clicks)
    {
        int col = Math.Clamp((int)(pt.X / Cell), 0, Math.Max(0, (int)(ActualWidth / Cell) - 1));
        int row = Math.Clamp((int)(pt.Y / Line), 0, Math.Max(0, (int)(ActualHeight / Line) - 1));
        bool right = pt.X / Cell - col >= .5;
        int mods = Modifiers();
        var sample = (col, row, right, button, mods);
        if (action == "move" && lastMotion == sample) return;
        lastMotion = action == "move" ? sample : null;
        Command?.Invoke("mouse", new { action, button, clicks, col, row, right, mods });
    }

    internal void Paste()
    {
        try
        {
            if (Clipboard.ContainsText()) Command?.Invoke("paste", new { text = Clipboard.GetText() });
        }
        catch (System.Runtime.InteropServices.ExternalException)
        {
            System.Media.SystemSounds.Beep.Play();
        }
    }
}
