package dev.yaac.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Settings → About → Update YAAC on the PC (docs/install.md#updating-from-the-phone). */
class PcUpdateTest {
    private fun b(s: String) = BuildLabel.parse(s)

    @Test
    fun the_pc_is_offered_only_this_phones_newer_release_build() {
        assertTrue(Updates.offersPcUpdate(b("0.1.0+1060"), b("0.1.0+1050")))
        assertFalse("the same build", Updates.offersPcUpdate(b("0.1.0+1060"), b("0.1.0+1060")))
        assertFalse("never a downgrade", Updates.offersPcUpdate(b("0.1.0+1049"), b("0.1.0+1050")))
        assertFalse("a local phone build", Updates.offersPcUpdate(b("0.1.0+dev.ab"), b("0.1.0+1050")))
        assertFalse("a development hub", Updates.offersPcUpdate(b("0.1.0+1060"), b("0.1.0+dev.cd")))
        assertFalse("no hub yet", Updates.offersPcUpdate(b("0.1.0+1060"), null))
    }

    @Test
    fun the_button_asks_before_it_sends() {
        val action = FrameJson.parseToJsonElement(
            """{"id":"pc.update","label":"Update YAAC on desk to build 1060","kind":"primary",
               "confirm":{"title":"Update YAAC on desk?","text":"desk downloads YAAC 0.1.0+1060…","ok":"Update"}}""")
        assertEquals(ActionConfirm("Update YAAC on desk?", "desk downloads YAAC 0.1.0+1060…", "Update"), ActionConfirm.parse(action))
        assertNull(ActionConfirm.parse(FrameJson.parseToJsonElement("""{"id":"about.refresh","label":"Refresh"}""")))
    }
}
