package dev.yaac.android.ui.pane

/**
 * The composer's saved caret (D05): Add to composer inserts the excerpt where the caret was when
 * the composer was last edited or moved, as WPF passes `ComposerPane.CaretIndex`. The composer
 * reports its caret to the controller with `composer.edit` / `composer.caret` {key, caret};
 * [observe] sees every command on its way out and keeps the last caret per composer key.
 * The caret is in UTF-16 units; the controller converts it (`scalar_caret`) and clamps it.
 */
object ComposerCaret {
    private val carets = HashMap<String, Int>()
    @Volatile private var lastKey: String? = null

    @Synchronized
    fun observe(command: String, payload: Any?) {
        if (command != "composer.edit" && command != "composer.caret") return
        val p = payload as? Map<*, *> ?: return
        val key = p["key"] as? String ?: return
        val caret = (p["caret"] as? Number)?.toInt() ?: return
        carets[key] = caret
        lastKey = key
    }

    /** The caret of composer [key], else of the composer used last; null sends none (the controller appends). */
    @Synchronized
    fun caret(key: String?): Int? = key?.let { carets[it] } ?: lastKey?.takeIf { key == null }?.let { carets[it] }

    @Synchronized
    internal fun reset() { carets.clear(); lastKey = null }
}
