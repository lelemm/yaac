package dev.yaac.android.ui.pane

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.longOrNull

/*
 * The terminal's pure logic, kept free of Compose and Android so it runs in JVM unit tests:
 * the `terminalFrame` wire (crates/yaac-app/src/wpf/terminal.rs, decoded as WPF's
 * TerminalFrame.cs does), the screen it builds up, hardware-key routing (K04), touch
 * gestures (cells, scrollback swipes) and paste.
 */

/** One run `[col, width, ink, paper, flags, text]`: `paper` -1 is the canvas; flags 1 bold, 2 italic, 4 underline, 8 strike, 16 combining, 32 double underline. */
data class TermRun(val col: Int, val width: Int, val ink: Long, val paper: Long, val flags: Int, val text: String) {
    val bold get() = flags and 1 != 0
    val italic get() = flags and 2 != 0
    val underline get() = flags and (4 or 32) != 0
    val strike get() = flags and 8 != 0
    /** Columns this run covers: wide runs take two per character (a combining cluster is one character). */
    val columns: Int get() = if (flags and 16 != 0) width else width * text.codePointCount(0, text.length)
}

/** `[col, row, shape, colour, wide, blinking]`; shape is `hidden`, `block`, `hollow`, `underline` or `bar`. */
data class TermCursor(val col: Int, val row: Int, val shape: String, val color: Long, val wide: Boolean, val blinking: Boolean) {
    val visible get() = shape != "hidden"
}

data class TermFrame(
    val tab: Long,
    val panel: Boolean,
    val serial: Long?,
    val height: Int,
    val rows: List<Pair<Int, List<TermRun>>>,
    val cursor: TermCursor?,
    val search: String?,
    val matches: Int,
    val found: Boolean,
    val mouse: Boolean,
    val selection: Boolean,
)

object TerminalWire {
    private fun JsonElement?.long(): Long? = (this as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull
    private fun JsonElement?.bool(): Boolean = (this as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull ?: false
    private fun JsonElement?.string(): String? = (this as? JsonPrimitive)?.takeIf { it.isString }?.content

    fun run(e: JsonElement?): TermRun? {
        val a = e as? JsonArray ?: return null
        if (a.size < 6) return null
        return TermRun(a[0].long()?.toInt() ?: return null, a[1].long()?.toInt() ?: 1, a[2].long() ?: 0, a[3].long() ?: -1,
            a[4].long()?.toInt() ?: 0, a[5].string() ?: "")
    }

    /** The frame, or null when it is not one (no tab). Malformed rows and runs are skipped. */
    fun decode(f: JsonObject): TermFrame? {
        val tab = f["tab"].long() ?: return null
        val rows = (f["rows"] as? JsonArray).orEmpty().mapNotNull { r ->
            val pair = r as? JsonArray ?: return@mapNotNull null
            val index = pair.getOrNull(0).long()?.toInt() ?: return@mapNotNull null
            index to (pair.getOrNull(1) as? JsonArray).orEmpty().mapNotNull(::run)
        }
        val c = f["cursor"] as? JsonArray
        val cursor = c?.let {
            TermCursor(it.getOrNull(0).long()?.toInt() ?: 0, it.getOrNull(1).long()?.toInt() ?: 0, it.getOrNull(2).string() ?: "block",
                it.getOrNull(3).long() ?: 0xFFFFFF, it.getOrNull(4).bool(), it.getOrNull(5).bool())
        }
        return TermFrame(tab, f["panel"].bool(), f["frame"].long(), f["height"].long()?.toInt() ?: 0, rows, cursor,
            f["search"].string()?.takeIf { it.isNotEmpty() }, f["matches"].long()?.toInt() ?: 0, f["found"].bool(),
            f["mouse"].bool(), f["selection"].bool())
    }

    /** WPF's search status: "No results", "N here" (matches on screen), or nothing. */
    fun searchStatus(search: String?, found: Boolean, matches: Int): String = when {
        search.isNullOrEmpty() -> ""
        !found -> "No results"
        matches > 0 -> "$matches here"
        else -> ""
    }
}

/** What a terminal shows: the rows by index, the height and the cursor, built from frames. */
data class TermScreen(
    val height: Int = 0,
    val rows: Map<Int, List<TermRun>> = emptyMap(),
    val cursor: TermCursor? = null,
    val search: String? = null,
    val matches: Int = 0,
    val found: Boolean = false,
    val mouse: Boolean = false,
    val selection: Boolean = false,
    /** Frames applied; a new screen (switch, restart) starts again from full frames. */
    val frames: Int = 0,
) {
    /** A frame replaces the rows it carries; rows past a smaller height are dropped. */
    fun apply(f: TermFrame): TermScreen {
        val next = HashMap<Int, List<TermRun>>(rows.size + f.rows.size)
        for ((i, r) in rows) if (i < f.height) next[i] = r
        for ((i, r) in f.rows) if (i < f.height) next[i] = r
        return TermScreen(f.height, next, f.cursor, f.search, f.matches, f.found, f.mouse, f.selection, frames + 1)
    }

    /** The plain text of a row, runs laid out at their columns (for tests and accessibility). */
    fun rowText(row: Int): String {
        val sb = StringBuilder()
        var col = 0
        for (r in rows[row].orEmpty()) {
            if (r.col > col) { repeat(r.col - col) { sb.append(' ') }; col = r.col }
            sb.append(r.text); col += r.columns
        }
        return sb.toString()
    }
}

/** A command bound in Settings → Keyboard (the `shortcuts` view, WPF's `Shortcut`). */
data class Shortcut(val id: String, val key: String, val ctrl: Boolean, val shift: Boolean, val alt: Boolean, val inTerminal: Boolean, val repeat: Boolean) {
    companion object {
        fun list(view: JsonElement?): List<Shortcut> = (view as? JsonArray).orEmpty().mapNotNull { e ->
            val o = e as? JsonObject ?: return@mapNotNull null
            fun b(k: String) = (o[k] as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull ?: false
            val id = (o["id"] as? JsonPrimitive)?.content ?: return@mapNotNull null
            val key = (o["key"] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content ?: return@mapNotNull null
            Shortcut(id, key, b("ctrl"), b("shift"), b("alt"), b("inTerminal"), b("repeat"))
        }
    }
}

/** Where a hardware key pressed in a focused terminal goes (K04). */
sealed interface KeyRoute {
    /** A command marked Works in the terminal: `shell.shortcut {id, inText, terminalFocus}`. */
    data class Command(val id: String) : KeyRoute
    /** To the shell: `terminal.key {key, mods}`. */
    data class Key(val name: String, val mods: Int) : KeyRoute
    /** Typed text: left to the text field, which sends `terminal.text`. */
    data object Text : KeyRoute
    /** Swallowed (a held key repeating a command that does not repeat; a lone modifier). */
    data object Consume : KeyRoute
}

/**
 * Android key codes (`android.view.KeyEvent.KEYCODE_*`, stable public constants) mapped to the
 * names the controller reads: the terminal's keys (wpf/terminal/input.rs `key`) and the key
 * bindings' canonical names (WPF's `CanonicalKey`, as Settings records them).
 */
object TermKeys {
    const val A = 29; const val Z = 54; const val D0 = 7; const val D9 = 16
    const val NUM0 = 144; const val NUM9 = 153; const val F1 = 131; const val F12 = 142
    const val ENTER = 66; const val NUM_ENTER = 160; const val TAB = 61; const val ESCAPE = 111; const val DEL = 67; const val FORWARD_DEL = 112
    const val INSERT = 124; const val HOME = 122; const val END = 123; const val PAGE_UP = 92; const val PAGE_DOWN = 93
    const val UP = 19; const val DOWN = 20; const val LEFT = 21; const val RIGHT = 22; const val SPACE = 62
    const val EQUALS = 70; const val PLUS = 81; const val MINUS = 69; const val COMMA = 55; const val PERIOD = 56; const val SLASH = 76
    const val BACKSLASH = 73; const val SEMICOLON = 74; const val APOSTROPHE = 75; const val GRAVE = 68; const val LEFT_BRACKET = 71; const val RIGHT_BRACKET = 72
    const val NUM_ADD = 157; const val NUM_SUBTRACT = 156; const val NUM_MULTIPLY = 155; const val NUM_DIVIDE = 154; const val NUM_DOT = 158
    private val MODIFIERS = setOf(57, 58, 59, 60, 113, 114, 117, 118, 115, 116, 119)

    fun mods(shift: Boolean, alt: Boolean, ctrl: Boolean, meta: Boolean) =
        (if (shift) 1 else 0) or (if (alt) 2 else 0) or (if (ctrl) 4 else 0) or (if (meta) 8 else 0)

    /** Keys the shell gets by name whatever the modifiers. */
    fun special(code: Int): String? = when (code) {
        ENTER, NUM_ENTER -> "Enter"; TAB -> "Tab"; ESCAPE -> "Escape"; DEL -> "Backspace"; FORWARD_DEL -> "Delete"
        INSERT -> "Insert"; HOME -> "Home"; END -> "End"; PAGE_UP -> "PageUp"; PAGE_DOWN -> "PageDown"
        UP -> "Up"; DOWN -> "Down"; LEFT -> "Left"; RIGHT -> "Right"
        in F1..F12 -> "F" + (code - F1 + 1)
        else -> null
    }

    /** The binding name of a key, as Settings records it. */
    fun canonical(code: Int, shift: Boolean): String? = when (code) {
        in A..Z -> ('A' + (code - A)).toString()
        in D0..D9 -> (code - D0).toString()
        in NUM0..NUM9 -> (code - NUM0).toString()
        in F1..F12 -> "F" + (code - F1 + 1)
        ESCAPE -> "Escape"; DEL -> "Backspace"; ENTER, NUM_ENTER -> "Enter"; TAB -> "Tab"; SPACE -> "Space"
        INSERT -> "Insert"; FORWARD_DEL -> "Delete"; HOME -> "Home"; END -> "End"; PAGE_UP -> "PageUp"; PAGE_DOWN -> "PageDown"
        LEFT -> "Left"; RIGHT -> "Right"; UP -> "Up"; DOWN -> "Down"
        EQUALS -> if (shift) "Plus" else "Equals"
        PLUS, NUM_ADD -> "Plus"; MINUS, NUM_SUBTRACT -> "Minus"; COMMA -> "Comma"; PERIOD, NUM_DOT -> "Period"
        SLASH, NUM_DIVIDE -> "Slash"; BACKSLASH -> "Backslash"; SEMICOLON -> if (shift) "Colon" else "Semicolon"
        APOSTROPHE -> "Quote"; GRAVE -> "Backtick"; LEFT_BRACKET -> "OpenBracket"; RIGHT_BRACKET -> "CloseBracket"
        else -> null
    }

    /** The name a chord with Ctrl or Alt sends to the shell: WPF lowercases letters without Shift; space is " ". */
    private fun chordName(code: Int, shift: Boolean, char: Char?): String? = when (code) {
        in A..Z -> ('a' + (code - A)).let { if (shift) it.uppercaseChar() else it }.toString()
        in D0..D9 -> (code - D0).toString()
        SPACE -> " "
        NUM_ADD -> "Add"; NUM_SUBTRACT -> "Subtract"; NUM_MULTIPLY -> "Multiply"; NUM_DIVIDE -> "Divide"; NUM_DOT -> "Decimal"
        in NUM0..NUM9 -> "NumPad" + (code - NUM0)
        else -> char?.takeIf { !it.isISOControl() }?.toString()
    }

    /**
     * A key pressed while the terminal has focus. Bindings marked Works in the terminal run their
     * command; every other key reaches the shell (WPF `InvokeShortcut` with `TerminalFocused`).
     * [char] is the key's character without Ctrl/Alt (for punctuation chords).
     */
    fun route(code: Int, char: Char?, shift: Boolean, alt: Boolean, ctrl: Boolean, meta: Boolean, repeat: Boolean, shortcuts: List<Shortcut>): KeyRoute {
        if (code in MODIFIERS) return KeyRoute.Consume
        canonical(code, shift)?.let { name ->
            val hit = shortcuts.firstOrNull { it.key == name && it.ctrl == ctrl && it.alt == alt && it.shift == shift }
                ?: if (shift) shortcuts.firstOrNull { it.key == name && it.ctrl == ctrl && it.alt == alt && !it.shift } else null
            if (hit != null && hit.inTerminal) return if (repeat && !hit.repeat) KeyRoute.Consume else KeyRoute.Command(hit.id)
        }
        val mods = mods(shift, alt, ctrl, meta)
        special(code)?.let { return KeyRoute.Key(it, mods) }
        if (ctrl || alt || meta) return chordName(code, shift, char)?.let { KeyRoute.Key(it, mods) } ?: KeyRoute.Consume
        return KeyRoute.Text
    }
}

/** A touch on the grid, as the cell under it and whether it is on the cell's right half (WPF `MouseAction`). */
data class Cell(val col: Int, val row: Int, val right: Boolean)

object TermGestures {
    fun cellAt(x: Float, y: Float, cellWidth: Float, cellHeight: Float, cols: Int, rows: Int): Cell {
        val fx = x / cellWidth.coerceAtLeast(1f)
        val col = fx.toInt().coerceIn(0, (cols - 1).coerceAtLeast(0))
        val row = (y / cellHeight.coerceAtLeast(1f)).toInt().coerceIn(0, (rows - 1).coerceAtLeast(0))
        return Cell(col, row, fx - col >= .5f)
    }

    /** The grid size that fits a pane, as `terminal.resize` takes it (the controller clamps to 2–1000 × 1–500). */
    fun fit(widthPx: Float, heightPx: Float, cellWidth: Float, cellHeight: Float): Pair<Int, Int> =
        (widthPx / cellWidth.coerceAtLeast(1f)).toInt().coerceIn(2, 1000) to (heightPx / cellHeight.coerceAtLeast(1f)).toInt().coerceIn(1, 500)
}

/**
 * Scrollback by swipe (E05): vertical drag distance becomes wheel notches, one per line height.
 * A finger moving down shows older output (wheel "Up"), as on every touch screen.
 */
class WheelAccumulator(private val lineHeight: Float) {
    private var pending = 0f
    /** The notches for a drag of [dy] pixels: positive is "Up" (older), negative "Down". */
    fun drag(dy: Float): Int {
        pending += dy
        val step = lineHeight.coerceAtLeast(1f)
        val notches = (pending / step).toInt()
        pending -= notches * step
        return notches
    }
    fun reset() { pending = 0f }
}

/** Paste (E07): what the controller is asked, or why nothing is sent. */
sealed interface PasteOutcome {
    data class Send(val text: String) : PasteOutcome
    /** Android gave no text: the clipboard is empty, holds no text, or reading it was refused. */
    data class Notice(val text: String) : PasteOutcome
}

object TermPaste {
    /** The line count the controller states in its confirmation (terminal/tabs.rs `terminal.paste`). */
    fun lines(text: String): Int = text.replace("\r\n", "\n").split('\n', '\r').size

    fun outcome(clip: String?, readFailed: Boolean): PasteOutcome = when {
        readFailed -> PasteOutcome.Notice("Android refused to read the clipboard.")
        clip.isNullOrEmpty() -> PasteOutcome.Notice("The clipboard holds no text.")
        else -> PasteOutcome.Send(clip)
    }

    /** The confirmation's sentence, as docs/13 words it. */
    fun question(lines: Int) = "Paste $lines lines? They will run."
}
