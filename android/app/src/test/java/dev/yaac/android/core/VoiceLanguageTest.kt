package dev.yaac.android.core

import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceLanguageTest {
    @Test
    fun `the phone's locale becomes a BCP-47 tag`() {
        assertEquals("pt-BR", VoiceLanguage.bcp47(Locale.forLanguageTag("pt-BR")))
        assertEquals("pt-BR", VoiceLanguage.bcp47(Locale("pt", "BR")))
        assertEquals("es-419", VoiceLanguage.bcp47(Locale.forLanguageTag("es-419")))
        assertEquals("zh-Hans-CN", VoiceLanguage.bcp47(Locale.forLanguageTag("zh-Hans-CN")))
        assertEquals("de", VoiceLanguage.bcp47(Locale.GERMAN))
        assertNull(VoiceLanguage.bcp47(Locale.ROOT))
    }

    @Test
    fun `the stored choice wins and blank follows the phone`() {
        val phone = Locale.forLanguageTag("pt-BR")
        assertEquals("pt-BR", VoiceLanguage.effective("", phone))
        assertEquals("fr-FR", VoiceLanguage.effective("fr-FR", phone))
        assertNull(VoiceLanguage.effective("", Locale.ROOT))
        assertEquals("Same as this device (Português (Brasil))", VoiceLanguage.label("", phone))
        assertEquals("Deutsch", VoiceLanguage.label("de-DE", phone))
    }

    @Test
    fun `listed tags are canonical`() {
        for ((tag, _) in VoiceLanguage.choices) assertEquals(tag, Locale.forLanguageTag(tag).toLanguageTag())
        assertTrue(VoiceLanguage.choices.any { it.first == "pt-BR" })
    }
}
