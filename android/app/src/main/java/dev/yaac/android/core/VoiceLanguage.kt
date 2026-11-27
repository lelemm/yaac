package dev.yaac.android.core

import android.content.Context
import java.util.Locale

/*
 * The language of voice calls and dictation on this phone (docs/plan/voice-call.md § Language).
 * The choice is this phone's own (Settings → Voice call language, kept in SharedPreferences,
 * never in the hub): "" follows the phone's language. It goes with `composer.callStart` and
 * `composer.voiceStart` as `language`, a BCP-47 tag; the controller passes it on.
 */
object VoiceLanguage {
    private const val PREFS = "yaac.voice"
    private const val KEY = "language"

    /** What Settings offers, as BCP-47 tags with their own names (as the desktop's list). */
    val choices: List<Pair<String, String>> = listOf(
        "en-US" to "English (United States)",
        "en-GB" to "English (United Kingdom)",
        "pt-BR" to "Português (Brasil)",
        "pt-PT" to "Português (Portugal)",
        "es-ES" to "Español (España)",
        "es-419" to "Español (Latinoamérica)",
        "fr-FR" to "Français",
        "de-DE" to "Deutsch",
        "it-IT" to "Italiano",
        "nl-NL" to "Nederlands",
        "pl-PL" to "Polski",
        "ru-RU" to "Русский",
        "uk-UA" to "Українська",
        "tr-TR" to "Türkçe",
        "ar" to "العربية",
        "hi-IN" to "हिन्दी",
        "ja-JP" to "日本語",
        "ko-KR" to "한국어",
        "zh-CN" to "中文（简体）",
        "zh-TW" to "中文（繁體）",
    )

    const val SAME_AS_DEVICE = "Same as this device"

    /** [locale] as a BCP-47 tag (`pt-BR`), or null for the root or an undetermined locale. */
    fun bcp47(locale: Locale): String? {
        val tag = locale.toLanguageTag()
        return tag.takeIf { it.isNotEmpty() && it != "und" && !it.startsWith("und-") }
    }

    /** The language to send: the stored [choice], else the phone's [device] locale. */
    fun effective(choice: String, device: Locale): String? = choice.ifEmpty { null } ?: bcp47(device)

    /** The label Settings shows for [choice] ("" is "Same as this device (…)"). */
    fun label(choice: String, device: Locale): String {
        if (choice.isEmpty()) {
            val own = bcp47(device)?.let { tag -> choices.firstOrNull { it.first == tag }?.second ?: device.getDisplayName(device).ifEmpty { tag } }
            return if (own != null) "$SAME_AS_DEVICE ($own)" else SAME_AS_DEVICE
        }
        return choices.firstOrNull { it.first == choice }?.second ?: choice
    }

    fun choice(context: Context): String = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "") ?: ""

    fun choose(context: Context, tag: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, tag).apply()
    }

    /** What to send now from [context]: the choice or the phone's language. */
    fun current(context: Context): String? = effective(choice(context), Locale.getDefault())
}
