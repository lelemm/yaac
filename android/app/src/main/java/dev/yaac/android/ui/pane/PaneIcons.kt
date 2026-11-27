package dev.yaac.android.ui.pane

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import dev.yaac.android.ui.svgPath

/** The side pane's own line icons (24-unit grid, as `ui/Icons.kt`), kept here so the pane is self-contained. */
enum class PaneGlyph(val path: String) {
    Terminal("M4 17l6-6-6-6 M12 19h8"),
    Folder("M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z"),
    File("M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z M14 2v4a2 2 0 0 0 2 2h4"),
    Diff("M12 3v14 M5 10h14 M5 21h14"),
    Globe("M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z M2 12h20 M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"),
    Chat("M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"),
    Bell("M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9 M10.3 21a1.94 1.94 0 0 0 3.4 0"),
    Search("M11 3a8 8 0 1 0 0 16a8 8 0 1 0 0-16z M21 21l-4.3-4.3"),
    Eraser("M7 21l-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21 M22 21H7 M5 11l9 9"),
    Reload("M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8 M21 3v5h-5 M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16 M8 16H3v5"),
    Clipboard("M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2 M9 2h6a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z"),
    AddToComposer("M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7 M15 3h6v6 M10 14L21 3"),
    Expand("M15 3h6v6 M9 21H3v-6 M21 3l-7 7 M3 21l7-7"),
    Restore("M4 14h6v6 M20 10h-6V4 M14 10l7-7 M3 21l7-7"),
    Up("M18 15l-6-6-6 6"),
    Down("M6 9l6 6 6-6"),
    Close("M18 6L6 18 M6 6l12 12"),
    More("M5 12h.01 M12 12h.01 M19 12h.01"),
    Split("M12 3v18 M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"),
    Rows("M3 5h18 M3 12h18 M3 19h18"),
}

@Composable
fun PaneIcon(glyph: PaneGlyph, tint: Color, modifier: Modifier = Modifier, size: Dp = 18.dp, strokeWidth: Float = 2f) {
    Canvas(modifier.size(size)) {
        val path = svgPath(glyph.path)
        val s = this.size.minDimension / 24f
        scale(s, s, pivot = Offset.Zero) { drawPath(path, tint, style = Stroke(width = strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round)) }
    }
}

fun glyphOf(icon: TabIcon): PaneGlyph = when (icon) {
    TabIcon.Chat -> PaneGlyph.Chat
    TabIcon.Files -> PaneGlyph.Folder
    TabIcon.File -> PaneGlyph.File
    TabIcon.Diff -> PaneGlyph.Diff
    TabIcon.Terminal -> PaneGlyph.Terminal
    TabIcon.Browser -> PaneGlyph.Globe
}
