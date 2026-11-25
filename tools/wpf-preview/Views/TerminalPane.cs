using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace Yaac.WpfPreview.Views;

internal sealed class TerminalPane : DockPanel
{
    private readonly bool panel;
    private readonly TerminalGrid screen = new() { Margin = new Thickness(8,4,8,4) };
    private readonly TextBlock label = new() { VerticalAlignment = VerticalAlignment.Center,
        TextTrimming = TextTrimming.CharacterEllipsis, Margin = new Thickness(8,0,6,0) };
    private readonly Button search, restart;
    private readonly TextBox query = new() { MinWidth = 60, MaxWidth = 320, Margin = new Thickness(6,2,4,2),
        VerticalContentAlignment = VerticalAlignment.Center, ToolTip = "Search" };
    private readonly TextBlock matches = new() { VerticalAlignment = VerticalAlignment.Center };
    private readonly DockPanel searchBar = new();
    private readonly Border toolbar, banner;
    private readonly Button bannerRestart = new() { Content = "Restart", Margin = new Thickness(8,4,8,4) };
    private readonly TextBlock status = new() { VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(12,0,4,0) };
    private bool binding;
    private bool focusSelectionSearch;
    internal event Action<string, object?>? Command;
    internal event Action<ulong>? Painted;
    internal bool GridFocused => screen.IsKeyboardFocused;
    internal void Paste() => screen.Paste();
    internal void FocusScreen() => screen.Focus();

    internal TerminalPane(bool panel)
    {
        this.panel = panel;
        Background = Palette.Brush("YaacCanvas");
        var bar = new DockPanel { Height = 35 };
        toolbar = new Border { BorderThickness = new Thickness(0,0,0,1), BorderBrush = Palette.Brush("YaacRule"), Child = bar };
        SetDock(toolbar, Dock.Top);
        Children.Add(toolbar);
        var actions = new StackPanel { Orientation = Orientation.Horizontal, Margin = new Thickness(0,0,5,0) };
        if (!panel) { SetDock(actions, Dock.Right); bar.Children.Add(actions); }
        search = Icon(actions, "Search", "Search (Ctrl+Shift+F)", Find);
        Icon(actions, "Clear", "Clear terminal", () => Command?.Invoke("clear",null));
        restart = Icon(actions, "Reload", "Restart shell", Restart);
        label.Foreground = matches.Foreground = Palette.Brush("YaacMuted");
        var navigation = new StackPanel { Orientation = Orientation.Horizontal };
        SetDock(navigation,Dock.Right);
        searchBar.Children.Add(navigation);
        navigation.Children.Add(matches);
        Icon(navigation,"Previous change","Older match (Enter)",() => Next(true));
        Icon(navigation,"Next change","Newer match (Shift+Enter)",() => Next(false));
        Icon(navigation,"Close","Close search (Esc)",CloseSearch);
        searchBar.Children.Add(query);
        var center = new Grid();
        center.Children.Add(label);
        center.Children.Add(searchBar);
        bar.Children.Add(center);
        ShowSearch(false);
        var content = new Grid();
        Children.Add(content);
        content.Children.Add(screen);
        var message = new DockPanel();
        SetDock(bannerRestart, Dock.Right);
        message.Children.Add(bannerRestart);
        message.Children.Add(status);
        bannerRestart.Click += (_,_) => Restart();
        banner = new Border { VerticalAlignment = VerticalAlignment.Bottom, Height = 40,
            BorderThickness = new Thickness(0,1,0,0), BorderBrush = Palette.Brush("YaacRule"),
            Background = Palette.Brush("YaacPanel"), Child = message, Visibility = Visibility.Collapsed };
        content.Children.Add(banner);
        screen.Command += (name, data) =>
        {
            if (name == "searchSelection") focusSelectionSearch = true;
            Command?.Invoke(name, data);
        };
        screen.Painted += id => Painted?.Invoke(id);
        screen.Find += Find;
        query.TextChanged += (_,_) => { if (!binding) Command?.Invoke("search",new {query = query.Text}); };
        query.PreviewKeyDown += (_,e) =>
        {
            if (e.Key == Key.Enter) Next(!Keyboard.Modifiers.HasFlag(ModifierKeys.Shift));
            else if (e.Key == Key.Escape) CloseSearch();
            else return;
            e.Handled = true;
        };
    }

    private static Button Icon(Panel parent,string glyph,string tip,Action click)
    {
        var button = new Button();
        PaneIcon.Set(button,glyph,tip);
        button.Click += (_,_) => click();
        parent.Children.Add(button);
        return button;
    }
    private void Restart() => Command?.Invoke("restart",null);
    private void Next(bool backward) { Command?.Invoke("next",new {backward}); query.Focus(); }
    private void Find() { ShowSearch(true); query.Focus(); query.SelectAll(); }
    private void CloseSearch() { query.Clear(); ShowSearch(false); screen.Focus(); }
    private void ShowSearch(bool show)
    {
        searchBar.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        toolbar.Visibility = panel && !show ? Visibility.Collapsed : Visibility.Visible;
        search.Visibility = label.Visibility = show ? Visibility.Collapsed : Visibility.Visible;
    }

    internal void Bind(JsonElement tab, bool switched)
    {
        string cwd = tab.Value("cwd").String();
        label.Text = tab.Value("shell").String() + " · " + cwd.TrimEnd('/','\\').Split('/','\\').LastOrDefault();
        label.ToolTip = cwd;
        var exit = tab.Value("exitCode");
        bool exited = exit.ValueKind == JsonValueKind.Number;
        status.Text = exited ? $"Process exited with code {exit.GetInt32()}" : tab.Value("error").String();
        screen.Exited = exited || tab.Flag("failed");
        banner.Visibility = status.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        restart.Visibility = bannerRestart.Visibility = screen.Exited ? Visibility.Visible : Visibility.Collapsed;
        if (!switched) return;
        focusSelectionSearch = false;
        screen.Reset();
        ShowSearch(false);
        binding = true;
        query.Clear();
        binding = false;
        screen.Focus();
        screen.ResizeGrid();
    }

    internal void Frame(TerminalFrame frame)
    {
        if (frame.Search != null && !query.IsKeyboardFocused)
        {
            binding = true;
            ShowSearch(true);
            query.Text = frame.Search;
            binding = false;
            if (focusSelectionSearch)
            {
                query.Focus();
                query.SelectAll();
                focusSelectionSearch = false;
            }
        }
        matches.Text = frame.SearchStatus;
        screen.MouseReporting = frame.Mouse;
        screen.Selection = frame.Selection;
        screen.Queue(frame);
    }
}
