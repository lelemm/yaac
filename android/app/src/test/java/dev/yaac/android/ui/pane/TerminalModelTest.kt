package dev.yaac.android.ui.pane

import dev.yaac.android.core.FrameJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalModelTest {
    private fun frame(json: String) = TerminalWire.decode(FrameJson.parseToJsonElement(json) as JsonObject)!!

    // The shape crates/yaac-app/src/wpf/terminal.rs writes (`Message`), with wire.rs runs.
    private val first = """{"search":null,"matches":0,"found":false,"panel":false,"tab":7,"frame":1,"height":3,
        "rows":[[0,[[0,1,13421772,-1,1,"first"]]],[1,[]],[2,[[0,2,42,-1,0,"界表"],[4,2,42,-1,16,"日́"],[6,1,42,3355443,4,"x"]]]],
        "cursor":[5,0,"block",16777215,false,true],"glyph":null,"mouse":false,"selection":false}"""

    @Test
    fun decodesRunsCursorAndFlags() {
        val f = frame(first)
        assertEquals(7L, f.tab); assertEquals(1L, f.serial); assertEquals(3, f.height); assertFalse(f.panel)
        val run = f.rows[0].second.single()
        assertEquals(TermRun(0, 1, 13421772, -1, 1, "first"), run)
        assertTrue(run.bold); assertFalse(run.italic)
        val wide = f.rows[2].second
        assertEquals(4, wide[0].columns) // two wide glyphs
        assertEquals(2, wide[1].columns) // one combining cluster, wide
        assertTrue(wide[2].underline); assertEquals(3355443L, wide[2].paper)
        assertEquals(TermCursor(5, 0, "block", 16777215, false, true), f.cursor)
        assertNull(f.search)
    }

    @Test
    fun damageFramesPatchRowsAndAShorterHeightDropsRows() {
        var s = TermScreen().apply(frame(first))
        assertEquals("first", s.rowText(0))
        assertEquals("界表日́x", s.rowText(2))
        s = s.apply(frame("""{"tab":7,"frame":2,"height":3,"rows":[[1,[[2,1,1,-1,0,"second"]]]],"cursor":[0,1,"bar",1,false,false]}"""))
        assertEquals("first", s.rowText(0))
        assertEquals("  second", s.rowText(1))
        s = s.apply(frame("""{"tab":7,"frame":3,"height":2,"rows":[],"cursor":[0,0,"hidden",1,false,false]}"""))
        assertEquals(setOf(0, 1), s.rows.keys)
        assertFalse(s.cursor!!.visible)
        assertEquals(3, s.frames)
    }

    @Test
    fun malformedFramesAreSkipped() {
        assertNull(TerminalWire.decode(FrameJson.parseToJsonElement("""{"rows":[]}""") as JsonObject))
        val f = frame("""{"tab":1,"height":1,"rows":[["x",[]],[0,[[0,1],[0,1,1,-1,0,"ok"]]]]}""")
        assertEquals(listOf(0 to listOf(TermRun(0, 1, 1, -1, 0, "ok"))), f.rows)
    }

    @Test
    fun searchStatusMatchesWpf() {
        assertEquals("", TerminalWire.searchStatus(null, false, 0))
        assertEquals("No results", TerminalWire.searchStatus("foo", false, 0))
        assertEquals("3 here", TerminalWire.searchStatus("foo", true, 3))
        assertEquals("", TerminalWire.searchStatus("foo", true, 0))
        val f = frame("""{"tab":1,"height":1,"rows":[],"search":"cargo","matches":2,"found":true,"selection":true,"mouse":true}""")
        assertEquals("cargo", f.search); assertTrue(f.selection); assertTrue(f.mouse)
    }

    @Test
    fun keyCodesAreAndroids() {
        // The constants are copied so the model stays free of Android; check them against the SDK.
        assertEquals(android.view.KeyEvent.KEYCODE_A, TermKeys.A)
        assertEquals(android.view.KeyEvent.KEYCODE_Z, TermKeys.Z)
        assertEquals(android.view.KeyEvent.KEYCODE_0, TermKeys.D0)
        assertEquals(android.view.KeyEvent.KEYCODE_F1, TermKeys.F1)
        assertEquals(android.view.KeyEvent.KEYCODE_F12, TermKeys.F12)
        assertEquals(android.view.KeyEvent.KEYCODE_ENTER, TermKeys.ENTER)
        assertEquals(android.view.KeyEvent.KEYCODE_ESCAPE, TermKeys.ESCAPE)
        assertEquals(android.view.KeyEvent.KEYCODE_DEL, TermKeys.DEL)
        assertEquals(android.view.KeyEvent.KEYCODE_FORWARD_DEL, TermKeys.FORWARD_DEL)
        assertEquals(android.view.KeyEvent.KEYCODE_MOVE_HOME, TermKeys.HOME)
        assertEquals(android.view.KeyEvent.KEYCODE_PAGE_DOWN, TermKeys.PAGE_DOWN)
        assertEquals(android.view.KeyEvent.KEYCODE_DPAD_RIGHT, TermKeys.RIGHT)
        assertEquals(android.view.KeyEvent.KEYCODE_GRAVE, TermKeys.GRAVE)
        assertEquals(android.view.KeyEvent.KEYCODE_NUMPAD_0, TermKeys.NUM0)
        assertEquals(android.view.KeyEvent.KEYCODE_NUMPAD_DOT, TermKeys.NUM_DOT)
        assertEquals(android.view.KeyEvent.KEYCODE_INSERT, TermKeys.INSERT)
    }

    private val bindings = listOf(
        Shortcut("commander", "K", ctrl = true, shift = false, alt = false, inTerminal = true, repeat = false),
        Shortcut("new-thread", "N", ctrl = true, shift = false, alt = false, inTerminal = false, repeat = false),
        Shortcut("next-thread", "Down", ctrl = true, shift = false, alt = true, inTerminal = true, repeat = true),
        Shortcut("toggle-terminal", "Backtick", ctrl = true, shift = false, alt = false, inTerminal = true, repeat = false),
    )

    @Test
    fun boundCommandsMarkedWorksInTheTerminalRunTheRestReachTheShell() {
        assertEquals(KeyRoute.Command("commander"), TermKeys.route(TermKeys.A + 10, 'k', false, false, true, false, false, bindings))
        // Shift falls back to the binding without Shift, as WPF does.
        assertEquals(KeyRoute.Command("commander"), TermKeys.route(TermKeys.A + 10, 'k', true, false, true, false, false, bindings))
        // Ctrl+N is bound but not in the terminal: the shell gets ^N.
        assertEquals(KeyRoute.Key("n", 4), TermKeys.route(TermKeys.A + 13, 'n', false, false, true, false, false, bindings))
        assertEquals(KeyRoute.Command("toggle-terminal"), TermKeys.route(TermKeys.GRAVE, '`', false, false, true, false, false, bindings))
        // A held key repeats only commands that repeat.
        assertEquals(KeyRoute.Consume, TermKeys.route(TermKeys.A + 10, 'k', false, false, true, false, true, bindings))
        assertEquals(KeyRoute.Command("next-thread"), TermKeys.route(TermKeys.DOWN, null, false, true, true, false, true, bindings))
    }

    @Test
    fun keysForTheShell() {
        assertEquals(KeyRoute.Key("c", 4), TermKeys.route(TermKeys.A + 2, 'c', false, false, true, false, false, emptyList()))
        assertEquals(KeyRoute.Key("C", 5), TermKeys.route(TermKeys.A + 2, 'c', true, false, true, false, false, emptyList()))
        assertEquals(KeyRoute.Key("Up", 0), TermKeys.route(TermKeys.UP, null, false, false, false, false, false, emptyList()))
        assertEquals(KeyRoute.Key("F5", 1), TermKeys.route(TermKeys.F1 + 4, null, true, false, false, false, false, emptyList()))
        assertEquals(KeyRoute.Key("Backspace", 0), TermKeys.route(TermKeys.DEL, null, false, false, false, false, false, emptyList()))
        assertEquals(KeyRoute.Key(" ", 4), TermKeys.route(TermKeys.SPACE, ' ', false, false, true, false, false, emptyList()))
        assertEquals(KeyRoute.Key("x", 2), TermKeys.route(TermKeys.A + 23, 'x', false, true, false, false, false, emptyList()))
        assertEquals(KeyRoute.Key("[", 4), TermKeys.route(TermKeys.LEFT_BRACKET, '[', false, false, true, false, false, emptyList()))
        // Plain typing is left to the text field (terminal.text); a lone modifier does nothing.
        assertEquals(KeyRoute.Text, TermKeys.route(TermKeys.A, 'a', false, false, false, false, false, bindings))
        assertEquals(KeyRoute.Consume, TermKeys.route(113, null, false, false, true, false, false, bindings))
        assertEquals(4 or 1 or 2 or 8, TermKeys.mods(shift = true, alt = true, ctrl = true, meta = true))
    }

    @Test
    fun shortcutsViewIsRead() {
        val view = FrameJson.parseToJsonElement("""[{"id":"commander","key":"K","ctrl":true,"shift":false,"alt":false,"inText":true,"inTerminal":true,"repeat":false},{"id":"x"}]""")
        assertEquals(listOf(Shortcut("commander", "K", true, false, false, true, false)), Shortcut.list(view))
    }

    @Test
    fun touchesMapToCellsAndSwipesToWheelNotches() {
        assertEquals(Cell(3, 2, true), TermGestures.cellAt(3.6f * 8, 2.1f * 16, 8f, 16f, 80, 24))
        assertEquals(Cell(79, 23, true), TermGestures.cellAt(10_000f, 10_000f, 8f, 16f, 80, 24))
        assertEquals(Cell(0, 0, false), TermGestures.cellAt(-5f, -5f, 8f, 16f, 80, 24))
        assertEquals(40 to 25, TermGestures.fit(320f, 400f, 8f, 16f))
        assertEquals(2 to 1, TermGestures.fit(1f, 1f, 8f, 16f))
        val w = WheelAccumulator(16f)
        assertEquals(0, w.drag(10f))
        assertEquals(1, w.drag(10f)) // 20 px: one notch, 4 px carried
        assertEquals(2, w.drag(30f))
        assertEquals(-1, w.drag(-20f))
        w.reset(); assertEquals(0, w.drag(-15f))
    }

    @Test
    fun pasteCountsLinesLikeTheControllerAndReportsRefusedReads() {
        assertEquals(1, TermPaste.lines("ls"))
        assertEquals(3, TermPaste.lines("a\r\nb\nc"))
        assertEquals(3, TermPaste.lines("a\rb\n"))
        assertEquals(PasteOutcome.Send("ls -la"), TermPaste.outcome("ls -la", false))
        assertTrue(TermPaste.outcome(null, true) is PasteOutcome.Notice)
        assertEquals(PasteOutcome.Notice("The clipboard holds no text."), TermPaste.outcome("", false))
        assertEquals("Paste 12 lines? They will run.", TermPaste.question(12))
    }
}
