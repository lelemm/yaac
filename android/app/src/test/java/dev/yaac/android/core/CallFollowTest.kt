package dev.yaac.android.core

import dev.yaac.android.core.CallFollow.Pos
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CallFollowTest {
    private val slack = 8

    @Test fun growingContentKeepsFollowing() {
        // A line grew: the height went up, the position did not move.
        assertTrue(CallFollow.next(true, Pos(100, 100), Pos(100, 180), slack))
    }

    @Test fun scrollingUpStopsFollowing() {
        assertFalse(CallFollow.next(true, Pos(180, 180), Pos(90, 180), slack))
    }

    @Test fun growingWhileScrolledUpStaysPut() {
        assertFalse(CallFollow.next(false, Pos(90, 180), Pos(90, 260), slack))
    }

    @Test fun scrollingBackToTheBottomFollowsAgain() {
        assertTrue(CallFollow.next(false, Pos(90, 260), Pos(255, 260), slack))
        assertTrue(CallFollow.next(false, Pos(90, 260), Pos(260, 260), slack))
    }

    @Test fun ourOwnScrollToTheBottomKeepsFollowing() {
        assertTrue(CallFollow.next(true, Pos(100, 180), Pos(180, 180), slack))
    }

    @Test fun shrinkingContentAtTheBottomFollows() {
        // An unfinished line was replaced by a shorter one; the position clamps to the new end.
        assertTrue(CallFollow.next(false, Pos(90, 260), Pos(200, 200), slack))
    }

    @Test fun noOverflowIsTheBottom() {
        assertTrue(CallFollow.atBottom(Pos(0, 0), slack))
    }
}
