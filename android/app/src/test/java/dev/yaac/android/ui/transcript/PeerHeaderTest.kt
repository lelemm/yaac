package dev.yaac.android.ui.transcript

import org.junit.Assert.assertEquals
import org.junit.Test

class PeerHeaderTest {
    @Test
    fun runsAreFromTitleAndTarget() {
        val runs = peerHeaderRuns(null, "Interface Android para o YAAC (fork)", "wsl: Ubuntu")
        assertEquals(
            listOf(HeaderRun("From ", false), HeaderRun("Interface Android para o YAAC (fork)", true), HeaderRun(" on wsl: Ubuntu", false)),
            runs,
        )
        assertEquals("From Interface Android para o YAAC (fork) on wsl: Ubuntu", runs.joinToString("") { it.text })
    }

    @Test
    fun blankTargetAndHeadingFallBack() {
        assertEquals(listOf(HeaderRun("From ", false), HeaderRun("A", true)), peerHeaderRuns(" ", "A", ""))
        assertEquals("Reply from A", peerHeaderRuns("Reply from", "A", null).joinToString("") { it.text })
    }
}
