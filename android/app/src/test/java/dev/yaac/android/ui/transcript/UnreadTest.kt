package dev.yaac.android.ui.transcript

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private fun user(id: String, turn: String) = UnreadRow(id, id, turn, "user")
private fun agent(id: String, turn: String) = UnreadRow(id, id, turn, "agent")
private fun artifact(id: String) = UnreadRow(id, null, null, "artifact")

class UnreadTest {
    private val rows = listOf(user("u1", "t1"), agent("a1", "t1"), user("u2", "t2"), agent("a2", "t2"), artifact("art"))

    @Test
    fun seenAtTakesTheItemAtOrBeforeTheIndexWithItsPrompt() {
        assertEquals(SeenPoint("a2", "a2", "t2", "u2"), Unread.seenAt(rows, 4)) // the artifact card is not an item
        assertEquals(SeenPoint("a1", "a1", "t1", "u1"), Unread.seenAt(rows, 1))
        assertEquals(SeenPoint("u1", "u1", "t1", "u1"), Unread.seenAt(rows, -3))
        assertNull(Unread.seenAt(emptyList(), 0))
        assertNull(Unread.seenAt(listOf(artifact("x")), 0))
    }

    @Test
    fun dividerGoesAboveTheFirstItemAfterTheSeenOne() {
        val seen = SeenPoint("a1", "a1", "t1", "u1")
        assertEquals(Unread.Outcome.Divider("u2", 2), Unread.resolve(rows, seen, emptySet(), hasOlder = false, jumped = false))
        // Seen the last item: nothing new (the artifact card does not count).
        assertEquals(Unread.Outcome.Nothing, Unread.resolve(rows, SeenPoint("a2", "a2", "t2", "u2"), emptySet(), false, false))
    }

    @Test
    fun aFoldedRowIsFoundByItsItemOrItsTurn() {
        // The seen command folded into a run whose actionId is its item.
        val folded = listOf(user("u1", "t1"), UnreadRow("~run:c1", "c1", "t1", "activity"), agent("a1", "t1"))
        assertEquals(Unread.Outcome.Divider("a1", 1), Unread.resolve(folded, SeenPoint("c1", "c1", "t1", "u1"), emptySet(), false, false))
        // Folded into the turn's "Worked for…" line: what follows that line is new.
        val worked = listOf(user("u1", "t1"), UnreadRow("~worked:t1", "c0", "t1", "worked"), agent("a1", "t1"), user("u2", "t2"))
        assertEquals(Unread.Outcome.Divider("a1", 2), Unread.resolve(worked, SeenPoint("c5", "c5", "t1", "u1"), emptySet(), false, false))
        // Neither: the turn's first row stands for it.
        val bare = listOf(user("u1", "t1"), agent("a1", "t1"))
        assertEquals(Unread.Outcome.Divider("a1", 1), Unread.resolve(bare, SeenPoint("c5", "c5", "t1", "u1"), emptySet(), false, false))
    }

    @Test
    fun anOlderSeenPointJumpsThroughItsPromptOnce() {
        val seen = SeenPoint("a0", "a0", "t0", "u0")
        assertEquals(Unread.Outcome.Jump("u0"), Unread.resolve(rows, seen, setOf("u0", "u1"), hasOlder = true, jumped = false))
        assertEquals(Unread.Outcome.Nothing, Unread.resolve(rows, seen, setOf("u0"), hasOlder = true, jumped = true))
        assertEquals(Unread.Outcome.Nothing, Unread.resolve(rows, seen, setOf("u0"), hasOlder = false, jumped = false))
        assertEquals(Unread.Outcome.Nothing, Unread.resolve(rows, seen, setOf("u1"), hasOlder = true, jumped = false))
    }

    @Test
    fun aVisitWaitsForThePageThenScrollsOnceAndKeepsTheDivider() {
        val v = UnreadVisit(SeenPoint("a1", "a1", "t1", "u1"), enabled = true)
        assertTrue(v.busy)
        assertNull(v.step(rows, ready = false, prompts = emptySet(), hasOlder = false, focused = false))
        assertTrue(v.busy)
        assertEquals(UnreadVisit.Effect.ScrollTo("u2"), v.step(rows, true, emptySet(), false, false))
        assertFalse(v.busy)
        assertEquals("u2", v.dividerBefore)
        assertEquals(2, v.count)
        // New items stream in: the divider stays, nothing scrolls again.
        val more = rows + agent("a3", "t2")
        assertNull(v.step(more, true, emptySet(), false, false))
        assertEquals("u2", v.dividerBefore)
        assertEquals(2, v.count)
        v.clear()
        assertNull(v.dividerBefore)
    }

    @Test
    fun theDividerFollowsItsRowWhenItFolds() {
        val v = UnreadVisit(SeenPoint("u1", "u1", "t1", "u1"), true)
        val live = listOf(user("u1", "t1"), UnreadRow("c1", "c1", "t1", "tool"), agent("a1", "t1"))
        assertEquals(UnreadVisit.Effect.ScrollTo("c1"), v.step(live, true, emptySet(), false, false))
        val folded = listOf(user("u1", "t1"), UnreadRow("~worked:t1", "c1", "t1", "worked"), agent("a1", "t1"))
        v.step(folded, true, emptySet(), false, false)
        assertEquals("~worked:t1", v.dividerBefore)
    }

    @Test
    fun nothingNewOrNoSeenPointOrOffLeavesTheViewAlone() {
        val atEnd = UnreadVisit(SeenPoint("a2", "a2", "t2", "u2"), true)
        assertNull(atEnd.step(rows, true, emptySet(), false, false))
        assertFalse(atEnd.busy)
        assertNull(atEnd.dividerBefore)
        val first = UnreadVisit(null, true)
        assertFalse(first.busy)
        assertNull(first.step(rows, true, emptySet(), false, false))
        val off = UnreadVisit(SeenPoint("a1", "a1", "t1", "u1"), enabled = false)
        assertFalse(off.busy)
        assertNull(off.step(rows, true, emptySet(), false, false))
        assertNull(off.dividerBefore)
    }

    @Test
    fun aFocusWinsTheScrollButTheDividerIsDrawn() {
        val v = UnreadVisit(SeenPoint("a1", "a1", "t1", "u1"), true)
        assertNull(v.step(rows, true, emptySet(), false, focused = true))
        assertEquals("u2", v.dividerBefore)
    }

    @Test
    fun aJumpWaitsForItsPromptRowThenScrolls() {
        val v = UnreadVisit(SeenPoint("a0", "a0", "t0", "u0"), true)
        assertEquals(UnreadVisit.Effect.Jump("u0"), v.step(rows, true, setOf("u0"), true, false))
        assertTrue(v.jumping)
        // The old page again (the jump has not landed): wait.
        assertNull(v.step(rows, true, setOf("u0"), true, false))
        assertTrue(v.jumping)
        val older = listOf(user("u0", "t0"), agent("a0", "t0"), user("u1", "t1"))
        // Its own jump's focus does not stop the scroll.
        assertEquals(UnreadVisit.Effect.ScrollTo("u1"), v.step(older, true, setOf("u0"), true, focused = true))
        assertFalse(v.busy)
    }

    @Test
    fun aJumpThatFindsNothingOrNeverLandsReturnsToTheLatestPage() {
        val v = UnreadVisit(SeenPoint("a0", "a0", "t0", "u0"), true)
        v.step(rows, true, setOf("u0"), true, false)
        val other = listOf(user("u0", "t9"), agent("x", "t9"))
        assertEquals(UnreadVisit.Effect.Latest, v.step(other, true, setOf("u0"), true, false))
        val w = UnreadVisit(SeenPoint("a0", "a0", "t0", "u0"), true)
        w.step(rows, true, setOf("u0"), true, false)
        assertEquals(UnreadVisit.Effect.Latest, w.giveUp())
        assertFalse(w.busy)
        assertNull(w.giveUp())
    }

    @Test
    fun theStoreKeepsTheMostRecentThreadsAndRoundTrips() {
        val s = SeenStore(capacity = 3)
        for (t in listOf("a", "b", "c")) assertTrue(s.put(t, SeenPoint("r$t", null, null, null)))
        assertFalse(s.put("c", SeenPoint("rc", null, null, null)))
        s["a"] // used: "b" is now the oldest
        s.put("d", SeenPoint("rd", "i", "t", "p"))
        assertEquals(3, s.size)
        assertNull(s["b"])
        val back = SeenStore.decode(s.encode(), capacity = 3)
        assertEquals(SeenPoint("rd", "i", "t", "p"), back["d"])
        assertEquals(SeenPoint("ra", null, null, null), back["a"])
        assertNull(back["b"])
        // Recency survives the round trip: "c" is the oldest now.
        back.put("e", SeenPoint("re", null, null, null))
        assertNull(back["c"])
        assertEquals(0, SeenStore.decode("not json").size)
        assertEquals(1, SeenStore.decode("""[{"t":"x","r":"y"},{"t":"z"},3]""").size)
    }
}
