package dev.yaac.android.ui.transcript

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.graphics.Canvas
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.yaac.android.core.PlatformBridge
import dev.yaac.android.core.get
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.CodeBlock
import dev.yaac.android.ui.IconButton
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.T
import dev.yaac.android.ui.TextAction
import dev.yaac.android.ui.YIcon
import dev.yaac.android.ui.decodePng
import kotlinx.serialization.json.JsonObject
import java.io.ByteArrayOutputStream
import java.io.File

/**
 * The controller's image viewer (`imageViewer` {id, name, png, error}, T14): full screen,
 * pinch to zoom about the fingers, drag to pan within the picture, double tap to zoom in or
 * back to fit, and − / % / + / **Reset** at the bottom.
 */
@Composable
fun ZoomImageViewer(frame: JsonObject, onClose: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val bitmap = remember(frame["png"].text) { decodePng(frame["png"].text) }
    var zoom by remember(frame["id"].text) { mutableStateOf(Zoom()) }
    var size by remember { mutableStateOf(Vec(1f, 1f)) }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(c.canvas)) {
            if (bitmap != null) {
                Image(bitmap, frame["name"].text, contentScale = ContentScale.Fit, modifier = Modifier.fillMaxSize()
                    .onSizeChanged { size = Vec(it.width.toFloat(), it.height.toFloat()) }
                    .pointerInput(Unit) { detectTapGestures(onDoubleTap = { p -> zoom = zoom.toggle(size, Vec(p.x, p.y)) }) }
                    .pointerInput(Unit) {
                        detectTransformGestures { centroid, pan, factor, _ ->
                            zoom = zoom.transform(size, Vec(centroid.x, centroid.y), factor, Vec(pan.x, pan.y))
                        }
                    }
                    .graphicsLayer(scaleX = zoom.scale, scaleY = zoom.scale, translationX = zoom.pan.x, translationY = zoom.pan.y))
            } else {
                T(frame["error"].text ?: "Loading…", type.ui, if (frame["error"].text != null) c.failure else c.muted, modifier = Modifier.align(Alignment.Center))
            }
            Row(Modifier.fillMaxWidth().statusBarsPadding().background(c.canvas.copy(alpha = 0.8f)).padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(YIcon.Close, "Close", onClick = onClose)
                T(frame["name"].str, type.heading, maxLines = 1)
            }
            if (bitmap != null) {
                Row(Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 12.dp).clip(RoundedCornerShape(20.dp))
                    .background(c.raised).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    TextAction("−", enabled = zoom.scale > Zoom.MIN) { zoom = zoom.step(size, zoomIn = false) }
                    T("${zoom.percent}%", type.small, c.muted)
                    TextAction("+", enabled = zoom.scale < Zoom.MAX) { zoom = zoom.step(size, zoomIn = true) }
                    TextAction("Reset", enabled = !zoom.fitted) { zoom = Zoom() }
                }
            }
        }
    }
}

/** Saves [bytes] through the document picker, off the main thread (the picker blocks it). */
private fun save(cache: File, extension: String, bytes: ByteArray) {
    Thread {
        try {
            val staged = DiagramFiles.stage(cache, extension, bytes)
            PlatformBridge.saveFile(DiagramFiles.name(extension), DiagramFiles.filter(extension), arrayOf(extension), staged.absolutePath)
            staged.parentFile?.deleteRecursively()
        } catch (e: Exception) {
            PlatformBridge.message("The diagram was not saved", e.message ?: e.toString())
        }
    }.start()
}

/**
 * A mermaid diagram (T15), drawn by the bundled mermaid (the desktop draws it with the same
 * library), with **Save PNG**, **Save SVG** and **Save source**. The page is local and offline:
 * no network, no file access beyond the bundled script.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun MermaidDiagram(source: String) {
    val c = LocalYaac.current
    val type = LocalType.current
    val cache = LocalContext.current.cacheDir
    var height by remember(source) { mutableFloatStateOf(0f) }
    var failed by remember(source) { mutableStateOf<String?>(null) }
    var svg by remember(source) { mutableStateOf<String?>(null) }
    val web = remember(source) { arrayOfNulls<WebView>(1) }
    val html = remember(source, c.dark) {
        val escaped = source.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
        """<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1">
<style>html,body{margin:0;background:transparent}#d{display:flex}svg{max-width:100%;height:auto}</style>
<script src="mermaid.min.js"></script></head><body><pre class="mermaid" id="d">$escaped</pre>
<script>mermaid.initialize({startOnLoad:false,theme:'${if (c.dark) "dark" else "default"}',securityLevel:'strict'});
mermaid.run({nodes:[document.getElementById('d')]}).then(function(){var s=document.querySelector('#d svg');if(s)yaac.svg(new XMLSerializer().serializeToString(s));yaac.height(document.body.scrollHeight)}).catch(function(e){yaac.failed(String(e))});</script></body></html>"""
    }
    // The WebView loads only on request: a transcript must never depend on it (a broken
    // system WebView aborts the whole process natively, which nothing can catch).
    var shown by remember(source) { mutableStateOf(false) }
    val sourceBytes = { source.toByteArray(Charsets.UTF_8) }
    if (failed != null || !shown) {
        CodeBlock(source, source, "mermaid")
        DisableSelection {
            failed?.let { T("The diagram could not be drawn · $it", type.small, c.failure, maxLines = 3) }
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (failed == null) TextAction("Show diagram", style = type.small) { shown = true }
                TextAction("Save source", style = type.small) { save(cache, "mmd", sourceBytes()) }
            }
        }
        return
    }
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
                    @JavascriptInterface fun svg(markup: String) { post { svg = markup } }
                }, "yaac")
                web[0] = this
            }
        },
        update = { view -> if (view.tag != html) { view.tag = html; view.loadDataWithBaseURL("file:///android_asset/mermaid/", html, "text/html", "utf-8", null) } },
        modifier = Modifier.fillMaxWidth().height(if (height > 0) (height + 8).dp else 160.dp).clip(RoundedCornerShape(6.dp)),
    )
    DisableSelection {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val drawn = svg != null
            TextAction("Save PNG", enabled = drawn, style = type.small) {
                val view = web[0] ?: return@TextAction
                if (view.width <= 0 || view.height <= 0) return@TextAction
                // The drawn view on the transcript's own background (the page is transparent).
                val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
                Canvas(bitmap).apply { drawColor(c.canvas.toArgb()); view.draw(this) }
                val bytes = ByteArrayOutputStream().use { out -> bitmap.compress(Bitmap.CompressFormat.PNG, 100, out); out.toByteArray() }
                bitmap.recycle()
                save(cache, "png", bytes)
            }
            TextAction("Save SVG", enabled = drawn, style = type.small) {
                DiagramFiles.svgDocument(svg)?.let { save(cache, "svg", it.toByteArray(Charsets.UTF_8)) }
            }
            TextAction("Save source", style = type.small) { save(cache, "mmd", sourceBytes()) }
        }
    }
}
