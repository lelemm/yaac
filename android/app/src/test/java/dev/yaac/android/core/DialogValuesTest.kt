package dev.yaac.android.core

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DialogValuesTest {
    private val layout: JsonElement = Json.parseToJsonElement(
        """{"type":"column","children":[
            {"type":"text","text":"Ship it?"},
            {"type":"select","id":"env","options":[{"value":"prod","label":"Production"}],"default":"prod"},
            {"type":"textInput","id":"token","secret":true},
            {"type":"textInput","id":"note","default":"hi"},
            {"type":"number","id":"count","default":3},
            {"type":"checkbox","id":"dry","default":true},
            {"type":"multiSelect","id":"tags","options":[],"default":["a"]},
            {"type":"row","children":[{"type":"button","id":"go","label":"Go"}]},
            {"type":"sparkles","id":"future"}]}""")

    @Test fun defaultsStartTheDraftAndASecretStartsEmpty() {
        val v = DialogValues.initial(layout)
        assertEquals("prod", v["env"])
        assertEquals("", v["token"])
        assertEquals("hi", v["note"])
        assertEquals("3", v["count"])
        assertEquals(true, v["dry"])
        assertEquals(listOf("a"), v["tags"])
        assertFalse("buttons and unknown parts carry no value", v.containsKey("go") || v.containsKey("future"))
    }

    @Test fun aPressSendsOnlyThisLayoutsInputs() {
        val sent = DialogValues.payload(layout, mapOf("env" to "prod", "token" to "s3cret", "stale" to "x"))
        assertEquals(setOf("env", "token"), sent.keys)
    }

    @Test fun anInboxLocationIsNotAThread() {
        assertTrue(DialogValues.isInbox("devbox/~inbox"))
        assertFalse(DialogValues.isInbox("devbox/t1"))
        assertFalse(DialogValues.isInbox(null))
    }
}
