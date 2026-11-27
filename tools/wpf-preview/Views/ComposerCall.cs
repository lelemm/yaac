using System.Text.Json;
using System.Windows;
using System.Windows.Media;

namespace Yaac.WpfPreview.Views;

public partial class ComposerView
{
    private void RenderCall(JsonElement call)
    {
        var desktop = Field(call, "desktop");
        CallButton.Visibility = VisibleWhen(!shellMode && cancelEditAction is null && Field(desktop, "phone").ComposerFlag());
        CallButton.IsEnabled = editingEnabled && Field(desktop, "canStart").ComposerFlag();

        var consent = Field(desktop, "consent").String();
        var error = Field(desktop, "error").String();
        var reason = Field(desktop, "reason").String();
        var status = Field(desktop, "status").String();
        var clock = Field(desktop, "clock").String();
        if (clock.Length > 0)
            status += $" · {clock}";
        var message = new[] { consent, error, reason, status }.FirstOrDefault(text => text.Length > 0) ?? "";
        CallMessage.Foreground = (Brush)FindResource(error.Length > 0 ? "YaacFailure" : "YaacMuted");
        DisplayText(CallMessage, message);

        CallConfirm.Visibility = CallCancel.Visibility = VisibleWhen(consent.Length > 0);
        var mute = Field(desktop, "mute").String();
        CallMute.Content = mute;
        CallMute.Visibility = VisibleWhen(mute.Length > 0);
        CallEnd.Visibility = VisibleWhen(Field(desktop, "end").ComposerFlag());

        var transcript = Items(Field(call, "transcript")).TakeLast(3).Select(line =>
        {
            var role = Field(line, "role").String() == "user" ? "You" : "Agent";
            return $"{role} · {Field(line, "text").String()}";
        });
        var text = CallEnd.Visibility == Visibility.Visible ? string.Join(Environment.NewLine, transcript) : "";
        DisplayText(CallTranscript, text);
        CallBar.Visibility = VisibleWhen(message.Length > 0 || text.Length > 0);
    }

    private void CallClicked(object sender, RoutedEventArgs e) =>
        Command?.Invoke("composer.desktopCallAsk", new { key });

    private void CallConfirmed(object sender, RoutedEventArgs e) =>
        Command?.Invoke("composer.desktopCallConsent", new { key, accepted = true });

    private void CallCancelled(object sender, RoutedEventArgs e) =>
        Command?.Invoke("composer.desktopCallConsent", new { key, accepted = false });

    private void CallMuted(object sender, RoutedEventArgs e) =>
        Command?.Invoke("composer.desktopCallMute", new { key });

    private void CallEnded(object sender, RoutedEventArgs e) =>
        Command?.Invoke("composer.callEnd", new { key });
}
