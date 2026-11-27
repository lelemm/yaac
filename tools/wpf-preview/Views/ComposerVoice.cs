using System.Text.Json;
using System.Windows;
using System.Windows.Media.Animation;

namespace Yaac.WpfPreview.Views;

// Voice input (docs/plan/voice.md, docs/07-ui.md#voice-input). The controller records the
// microphone and owns the dictation; this only draws its `voice` object: a button bound to
// `voice.button` that sends `composer.voiceStart` or `composer.voiceStop`, and a quiet line with
// the words heard so far or the error. The words reach the field through `insert`, at the caret.
public partial class ComposerView
{
    private ComposerAction? voiceAction;
    private string voiceState = "";

    private void RenderVoice(JsonElement voice)
    {
        // Only when this controller records: a renderer that records itself (Android) is not WPF.
        var shown = voice.ValueKind == JsonValueKind.Object && Field(voice, "capture").String() == "controller";
        voiceAction = shown ? ReadAction(Field(voice, "button")) : null;
        SetButton(VoiceButton, voiceAction, editingEnabled && !shellMode);
        if (shellMode)
            VoiceButton.Visibility = Visibility.Collapsed;
        var state = shown ? Field(voice, "state").String() : "";
        if (state != voiceState)
        {
            voiceState = state;
            var listening = state is "starting" or "listening";
            // Listening: the button pulses until the dictation stops.
            if (listening)
            {
                var pulse = new DoubleAnimation(1, 0.55, TimeSpan.FromMilliseconds(700))
                {
                    AutoReverse = true,
                    RepeatBehavior = RepeatBehavior.Forever,
                };
                VoiceButton.BeginAnimation(OpacityProperty, pulse);
            }
            else
            {
                VoiceButton.BeginAnimation(OpacityProperty, null);
            }
        }
        var error = shown ? Field(voice, "error").String() : "";
        var status = shown ? Field(voice, "status").String() : "";
        VoiceStatus.Foreground = (System.Windows.Media.Brush)FindResource(error.Length > 0 ? "YaacFailure" : "YaacMuted");
        DisplayText(VoiceStatus, error.Length > 0 ? error : status);
    }

    private void VoiceClicked(object sender, RoutedEventArgs e)
    {
        if (voiceAction is not { Enabled: true } || !editingEnabled)
            return;
        Command?.Invoke(voiceAction.Id, new { key });
    }

    /// Esc while dictating drops the words (composer.voiceCancel); returns whether it did.
    private bool CancelVoice()
    {
        if (voiceState is not ("starting" or "listening" or "stopping"))
            return false;
        Command?.Invoke("composer.voiceCancel", new { key });
        return true;
    }
}
