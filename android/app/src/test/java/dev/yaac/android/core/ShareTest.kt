package dev.yaac.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ShareTest {
    @Test
    fun filesAttachAndTextAppendsToTheDraft() {
        val cmds = shareCommands("thread:local/t1", "Look at this", Share("https://example.com", listOf("/c/a.png")))
        assertEquals("composer.action", cmds[0].first)
        assertEquals(listOf("/c/a.png"), cmds[0].second["paths"])
        assertEquals("attachPaths", cmds[0].second["id"])
        assertEquals("composer.edit", cmds[1].first)
        assertEquals("Look at this\n\nhttps://example.com", cmds[1].second["text"])
        assertEquals(("Look at this\n\nhttps://example.com").length, cmds[1].second["caret"])
    }

    @Test
    fun anEmptyDraftTakesTheTextAsIs() {
        val cmds = shareCommands("draft:x", "", Share("  hello  ", emptyList()))
        assertEquals(1, cmds.size)
        assertEquals("hello", cmds[0].second["text"])
    }

    @Test
    fun connectionAddressesGetTheHubPort() {
        assertEquals("desk:47380", Connection.normalize("desk"))
        assertEquals("100.1.2.3:9000", Connection.normalize(" 100.1.2.3:9000 "))
        assertEquals("[fd7a::1]:47380", Connection.normalize("fd7a::1"))
        assertNull(Connection.normalize("bad host"))
        assertNull(Connection.normalize("desk:99999"))
    }
}
