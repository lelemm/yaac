package dev.yaac.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdatesTest {
    private fun asset(name: String) = """{"name":"$name","browser_download_url":"https://github.com/lelemm/yaac/releases/download/t/$name","size":42}"""
    private fun release(tag: String, name: String, vararg assets: String) =
        """{"tag_name":"$tag","name":"$name","draft":false,"assets":[${assets.joinToString(",") { asset(it) }}]}"""

    @Test
    fun build_labels_parse_releases_and_local_builds() {
        assertEquals(BuildLabel("0.1.0", "1049"), BuildLabel.parse("0.1.0+1049"))
        assertEquals(1049L, BuildLabel.parse("v0.1.0+1049")!!.number)
        assertNull(BuildLabel.parse("0.1.0+dev.00ab")!!.number)
        assertEquals("0.2.0-rc.1+7", BuildLabel.parse("0.2.0-rc.1+7").toString())
        assertNull(BuildLabel.parse("0.1.0"))
        assertNull(BuildLabel.parse("0.1.0+"))
        assertNull(BuildLabel.parse("not a build+1"))
        assertNull(BuildLabel.parse(null))
    }

    @Test
    fun a_release_is_known_by_its_title_or_its_build_tag() {
        val byTitle = Updates.parseRelease(release("v0.1.0", "YAAC 0.1.0+1050"))!!
        assertEquals(BuildLabel("0.1.0", "1050"), byTitle.build)
        val byTag = Updates.parseRelease(release("v0.1.0-build.1049", "something else"))!!
        assertEquals(BuildLabel("0.1.0", "1049"), byTag.build)
        assertNull(Updates.parseRelease("""{"tag_name":"v1","draft":true,"assets":[]}"""))
        assertNull(Updates.parseRelease("not json"))
    }

    @Test
    fun the_apk_is_the_one_of_the_wanted_build_in_either_name() {
        val ci = Updates.parseRelease(release("v0.1.0-build.1049", "YAAC 0.1.0+1049", "manifest.json", "yaac.exe", "yaac-android-0.1.0-build.1049.apk"))!!
        assertEquals("yaac-android-0.1.0-build.1049.apk", Updates.apk(ci)!!.name)
        val byHand = Updates.parseRelease(release("v0.1.0-build.1048", "YAAC 0.1.0+1048", "yaac-android-0.1.0+1048.apk"))!!
        assertEquals("yaac-android-0.1.0+1048.apk", Updates.apk(byHand)!!.name)
        // Another build's release never offers its APK for the hub's build.
        assertNull(Updates.apk(ci, BuildLabel("0.1.0", "1048")))
        val windowsOnly = Updates.parseRelease(release("v0.1.0-build.1047", "YAAC 0.1.0+1047", "yaac.exe"))!!
        assertNull(Updates.apk(windowsOnly))
    }

    @Test
    fun the_hubs_release_is_found_in_the_list_by_build() {
        val list = Updates.parseReleases("[" + listOf(
            release("v0.1.0-build.1050", "YAAC 0.1.0+1050", "yaac-android-0.1.0-build.1050.apk"),
            release("v0.1.0", "YAAC 0.1.0+1049", "yaac-android-0.1.0-build.1049.apk"),
            release("v0.1.0-build.1047", "YAAC 0.1.0+1047", "yaac.exe"),
        ).joinToString(",") + "]")
        assertEquals("v0.1.0", Updates.releaseFor(list, BuildLabel("0.1.0", "1049"))!!.tag)
        assertNull(Updates.releaseFor(list, BuildLabel("0.1.0", "1047")))
        assertNull(Updates.releaseFor(list, BuildLabel("0.1.0", "999")))
        assertEquals("https://api.github.com/repos/lelemm/yaac/releases/tags/v0.1.0-build.1049", Updates.tagUrl(BuildLabel("0.1.0", "1049")))
    }

    @Test
    fun releases_are_ordered_by_build_number_and_local_builds_are_oldest() {
        val b1049 = BuildLabel("0.1.0", "1049")
        val b1050 = BuildLabel("0.1.0", "1050")
        assertTrue(Updates.isNewer(b1050, b1049))
        assertFalse(Updates.isNewer(b1049, b1050))
        assertFalse(Updates.isNewer(b1049, b1049))
        assertTrue(Updates.isNewer(b1049, BuildLabel("0.1.0", "dev")))
        assertTrue(Updates.isNewer(b1049, null))
        assertFalse(Updates.isNewer(BuildLabel("0.1.0", "dev"), b1049))
        // Android refuses a lower versionCode over the installed app.
        assertTrue(Updates.needsUninstall(b1049, b1050))
        assertFalse(Updates.needsUninstall(b1050, b1049))
        assertFalse(Updates.needsUninstall(b1049, BuildLabel("0.1.0", "dev")))
    }

    @Test
    fun the_manifest_gives_the_apks_hash_when_it_lists_it() {
        val sha = "ab".repeat(32)
        val manifest = """{"schema":1,"artifacts":[{"name":"yaac.exe","sha256":"${"cd".repeat(32)}"},{"name":"yaac-android-0.1.0-build.1049.apk","component":"android","sha256":"${sha.uppercase()}"}]}"""
        assertEquals(sha, Updates.manifestSha256(manifest, "yaac-android-0.1.0-build.1049.apk"))
        assertNull(Updates.manifestSha256(manifest, "yaac-android-0.1.0+1048.apk"))
        assertNull(Updates.manifestSha256("""{"artifacts":[{"name":"x.apk","sha256":"short"}]}""", "x.apk"))
        assertNull(Updates.manifestSha256("nope", "x.apk"))
    }
}
