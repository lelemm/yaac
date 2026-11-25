using System.ComponentModel;
using System.Text.Json;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public partial class FilesTree : UserControl
{
    internal event Action<string, object>? Command;

    private static readonly Node[] LoadingChildren = [new("", "Loading…", false)];
    private bool rendering;
    private IReadOnlyList<Node> roots = [];
    private string owner = "";

    public FilesTree()
    {
        InitializeComponent();
        Tree.AddHandler(TreeViewItem.ExpandedEvent, new RoutedEventHandler(Expansion));
        Tree.AddHandler(TreeViewItem.CollapsedEvent, new RoutedEventHandler(Expansion));
    }

    internal void Render(JsonElement state, string thread)
    {
        rendering = true;
        try
        {
            if (thread != owner) roots = [];
            owner = thread;
            var folders = state.Value("folders");
            var next = UpdateFolder(roots, folders, state.Value("errors"), "");
            if (!ReferenceEquals(roots, next) || !ReferenceEquals(Tree.ItemsSource, next))
                Tree.ItemsSource = next;
            roots = next;
        }
        finally
        {
            rendering = false;
        }
    }

    private static IReadOnlyList<Node> UpdateFolder(
        IReadOnlyList<Node> previous, JsonElement folders, JsonElement errors, string path)
    {
        if (folders.Value(path) is not { ValueKind: JsonValueKind.Array } entries)
        {
            if (path.Length == 0) return [];
            if (errors.Value(path) is not { ValueKind: JsonValueKind.String } error) return LoadingChildren;
            var message = error.GetString()!;
            return previous.Count == 1 && previous[0].Name == message ? previous : [new("", message, false)];
        }

        var existing = previous.ToDictionary(node => node.Path, StringComparer.Ordinal);
        var next = new List<Node>(entries.GetArrayLength());
        foreach (var entry in entries.EnumerateArray())
        {
            var name = entry.GetProperty("name").GetString()!;
            var entryPath = path.Length == 0 ? name : path + "/" + name;
            var directory = entry.Value("directory").ValueKind == JsonValueKind.True;
            if (!existing.TryGetValue(entryPath, out var node) || node.Directory != directory)
                node = new Node(entryPath, name, directory);

            var expanded = directory && entry.Value("expanded").ValueKind == JsonValueKind.True;
            IReadOnlyList<Node> children = expanded
                ? UpdateFolder(node.Children, folders, errors, entryPath)
                : directory ? LoadingChildren : [];
            node.Update(entry, expanded, children);
            next.Add(node);
        }

        return previous.SequenceEqual(next) ? previous : next;
    }

    private void Expansion(object sender, RoutedEventArgs e)
    {
        if (rendering || e.OriginalSource is not TreeViewItem { DataContext: Node { Directory: true } node } item)
            return;
        if (node.Expanded != item.IsExpanded)
            Command?.Invoke("expand", new { path = node.Path, expanded = item.IsExpanded });
    }

    private void FolderRowClicked(object sender, MouseButtonEventArgs e)
    {
        if (sender is Border { TemplatedParent: TreeViewItem { DataContext: Node { Directory: true } node } item })
        {
            item.IsSelected = true;
            Command?.Invoke("expand", new { path = node.Path, expanded = !node.Expanded });
            e.Handled = true;
        }
    }

    private void OpenName(object sender, RoutedEventArgs e) => Open(sender, diff: false, duplicate: false);
    private void OpenMark(object sender, RoutedEventArgs e) => Open(sender, diff: true, duplicate: false);
    private void MiddleName(object sender, MouseButtonEventArgs e) => MiddleOpen(sender, e, diff: false);
    private void MiddleMark(object sender, MouseButtonEventArgs e) => MiddleOpen(sender, e, diff: true);

    private void MiddleOpen(object sender, MouseButtonEventArgs e, bool diff)
    {
        if (e.ChangedButton != MouseButton.Middle) return;
        Open(sender, diff, duplicate: true);
        e.Handled = true;
    }

    private void Open(object sender, bool diff, bool duplicate)
    {
        if (sender is not Button { DataContext: Node node } || node.Path.Length == 0) return;
        for (DependencyObject? parent = (Button)sender; parent != null; parent = VisualTreeHelper.GetParent(parent))
        {
            if (parent is not TreeViewItem item) continue;
            item.IsSelected = true;
            break;
        }
        if (node.Directory)
        {
            Command?.Invoke("expand", new { path = node.Path, expanded = !node.Expanded });
        }
        else
        {
            Command?.Invoke(diff ? "diff" : "open", new { path = node.Path, duplicate });
        }
    }

    private sealed class Node(string path, string name, bool directory) : INotifyPropertyChanged
    {
        public string Path { get; } = path;
        public string Name { get; } = name;
        public bool Directory { get; } = directory;
        public bool CanOpen => Path.Length > 0;
        public bool ChangedFolder { get; private set; }
        public bool Expanded { get; private set; }
        public bool Ignored { get; private set; }
        public string Mark { get; private set; } = "";
        public Visibility MarkVisibility => Directory || Mark.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        public Brush MarkBrush => Palette.Brush(Ignored ? "YaacMuted" : Mark switch
        {
            "A" or "U" => "YaacSuccess",
            "D" or "!" => "YaacFailure",
            "R" => "YaacUnread",
            _ => "YaacWarning",
        });
        public string MarkLabel => Mark switch
        {
            "M" => "Modified · Open diff",
            "A" => "Added · Open diff",
            "D" => "Deleted · Open diff",
            "R" => "Renamed · Open diff",
            "U" => "Untracked · Open diff",
            "!" => "Conflicted · Open diff",
            _ => "",
        };
        public IReadOnlyList<Node> Children { get; private set; } = [];

        public event PropertyChangedEventHandler? PropertyChanged;

        internal void Update(JsonElement entry, bool expanded, IReadOnlyList<Node> children)
        {
            var changed = Directory && entry.Value("changed").ValueKind == JsonValueKind.True;
            var ignored = entry.Value("ignored").ValueKind == JsonValueKind.True;
            var mark = entry.Value("mark") is { ValueKind: JsonValueKind.String } value
                ? value.GetString()! : "";
            if (ChangedFolder == changed && Expanded == expanded && Ignored == ignored && Mark == mark
                && ReferenceEquals(Children, children)) return;

            ChangedFolder = changed;
            Expanded = expanded;
            Ignored = ignored;
            Mark = mark;
            Children = children;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(null));
        }
    }
}
