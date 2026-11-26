package dev.yaac.android.core

import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameMergerTest {
    private fun ids(v: kotlinx.serialization.json.JsonElement?) = v["rows"].objects.map { it["id"].str }

    @Test
    fun rowChangesPatchRowsByIdAndKeepTheOrder() {
        val m = FrameMerger()
        m.merge("""{"views":{"transcript":{"threadId":"a","rows":[{"id":"1","text":"x"},{"id":"2","text":"y"}],"prompts":[{"id":"1"}]}}}""")
        val u = m.merge("""{"views":{"transcript":{"threadId":"a","rowChanges":{"rows":[{"id":"2","text":"z"}],"order":null}}}}""")!!
        val t = u.views.single().second
        assertEquals(listOf("1", "2"), ids(t))
        assertEquals("z", t["rows"].objects[1]["text"].str)
        // A transcript without `prompts` keeps the previous index.
        assertEquals("1", t["prompts"].objects.single()["id"].str)
    }

    @Test
    fun orderDropsRowsAndReorders() {
        val m = FrameMerger()
        m.merge("""{"views":{"transcript":{"threadId":"a","rows":[{"id":"1"},{"id":"2"},{"id":"3"}]}}}""")
        val t = m.merge("""{"views":{"transcript":{"threadId":"a","rowChanges":{"rows":[{"id":"4"}],"order":["3","1","4"]}}}}""")!!.views.single().second
        assertEquals(listOf("3", "1", "4"), ids(t))
    }

    @Test
    fun anotherThreadReplacesEveryRow() {
        val m = FrameMerger()
        m.merge("""{"views":{"transcript":{"threadId":"a","rows":[{"id":"1"}],"prompts":[{"id":"1"}]}}}""")
        val t = m.merge("""{"views":{"transcript":{"threadId":"b","rows":[{"id":"9"}]}}}""")!!.views.single().second
        assertEquals(listOf("9"), ids(t))
        assertTrue(t["prompts"].isNullish)
    }

    @Test
    fun otherViewsAreReplacedWholeAndEffectsSurface() {
        val m = FrameMerger()
        val u = m.merge("""{"copy":"abc","views":{"shell":{"title":"T"},"captionUpdate":null}}""")!!
        assertEquals("T", (u.views.first { it.first == "shell" }.second as JsonObject)["title"].str)
        assertNull(u.views.first { it.first == "captionUpdate" }.second)
        assertEquals(FrameEffect.Copy("abc"), u.effects.single())
        assertEquals("boom", m.merge("""{"stopped":{"error":"boom"}}""")!!.stopped)
    }
}
