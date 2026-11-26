package dev.yaac.android.ui

import android.annotation.SuppressLint
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.yaac.android.core.get
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonObject

/** A base64 PNG (as the controller sends previews and the image viewer) as a bitmap, or null. */
fun decodePng(base64: String?): ImageBitmap? = base64?.let {
    runCatching {
        val bytes = android.util.Base64.decode(it, android.util.Base64.DEFAULT)
        android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
    }.getOrNull()
}

/**
 * An image in the transcript (A26, WPF `TranscriptImage`): asks for its preview once shown
 * (`request`), draws it, and offers Open (`action`, the full-screen viewer).
 */
@Composable
fun ImagePart(part: JsonObject, submit: (ActionItem) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val preview = part["preview"].obj()
    val png = preview?.get("png").text
    val bitmap = remember(png) { decodePng(png) }
    val request = ActionItem.of(part["request"])
    LaunchedEffect(request?.id) { if (png == null && preview?.get("error").text == null && request?.enabled == true) submit(request) }
    val open = ActionItem.of(part["action"])
    Column(Modifier.padding(vertical = 4.dp)) {
        if (bitmap != null) Image(bitmap, part["label"].text, Modifier.fillMaxWidth().heightIn(max = 320.dp).clip(RoundedCornerShape(6.dp))
            .tapAndHold({ open?.takeIf { it.enabled }?.let(submit) }, null), contentScale = ContentScale.Fit, alignment = Alignment.CenterStart)
        else T(preview?.get("error").text ?: "Loading ${part["label"].text ?: "image"}…", type.small, if (preview?.get("error").text != null) c.failure else c.muted)
        open?.let { a -> TextAction(a.label, a.enabled, style = type.small) { submit(a) } }
    }
}

/** The controller's image viewer (`imageViewer` {id, name, png, error}): full screen, pinch to zoom. */
@Composable
fun ImageViewer(frame: JsonObject, onClose: () -> Unit) {
    val c = LocalYaac.current
    val bitmap = remember(frame["png"].text) { decodePng(frame["png"].text) }
    var scale by remember { mutableFloatStateOf(1f) }
    var pan by remember { mutableStateOf(Offset.Zero) }
    val state = rememberTransformableState { zoom, move, _ -> scale = (scale * zoom).coerceIn(1f, 8f); pan += move }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(c.canvas)) {
            if (bitmap != null) Image(bitmap, frame["name"].text, Modifier.fillMaxSize().transformable(state)
                .graphicsLayer(scaleX = scale, scaleY = scale, translationX = pan.x, translationY = pan.y), contentScale = ContentScale.Fit)
            else T(frame["error"].text ?: "Loading…", LocalType.current.ui, c.muted, modifier = Modifier.align(Alignment.Center))
            Row(Modifier.fillMaxWidth().statusBarsPadding().padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(YIcon.Close, "Close", onClick = onClose)
                T(frame["name"].str, LocalType.current.heading, maxLines = 1)
            }
        }
    }
}

/**
 * A mermaid diagram, drawn by the bundled mermaid (the desktop draws it with the same library).
 * The page is local and offline: no network, no file access beyond the bundled script.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun MermaidBlock(source: String) {
    val c = LocalYaac.current
    var height by remember(source) { mutableFloatStateOf(0f) }
    var failed by remember(source) { mutableStateOf<String?>(null) }
    val bg = String.format("#%06X", c.canvas.value.toLong().shr(32).toInt() and 0xFFFFFF)
    val html = remember(source, c.dark) {
        val escaped = source.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
        """<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1">
<style>html,body{margin:0;background:transparent}#d{display:flex}svg{max-width:100%;height:auto}</style>
<script src="mermaid.min.js"></script></head><body><pre class="mermaid" id="d">$escaped</pre>
<script>mermaid.initialize({startOnLoad:false,theme:'${if (c.dark) "dark" else "default"}',securityLevel:'strict'});
mermaid.run({nodes:[document.getElementById('d')]}).then(function(){yaac.height(document.body.scrollHeight)}).catch(function(e){yaac.failed(String(e))});</script></body></html>"""
    }
    if (failed != null) { CodeBlock(source, source, "mermaid"); return }
    val density = androidx.compose.ui.platform.LocalDensity.current.density
    AndroidView(
        factory = { ctx ->
            WebView(ctx).apply {
                setBackgroundColor(android.graphics.Color.TRANSPARENT)
                settings.javaScriptEnabled = true
                settings.blockNetworkLoads = true
                settings.allowFileAccess = true // only file:///android_asset/mermaid/
                settings.allowContentAccess = false
                isVerticalScrollBarEnabled = false
                addJavascriptInterface(object {
                    @JavascriptInterface fun height(px: Int) { post { height = px.toFloat() } }
                    @JavascriptInterface fun failed(message: String) { post { failed = message } }
                }, "yaac")
            }
        },
        update = { web -> if (web.tag != html) { web.tag = html; web.loadDataWithBaseURL("file:///android_asset/mermaid/", html, "text/html", "utf-8", null) } },
        modifier = Modifier.fillMaxWidth().height(if (height > 0) (height + 8).dp else 160.dp).clip(RoundedCornerShape(6.dp)),
    )
    @Suppress("UNUSED_VARIABLE") val unused = bg to density
}
