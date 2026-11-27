package dev.yaac.android.ui.pane

import dev.yaac.android.core.FrameJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DocumentModelTest {
    private fun doc(json: String) = DocumentModel.read(FrameJson.parseToJsonElement(json) as JsonObject)

    // wpf/files.rs `WireDocument`: a diff with a fold of 3 unchanged lines between two hunks.
    private val diff = doc("""{"file":null,
        "old":{"text":"a\nb\nc\nd\ne\nold","styles":{"5":{"spans":[{"start":0,"end":3,"scope":"keyword"}],"words":[[0,3]]}}},
        "new":{"text":"a\nb\nc\nd\ne\nnew\nadded","styles":{"5":{"spans":[],"words":[[0,3]]}}},
        "rows":[[0,0,0],[1,1,3],[4,4,0],[5,null,0],[null,5,0],[null,6,0]],"sideBySide":false}""")

    @Test
    fun foldsCloseUntilUnfolded() {
        val closed = diff.layout(emptySet())
        assertEquals(listOf(LineKind.Context, LineKind.Fold, LineKind.Context, LineKind.Removed, LineKind.Added, LineKind.Added), closed.map { it.kind })
        assertEquals("⋯ 3 unchanged lines", closed[1].text)
        assertEquals(listOf(1), diff.foldRows)
        val open = diff.layout(setOf(1))
        assertEquals(8, open.size)
        assertEquals(listOf("b", "c", "d"), open.subList(1, 4).map { it.text })
        assertTrue(open[1].foldStart)
        assertEquals(listOf(0, 1, 2), open.subList(1, 4).map { it.offset })
        assertEquals("old", open[5].text); assertEquals(5, open[5].old); assertEquals(-1, open[5].new)
        assertEquals(listOf(0..2), open[5].source!!.words)
        assertEquals("keyword", open[5].source!!.spans.single().scope)
    }

    @Test
    fun aFileHasOneLinePerSourceLine() {
        val f = doc("""{"file":{"text":"fn main() {\n}\n","styles":{"0":{"spans":[{"start":0,"end":2,"scope":"keyword"},{"start":5,"end":99,"scope":"bad"}],"words":null}}},"sideBySide":false}""")
        val lines = f.layout(emptySet())
        assertEquals(listOf("fn main() {", "}", ""), lines.map { it.text })
        assertEquals(LineKind.File, lines[0].kind)
        assertEquals(1, lines[0].source!!.spans.size) // out-of-range spans are dropped
    }

    @Test
    fun addingAFileRangeSendsSourceLines() {
        val f = doc("""{"file":{"text":"a\nb\nc","styles":{}},"sideBySide":false}""")
        val lines = f.layout(emptySet())
        assertEquals(mapOf<String, Any?>("document" to 9L, "first" to 0, "last" to 2), Excerpts.addData(9, f, lines, LineSelection(0, 2)))
        assertEquals(mapOf<String, Any?>("document" to 9L, "first" to 1, "last" to 1, "firstColumn" to 0, "lastColumn" to 1),
            Excerpts.addData(9, f, lines, LineSelection(1, 1, 0, 1)))
        assertEquals("b\nc", Excerpts.text(lines, LineSelection(1, 2)))
    }

    @Test
    fun addingADiffRangeSendsRowsOffsetsAndClosedFolds() {
        val closed = diff.layout(emptySet())
        // Across the closed fold: it stays folded and is listed.
        assertEquals(mapOf("document" to 3L, "side" to "unified", "first" to 0, "firstOffset" to 0, "last" to 3, "lastOffset" to 0, "folded" to listOf(1)),
            Excerpts.addData(3, diff, closed, LineSelection(0, 3)))
        // Starting on a closed fold: trimmed to the first real line.
        assertEquals(2, Excerpts.addData(3, diff, closed, LineSelection(1, 2))!!["first"])
        assertNull(Excerpts.addData(3, diff, closed, LineSelection(1, 1)))
        // Ending inside an unfolded fold: the fold's row with the line's offset, nothing folded.
        val open = diff.layout(setOf(1))
        assertEquals(mapOf("document" to 3L, "side" to "unified", "first" to 0, "firstOffset" to 0, "last" to 1, "lastOffset" to 1, "folded" to emptyList<Int>()),
            Excerpts.addData(3, diff, open, LineSelection(0, 2)))
        assertEquals("a\nb", Excerpts.text(open, LineSelection(0, 1)))
        assertEquals("a\ne", Excerpts.text(closed, LineSelection(0, 2)))
    }

    @Test
    fun sideBySidePairsMarkReplacements() {
        val sbs = doc("""{"old":{"text":"x\nold","styles":{"1":{"spans":[],"words":[[0,1]]}}},"new":{"text":"x\nnew\nmore","styles":{"1":{"spans":[],"words":[[0,1]]}}},
            "rows":[[0,0,0],[1,1,0],[null,2,0]],"sideBySide":true}""")
        val p = sbs.pairs(emptySet())
        assertEquals(LineKind.Context, p[0].left.kind); assertEquals(LineKind.Context, p[0].right.kind)
        assertEquals(LineKind.Removed, p[1].left.kind); assertEquals(LineKind.Added, p[1].right.kind)
        assertEquals("", p[2].left.text); assertEquals(LineKind.Added, p[2].right.kind)
        assertEquals(mapOf("document" to 1L, "side" to "new", "first" to 1, "firstOffset" to 0, "last" to 1, "lastOffset" to 0, "folded" to emptyList<Int>()), Excerpts.addData(1, sbs, p.map { it.right }, LineSelection(1, 1), "new"))
    }

    @Test
    fun changeNavigationUsesTheTopRowAndLabelsTheOrdinal() {
        val lines = diff.layout(setOf(1))
        assertEquals(mapOf("document" to 4L, "row" to 1, "forward" to true), ChangeNav.data(4, lines, 2, true))
        assertEquals(mapOf("document" to 4L, "row" to null, "forward" to false), ChangeNav.data(4, emptyList(), 0, false))
        assertEquals(5, diff.indexOfRow(lines, 3))
        assertEquals(1, diff.indexOfRow(lines, 1))
        assertEquals("Change 2 of 5", ChangeNav.label(2, 5))
        assertEquals("5 changes", ChangeNav.label(null, 5))
        assertEquals("1 change", ChangeNav.label(null, 1))
        assertEquals("No changes", ChangeNav.label(null, 0))
    }

    @Test
    fun tapsStartAndExtendTheLineSelection() {
        var s = LineSelection.start(4)
        s = s.tap(1); assertEquals(LineSelection(1, 4), s)
        s = s.tap(7); assertEquals(LineSelection(7, 7), s)
        assertEquals(1, s.count)
    }
}
