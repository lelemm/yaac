package dev.yaac.android.ui.transcript

import dev.yaac.android.core.FrameJson
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files

private fun obj(s: String): JsonObject = FrameJson.parseToJsonElement(s).jsonObject

class AnchorTest {
    @Test
    fun restoreAppliesOncePerTokenAndForgetsWithoutOne() {
        val r = AnchorRestore()
        assertEquals(Pin("a", 12.5f), r.incoming("12.5:a", obj("""{"id":"a","offset":12.5}""")))
        assertNull(r.incoming("12.5:a", obj("""{"id":"a","offset":12.5}""")))
        assertNull(r.incoming(null, null))
        // The same token again after it was dropped applies again (a new Back to this place).
        assertEquals(Pin("a", 12.5f), r.incoming("12.5:a", obj("""{"id":"a","offset":12.5}""")))
    }

    @Test
    fun restoreReadsABareIdAndClampsTheOffset() {
        val r = AnchorRestore()
        assertEquals(Pin("x", 0f), r.incoming("t", FrameJson.parseToJsonElement("\"x\"")))
        assertEquals(Pin("y", 0f), r.incoming("u", obj("""{"id":"y","offset":-4}""")))
        // A token without a usable anchor is not consumed.
        assertNull(r.incoming("v", obj("""{"offset":3}""")))
        assertEquals("u", r.token)
    }

    @Test
    fun reporterSendsTheEndOnceWhileFollowing() {
        val r = AnchorReporter()
        val row = obj("""{"id":"r1","actionId":"i1"}""")
        assertEquals(mapOf("id" to "", "offset" to 0, "threadId" to "t"), r.report("t", true, false, row, 30f))
        assertNull(r.report("t", true, false, row, 60f))
        val p = r.report("t", false, false, row, 30.4f)!!
        assertEquals("r1", p["id"]); assertEquals("i1", p["itemId"]); assertEquals(30.4, p["offset"] as Double, 0.01)
        assertNull("under a dp is not a move", r.report("t", false, false, row, 30.9f))
        assertEquals(32.0, r.report("t", false, false, row, 32f)!!["offset"] as Double, 0.01)
        // Following while the history is away reports the row, not the end.
        assertEquals("r1", r.report("t", true, true, row, 50f)!!["id"])
    }

    @Test
    fun relocateKeepsTheFoldRowThatTookThePlace() {
        val old = listOf("u1", "a1", "c1", "c2", "a2", "u2")
        val new = listOf("u1", "~worked:t1", "a2", "u2")
        assertEquals("~worked:t1", relocate(old, new, "c1"))
        assertEquals("~worked:t1", relocate(old, new, "a1"))
        assertNull("still there", relocate(old, new, "a2"))
        assertNull("not known", relocate(old, new, "zz"))
    }

    @Test
    fun relocateFallsBackToTheNearestSurvivor() {
        assertEquals("u1", relocate(listOf("u1", "c1", "a2"), listOf("u1", "a2"), "c1"))
        assertEquals("a2", relocate(listOf("c1", "a2"), listOf("a2"), "c1"))
        assertNull(relocate(listOf("c1"), emptyList(), "c1"))
    }
}

class CopyTextTest {
    @Test
    fun copyMessagePrefersCopyTextThenJoinsTextParts() {
        assertEquals("ran it", copyMessage(obj("""{"kind":"call","copyText":"ran it"}""")))
        assertNull(copyMessage(obj("""{"kind":"call","text":"x"}""")))
        val user = obj("""{"kind":"user","text":"look","parts":[{"kind":"pasted","text":"a\nb"},{"kind":"image","text":"no"},{"kind":"command","text":"/model x"}]}""")
        assertEquals("look\na\nb\n/model x", copyMessage(user))
        assertNull(copyMessage(obj("""{"kind":"agent"}""")))
    }

    @Test
    fun gutterCountsDrawnLines() {
        assertEquals("1", lineNumberGutter(""))
        assertEquals("1\n2", lineNumberGutter("a\nb\n"))
        assertEquals(10, codeLines((1..10).joinToString("\n")))
        val g = lineNumberGutter((1..10).joinToString("\n")).lines()
        assertEquals(" 1", g.first()); assertEquals("10", g.last())
    }
}

class ZoomTest {
    private val size = Vec(1000f, 2000f)

    @Test
    fun pinchKeepsThePointUnderTheFingers() {
        val focus = Vec(800f, 300f)
        val z = Zoom().transform(size, focus, 2f)
        assertEquals(2f, z.scale)
        // Content point under the focus: centre + (focus - centre - pan) / scale stays the same.
        val centre = size * 0.5f
        val q = (focus - centre - z.pan) * (1f / z.scale)
        assertEquals(focus.x - centre.x, q.x, 0.01f); assertEquals(focus.y - centre.y, q.y, 0.01f)
    }

    @Test
    fun scaleAndPanAreBounded() {
        assertEquals(Zoom.MIN, Zoom().transform(size, Vec(0f, 0f), 0.2f).scale)
        assertEquals(Zoom.MAX, Zoom().transform(size, Vec(0f, 0f), 100f).scale)
        val z = Zoom(2f).transform(size, size * 0.5f, 1f, Vec(5000f, -5000f))
        assertEquals(500f, z.pan.x); assertEquals(-1000f, z.pan.y)
        assertEquals(Vec(0f, 0f), Zoom().transform(size, size * 0.5f, 1f, Vec(40f, 40f)).pan)
    }

    @Test
    fun doubleTapAndResetReturnToFit() {
        val z = Zoom().toggle(size, Vec(100f, 100f))
        assertEquals(2.5f, z.scale); assertFalse(z.fitted)
        assertTrue(z.toggle(size, Vec(1f, 1f)).fitted)
        val stepped = Zoom().step(size, zoomIn = true)
        assertEquals(150, stepped.percent); assertEquals(Vec(0f, 0f), stepped.pan)
        assertTrue(stepped.step(size, zoomIn = false).fitted)
    }
}

class DiagramFilesTest {
    @Test
    fun svgBecomesAStandaloneDocument() {
        assertEquals("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" id=\"d\"><g/></svg>",
            DiagramFiles.svgDocument("<svg id=\"d\"><g/></svg>"))
        val already = "<svg xmlns=\"http://www.w3.org/2000/svg\"><g/></svg>"
        assertTrue(DiagramFiles.svgDocument(already)!!.endsWith(already))
        assertNull(DiagramFiles.svgDocument("<div/>")); assertNull(DiagramFiles.svgDocument(null))
    }

    @Test
    fun stagesUnderTheCacheWithTheDesktopName() {
        val dir = Files.createTempDirectory("yaac").toFile()
        val f = DiagramFiles.stage(dir, "mmd", "graph TD".toByteArray())
        assertEquals("diagram.mmd", f.name)
        assertEquals("graph TD", f.readText())
        assertTrue(f.canonicalPath.startsWith(dir.canonicalPath))
        assertEquals("PNG image", DiagramFiles.filter("png"))
        dir.deleteRecursively()
    }
}

class HeaderBudgetTest {
    @Test
    fun reasonOnlyWithABudget() {
        assertNull(budgetReason(obj("""{"budget":false}""")))
        // The reason is sent even when the tag already says the budget stopped it.
        assertEquals("x", budgetReason(obj("""{"budget":false,"budgetReason":"x"}""")))
        assertEquals("41 of 100 turns used", budgetReason(obj("""{"budget":true,"budgetReason":"41 of 100 turns used"}""")))
        assertEquals("Agent budget reached", budgetReason(obj("""{"budget":true,"budgetReason":null}""")))
    }
}
