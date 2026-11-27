package dev.yaac.android.core

import kotlinx.serialization.json.JsonElement

/**
 * A settings action that asks first (`confirm: { title, text, ok }` in the controller's view,
 * e.g. About → Update YAAC on the PC): the app shows a dialog and sends the action only on [ok].
 */
data class ActionConfirm(val title: String, val text: String, val ok: String) {
    companion object {
        fun parse(action: JsonElement?): ActionConfirm? {
            val c = action["confirm"].obj ?: return null
            val title = c["title"].text ?: return null
            return ActionConfirm(title, c["text"].str, c["ok"].text ?: "OK")
        }
    }
}
