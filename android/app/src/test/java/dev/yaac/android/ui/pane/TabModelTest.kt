package dev.yaac.android.ui.pane

import dev.yaac.android.core.FrameJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TabModelTest {
    private fun tab(json: String) = Tabs.info(FrameJson.parseToJsonElement(json) as JsonObject)

    // wpf/terminal/tabs.rs `describe`.
    @Test
    fun terminalTabsShowTitleFolderAndMarks() {
        val t = tab("""{"kind":"terminal","id":4,"title":"cargo test","shell":"bash","cwd":"/home/u/api/","failed":false,"unread":true,"bell":0,"exitCode":null,"error":null}""")
        assertEquals("cargo test", t.title); assertEquals("api", t.detail); assertEquals(TabMark.Unseen, t.mark); assertEquals(TabIcon.Terminal, t.icon)
        assertNull(Tabs.terminalStatus(t)); assertFalse(Tabs.canRestart(t))
        assertEquals(TabMark.Bell, tab("""{"kind":"terminal","id":4,"shell":"bash","cwd":"C:\\src\\web","unread":true,"bell":1}""").mark)
        assertEquals("web", tab("""{"kind":"terminal","id":4,"shell":"pwsh","cwd":"C:\\src\\web"}""").detail)
        assertEquals("pwsh", tab("""{"kind":"terminal","id":4,"title":"","shell":"pwsh"}""").title)
        val exited = tab("""{"kind":"terminal","id":4,"shell":"bash","exitCode":2,"bell":1}""")
        assertEquals(TabMark.Exited, exited.mark); assertEquals("Process exited with code 2", Tabs.terminalStatus(exited)); assertTrue(Tabs.canRestart(exited))
        val failed = tab("""{"kind":"terminal","id":4,"shell":"bash","failed":true,"error":"No such shell"}""")
        assertEquals(TabMark.Failed, failed.mark); assertEquals("No such shell", Tabs.terminalStatus(failed))
        assertEquals("Reconnecting…", Tabs.terminalStatus(tab("""{"kind":"terminal","id":4,"error":"Reconnecting…"}""")))
        assertEquals("Terminal, cargo test, in api, new output, selected", Tabs.describe(t, true))
    }

    @Test
    fun fileAndBrowserTabs() {
        val d = tab("""{"kind":"diff","id":2,"path":"src/lib/main.rs","mark":"M"}""")
        assertEquals("main.rs", d.title); assertEquals("src/lib", d.detail); assertEquals("M", d.git); assertEquals(TabIcon.Diff, d.icon)
        assertNull(tab("""{"kind":"file","id":2,"path":"README.md"}""").detail)
        assertEquals("Files", tab("""{"kind":"files","id":1}""").title)
        val b = tab("""{"kind":"browser","id":3,"title":"","url":"localhost:3000","loading":true}""")
        assertEquals("localhost:3000", b.title); assertEquals(TabMark.Loading, b.mark)
        assertEquals("New tab", tab("""{"kind":"browser","id":3}""").title)
    }

    @Test
    fun renameSavesCancelsAndClears() {
        assertEquals(mapOf("name" to "build"), Rename.data("bash", " build "))
        assertNull(Rename.data("bash", "bash"))
        assertEquals(mapOf("name" to ""), Rename.data("my name", "  "))
    }

    @Test
    fun theComposerCaretFollowsItsCommands() {
        ComposerCaret.reset()
        assertNull(ComposerCaret.caret("t1"))
        ComposerCaret.observe("composer.edit", mapOf("key" to "t1", "text" to "hello", "caret" to 5))
        ComposerCaret.observe("composer.caret", mapOf("key" to "t1", "caret" to 2))
        ComposerCaret.observe("terminal.key", mapOf("key" to "t2", "caret" to 9))
        assertEquals(2, ComposerCaret.caret("t1"))
        assertNull(ComposerCaret.caret("t2"))
        assertEquals(2, ComposerCaret.caret(null))
    }
}
