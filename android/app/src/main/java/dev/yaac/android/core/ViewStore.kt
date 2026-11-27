package dev.yaac.android.core

import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * One-shot effects a frame carries beside its views (the WPF renderer handles them in
 * `MainWindow.Receive`).
 */
sealed interface FrameEffect {
    data class Copy(val text: String) : FrameEffect
    data class Error(val text: String) : FrameEffect
    data class Window(val command: String) : FrameEffect
    data class FilesDocument(val frame: JsonObject) : FrameEffect
    data class Terminal(val frame: JsonObject) : FrameEffect
    data class ImageViewer(val frame: JsonObject) : FrameEffect
    data class FilesChange(val frame: JsonObject) : FrameEffect
    data class PasteConfirm(val frame: JsonObject) : FrameEffect
    /** `terminalReadPaste` {tab, panel}: the shell's paste key; read the clipboard and send `terminal.paste`. */
    data class ReadPaste(val frame: JsonObject) : FrameEffect
    /** `terminalNotice`: a quiet notice ("Copied by the terminal"). */
    data class TerminalNotice(val text: String) : FrameEffect
}

/**
 * The views the controller has sent, merged exactly as WPF merges them:
 *
 * - each key under `views` replaces that whole view (`shell`, `sidebar`, `composer`, …);
 * - except `transcript`, whose rows may come as `rowChanges` (`{rows: changed, order?}`): the
 *   changed rows replace those with the same id, and `order`, when present, is the new order
 *   (rows not in it are dropped). A transcript without `prompts` keeps the previous index.
 *
 * Parsing and merging run on the transport's thread; the result is published to the main thread,
 * and only views that actually changed are replaced, so Compose recomposes only on change.
 */
/** What one frame changes: whole views to replace, and one-shot effects. */
class FrameUpdate(val views: List<Pair<String, JsonElement?>>, val effects: List<FrameEffect>, val stopped: String?)

/**
 * Merges frames exactly as WPF does (pure, so it can be tested off-device):
 *
 * - each key under `views` replaces that whole view (`shell`, `sidebar`, `composer`, …);
 * - except `transcript`, whose rows may come as `rowChanges` (`{rows: changed, order?}`): the
 *   changed rows replace those with the same id, and `order`, when present, is the new order
 *   (rows not in it are dropped). A transcript without `prompts` keeps the previous index.
 *   The merged transcript always carries the full `rows`.
 */
class FrameMerger {
    private var rows = LinkedHashMap<String, JsonElement>()
    private var order: List<String> = emptyList()
    private var prompts: JsonElement? = null
    private var transcriptThread: String? = null
    private val last = HashMap<String, JsonElement?>()

    fun merge(line: String): FrameUpdate? {
        val frame = try { FrameJson.parseToJsonElement(line) as? JsonObject } catch (_: Exception) { null } ?: return null
        val updates = ArrayList<Pair<String, JsonElement?>>()
        val fx = ArrayList<FrameEffect>()
        frame["copy"].text?.let { fx += FrameEffect.Copy(it) }
        frame["error"].text?.let { fx += FrameEffect.Error(it) }
        frame["window"].text?.let { fx += FrameEffect.Window(it) }
        (frame["filesDocument"] as? JsonObject)?.let { fx += FrameEffect.FilesDocument(it) }
        (frame["terminalFrame"] as? JsonObject)?.let { fx += FrameEffect.Terminal(it) }
        (frame["imageViewer"] as? JsonObject)?.let { fx += FrameEffect.ImageViewer(it) }
        (frame["filesChange"] as? JsonObject)?.let { fx += FrameEffect.FilesChange(it) }
        (frame["terminalPasteConfirm"] as? JsonObject)?.let { fx += FrameEffect.PasteConfirm(it) }
        // A terminal's Copy or a program's OSC 52 write: the same clipboard write as `copy`.
        frame["terminalCopy"].text?.let { fx += FrameEffect.Copy(it) }
        frame["terminalNotice"].text?.let { fx += FrameEffect.TerminalNotice(it) }
        (frame["terminalReadPaste"] as? JsonObject)?.let { fx += FrameEffect.ReadPaste(it) }
        // The native controller's last frame: `{"stopped": {"error": …}}`.
        val stopped = (frame["stopped"] as? JsonObject)?.let { it["error"].text ?: "" }
        (frame["views"] as? JsonObject)?.forEach { (name, value) ->
            val next = if (name == "transcript") transcript(value) else value.takeUnless { it is JsonNull }
            // Unchanged views are dropped here, on the transport's thread, so the UI thread
            // never compares large trees (invariant 7).
            if (!last.containsKey(name) || last[name] != next) { last[name] = next; updates += name to next }
        }
        if (updates.isEmpty() && fx.isEmpty() && stopped == null) return null
        return FrameUpdate(updates, fx, stopped)
    }

    private fun transcript(view: JsonElement): JsonElement? {
        val v = view as? JsonObject ?: return null
        val thread = v["threadId"].text
        val changes = v["rowChanges"] as? JsonObject
        if (changes == null || thread != transcriptThread) {
            rows = LinkedHashMap()
            v["rows"].list.forEachIndexed { i, row ->
                val id = row["id"].text ?: "~row:$i"
                rows.putIfAbsent(id, row)
            }
            order = rows.keys.toList()
        } else {
            changes["rows"].list.forEach { row -> row["id"].text?.let { rows[it] = row } }
            val next = changes["order"]
            if (next is JsonArray) {
                order = next.mapNotNull { it.text }
                rows.keys.retainAll(order.toHashSet())
            }
        }
        if (v.containsKey("prompts") || thread != transcriptThread) prompts = v["prompts"]
        transcriptThread = thread
        val merged = LinkedHashMap<String, JsonElement>(v)
        merged.remove("rowChanges")
        merged["rows"] = JsonArray(order.mapNotNull { rows[it] })
        merged["prompts"] = prompts ?: JsonNull
        return JsonObject(merged)
    }
}

/**
 * The views the controller has sent, as Compose state. Frames are merged on the transport's
 * thread ([FrameMerger]) and published to the main thread; only views that actually changed are
 * replaced, so Compose recomposes only on change (invariant 7).
 */
@Stable
class ViewStore {
    private val views = HashMap<String, androidx.compose.runtime.MutableState<JsonElement?>>()
    private val main = Handler(Looper.getMainLooper())
    private val merger = FrameMerger()
    private data class LocalEdit(val key: String?, val textKey: String?, val revision: Long, val text: String) {
        fun matches(view: JsonElement?) = key == view["key"].text && textKey == view["textKey"].text
            && revision == view["textRevision"].long
    }
    private var localEdit: LocalEdit? = null

    var status by mutableStateOf<String?>("Starting…")
    /** Set when the controller ended (`{"stopped": {"error"}}`): the error, or "" for none. */
    var stopped by mutableStateOf<String?>(null)
    var effects: (FrameEffect) -> Unit = {}
    /**
     * Called on the main thread after each update is published, whether or not an activity is
     * open: the voice call ([CallManager]) follows its frames with the screen off.
     */
    val listeners = java.util.concurrent.CopyOnWriteArrayList<(FrameUpdate) -> Unit>()

    private fun slot(name: String) = views.getOrPut(name) { mutableStateOf(null) }

    /** The latest value of one view, observed by Compose. Main thread only. */
    fun view(name: String): JsonElement? {
        val value = slot(name).value
        if (name != "composer" || value !is JsonObject) return value
        val edit = localEdit ?: return value
        if (!edit.matches(value)) return value
        // The editor can be recreated while the controller is omitting its text. Seed the
        // new field with the latest local edit. Keep the original frame's "text" for echoes.
        return JsonObject(value + ("localText" to JsonPrimitive(edit.text)))
    }

    internal fun rememberComposerEdit(payload: JsonElement) {
        if (!payload["local"].flag()) return
        val revision = payload["textRevision"].long ?: return
        val text = payload["text"]?.str ?: return
        localEdit = LocalEdit(payload["key"].text, payload["textKey"].text, revision, text)
    }

    /** Development: every frame line is appended here when `log.tag.yaac-dump` is DEBUG. */
    @Volatile var dump: java.io.File? = null

    /** Called on the transport's thread for every frame line. */
    fun ingest(line: String) {
        dump?.runCatching { appendText(line + "\n") }
        val update = merger.merge(line) ?: return
        if (android.util.Log.isLoggable("yaac-frames", android.util.Log.DEBUG))
            android.util.Log.d("yaac-frames", update.views.joinToString(",") { it.first } + " " + line.length)
        main.post {
            for ((name, value) in update.views) {
                if (name == "composer" && localEdit?.matches(value) == false) localEdit = null
                slot(name).value = value
            }
            update.stopped?.let { stopped = it; status = null }
            update.effects.forEach(effects)
            listeners.forEach { it(update) }
        }
    }
}

/** Sends the commands WPF sends: `{"command": name, "payload": …}`. */
class Controller(private val transport: FrameTransport, val store: ViewStore) {
    fun start() = transport.start(store::ingest) { s -> Handler(Looper.getMainLooper()).post { store.status = s } }
    fun stop() = transport.stop()

    fun send(command: String, payload: Any? = null) {
        if (android.util.Log.isLoggable("yaac-cmd", android.util.Log.DEBUG)) android.util.Log.d("yaac-cmd", command)
        val body = toJson(payload)
        if (command == "composer.edit") store.rememberComposerEdit(body)
        val line = JsonObject(mapOf("command" to JsonPrimitive(command), "payload" to body))
        transport.send(line.toString())
    }
}
