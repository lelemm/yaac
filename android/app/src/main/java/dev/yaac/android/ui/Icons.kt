package dev.yaac.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.core.LinearEasing
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import java.util.concurrent.ConcurrentHashMap

/**
 * The app's one small icon set: line icons on a 24-unit grid, drawn as vector strokes at the
 * text's size. Harness logos are separate ([Logo]): they come from the controller as SVG paths.
 */
enum class YIcon(val path: String, val filled: Boolean = false) {
    Back("M19 12H5 M12 19l-7-7 7-7"),
    Search("M11 3a8 8 0 1 0 0 16a8 8 0 1 0 0-16z M21 21l-4.3-4.3"),
    Sort("M21 16l-4 4-4-4 M17 20V4 M3 8l4-4 4 4 M7 4v16"),
    Plus("M5 12h14 M12 5v14"),
    Command("M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3"),
    More("M12 5.5a.5.5 0 1 0 0 .01 M12 12a.5.5 0 1 0 0 .01 M12 18.5a.5.5 0 1 0 0 .01"),
    Settings("M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z M12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6z"),
    Check("M20 6L9 17l-5-5"),
    Close("M18 6L6 18 M6 6l12 12"),
    Play("M7 4.5l12 7.5-12 7.5z", filled = true),
    PlayLine("M7 4.5l12 7.5-12 7.5z"),
    Pause("M7 5h3v14H7z M14 5h3v14h-3z", filled = true),
    Hourglass("M5 22h14 M5 2h14 M17 22v-4.17a2 2 0 0 0-.59-1.42L12 12l-4.41 4.41A2 2 0 0 0 7 17.83V22 M7 2v4.17a2 2 0 0 0 .59 1.42L12 12l4.41-4.41A2 2 0 0 0 17 6.17V2"),
    Branch("M6 3v12 M18 9a3 3 0 1 0 0-6a3 3 0 1 0 0 6z M6 21a3 3 0 1 0 0-6a3 3 0 1 0 0 6z M18 9a9 9 0 0 1-9 9"),
    Users("M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2 M9 3a4 4 0 1 0 0 8a4 4 0 1 0 0-8z M22 21v-2a4 4 0 0 0-3-3.87 M16 3.13a4 4 0 0 1 0 7.75"),
    ChevronRight("M9 18l6-6-6-6"),
    ChevronDown("M6 9l6 6 6-6"),
    CaretRight("M9 6l7 6-7 6z", filled = true),
    CaretDown("M6 9l6 7 6-7z", filled = true),
    Hub("M18 2a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M6 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M18 16a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M8.59 13.51l6.83 3.98 M15.41 6.51l-6.82 3.98"),
    Unlink("M18.84 12.25l1.72-1.71a5 5 0 0 0-7.07-7.07l-1.72 1.71 M5.17 11.75l-1.71 1.71a5 5 0 0 0 7.07 7.07l1.71-1.71 M8 2v3 M2 8h3 M16 22v-3 M22 16h-3"),
    File("M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z M14 2v4a2 2 0 0 0 2 2h4 M8 13h8 M8 17h6"),
    Image("M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z M9 7a2 2 0 1 0 0 4a2 2 0 1 0 0-4z M21 15l-3.1-3.1a2 2 0 0 0-2.8 0L6 21"),
    ArrowDown("M12 5v14 M19 12l-7 7-7-7"),
    ArrowUp("M12 19V5 M5 12l7-7 7 7"),
    CirclePlus("M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z M8 12h8 M12 8v8"),
    Copy("M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H10a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2z M4 16a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2"),
    Bolt("M13 2L3 14h9l-1 8 10-12h-9l1-8z"),
    Lock("M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z M7 11V7a5 5 0 0 1 10 0v4"),
    Warning("M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z M12 9v4 M12 17h.01"),
    Tree("M12 2a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M5 16a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M19 16a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M12 8v4 M5 16v-2a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v2"),
    History("M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8 M3 3v5h5 M12 7v5l4 2"),
    Grip("M9 5.5a.6.6 0 1 0 0 .01 M15 5.5a.6.6 0 1 0 0 .01 M9 12a.6.6 0 1 0 0 .01 M15 12a.6.6 0 1 0 0 .01 M9 18.5a.6.6 0 1 0 0 .01 M15 18.5a.6.6 0 1 0 0 .01"),
    BoltFilled("M13 2L3 14h9l-1 8 10-12h-9l1-8z", filled = true),
    Brain("M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18z M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18z M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4 M17.599 6.5a3 3 0 0 0 .399-1.375 M6.003 5.125A3 3 0 0 0 6.401 6.5 M3.477 10.896a4 4 0 0 1 .585-.396 M19.938 10.5a4 4 0 0 1 .585.396 M6 18a4 4 0 0 1-1.967-.516 M19.967 17.484A4 4 0 0 1 18 18"),
    Bot("M12 8V4H8 M6 8h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2z M2 14h2 M20 14h2 M15 13v2 M9 13v2"),
    Shield("M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"),
    Fork("M6 3a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M18 3a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M12 15a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9 M12 12v3"),
    Spinner("M12 2v4 M16.2 7.8l2.9-2.9 M18 12h4 M16.2 16.2l2.9 2.9 M12 18v4 M4.9 19.1l2.9-2.9 M2 12h4 M4.9 4.9l2.9 2.9"),
}

private val parsed = ConcurrentHashMap<String, Path>()

fun svgPath(d: String): Path = parsed.getOrPut(d) { PathParser().parsePathString(d).toPath() }

@Composable
fun Icon(icon: YIcon, tint: Color, modifier: Modifier = Modifier, size: Dp = 18.dp, strokeWidth: Float = 2f) {
    Canvas(modifier.size(size)) {
        val path = svgPath(icon.path)
        val s = this.size.minDimension / 24f
        scale(s, s, pivot = Offset.Zero) {
            if (icon.filled) drawPath(path, tint, style = Fill)
            else drawPath(path, tint, style = Stroke(width = strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}

/** A harness logo: an SVG path from the frame, fitted into a square and filled with [tint]. */
@Composable
fun Logo(d: String, tint: Color, modifier: Modifier = Modifier, size: Dp = 16.dp) {
    val path = remember(d) { runCatching { svgPath(d) }.getOrNull() } ?: return
    Canvas(modifier.size(size)) {
        val b = path.getBounds()
        if (b.width <= 0f || b.height <= 0f) return@Canvas
        val s = minOf(this.size.width / b.width, this.size.height / b.height)
        val dx = (this.size.width - b.width * s) / 2f
        val dy = (this.size.height - b.height * s) / 2f
        translate(dx, dy) {
            scale(s, s, pivot = Offset.Zero) {
                translate(-b.left, -b.top) { drawPath(path, tint) }
            }
        }
    }
}


/** The running mark, as WPF's `RunningSpinner`: an open arc turning once every 2.5 s (1.8 s in Fast, 0.35 s in Ultrafast). */
@Composable
fun RunningSpinner(tint: Color, size: Dp = 16.dp, fast: Boolean = false, ultrafast: Boolean = false) {
    val period = if (ultrafast) 350 else if (fast) 1_800 else 2_500
    val turn = rememberInfiniteTransition(label = "running")
    val angle by turn.animateFloat(0f, 360f, infiniteRepeatable(tween<Float>(period, easing = LinearEasing)), label = "angle")
    val arc = remember { svgPath("M15.5 5.5A7 7 0 1 1 7 2.3") }
    Canvas(Modifier.size(size)) {
        // The arc's circle is centred at (9.37, 8.89) with radius 7: fit that circle, then turn it.
        val s = (this.size.minDimension - 2.dp.toPx()) / 16f
        rotate(angle) {
            translate(center.x - 9.37f * s, center.y - 8.89f * s) {
                scale(s, s, pivot = Offset.Zero) {
                    drawPath(arc, tint, style = Stroke(width = 1.8.dp.toPx() / s, cap = StrokeCap.Round))
                }
            }
        }
    }
}
