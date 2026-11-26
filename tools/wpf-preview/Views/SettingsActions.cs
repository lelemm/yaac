using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;

namespace Yaac.WpfPreview.Views;

public partial class SettingsView
{
    private sealed class ActionControl
    {
        private readonly SettingsView owner;
        private readonly Action<ActionControl, bool?, Dictionary<string, object?>> invoke;
        private readonly Dictionary<string, MenuEntry> children = new(StringComparer.Ordinal);
        private JsonElement initialValues;
        private ButtonBase control = null!;
        private string? kind;
        private bool enabled;
        private bool applicable = true;
        private bool checkedValue;
        private string detail = "";
        private string copyText = "";

        public ContentControl Root { get; } = new()
        {
            Margin = new Thickness(0, 2, 6, 4),
            VerticalAlignment = VerticalAlignment.Center,
            Focusable = false
        };

        public string Id { get; private set; } = "";
        public bool Default { get; private set; }
        public bool Cancel { get; private set; }
        public bool Submit { get; private set; }
        public bool Clear { get; private set; }
        public string CopyField { get; private set; } = "";

        public ActionControl(SettingsView owner, Action<ActionControl, bool?, Dictionary<string, object?>> invoke)
        {
            this.owner = owner;
            this.invoke = invoke;
        }

        public void Apply(JsonElement action)
        {
            Id = Str(action, "id");
            Default = Flag(action, "default");
            Cancel = Flag(action, "cancel");
            Submit = Flag(action, "submit");
            Clear = Flag(action, "clear");
            enabled = Id.Length > 0 && Flag(action, "enabled");
            checkedValue = Flag(action, "checked");
            initialValues = SnapshotValues(action);
            copyText = Str(action, "text");
            CopyField = Str(action, "copyField");
            var label = Str(action, "label", Id);
            detail = Str(action, "detail");
            var newKind = Str(action, "kind");
            if (Items(action, "children").Any())
                newKind = "menu";

            if (kind != newKind)
                Create(newKind);

            if (control is ToggleButton toggle && toggle.IsChecked != checkedValue)
                toggle.IsChecked = checkedValue;
            var text = kind == "menu" ? label + "  ▾" : label;
            if (!Equals(control.Content, text))
                control.Content = text;

            AutomationProperties.SetName(control, label);
            Describe(control, detail);
            if (control.ContextMenu is { } menu)
                SyncMenu(menu.Items, children, Items(action, "children"), owner.SendAction);

            Validate(applicable);
        }

        private void Create(string newKind)
        {
            kind = newKind;
            children.Clear();
            if (kind == "toggle")
            {
                control = new CheckBox
                {
                    Style = (Style)owner.FindResource("YaacSwitch"),
                    VerticalContentAlignment = VerticalAlignment.Center,
                    MinHeight = 28
                };
            }
            else
            {
                var button = new Button { Padding = new Thickness(10, 4, 10, 4), MinHeight = 28 };
                if (kind == "primary")
                    button.Style = (Style)owner.FindResource("YaacPrimaryButton");
                else if (kind == "link")
                    button.Style = (Style)owner.FindResource("YaacLinkButton");
                else if (kind == "menu")
                {
                    button.ContextMenu = DropdownPlacement.CreateMenu(button);
                }

                control = button;
            }

            control.Click += (_, _) => Activate();
            Root.Content = control;
        }

        public void Validate(bool valid)
        {
            applicable = valid;
            Root.IsEnabled = enabled && valid;
        }

        public void Invoke()
        {
            if (!Root.IsEnabled)
                return;
            if (control is ToggleButton toggle)
                toggle.IsChecked = !checkedValue;
            Activate();
        }

        private void Activate()
        {
            if (!Root.IsEnabled)
                return;
            if (control.ContextMenu is { } menu)
            {
                menu.IsOpen = true;
                return;
            }

            if (kind == "copy" && copyText.Length > 0)
            {
                try
                {
                    Clipboard.SetText(copyText);
                }
                catch (System.Runtime.InteropServices.ExternalException)
                {
                    owner.SendAction(Id, ReadValues(initialValues));
                }
                return;
            }

            var check = control is ToggleButton toggle ? toggle.IsChecked : null;
            invoke(this, check, ReadValues(initialValues));
        }
    }

    private sealed class MenuEntry
    {
        private readonly Action<string, Dictionary<string, object?>?> send;
        private readonly Dictionary<string, MenuEntry> children = new(StringComparer.Ordinal);
        private string id = "";
        private JsonElement values;

        public MenuItem Root { get; } = new();

        public MenuEntry(Action<string, Dictionary<string, object?>?> send)
        {
            this.send = send;
            Root.Click += (_, args) =>
            {
                if (args.Source != Root || Root.HasItems || !Root.IsEnabled)
                    return;
                var payload = ReadValues(values);
                if (Root.IsCheckable)
                    payload["checked"] = Root.IsChecked;
                this.send(id, payload);
                args.Handled = true;
            };
        }

        public void Apply(JsonElement item)
        {
            id = Str(item, "id");
            var label = Str(item, "label", id);
            Root.IsEnabled = id.Length > 0 && Flag(item, "enabled");
            var check = Prop(item, "checked");
            Root.IsCheckable = check.ValueKind is JsonValueKind.True or JsonValueKind.False;
            Root.IsChecked = check.ValueKind == JsonValueKind.True;
            if (!Equals(Root.Header, label))
                Root.Header = label;
            values = SnapshotValues(item);
            Describe(Root, Str(item, "detail"));
            SyncMenu(Root.Items, children, Items(item, "children"), send);
        }
    }

    private static JsonElement SnapshotValues(JsonElement json)
    {
        var values = Prop(json, "values");
        return values.ValueKind == JsonValueKind.Object ? values.Clone() : default;
    }

    private static Dictionary<string, object?> ReadValues(JsonElement json)
    {
        var values = new Dictionary<string, object?>(StringComparer.Ordinal);
        if (json.ValueKind == JsonValueKind.Object)
            foreach (var pair in json.EnumerateObject())
                values[pair.Name] = pair.Value.Clone();
        return values;
    }

    private static void SyncMenu(ItemCollection host, Dictionary<string, MenuEntry> saved,
        IEnumerable<JsonElement> items, Action<string, Dictionary<string, object?>?> send)
    {
        var present = new HashSet<string>(StringComparer.Ordinal);
        var index = 0;
        foreach (var item in items)
        {
            var id = Str(item, "id");
            if (id.Length == 0 || !present.Add(id))
                continue;
            if (!saved.TryGetValue(id, out var entry))
            {
                entry = new MenuEntry(send);
                saved.Add(id, entry);
            }

            entry.Apply(item);
            if (index >= host.Count || host[index] != entry.Root)
            {
                if (host.Contains(entry.Root))
                    host.Remove(entry.Root);
                host.Insert(index, entry.Root);
            }
            index++;
        }

        foreach (var id in saved.Keys.Where(id => !present.Contains(id)).ToArray())
        {
            host.Remove(saved[id].Root);
            saved.Remove(id);
        }
    }
}
