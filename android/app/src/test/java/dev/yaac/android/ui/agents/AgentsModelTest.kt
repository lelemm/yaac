package dev.yaac.android.ui.agents

import dev.yaac.android.core.FrameJson
import dev.yaac.android.core.get
import dev.yaac.android.core.objects
import kotlinx.serialization.json.JsonElement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentsModelTest {
    private fun json(s: String): JsonElement = FrameJson.parseToJsonElement(s)

    /** All three relations (SC46): a parent, two workers (one sending, one finished and unseen), a contact receiving. */
    private val three = json(
        """{"visible":true,
        "groups":[
          {"id":"spawnedBy","label":"Spawned by","glyph":"↑","count":1,"rows":[{"id":"p","title":"orchestrator","mark":"running"}],"docked":{"rowIds":["p"],"foldIds":[],"foldLabel":null,"count":1}},
          {"id":"workers","label":"Workers","glyph":"↓","count":2,"rows":[
             {"id":"w1","title":"users","mark":"needsYou","flow":"sending","attention":"approval"},
             {"id":"w2","title":"billing","mark":"done","finished":true,"unseen":true}],
           "docked":{"rowIds":["w1"],"foldIds":["w2"],"foldLabel":"1 finished","count":2}},
          {"id":"contacts","label":"In contact with","glyph":"↔","count":1,"rows":[{"id":"c","title":"ci","flow":"receiving","provider":"Codex","model":"gpt","target":"devbox"}],"docked":{"rowIds":["c"],"foldIds":[],"count":1}}],
        "siblings":[{"id":"self","title":"me","self":true}],
        "root":{"id":"r","title":"Root task","canOpen":true},
        "pill":{"rowIds":["p","w1","w2","c"],"hidden":0,"alive":0,"needsYou":0,"firstHiddenId":null}}""",
    )

    @Test
    fun thePillClustersTheThreeRelationsWithTheirGlyphsAndMarks() {
        val e = pillEntries(three)
        val shape = e.map {
            when (it) {
                is PillEntry.Cluster -> "[${it.glyph}]"
                is PillEntry.Agent -> it.id
                PillEntry.Rule -> "-"
                else -> it.toString()
            }
        }
        assertEquals(listOf("[↑]", "p", "-", "[↓]", "w1", "w2", "-", "[↔]", "c"), shape)
        val agents = e.filterIsInstance<PillEntry.Agent>().associateBy { it.id }
        assertEquals("↑", agents.getValue("w1").marks.flow)
        assertTrue(agents.getValue("w1").marks.needsYou)
        assertEquals("↓", agents.getValue("c").marks.flow)
        assertTrue(agents.getValue("w2").marks.unseen)
        assertTrue(agents.getValue("w2").marks.finished)
        assertFalse(agents.getValue("p").marks.needsYou)
    }

    @Test
    fun missingGlyphsFallBackToWpfDefaults() {
        val v = json("""{"groups":[{"id":"contacts","rows":[{"id":"c"}]}],"pill":{"rowIds":["c"]}}""")
        assertEquals(PillEntry.Cluster("contacts", "⇄", "In contact with"), pillEntries(v).first())
    }

    @Test
    fun thePillFollowsItsOwnOrderWithinAGroup() {
        val v = json("""{"groups":[{"id":"workers","rows":[{"id":"a"},{"id":"b"},{"id":"c"}]}],"pill":{"rowIds":["c","a"],"hidden":1}}""")
        val ids = pillEntries(v).filterIsInstance<PillEntry.Agent>().map { it.id }
        assertEquals(listOf("c", "a"), ids)
    }

    @Test
    fun manyAgentsShowMoreWithAliveAndTheFirstHidden() {
        val rows = (1..30).joinToString(",") { """{"id":"w$it"}""" }
        val v = json("""{"groups":[{"id":"workers","rows":[$rows]}],"pill":{"rowIds":["w1","w2","w3","w4","w5","w6","w7"],"hidden":23,"alive":4,"needsYou":1,"firstHiddenId":"w8"}}""")
        val more = pillEntries(v).last() as PillEntry.More
        assertEquals(PillEntry.More(23, 4, true, "w8"), more)
        assertEquals(listOf("more", "(23)", "4 alive"), more.lines)
        assertEquals(PillEntry.Rule, pillEntries(v).let { it[it.size - 2] })
    }

    @Test
    fun withoutCountsThePillComputesHiddenAndFirstHidden() {
        val v = json("""{"groups":[{"id":"workers","rows":[{"id":"a"},{"id":"b"},{"id":"c"}]}],"pill":{"rowIds":["a"]}}""")
        val more = pillEntries(v).last() as PillEntry.More
        assertEquals(2, more.hidden)
        assertEquals("b", more.firstHiddenId)
        assertEquals(listOf("more", "(2)"), more.lines)
    }

    @Test
    fun anExhaustedBudgetLeadsWithPausedAndABudgetAloneShowsRelated() {
        val paused = json("""{"budget":{"turns":"100 of 100","exhausted":true},"groups":[{"id":"workers","rows":[{"id":"a"}]}],"pill":{"rowIds":["a"]}}""")
        assertEquals(PillEntry.Paused, pillEntries(paused).first())
        val alone = json("""{"budget":{"turns":"1 of 100"},"groups":[],"pill":{"rowIds":[]}}""")
        assertEquals(listOf(PillEntry.Related), pillEntries(alone))
        assertTrue(pillEntries(json("""{"groups":[],"pill":{"rowIds":[]}}""")).isEmpty())
    }

    private fun shape(entries: List<SheetEntry>) = entries.map {
        when (it) {
            is SheetEntry.Heading -> "# ${it.label}"
            is SheetEntry.Agent -> it.id
            is SheetEntry.Fold -> "› ${it.label}${if (it.open) " (open)" else ""}"
            is SheetEntry.Root -> it.title
            is SheetEntry.Info -> "(${it.text})"
        }
    }

    @Test
    fun theSheetFoldsFinishedWorkersAndPinsSpawnedByAtTheFoot() {
        assertEquals(
            listOf("# Siblings (1)", "self", "# Workers (2)", "w1", "› 1 finished", "# In contact with (1)", "c", "# Spawned by", "root: Root task", "p"),
            shape(sheetEntries(three, emptySet())),
        )
        assertEquals(
            listOf("# Siblings (1)", "self", "# Workers (2)", "w1", "› 1 finished (open)", "w2", "# In contact with (1)", "c", "# Spawned by", "root: Root task", "p"),
            shape(sheetEntries(three, setOf("workers"))),
        )
        assertEquals("workers", foldedGroupOf(three, "w2"))
        assertNull(foldedGroupOf(three, "w1"))
    }

    @Test
    fun aRootWithoutAParentSaysStartedByYou() {
        val v = json("""{"groups":[{"id":"workers","rows":[{"id":"a"}]}],"root":{"id":null,"title":"Mine","canOpen":false}}""")
        val e = sheetEntries(v, emptySet())
        assertEquals(listOf("# Workers", "a", "# Spawned by", "root: Mine", "(started by you)"), shape(e))
        assertFalse((e[3] as SheetEntry.Root).canOpen)
    }

    @Test
    fun rowLinesJoinWhatIsPresent() {
        val row = three["groups"].objects[2]["rows"].objects[0]
        assertEquals("Codex · gpt · devbox", rowMeta(row))
        assertEquals("receiving", rowInfo(row))
        val w = json("""{"markLabel":"needs you","attention":"approval","unseen":true,"readOnly":true,"activity":"editing","age":"2 m"}""") as kotlinx.serialization.json.JsonObject
        assertEquals("needs you · approval · unread · read-only", rowInfo(w))
        assertEquals("editing · 2 m", rowActivity(w))
    }

    private val budget = json(
        """{"id":"local/root","turns":"41 of 100 agent turns","time":"14 h since you acted (limit 12 h)","exhausted":true,
        "reached":"Paused: 12 h without you","ranOut":"time","action":{"id":"continueBudget","label":"Continue","enabled":true},
        "timeMenu":{"title":"Extend time limit…","unavailable":null,"choices":[
          {"index":0,"label":"+1 h","detail":"until 15 h"},{"index":1,"label":"+4 h"},{"index":2,"label":"+12 h"},{"index":3,"label":"+24 h"},
          {"index":4,"label":"Remove time limit","remove":true}],"customDefault":2}}""",
    )

    @Test
    fun theBudgetSaysWhichLimitRanOut() {
        val b = Budget.of(budget)!!
        assertEquals("local/root", b.root)
        assertTrue(b.failing("time"))
        assertFalse(b.failing("turns"))
        assertTrue(Budget.of(json("""{"ranOut":"unknown"}"""))!!.failing("turns"))
        assertEquals("Paused: 12 h without you", b.reached)
    }

    @Test
    fun theTimeMenuSplitsPresetsFromRemove() {
        val m = Budget.of(budget)!!.timeMenu!!
        assertEquals("Extend time limit…", m.title)
        assertNull(m.unavailable)
        assertEquals(listOf("+1 h", "+4 h", "+12 h", "+24 h"), m.choices.map { it.label })
        assertEquals("until 15 h", m.choices[0].detail)
        assertEquals(TimeMenu.Choice(4, "Remove time limit", null), m.remove)
        assertEquals(2, m.customDefault)
        assertEquals(mapOf("id" to "local/root", "choice" to 1), timeLimitChoice("local/root", m.choices[1]))
        assertEquals(mapOf("id" to "local/root", "choice" to 4), timeLimitChoice("local/root", m.remove!!))
    }

    @Test
    fun anUnsupportedTargetExplainsWhy() {
        val m = TimeMenu.of(json("""{"title":"Set time limit…","unavailable":"devbox's yaacd is too old","choices":[]}"""))!!
        assertEquals("devbox's yaacd is too old", m.unavailable)
        assertNull(m.remove)
    }

    @Test
    fun customHoursAreDigitsFromOneToTenThousand() {
        assertEquals(7, TimeMenu.hours(" 7 "))
        assertEquals(10_000, TimeMenu.hours("10000"))
        for (bad in listOf("", "0", "-3", "+3", "2.5", "10001", "abc", "99999999999")) assertNull(bad, TimeMenu.hours(bad))
        assertEquals(mapOf("id" to "r", "hours" to 7), timeLimitHours("r", "7"))
        assertNull(timeLimitHours("r", "x"))
    }

    @Test
    fun aHeldFingerFindsTheRowUnderIt() {
        val rows = listOf(RowBounds("a", 0f, 50f), RowBounds("b", 50f, 100f), RowBounds("c", 120f, 170f))
        assertEquals("a", rowAt(0f, rows))
        assertEquals("b", rowAt(50f, rows))
        assertNull(rowAt(110f, rows))
        assertEquals("c", rowAt(169f, rows))
        assertNull(rowAt(-1f, rows))
    }
}
