package dev.yaac.android.core

/**
 * Whether the call bar's transcript keeps its newest line in view. It follows until the user
 * scrolls up, and follows again once they scroll back to the bottom. A change of the content's
 * height (a line grew, a line came) never stops following: only a move of the scroll position
 * while the height stayed the same is the user's.
 */
object CallFollow {
    /** A scroll position: [value] of [max], both in pixels. */
    data class Pos(val value: Int, val max: Int)

    fun atBottom(p: Pos, slack: Int): Boolean = p.value >= p.max - slack

    fun next(following: Boolean, prev: Pos, now: Pos, slack: Int): Boolean =
        if (now.max != prev.max) following || atBottom(now, slack) else atBottom(now, slack)
}
