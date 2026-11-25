using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Threading;
using Yaac.WpfPreview.Views;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private BrowserView? panelTabs;
    private bool panelOpen, panelMaximised;
    private double? panelHeight;
    private const string ToggleTerminal = "toggle-terminal-panel";
    private string panelThread = "";
    private bool TerminalFocused => BrowserPane.TerminalFocused || panelTabs?.TerminalFocused == true;

    private Window? terminalPasteDialog;
    private void ConfirmTerminalPaste(JsonElement prompt)
    {
        terminalPasteDialog?.Close();
        var dialog = Overlay("Paste into terminal", 260);
        terminalPasteDialog = dialog;
        var box = new StackPanel { Margin = new Thickness(16) };
        box.Children.Add(new TextBlock { Text = $"Paste {prompt.GetProperty("lines").GetInt32()} lines? They will run." });
        var remember = new CheckBox { Content = "Don’t ask again", Margin = new Thickness(0,12,0,12) };
        box.Children.Add(remember);
        var buttons = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right };
        var cancel = new Button { Content = "Cancel", Margin = new Thickness(0,0,8,0) };
        var paste = new Button { Content = "Paste" };
        buttons.Children.Add(cancel);
        buttons.Children.Add(paste);
        box.Children.Add(buttons);
        dialog.Content = box;
        bool allow = false;
        string thread = prompt.GetProperty("thread").GetString()!;
        ulong tab = prompt.GetProperty("tab").GetUInt64(), token = prompt.GetProperty("token").GetUInt64();
        bool panel = prompt.Flag("panel");
        dialog.Closed += (_, _) =>
        {
            terminalPasteDialog = null;
            Send("terminal.pasteDecision", new { thread, tab, panel, data = new { token, allow, remember = remember.IsChecked == true } });
        };
        paste.Click += (_, _) => { allow = true; dialog.Close(); };
        cancel.Click += (_, _) => dialog.Close();
        dialog.PreviewKeyDown += (_, e) => { if (e.Key == System.Windows.Input.Key.Escape) dialog.Close(); };
        dialog.Show();
        cancel.Focus();
    }

    private void RenderTerminalPanel(JsonElement browser)
    {
        var panel = browser.Value("terminalPanel");
        bool open = panel.Flag("open"), maximised = panel.Flag("maximised");
        string thread = browser.Value("thread").Text();
        bool opening = open && (!panelOpen || thread != panelThread);
        bool closing = !open && panelOpen && thread == panelThread;
        panelOpen = open;
        panelMaximised = open && maximised;
        panelThread = thread;
        panelHeight = panel.Value("share").ValueKind == JsonValueKind.Number ? panel.GetProperty("share").GetDouble() : null;
        bool available = browser.TryGetProperty("terminalReason", out var reason) && string.IsNullOrEmpty(reason.OptionalText());
        ComposerPane.TerminalToggle.Visibility = available ? Visibility.Visible : Visibility.Collapsed;
        if (available) ComposerPane.ControlActions.Visibility = Visibility.Visible;
        ComposerPane.TerminalToggle.Background = panelOpen ? Palette.Brush("YaacSelected") : System.Windows.Media.Brushes.Transparent;
        UpdateTerminalTip();
        TerminalPanelHost.Visibility = open ? Visibility.Visible : Visibility.Collapsed;
        ChatWork.Visibility = panelMaximised ? Visibility.Collapsed : Visibility.Visible;
        TerminalDivider.Visibility = open && !maximised ? Visibility.Visible : Visibility.Collapsed;
        SizeTerminalPanel();
        if (open)
        {
            if (panelTabs is null)
            {
                panelTabs = new BrowserView { IsTerminalPanel = true };
                panelTabs.Command += Send;
                panelTabs.ComposerCaret = () => ComposerPane.CaretIndex;
                TerminalPanelHost.Content = panelTabs;
            }
            panelTabs.Render(panel);
            if (opening) Dispatcher.BeginInvoke(DispatcherPriority.Input, () => { if (panelOpen) panelTabs.FocusTerminal(); });
        }
        else if (closing) ComposerPane.Editor.Focus();
    }

    private void SizeTerminalPanel()
    {
        if (TerminalDivider.IsDragging) return;
        double room = ChatRegion.ActualHeight;
        bool split = panelOpen && !panelMaximised;
        double minChat = split ? Math.Min(room, 160 + ComposerColumn.ActualHeight + HeaderDetails.ActualHeight) : 0;
        double maxPanel = Math.Max(0, room - minChat - 5);
        ConversationRow.MinHeight = minChat;
        ConversationRow.Height = panelMaximised ? new GridLength(0) : new GridLength(1, GridUnitType.Star);
        TerminalPanelRow.MinHeight = split ? Math.Min(120,maxPanel) : 0;
        TerminalDividerRow.Height = new GridLength(split ? 5 : 0);
        TerminalPanelRow.Height = panelMaximised ? new GridLength(1,GridUnitType.Star)
            : new GridLength(split ? Math.Clamp(room * (panelHeight ?? 1.0 / 3), TerminalPanelRow.MinHeight, maxPanel) : 0);
    }

    private void TerminalHeightChanged(object sender, DragCompletedEventArgs e)
    {
        if (!e.Canceled && panelOpen && !panelMaximised)
            Send("terminal.panelHeight", new { thread = panelThread, share = TerminalPanelRow.ActualHeight / Math.Max(1, ChatRegion.ActualHeight) });
    }

    private void UpdateTerminalTip()
    {
        var binding = shortcuts.FirstOrDefault(p => p.Value.Id == ToggleTerminal);
        var tip = "Terminal";
        if (binding.Value != null)
        {
            var m = binding.Key.Modifiers;
            var key = binding.Key.Key == "Backtick" ? "`" : binding.Key.Key;
            var chord = (m.HasFlag(System.Windows.Input.ModifierKeys.Control) ? "Ctrl+" : "")
                + (m.HasFlag(System.Windows.Input.ModifierKeys.Alt) ? "Alt+" : "")
                + (m.HasFlag(System.Windows.Input.ModifierKeys.Shift) ? "Shift+" : "") + key;
            tip += $" ({chord})";
        }
        ComposerPane.TerminalToggle.ToolTip = tip;
    }
}
