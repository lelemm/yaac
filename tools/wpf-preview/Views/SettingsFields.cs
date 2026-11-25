using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using Microsoft.Win32;

namespace Yaac.WpfPreview.Views;

public partial class SettingsView
{
    private sealed class Field
    {
        private readonly SettingsView owner;
        private readonly Action changed;
        private readonly TextBlock label = Heading(12);
        private readonly TextBlock detail = Text(12);
        private readonly ContentControl body = new() { Focusable = false };
        private readonly TextBlock placeholder = Text(13);
        private readonly Dictionary<string, ContentControl> options = new(StringComparer.Ordinal);
        private string type = "";
        private string remote = "";
        private string fontSample = "";
        private bool codeFont;
        private string shortcutLabel = "";
        private readonly string radioGroup = Guid.NewGuid().ToString();
        private bool initialized;
        private bool applying;
        private bool dirty;
        private bool required;
        private bool clearOnSubmit;
        private bool enabled;
        private bool revealed;
        private bool recording;
        private string validation = "";
        private string? selection;
        private TextBox? text;
        private PasswordBox? secret;
        private TextBox? reveal;
        private Button? revealButton;
        private ComboBox? combo;
        private WrapPanel? radios;
        private Button? shortcut;
        private ProgressBar? progress;

        public StackPanel Root { get; } = new()
        {
            Margin = new Thickness(0, 2, 10, 5)
        };
        public string Id { get; }
        public string ActionId { get; private set; } = "";
        public bool IsShortcut => type == "shortcut";
        public bool IncludeInSubmit => enabled && Id.Length > 0 && type is not ("shortcut" or "progress");
        public bool NeedsAny => IncludeInSubmit && validation == "any";

        // The caller reads a password only when dispatching an explicit submit.
        public string Value => type switch
        {
            "password" => revealed ? reveal?.Text ?? "" : secret?.Password ?? "",
            "choice" or "radio" => selection ?? "",
            _ => text?.Text ?? ""
        };

        public bool HasValue
        {
            get
            {
                if (type != "password")
                    return !string.IsNullOrWhiteSpace(Value);

                using var secure = secret?.SecurePassword;
                return revealed ? !string.IsNullOrEmpty(reveal?.Text) : secure is { Length: > 0 };
            }
        }

        public bool IsValid => !IncludeInSubmit ||
            ((!required || HasValue) && (validation != "port" ||
                (type is "text" or "file" && ushort.TryParse(Value.Trim(), NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out var port) && port > 0)));

        public Field(SettingsView owner, Action changed, string id)
        {
            this.owner = owner;
            this.changed = changed;
            Id = id;

            label.Margin = new Thickness(1, 0, 0, 4);
            detail.Margin = new Thickness(1, 4, 0, 0);
            placeholder.Foreground = owner.Tone("");
            placeholder.TextTrimming = TextTrimming.CharacterEllipsis;
            placeholder.TextWrapping = TextWrapping.NoWrap;
            placeholder.VerticalAlignment = VerticalAlignment.Center;
            placeholder.IsHitTestVisible = false;

            Root.Children.Add(label);
            Root.Children.Add(body);
            Root.Children.Add(detail);
        }

        public void Apply(JsonElement field, bool reset)
        {
            applying = true;
            try
            {
                var subtype = Str(field, "type", "text");
                if (!initialized || type != subtype)
                {
                    Create(subtype);
                    reset = true;
                }

                var name = Str(field, "label");
                var hint = Str(field, "placeholder");
                var description = Str(field, "detail");
                Say(label, name);
                owner.SayDetail(detail, description);
                Say(placeholder, hint);
                Describe(body, description);
                var input = (Control?)secret ?? (Control?)text ?? combo ?? (Control?)shortcut ?? progress;
                if (input is not null)
                {
                    AutomationProperties.SetName(input, name.Length == 0 ? hint : name);
                    AutomationProperties.SetLabeledBy(input, label);
                    Describe(input, description);
                }
                if (reveal is not null)
                    AutomationProperties.SetName(reveal, name);

                fontSample = Str(field, "sample");
                codeFont = Flag(field, "codeFont");
                required = Flag(field, "required");
                clearOnSubmit = Flag(field, "clear", type is "text" or "file");
                enabled = Flag(field, "enabled", true);
                validation = Str(field, "validation");
                ActionId = Str(field, "action");
                body.IsEnabled = enabled;
                if (recording && (!enabled || ActionId.Length == 0))
                    owner.StopCapture();

                if (reset)
                    dirty = false;
                if (type == "password")
                {
                    if (reset)
                        ClearSecret();
                    if (revealButton is not null)
                    {
                        var canReveal = Flag(field, "show");
                        if (!canReveal && revealed)
                            ShowSecret(false);
                        revealButton.Visibility = canReveal ? Visibility.Visible : Visibility.Collapsed;
                    }
                }
                else
                {
                    var previous = remote;
                    remote = Str(field, "value");
                    var sync = reset || (!dirty && previous != remote) || (dirty && Value == remote);
                    if (sync)
                        dirty = false;
                    if (combo is not null || radios is not null)
                    {
                        var selectedByController = SyncOptions(Items(field, "options"));
                        if (sync || (selection is not null && !options.ContainsKey(selection)))
                        {
                            dirty = false;
                            selection = options.ContainsKey(remote) ? remote : selectedByController;
                        }
                        else if (!dirty && remote.Length == 0 && selectedByController is not null)
                            selection = selectedByController;
                        SyncSelection();
                    }
                    if (text is not null && sync && text.Text != remote)
                        text.Text = remote;
                    if (shortcut is not null)
                    {
                        shortcutLabel = remote.Length == 0 ? Str(field, "placeholder", "Add a key") : remote;
                        shortcut.IsEnabled = ActionId.Length > 0;
                        SetRecording(recording);
                    }
                    if (progress is not null)
                    {
                        var numeric = double.TryParse(remote, NumberStyles.Float, CultureInfo.InvariantCulture, out var percent)
                            && double.IsFinite(percent);
                        progress.IsIndeterminate = !numeric;
                        progress.Value = numeric ? Math.Clamp(percent, 0, 100) : 0;
                    }
                }

                ShowPlaceholder();
            }
            finally
            {
                applying = false;
            }
        }

        private void Create(string subtype)
        {
            Detach();
            initialized = true;
            type = subtype;
            selection = null;
            options.Clear();
            text = null;
            secret = null;
            reveal = null;
            revealButton = null;
            combo = null;
            radios = null;
            shortcut = null;
            progress = null;
            body.Content = null;
            body.Content = subtype switch
            {
                "password" => CreateSecret(),
                "file" => CreatePath(),
                "choice" => CreateChoice(),
                "radio" => radios = new WrapPanel { MaxWidth = 540 },
                "shortcut" => shortcut = CreateShortcut(),
                "progress" => progress = new ProgressBar { Minimum = 0, Maximum = 100, Height = 6, Width = 176 },
                _ => CreateText()
            };
            body.HorizontalContentAlignment = HorizontalAlignment.Left;
            Root.MaxWidth = subtype == "radio" ? 540 : 380;
        }

        private Grid WithPlaceholder(FrameworkElement input, double width)
        {
            var grid = new Grid { Width = width };
            grid.Children.Add(input);
            placeholder.Margin = new Thickness(9, 0, input is ComboBox ? 29 : 9, 0);
            grid.Children.Add(placeholder);
            return grid;
        }

        private Grid CreateText()
        {
            text = new TextBox { MinHeight = 29, Padding = new Thickness(8, 4, 8, 4) };
            text.TextChanged += (_, _) => Edited();
            return WithPlaceholder(text, 180);
        }

        private UIElement CreatePath()
        {
            text = new TextBox
            {
                MinHeight = 29,
                Padding = new Thickness(8, 4, 8, 4),
            };
            text.TextChanged += (_, _) => Edited();
            text.SetResourceReference(Control.FontFamilyProperty, YaacFonts.CodeResource);
            var browse = new Button
            {
                Content = "Browse…",
                Margin = new Thickness(6, 0, 0, 0),
                Padding = new Thickness(9, 4, 9, 4)
            };
            AutomationProperties.SetName(browse, "Browse for key file");
            browse.Click += (_, _) =>
            {
                var dialog = new OpenFileDialog { Title = "Choose a key file", CheckFileExists = true };
                var window = Window.GetWindow(owner);
                var result = window is null ? dialog.ShowDialog() : dialog.ShowDialog(window);
                if (result == true && Root.IsVisible && text is not null)
                    text.Text = dialog.FileName;
            };
            var stack = new StackPanel { Orientation = Orientation.Horizontal };
            stack.Children.Add(WithPlaceholder(text, 220));
            stack.Children.Add(browse);
            return stack;
        }

        private UIElement CreateSecret()
        {
            secret = new PasswordBox { MinHeight = 29, Padding = new Thickness(8, 4, 8, 4) };
            secret.PasswordChanged += (_, _) => Edited();
            reveal = new TextBox
            {
                MinHeight = 29,
                Padding = new Thickness(8, 4, 8, 4),
                IsUndoEnabled = false,
                IsInactiveSelectionHighlightEnabled = false,
                Visibility = Visibility.Collapsed
            };
            // The reveal is meant for visual checking. Keep its contents out of clipboard workflows.
            DataObject.AddCopyingHandler(reveal, (_, args) => args.CancelCommand());
            reveal.TextChanged += (_, _) => Edited();
            revealButton = new Button
            {
                Content = "Show",
                Margin = new Thickness(6, 0, 0, 0),
                Padding = new Thickness(9, 4, 9, 4),
                Visibility = Visibility.Collapsed
            };
            revealButton.Click += (_, _) => ShowSecret(!revealed, true);

            var grid = WithPlaceholder(secret, 210);
            grid.Children.Insert(1, reveal);
            var stack = new StackPanel { Orientation = Orientation.Horizontal };
            stack.Children.Add(grid);
            stack.Children.Add(revealButton);
            return stack;
        }

        private void ShowSecret(bool show, bool focus = false)
        {
            if (secret is null || reveal is null || revealButton is null || revealed == show)
                return;

            var wasApplying = applying;
            applying = true;
            try
            {
                if (show)
                {
                    reveal.Text = secret.Password;
                    secret.Clear();
                }
                else
                {
                    secret.Password = reveal.Text;
                    reveal.Clear();
                }
                revealed = show;
                reveal.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
                secret.Visibility = show ? Visibility.Collapsed : Visibility.Visible;
                revealButton.Content = show ? "Hide" : "Show";
                if (focus)
                {
                    if (show)
                    {
                        reveal.Focus();
                        reveal.CaretIndex = reveal.Text.Length;
                    }
                    else
                    {
                        secret.Focus();
                    }
                }
            }
            finally
            {
                applying = wasApplying;
            }

            ShowPlaceholder();
        }

        private Grid CreateChoice()
        {
            combo = new ComboBox { MinHeight = 29, MaxDropDownHeight = 260 };
            combo.SelectionChanged += (_, _) =>
            {
                if (!applying && combo.SelectedItem is ComboBoxItem { Tag: string id })
                    Choose(id);
            };
            return WithPlaceholder(combo, 222);
        }

        private Button CreateShortcut()
        {
            var button = new Button
            {
                Padding = new Thickness(9, 4, 9, 4),
                MinWidth = 80,
                MinHeight = 27
            };
            button.Click += (_, _) => owner.StartCapture(this);
            button.LostKeyboardFocus += (_, _) =>
            {
                if (owner.capturing == this)
                    owner.StopCapture();
            };
            return button;
        }

        private string? SyncOptions(IEnumerable<JsonElement> items)
        {
            string? checkedOption = null;
            var present = new HashSet<string>(StringComparer.Ordinal);
            var index = 0;
            foreach (var item in items)
            {
                var id = Str(item, "id");
                if (id.Length == 0 || !present.Add(id))
                    continue;
                if (!options.TryGetValue(id, out var option))
                {
                    if (combo is not null)
                    {
                        option = new ComboBoxItem { Tag = id };
                    }
                    else
                    {
                        var radio = new RadioButton
                        {
                            Tag = id,
                            GroupName = radioGroup,
                            Margin = new Thickness(0, 2, 14, 4)
                        };
                        radio.Checked += (_, _) =>
                        {
                            if (!applying)
                                Choose(id);
                        };
                        option = radio;
                    }
                    options.Add(id, option);
                }

                var optionLabel = Str(item, "label", id);
                var notes = Str(item, "detail");
                var content = combo is not null && notes.Length > 0 ? optionLabel + " · " + notes : optionLabel;
                if (!Equals(option.Content, content))
                    option.Content = content;
                if (combo is not null && fontSample.Length > 0)
                {
                    option.ToolTip ??= new TextBlock
                    {
                        Text = fontSample,
                        FontFamily = YaacFonts.Preview(item.Value("fontPreview"), codeFont),
                        FontSize = 15,
                        Margin = new Thickness(5, 3, 5, 3),
                    };
                    ToolTipService.SetInitialShowDelay(option, 250);
                }
                else
                    Describe(option, notes);
                option.IsEnabled = Flag(item, "enabled", true);
                AutomationProperties.SetName(option, optionLabel);
                if (checkedOption is null && Flag(item, "checked"))
                    checkedOption = id;

                if (combo is not null && (index >= combo.Items.Count || combo.Items[index] != option))
                {
                    if (combo.Items.Contains(option))
                        combo.Items.Remove(option);
                    combo.Items.Insert(index, option);
                }
                else if (radios is not null && (index >= radios.Children.Count || radios.Children[index] != option))
                {
                    radios.Children.Remove(option);
                    radios.Children.Insert(index, option);
                }

                index++;
            }

            foreach (var id in options.Keys.Where(id => !present.Contains(id)).ToArray())
            {
                var option = options[id];
                combo?.Items.Remove(option);
                radios?.Children.Remove(option);
                options.Remove(id);
            }

            return checkedOption;
        }

        private void SyncSelection()
        {
            if (combo is not null)
            {
                var item = selection is not null && options.TryGetValue(selection, out var option) ? option : null;
                if (combo.SelectedItem != item)
                    combo.SelectedItem = item;
            }
            else if (radios is not null)
            {
                foreach (var (id, option) in options)
                    if (option is RadioButton radio && radio.IsChecked != (id == selection))
                        radio.IsChecked = id == selection;
            }
        }

        private void Choose(string value)
        {
            if (selection == value)
                return;

            selection = value;
            Edited();
            if (ActionId.Length > 0 && enabled && Id.Length > 0)
            {
                // Immediate choices remain controller-owned, even when the user selects twice before a snapshot.
                dirty = false;
                owner.SendAction(ActionId, new Dictionary<string, object?> { [Id] = value });
            }
        }

        private void Edited()
        {
            if (!applying)
            {
                dirty = true;
                changed();
            }

            ShowPlaceholder();
        }

        private void ShowPlaceholder()
        {
            var empty = type switch
            {
                "text" or "file" => string.IsNullOrEmpty(text?.Text),
                "choice" => selection is null,
                "password" => !HasValue,
                _ => false
            };
            placeholder.Visibility = empty && placeholder.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        }

        public void SetRecording(bool active)
        {
            recording = active;
            if (shortcut is null)
                return;

            var title = active ? "Press a key…  Esc cancels" : shortcutLabel;
            if (!Equals(shortcut.Content, title))
                shortcut.Content = title;
            AutomationProperties.SetHelpText(shortcut, active ? "Press a key. Escape cancels." : detail.Text);
        }

        public void ClearSecret()
        {
            if (secret is null)
                return;

            secret.Clear();
            reveal?.Clear();
            if (revealed)
                ShowSecret(false);
        }

        public void ClearDraft()
        {
            ClearSecret();
            if (!clearOnSubmit)
                return;
            text?.Clear();
            if (combo is not null || radios is not null)
            {
                applying = true;
                selection = null;
                SyncSelection();
                applying = false;
                ShowPlaceholder();
                changed();
            }
            dirty = true;
        }

        public void Detach()
        {
            if (owner.capturing == this)
                owner.StopCapture();
            ClearSecret();
            if (placeholder.Parent is Panel panel)
                panel.Children.Remove(placeholder);
        }
    }
}
