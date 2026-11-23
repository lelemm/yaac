using System.Text.Json;
using System.Windows;
using System.Windows.Controls;

namespace Yaac.WpfPreview;

public partial class MainWindow
{
    private sealed class PullRequestForm : StackPanel
    {
        private const string Id = "pullRequest";
        private readonly ComboBox target = new() { Margin = new Thickness(0, 2, 0, 8) };
        private readonly TextBox title = new() { Margin = new Thickness(0, 2, 0, 8) };
        private readonly TextBox description = new()
        {
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
            MinHeight = 100,
            Margin = new Thickness(0, 2, 0, 8),
        };
        private readonly CheckBox draft = new() { Content = "Draft", VerticalAlignment = VerticalAlignment.Center };
        private readonly Button ask = new() { Content = "Ask the agent to write it", Margin = new Thickness(12, 0, 0, 0) };
        private readonly TextBlock agentStatus = StatusLine("YaacMuted");
        private readonly TextBlock progress = StatusLine("YaacMuted");
        private readonly TextBlock notice = StatusLine("YaacMuted");
        private readonly TextBlock warning = StatusLine("YaacWarning");
        private readonly TextBlock error = StatusLine("YaacFailure");
        private string? branchOptions;
        private bool rendering;
        private ulong editRevision;

        public bool Draft => draft.IsChecked == true;

        public PullRequestForm(MainWindow window, string instance)
        {
            var options = new StackPanel { Orientation = Orientation.Horizontal };
            options.Children.Add(draft);
            options.Children.Add(ask);
            Children.Add(progress);
            Children.Add(new TextBlock { Text = "Into" });
            Children.Add(target);
            Children.Add(new TextBlock { Text = "Title" });
            Children.Add(title);
            Children.Add(new TextBlock { Text = "Description" });
            Children.Add(description);
            Children.Add(options);
            Children.Add(agentStatus);
            Children.Add(notice);
            Children.Add(warning);
            Children.Add(error);
            ToolTipService.SetShowOnDisabled(ask, true);
            target.SelectionChanged += (_, _) =>
            {
                if (!rendering && target.SelectedItem is ComboBoxItem { Tag: string branch })
                    window.Send("dialog.change", new { id = Id, instance, values = new { @base = branch } });
            };
            title.TextChanged += (_, _) => SendText();
            description.TextChanged += (_, _) => SendText();
            ask.Click += (_, _) => window.Send("dialog.askAgent", new { id = Id, instance });

            void SendText()
            {
                if (!rendering)
                    window.Send("dialog.change", new
                    {
                        id = Id,
                        instance,
                        editRevision = ++editRevision,
                        values = new { title = title.Text, body = description.Text },
                    });
            }
        }

        public void Render(JsonElement data)
        {
            rendering = true;
            var branches = data.GetProperty("branches");
            var signature = branches.GetRawText();
            if (signature != branchOptions)
            {
                // The checkbox belongs to the native form until the user submits.
                if (branchOptions == null)
                    draft.IsChecked = Flag(data, "draft");
                branchOptions = signature;
                target.Items.Clear();
                foreach (var branch in branches.EnumerateArray())
                    target.Items.Add(new ComboBoxItem { Tag = Text(branch, "id"), Content = Text(branch, "label") });
            }
            var selected = Text(data, "base");
            target.SelectedItem = target.Items.OfType<ComboBoxItem>().FirstOrDefault(item => Equals(item.Tag, selected));
            // Earlier controller snapshots may arrive while the user is still typing.
            if (data.GetProperty("editRevision").GetUInt64() >= editRevision)
            {
                SetText(title, Text(data, "prTitle"));
                SetText(description, Text(data, "description"));
            }
            var creating = Flag(data, "creating");
            var writing = Flag(data, "writing");
            title.IsEnabled = description.IsEnabled = !creating && !writing;
            draft.IsEnabled = !creating;
            target.IsEnabled = !creating && target.HasItems;
            var askBlocked = Text(data, "askBlocked");
            ask.IsEnabled = askBlocked.Length == 0;
            ask.ToolTip = ask.IsEnabled
                ? "Sends this thread’s agent a message asking for a title and description. This uses a turn."
                : askBlocked;
            agentStatus.Foreground = Palette.Brush(Flag(data, "agentFailed") ? "YaacFailure" : "YaacMuted");
            SetStatus(agentStatus, Text(data, "agentStatus"));
            SetStatus(progress, Flag(data, "loading") ? "Reading the branch…" : "");
            SetStatus(notice, Text(data, "notice"));
            SetStatus(warning, Text(data, "warning"));
            SetStatus(error, Text(data, "error"));
            rendering = false;
        }

        private static void SetText(TextBox box, string value)
        {
            if (box.Text != value)
                box.Text = value;
        }

        private static TextBlock StatusLine(string color) => new()
        {
            TextWrapping = TextWrapping.Wrap,
            Margin = new Thickness(0, 6, 0, 2),
            Foreground = Palette.Brush(color),
        };

        private static void SetStatus(TextBlock block, string text)
        {
            block.Text = text;
            block.Visibility = text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        }
    }
}
