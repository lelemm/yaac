package dev.yaac.android.ui.transcript

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.T
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import kotlin.math.abs
import kotlin.math.roundToInt

/** The track's inset from the top and bottom of the transcript. */
private val TrackInset = 24.dp
/** The free strip left of the transcript where the timeline is held open; the transcript is padded by it. */
val TimelineGutter = 26.dp
/** The edge zone a hold must start in: the gutter and a little of the margin beside it. */
private val EdgeZone = 30.dp

/** The scrub timeline's live state, shared by the gesture (on the parent) and the overlay. */
class TimelineState {
    var open by mutableStateOf(false)
    var active by mutableIntStateOf(-1)
    var fingerY by mutableFloatStateOf(0f)
    var height by mutableFloatStateOf(0f)
}

@Composable
fun rememberTimelineState(): TimelineState = remember { TimelineState() }

/**
 * The phone's prompt timeline (A11f), the gesture half: put it on the transcript's container.
 * Nothing happens until a finger rests [TimelineModel.HOLD_MS] at the left edge without moving.
 * It watches the initial pass and consumes nothing until then, so taps, scrolls and text selection
 * below are untouched; once open it owns the gesture until the finger lifts, then [jump]s.
 */
fun Modifier.timelineGestures(state: TimelineState, prompts: () -> List<JsonObject>, jump: (String) -> Unit, haptic: (Boolean) -> Unit): Modifier =
    this.onSizeChanged { state.height = it.height.toFloat() }.pointerInput(Unit) {
        val inset = TrackInset.toPx()
        val edge = EdgeZone.toPx()
        val slop = viewConfiguration.touchSlop
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            if (prompts().size < 2 || !TimelineModel.inEdge(down.position.x, edge)) return@awaitEachGesture
            // The hold: give up if the finger lifts or moves before the time is up.
            var gaveUp = false
            withTimeoutOrNull(TimelineModel.HOLD_MS) {
                while (!gaveUp) {
                    val ch = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id }
                    if (ch == null || !ch.pressed || (ch.position - down.position).getDistance() > slop) gaveUp = true
                }
            }
            if (gaveUp) return@awaitEachGesture
            val track = { state.height - 2 * inset }
            state.fingerY = down.position.y
            state.active = TimelineModel.indexAt(state.fingerY, inset, track(), prompts().size)
            state.open = true
            haptic(true)
            while (true) {
                val e = awaitPointerEvent(PointerEventPass.Initial)
                val ch = e.changes.firstOrNull { it.id == down.id }
                e.changes.forEach { it.consume() }
                if (ch == null || !ch.pressed) break
                state.fingerY = ch.position.y
                val i = TimelineModel.indexAt(state.fingerY, inset, track(), prompts().size)
                if (i != state.active) { state.active = i; haptic(false) }
            }
            prompts().getOrNull(state.active)?.let { jump(it["id"].str) }
            state.open = false
            state.active = -1
        }
    }

/** The drawing half: the scrim, the swelling dashes and the prompt under the finger. No input. */
@Composable
fun TimelineOverlay(state: TimelineState, prompts: List<JsonObject>) {
    val c = LocalYaac.current
    val type = LocalType.current
    val density = LocalDensity.current
    val reveal = remember { Animatable(0f) }
    LaunchedEffect(state.open) { reveal.animateTo(if (state.open) 1f else 0f, tween(if (state.open) 180 else 140)) }
    if (reveal.value <= 0f) return
    val inset = with(density) { TrackInset.toPx() }
    val count = prompts.size
    val track = state.height - 2 * inset
    val active = state.active
    // One animated position: the dashes swell around it, so the column ripples as the finger moves.
    val pos by animateFloatAsState(active.toFloat(), spring(stiffness = 500f), label = "timeline")
    Box(Modifier.fillMaxSize().alpha(reveal.value * 0.9f).background(c.canvas))
    val dashes = TimelineModel.visibleDashes(count, track, with(density) { 6.dp.toPx() }, active)
    Canvas(Modifier.fillMaxSize().alpha(reveal.value)) {
        val left = 10.dp.toPx()
        for (i in dashes) {
            val s = TimelineModel.swellAt(i, pos)
            val y = TimelineModel.yOf(i, inset, track, count)
            val len = 10.dp.toPx() + 26.dp.toPx() * s
            val color = if (i == active) c.text else c.muted.copy(alpha = 0.55f + 0.45f * s)
            drawLine(color, Offset(left, y), Offset(left + len, y), strokeWidth = (2.dp + 2.dp * s).toPx(), cap = StrokeCap.Round)
        }
    }
    prompts.getOrNull(active)?.let { p ->
        val cardTop = (state.fingerY - with(density) { 36.dp.toPx() })
            .coerceIn(0f, (state.height - with(density) { 140.dp.toPx() }).coerceAtLeast(0f))
        Column(
            Modifier.offset { IntOffset(56.dp.roundToPx(), cardTop.roundToInt()) }
                .padding(end = 16.dp).fillMaxWidth().alpha(reveal.value)
                .clip(RoundedCornerShape(8.dp)).background(c.raised).padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            T("${active + 1} of $count" + (p["stamp"].text?.let { " · $it" } ?: ""), type.small, c.muted)
            T(p["title"].str, type.ui, c.text, maxLines = 4)
        }
    }
}
