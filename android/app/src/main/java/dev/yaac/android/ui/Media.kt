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

/** The controller's image viewer (`imageViewer` {id, name, png, error}): see [ZoomImageViewer]. */
@Composable
fun ImageViewer(frame: JsonObject, onClose: () -> Unit) = dev.yaac.android.ui.transcript.ZoomImageViewer(frame, onClose)

/** A mermaid diagram with its Save PNG / SVG / source: see [dev.yaac.android.ui.transcript.MermaidDiagram]. */
@Composable
fun MermaidBlock(source: String) = dev.yaac.android.ui.transcript.MermaidDiagram(source)
