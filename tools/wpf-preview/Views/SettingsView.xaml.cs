using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Media3D;
using System.Windows.Threading;

namespace Yaac.WpfPreview.Views;

public partial class SettingsView : UserControl
{
    public event Action<string, object?>? Command;

    public bool IsRecordingShortcut => capturing is not null;

    private static readonly string[] PageIds =
        ["targets", "providers", "skills", "approvals", "appearance", "window", "android", "keyboard", "terminal", "notifications", "advanced", "about"];

    private readonly Dictionary<string, Surface> surfaces = new(StringComparer.Ordinal);
    private Surface? current;
    private Field? capturing;
    private bool closeAllowed = true;
    private bool settingPage;
    private int navigationVersion;

    public SettingsView()
    {
        InitializeComponent();
        foreach (var id in PageIds)
            Pages.Items.Add(new ListBoxItem { Content = PageTitle(id), Tag = id });

        IsVisibleChanged += (_, _) =>
        {
            if (!IsVisible)
                ClearSensitiveInputs();
            else if (current?.Mode == "firstRun")
                FocusFirstRun();
        };
    }

    /// <summary>Applies this window's settings or onboarding section of the Rust snapshot.</summary>
    public void Render(JsonElement view)
    {
        var requestedMode = Str(view, "mode");
        if (requestedMode == "hidden")
        {
            ClearSensitiveInputs();
            return;
        }
        var mode = requestedMode switch
        {
            "firstRun" => "firstRun",
            "updateNeeded" => "updateNeeded",
            _ => "settings"
        };
        var page = Str(view, "page").ToLowerInvariant();
        if (!PageIds.Contains(page))
            page = "targets";

        closeAllowed = Flag(view, "closeAllowed", true);
        settingPage = true;
        Navigation.Visibility = mode == "settings" ? Visibility.Visible : Visibility.Collapsed;
        Scroller.Margin = new Thickness(0, mode == "settings" ? 42 : 0, 0, 0);
        Grid.SetColumn(Scroller, mode == "settings" ? 1 : 0);
        Grid.SetColumnSpan(Scroller, mode == "settings" ? 1 : 2);
        if (mode == "settings")
            Pages.SelectedIndex = Array.IndexOf(PageIds, page);
        settingPage = false;

        var key = mode == "settings" ? page : mode;
        if (!surfaces.TryGetValue(key, out var next))
        {
            next = new Surface(this, mode);
            surfaces.Add(key, next);
        }

        next.Apply(view, mode == "settings" ? PageTitle(page) : mode == "firstRun" ? "YAAC" : "Update YAAC");
        if (current == next)
            return;

        if (current is not null)
        {
            current.Offset = Scroller.VerticalOffset;
            ClearSensitiveInputs();
        }
        current = next;
        SurfaceHost.Content = next.Root;
        RestoreScroll(next);
        if (mode == "firstRun" && IsVisible)
            FocusFirstRun();
    }

    private void FocusFirstRun()
    {
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(() =>
        {
            if (IsVisible && current?.Mode == "firstRun" && !IsKeyboardFocusWithin)
                Focus();
        }));
    }

    private void RestoreScroll(Surface surface)
    {
        // Apply after the new page has been measured so an old extent cannot clamp the offset.
        var version = ++navigationVersion;
        Dispatcher.BeginInvoke(DispatcherPriority.Loaded, new Action(() =>
        {
            if (navigationVersion == version)
                Scroller.ScrollToVerticalOffset(surface.Offset);
        }));
    }

    private void HandleScrollInput(object sender, MouseWheelEventArgs e) => navigationVersion++;

    private void ClearSensitiveInputs()
    {
        StopCapture();
        current?.ClearSecrets();
    }

    private void CloseClicked(object sender, RoutedEventArgs e) => Emit("settings.close", null);

    private void PageChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!settingPage && Pages.SelectedItem is ListBoxItem { Tag: string id })
            Emit("settings.page", new { id });
    }

    private void Emit(string name, object? payload) => Command?.Invoke(name, payload);

    private void SendAction(string id, Dictionary<string, object?>? values = null)
    {
        if (id.Length == 0)
            return;

        if (values is { Count: > 0 })
            Emit("settings.action", new { id, values });
        else
            Emit("settings.action", new { id });
    }

    private void HandleKey(object sender, KeyEventArgs e)
    {
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        var modifiers = Keyboard.Modifiers;
        if (capturing is not null)
        {
            e.Handled = true;
            if (e.IsRepeat)
                return;
            if (key == Key.Escape && modifiers == ModifierKeys.None)
                StopCapture();
            else if ((key == Key.Back || key == Key.Delete) && modifiers == ModifierKeys.None)
                FinishCapture("", modifiers);
            else if (CanonicalKey(key, modifiers) is { } name)
                FinishCapture(name, modifiers);
            return;
        }

        if (current is null || modifiers != ModifierKeys.None || e.IsRepeat)
            return;

        var origin = e.OriginalSource as DependencyObject;
        if (Ancestor<ComboBox>(origin) is { IsDropDownOpen: true } ||
            Ancestor<MenuItem>(origin) is not null)
            return;

        if (key == Key.Enter && Ancestor<TextBox>(origin) is { AcceptsReturn: true })
            return;

        var focusedRow = current.FindRow(origin);
        if (key == Key.Enter && Ancestor<ButtonBase>(origin) is null &&
            (focusedRow is not null || current.Mode == "settings" || origin == this))
        {
            var candidate = focusedRow is null ? current.FindAction(isCancel: false) : focusedRow.FindAction(isCancel: false);
            if (candidate is not null)
            {
                e.Handled = true;
                candidate.Invoke();
            }
        }
        else if (key == Key.Escape)
        {
            var candidate = focusedRow?.FindAction(isCancel: true) ?? current.FindAction(isCancel: true);
            if (candidate is not null)
            {
                e.Handled = true;
                candidate.Invoke();
            }
            else if (current.Mode == "settings" && closeAllowed)
            {
                e.Handled = true;
                Emit("settings.close", null);
            }
            else if (current.Mode == "settings")
            {
                e.Handled = true;
            }
        }
    }

    private void HandleMouse(object sender, MouseButtonEventArgs e)
    {
        if (capturing is null || e.ChangedButton is not (MouseButton.XButton1 or MouseButton.XButton2))
            return;

        e.Handled = true;
        FinishCapture(e.ChangedButton == MouseButton.XButton1 ? "MouseBack" : "MouseForward", ModifierKeys.None);
    }

    private void StartCapture(Field field)
    {
        if (capturing == field)
        {
            StopCapture();
            return;
        }

        StopCapture();
        capturing = field;
        field.SetRecording(true);
    }

    private void StopCapture()
    {
        var field = capturing;
        capturing = null;
        field?.SetRecording(false);
    }

    private void FinishCapture(string key, ModifierKeys modifiers)
    {
        if (capturing is not { } field)
            return;

        StopCapture();
        SendAction(field.ActionId, new Dictionary<string, object?>
        {
            ["key"] = key,
            ["ctrl"] = modifiers.HasFlag(ModifierKeys.Control),
            ["alt"] = modifiers.HasFlag(ModifierKeys.Alt),
            ["shift"] = modifiers.HasFlag(ModifierKeys.Shift),
            ["command"] = modifiers.HasFlag(ModifierKeys.Windows)
        });
    }

    internal static string? CanonicalKey(Key key, ModifierKeys modifiers)
    {
        if (key is >= Key.A and <= Key.Z or >= Key.F1 and <= Key.F24)
            return key.ToString();
        if (key is >= Key.D0 and <= Key.D9)
            return ((int)key - (int)Key.D0).ToString(System.Globalization.CultureInfo.InvariantCulture);
        if (key is >= Key.NumPad0 and <= Key.NumPad9)
            return ((int)key - (int)Key.NumPad0).ToString(System.Globalization.CultureInfo.InvariantCulture);

        return key switch
        {
            Key.Escape => "Escape",
            Key.Back => "Backspace",
            Key.Return => "Enter",
            Key.Tab => "Tab",
            Key.Space => "Space",
            Key.Insert => "Insert",
            Key.Delete => "Delete",
            Key.Home => "Home",
            Key.End => "End",
            Key.Prior => "PageUp",
            Key.Next => "PageDown",
            Key.Left or Key.Right or Key.Up or Key.Down => key.ToString(),
            Key.OemPlus => modifiers.HasFlag(ModifierKeys.Shift) ? "Plus" : "Equals",
            Key.Add => "Plus",
            Key.OemMinus or Key.Subtract => "Minus",
            Key.OemComma => "Comma",
            Key.OemPeriod or Key.Decimal => "Period",
            Key.OemQuestion or Key.Divide => "Slash",
            Key.OemPipe or Key.OemBackslash => "Backslash",
            Key.OemSemicolon => modifiers.HasFlag(ModifierKeys.Shift) ? "Colon" : "Semicolon",
            Key.OemQuotes => "Quote",
            Key.OemTilde => "Backtick",
            Key.OemOpenBrackets => "OpenBracket",
            Key.OemCloseBrackets => "CloseBracket",
            _ => null
        };
    }

    private Brush BrushFor(string name) => (Brush)FindResource(name);

    private Brush Tone(string tone) => BrushFor(tone switch
    {
        "error" => "YaacFailure",
        "success" => "YaacSuccess",
        "warning" => "YaacWarning",
        "accent" => "YaacLink",
        _ => "YaacMuted"
    });

    private static T? Ancestor<T>(DependencyObject? element) where T : DependencyObject
    {
        while (element is not null)
        {
            if (element is T match)
                return match;
            element = element is Visual or Visual3D
                ? VisualTreeHelper.GetParent(element)
                : LogicalTreeHelper.GetParent(element);
        }

        return null;
    }

    private static string PageTitle(string id) => char.ToUpperInvariant(id[0]) + id[1..];

    private static JsonElement Prop(JsonElement json, string property) =>
        json.ValueKind == JsonValueKind.Object && json.TryGetProperty(property, out var value) ? value : default;

    private static string Str(JsonElement json, string property, string fallback = "")
    {
        var value = Prop(json, property);
        return value.ValueKind switch
        {
            JsonValueKind.String => value.GetString() ?? fallback,
            JsonValueKind.Number => value.GetRawText(),
            _ => fallback
        };
    }

    private static bool Flag(JsonElement json, string property, bool fallback = false) =>
        Prop(json, property).ValueKind switch
        {
            JsonValueKind.True => true,
            JsonValueKind.False => false,
            _ => fallback
        };

    private static IEnumerable<JsonElement> Items(JsonElement json, string property)
    {
        var array = Prop(json, property);
        if (array.ValueKind == JsonValueKind.Array)
            foreach (var item in array.EnumerateArray())
                if (item.ValueKind == JsonValueKind.Object)
                    yield return item;
    }

    private static void Say(TextBlock block, string text)
    {
        if (block.Text != text)
            block.Text = text;
        block.Visibility = text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private static void Describe(FrameworkElement control, string description)
    {
        control.ToolTip = description.Length == 0 ? null : description;
        ToolTipService.SetShowOnDisabled(control, true);
        if (AutomationProperties.GetHelpText(control) != description)
            AutomationProperties.SetHelpText(control, description);
    }

    private static void Arrange<T>(
        Panel panel, Dictionary<string, T> saved, IEnumerable<JsonElement> items,
        Func<JsonElement, string> idOf, Func<JsonElement, T> create, Action<T, JsonElement> apply,
        Func<T, UIElement> element, Action<T>? remove = null)
    {
        var index = 0;
        var present = new HashSet<string>(StringComparer.Ordinal);
        foreach (var item in items)
        {
            var id = idOf(item);
            if (id.Length == 0)
                id = index.ToString(System.Globalization.CultureInfo.InvariantCulture);
            if (!present.Add(id))
                continue;
            if (!saved.TryGetValue(id, out var child))
            {
                child = create(item);
                saved.Add(id, child);
            }

            apply(child, item);
            var control = element(child);
            if (index >= panel.Children.Count || panel.Children[index] != control)
            {
                if (panel.Children.Contains(control))
                {
                    var focus = Keyboard.FocusedElement;
                    var focusedHere = control.IsKeyboardFocusWithin;
                    panel.Children.Remove(control);
                    panel.Children.Insert(index, control);
                    if (focusedHere && focus is UIElement input)
                        input.Focus();
                }
                else
                {
                    panel.Children.Insert(index, control);
                }
            }

            index++;
        }

        foreach (var id in saved.Keys.Where(id => !present.Contains(id)).ToArray())
        {
            var child = saved[id];
            remove?.Invoke(child);
            panel.Children.Remove(element(child));
            saved.Remove(id);
        }
    }
}
