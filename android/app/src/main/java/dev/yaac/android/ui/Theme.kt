package dev.yaac.android.ui

import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.text.selection.LocalTextSelectionColors
import androidx.compose.foundation.text.selection.TextSelectionColors
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import dev.yaac.android.R
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.long
import kotlinx.serialization.json.JsonElement

/** The palette tokens, by the names the WPF renderer uses (`tools/wpf-preview/Palette.cs`). */
private val Tokens: List<Triple<String, Long, Long>> = listOf(
    Triple("YaacCanvas", 0x16181D, 0xFFFFFF),
    Triple("YaacPanel", 0x1B1E24, 0xF5F6F8),
    Triple("YaacCard", 0x22262E, 0xF7F8FA),
    Triple("YaacRaised", 0x22262E, 0xF7F8FA),
    Triple("YaacInput", 0x1E2128, 0xF3F4F7),
    Triple("YaacRule", 0x2E333D, 0xDADDE3),
    Triple("YaacText", 0xD8DADE, 0x1A1D23),
    Triple("YaacMuted", 0x9AA2AF, 0x5B6472),
    Triple("YaacAccent", 0xE6E8EC, 0x1A1D23),
    Triple("YaacOnAccent", 0x16181D, 0xFFFFFF),
    Triple("YaacLink", 0xE6E8EC, 0x1A1D23),
    Triple("YaacSelected", 0x2A2E37, 0xE6E8EC),
    Triple("YaacHover", 0x1E2128, 0xF3F4F7),
    Triple("YaacUnread", 0x6495ED, 0x4A7FE0),
    Triple("YaacWarning", 0xF2B24C, 0x8F5600),
    Triple("YaacFailure", 0xFF7C87, 0xBF2636),
    Triple("YaacSuccess", 0x4CC38A, 0x17754D),
    Triple("YaacUser", 0x2D333E, 0xE3E7EE),
    Triple("YaacCodeKeyword", 0xC68CFF, 0x8A2BC2),
    Triple("YaacCodeString", 0x9ECE6A, 0x2E7D32),
    Triple("YaacCodeComment", 0x7E9C6E, 0x4E7A3A),
    Triple("YaacCodeNumber", 0xF09A5C, 0xB35000),
    Triple("YaacCodeType", 0x6CC4FF, 0x1A5FB4),
    Triple("YaacDiffAdded", 0x1B322C, 0xE7F5EB),
    Triple("YaacDiffRemoved", 0x39252D, 0xFBEAEC),
    Triple("YaacFast", 0xF5C518, 0x9A7000),
    Triple("YaacUltrafast", 0x5BA8FF, 0x1764BA),
    Triple("YaacShell", 0x9DBF7A, 0x4A6E2C),
    Triple("YaacFocus", 0xC9CED6, 0x3A4050),
)

@Immutable
class YaacColors(val dark: Boolean, private val values: Map<String, Color>) {
    operator fun get(name: String): Color = values[name] ?: Color.Magenta
    val canvas get() = this["YaacCanvas"]
    val panel get() = this["YaacPanel"]
    val card get() = this["YaacCard"]
    val raised get() = this["YaacRaised"]
    val input get() = this["YaacInput"]
    val rule get() = this["YaacRule"]
    val text get() = this["YaacText"]
    val muted get() = this["YaacMuted"]
    val accent get() = this["YaacAccent"]
    val onAccent get() = this["YaacOnAccent"]
    val selected get() = this["YaacSelected"]
    val unread get() = this["YaacUnread"]
    val warning get() = this["YaacWarning"]
    val failure get() = this["YaacFailure"]
    val success get() = this["YaacSuccess"]
    val user get() = this["YaacUser"]
    val focus get() = this["YaacFocus"]

    /** The colour of a tone word the controller sends (`warning`, `error`, `success`, `muted`). */
    fun tone(name: String?, fallback: Color): Color = when (name) {
        "warning" -> warning
        "error", "failed", "failure" -> failure
        "success", "done" -> success
        "muted" -> muted
        else -> fallback
    }

    companion object {
        /** As WPF's `Palette.SetTheme`: `custom` reads its own `dark` flag and per-token RGB. */
        fun of(theme: String?, custom: JsonElement?, systemDark: Boolean): YaacColors {
            val c = custom.takeIf { theme == "custom" && it is kotlinx.serialization.json.JsonObject }
            val dark = when {
                c != null -> c["dark"].flag(true)
                theme == "light" -> false
                theme == "system" -> systemDark
                else -> true
            }
            val map = Tokens.associate { (name, d, l) ->
                val rgb = c?.get(name).long ?: if (dark) d else l
                name to Color(0xFF000000 or (rgb and 0xFFFFFF))
            }
            return YaacColors(dark, map)
        }
    }
}

val LocalYaac = staticCompositionLocalOf { YaacColors.of("dark", null, true) }

val UiFont = FontFamily(
    Font(R.font.yaac_ui_regular, FontWeight.Normal),
    Font(R.font.yaac_ui_bold, FontWeight.Bold),
    Font(R.font.yaac_ui_bold, FontWeight.SemiBold),
)
val MonoFont = FontFamily(Font(R.font.yaac_mono, FontWeight.Normal))

@Immutable
class YaacType(scale: Float) {
    val ui = TextStyle(fontFamily = UiFont, fontSize = (14 * scale).sp, lineHeight = (19 * scale).sp)
    val small = TextStyle(fontFamily = UiFont, fontSize = (12 * scale).sp, lineHeight = (16 * scale).sp)
    val prose = TextStyle(fontFamily = UiFont, fontSize = (16 * scale).sp, lineHeight = (23 * scale).sp)
    val title = TextStyle(fontFamily = UiFont, fontSize = (17 * scale).sp, lineHeight = (22 * scale).sp, fontWeight = FontWeight.SemiBold)
    val heading = TextStyle(fontFamily = UiFont, fontSize = (15 * scale).sp, lineHeight = (20 * scale).sp, fontWeight = FontWeight.SemiBold)
    val mono = TextStyle(fontFamily = MonoFont, fontSize = (13 * scale).sp, lineHeight = (18 * scale).sp)
    val monoSmall = TextStyle(fontFamily = MonoFont, fontSize = (12 * scale).sp, lineHeight = (16 * scale).sp)
}

val LocalType = staticCompositionLocalOf { YaacType(1f) }

@Composable
fun YaacTheme(shell: JsonElement?, content: @Composable () -> Unit) {
    val systemDark = isSystemInDarkTheme()
    val theme = shell["theme"]
    val colors = YaacColors.of((theme as? kotlinx.serialization.json.JsonPrimitive)?.content, shell["customPalette"], systemDark)
    val scale = (shell["fontScale"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toFloatOrNull()?.coerceIn(0.7f, 2f) ?: 1f
    val type = YaacType(scale)
    // Material components are used only for their mechanics; every colour is a YAAC token,
    // with no accent and no elevation tint.
    val base: ColorScheme = if (colors.dark) darkColorScheme() else lightColorScheme()
    val scheme = base.copy(
        primary = colors.accent, onPrimary = colors.onAccent, secondary = colors.muted,
        background = colors.canvas, onBackground = colors.text, surface = colors.panel, onSurface = colors.text,
        surfaceVariant = colors.input, onSurfaceVariant = colors.muted, surfaceTint = Color.Transparent,
        surfaceContainerLow = colors.panel, surfaceContainer = colors.raised, surfaceContainerHigh = colors.raised,
        surfaceContainerHighest = colors.raised, surfaceContainerLowest = colors.canvas,
        outline = colors.rule, outlineVariant = colors.rule, error = colors.failure, scrim = Color.Black.copy(alpha = 0.5f),
    )
    // The system bars' icons follow the theme: light icons on dark, dark icons on light.
    val activity = androidx.compose.ui.platform.LocalContext.current as? androidx.activity.ComponentActivity
    androidx.compose.runtime.LaunchedEffect(colors.dark) {
        val t = android.graphics.Color.TRANSPARENT
        val style = if (colors.dark) androidx.activity.SystemBarStyle.dark(t) else androidx.activity.SystemBarStyle.light(t, t)
        activity?.enableEdgeToEdge(style, style)
    }
    MaterialTheme(colorScheme = scheme) {
        CompositionLocalProvider(
            LocalYaac provides colors,
            LocalType provides type,
            LocalContentColor provides colors.text,
            LocalTextSelectionColors provides TextSelectionColors(colors.focus, colors["YaacSelected"]),
            content = content,
        )
    }
}
