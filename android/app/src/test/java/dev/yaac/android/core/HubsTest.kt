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

    @Test
    fun aMovedHubKeepsItsKeySlotAndLabelAndDialsTheNewAddress() {
        val l = HubList().add("desk", "Home")!!.add("lab")!!.select("desk:47380")
        val m = l.applyMoved("desk:47380", HubMove("h1", "box.tail1234.ts.net", "box"))!!
        val h = m.list.byId("desk:47380")!!
        assertEquals("box.tail1234.ts.net:47380", h.address)
        assertEquals("Home", h.label)
        assertEquals(0, h.slot)
        assertEquals("desk:47380", m.list.selected)
        assertTrue(m.merged.isEmpty())
        // The same move again, or a move that is no address, changes nothing.
        assertNull(m.list.applyMoved("desk:47380", HubMove("h1", "box.tail1234.ts.net:47380", null)))
        assertNull(l.applyMoved("desk:47380", HubMove("h1", " ", null)))
        assertNull(l.applyMoved("gone:47380", HubMove("h1", "box", null)))
    }

    @Test
    fun aMovedHubIsStoredUnderItsKeyAndItsNewAddress() {
        val m = HubList().add("desk")!!.add("lab")!!.applyMoved("desk:47380", HubMove(null, "box", null))!!.list
        val back = HubList.decode(m.encode())
        assertEquals(m, back)
        assertEquals("box:47380", back.byId("desk:47380")!!.address)
        // Adding the new address selects the moved hub; adding the old one is a new hub with a key of its own.
        assertEquals("desk:47380", back.add("box")!!.selected)
        val again = back.add("desk")!!
        assertEquals(3, again.hubs.size)
        assertEquals("desk:47380#1", again.selected)
        assertEquals("desk:47380", again.current!!.address)
        assertEquals(again, HubList.decode(again.encode()))
    }

    @Test
    fun aMoveOntoAnotherSavedEntryOfTheSameHubMergesIt() {
        // Saved at the new address already: merged, its label used when the moved one has none.
        val l = HubList().add("desk")!!.add("box", "Box")!!.add("lab")!!.select("box:47380")
        val m = l.applyMoved("desk:47380", HubMove("h1", "box:47380", "box"))!!
        assertEquals(listOf("desk:47380", "lab:47380"), m.list.hubs.map { it.id })
        assertEquals(listOf("box:47380"), m.merged.map { it.id })
        assertEquals("Box", m.list.byId("desk:47380")!!.label)
        assertEquals("desk:47380", m.list.selected)
        // Saved at another address that answers with the same hubId (a MagicDNS name and an IP).
        val n = HubList().add("desk", "Desk")!!.add("100.64.0.9")!!.add("lab")!!.select("lab:47380")
        val ids = mapOf("100.64.0.9:47380" to "h1", "lab:47380" to "h2")
        val r = n.applyMoved("desk:47380", HubMove("h1", "box.tail1234.ts.net", null)) { ids[it.address] }!!
        assertEquals(listOf("100.64.0.9:47380"), r.merged.map { it.id })
        assertEquals("Desk", r.list.byId("desk:47380")!!.label)
        assertEquals("lab:47380", r.list.selected)
        assertEquals(listOf(0, 2), r.list.hubs.map { it.slot })
    }

    @Test
    fun theMoveIsReadFromTheShellView() {
        val shell = FrameJson.parseToJsonElement("""{"hub":{"movedTo":{"hubId":"h1","address":"box:47380","hostName":"box"}}}""")
        assertEquals(HubMove("h1", "box:47380", "box"), HubMove.parse(shell["hub"]["movedTo"]))
        assertNull(HubMove.parse(FrameJson.parseToJsonElement("""{"hub":{"movedTo":null}}""")["hub"]["movedTo"]))
        assertNull(HubMove.parse(null))
    }
}
