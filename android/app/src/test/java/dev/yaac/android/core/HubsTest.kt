package dev.yaac.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HubsTest {
    @Test
    fun addingSelectsAndNormalizes() {
        val one = HubList().add("desk")!!
        assertEquals("desk:47380", one.selected)
        assertEquals(0, one.current!!.slot)
        val two = one.add("100.64.0.2:7000", "Laptop")!!
        assertEquals(listOf("desk:47380", "100.64.0.2:7000"), two.hubs.map { it.id })
        assertEquals("100.64.0.2:7000", two.selected)
        assertEquals(1, two.current!!.slot)
        assertEquals("Laptop", two.current!!.label)
        assertNull(two.add("not a host"))
    }

    @Test
    fun addingAgainSelectsTheSavedHubAndKeepsItsSlot() {
        val l = HubList().add("desk")!!.add("lab")!!.add("desk:47380", "Desk")!!
        assertEquals(2, l.hubs.size)
        assertEquals("desk:47380", l.selected)
        assertEquals("Desk", l.current!!.label)
        assertEquals(0, l.current!!.slot)
        // No label keeps the one it has.
        assertEquals("Desk", l.add("desk")!!.current!!.label)
    }

    @Test
    fun removingMovesTheSelectionAndFreesTheSlot() {
        val l = HubList().add("a")!!.add("b")!!.add("c")!!.select("b:47380")
        val r = l.remove("b:47380")
        assertEquals(listOf("a:47380", "c:47380"), r.hubs.map { it.id })
        assertEquals("c:47380", r.selected)
        assertEquals("a:47380", r.remove("c:47380").selected)
        assertNull(HubList().add("a")!!.remove("a:47380").selected)
        // Removing another hub keeps the selection; the freed slot is reused.
        val k = l.select("a:47380").remove("b:47380")
        assertEquals("a:47380", k.selected)
        assertEquals(1, k.add("d")!!.current!!.slot)
    }

    @Test
    fun encodesAndDecodes() {
        val l = HubList.migrate("desk:47380").add("lab", "Lab")!!.select("desk:47380")
        val back = HubList.decode(l.encode())
        assertEquals(l, back)
        assertTrue(back.hubs[0].legacy)
        assertEquals("Lab", back.hubs[1].label)
    }

    @Test
    fun aDamagedStoreReadsAsEmptyOrSkipsBadEntries() {
        assertEquals(HubList(), HubList.decode("{"))
        assertEquals(HubList(), HubList.decode(null))
        val l = HubList.decode("""{"selected":"gone:1","hubs":[{"address":"a","slot":0},{"address":"a:47380","slot":1},{"address":"b","slot":0},{"slot":2},{"address":"c","slot":3}]}""")
        assertEquals(listOf("a:47380", "c:47380"), l.hubs.map { it.id })
        assertEquals("a:47380", l.selected)
    }

    @Test
    fun migratesTheOneHubOfAnOlderInstall() {
        val l = HubList.migrate("desk:47380")
        assertEquals(1, l.hubs.size)
        assertTrue(l.current!!.legacy)
        assertEquals(HubList(), HubList.migrate(null))
    }

    @Test
    fun namesPreferTheHubThenTheLabelThenTheHost() {
        val h = Hub("desk.tail1234.ts.net:47380", null, 0)
        assertEquals("DESK-PC", HubNames.display(h, "DESK-PC"))
        assertEquals("desk", HubNames.display(h, null))
        assertEquals("Home", HubNames.display(h.copy(label = "Home"), " "))
        assertEquals("100.64.0.2", HubNames.host("100.64.0.2:47380"))
        assertEquals("fd7a::1", HubNames.host("[fd7a::1]:47380"))
    }

    @Test
    fun initialsCollideIntoTwoLettersThenNumbers() {
        assertEquals(listOf("D", "L"), HubNames.initials(listOf("desk", "laptop")))
        assertEquals(listOf("DE", "DL", "L"), HubNames.initials(listOf("desk", "Dell laptop", "lab")))
        assertEquals(listOf("WO", "WH"), HubNames.initials(listOf("work", "WHALE")))
        assertEquals(listOf("D1", "D2"), HubNames.initials(listOf("desk", "Desk")))
        assertEquals(listOf("?"), HubNames.initials(listOf("—")))
        assertEquals(listOf("1"), HubNames.initials(listOf("100.64.0.2")))
    }

    @Test
    fun stateReadsTheController() {
        assertEquals(HubState.Stopped, HubState.of("gone", null, true, true))
        assertEquals(HubState.Connecting, HubState.of(null, "Starting…", false, true))
        assertEquals(HubState.Unreachable, HubState.of(null, null, true, false))
        assertEquals(HubState.Connected, HubState.of(null, null, true, true))
    }

    @Test
    fun notificationIdsAreNamespacedPerHub() {
        val a = HubNotifyIds.request(0, "t1#r1")
        val b = HubNotifyIds.request(1, "t1#r1")
        assertNotEquals(a, b)
        assertTrue(a > 0 && b > 0)
        // Clear of the fixed ids (ongoing 1, calls 40–42) and the summaries.
        assertTrue(a > 1000 && HubNotifyIds.summary(0) in 100..400)
        assertNotEquals(HubNotifyIds.summary(0), HubNotifyIds.summary(1))
        assertNotEquals(HubNotifyIds.group(0), HubNotifyIds.group(1))
        val ids = (0..2).map { HubNotifyIds.decision(a, it) } + a
        assertEquals(4, ids.toSet().size)
        assertEquals(HubNotifyIds.request(255, "x"), HubNotifyIds.request(900, "x"))
        assertTrue(HubNotifyIds.request(255, "x") > 0)
    }

    @Test
    fun theOngoingNotificationSumsTheHubs() {
        assertEquals("YAAC · 2 need you" to emptyList<String>(), HubSummary.ongoing(listOf(HubSummary.Entry("desk", 2, "YAAC · 2 need you"))))
        val (title, lines) = HubSummary.ongoing(listOf(HubSummary.Entry("desk", 2, "x"), HubSummary.Entry("lab", 0, "y"), HubSummary.Entry("home", 1, "z")))
        assertEquals("YAAC · 3 need you", title)
        assertEquals(listOf("desk · 2 need you", "home · 1 need you"), lines)
        assertEquals("YAAC", HubSummary.ongoing(listOf(HubSummary.Entry("a", 0, null), HubSummary.Entry("b", 0, null))).first)
    }
}
