package dev.yaac.android.core

import kotlinx.serialization.json.JsonElement

/**
 * The app's updates, as plain values (docs/14-android.md, Updates; unit-tested in
 * `UpdatesTest`): build labels, GitHub releases, and which APK of which release this phone
 * should install. The phone must run the hub's exact build (`hub/hello`), so the release that
 * matters is the one of the hub's build, not the latest. Network and installing are in
 * `AppUpdates`.
 */

/** A YAAC build, `<version>+<id>` (D135): `0.1.0+1049`; local builds have ids like `dev` or `dev.<hash>`. */
data class BuildLabel(val version: String, val id: String) {
    /** The release number, or null for a local build. */
    val number: Long? get() = id.toLongOrNull()
    override fun toString(): String = "$version+$id"

    companion object {
        private val VERSION = Regex("""[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?""")

        fun parse(text: String?): BuildLabel? {
            val s = text?.trim()?.removePrefix("v") ?: return null
            val at = s.indexOf('+')
            if (at <= 0 || at == s.length - 1) return null
            val version = s.substring(0, at)
            val id = s.substring(at + 1)
            if (!VERSION.matches(version) || id.any { it.isWhitespace() || it == '+' }) return null
            return BuildLabel(version, id)
        }
    }
}

data class ReleaseAsset(val name: String, val url: String, val size: Long)

data class Release(val tag: String, val name: String, val assets: List<ReleaseAsset>) {
    /** The build this release is: its title ("YAAC 0.1.0+1049"), else its tag ("v0.1.0-build.1049"). */
    val build: BuildLabel?
        get() = BuildLabel.parse(name.removePrefix("YAAC").trim())
            ?: Updates.TAG.matchEntire(tag)?.let { BuildLabel(it.groupValues[1], it.groupValues[2]) }
}

object Updates {
    const val REPO = "lelemm/yaac"
    const val API = "https://api.github.com/repos/$REPO"
    const val LATEST = "$API/releases/latest"
    /** The newest releases, to find one by build when its tag is not `v<version>-build.<id>`. */
    const val LIST = "$API/releases?per_page=100"
    /** A release made by *Run workflow* (`.github/workflows/release.yml`). */
    val TAG = Regex("""v([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+?)?)-build\.([0-9]+)""")

    /** Where the release of [build] is when the workflow tagged it itself. */
    fun tagUrl(build: BuildLabel): String = "$API/releases/tags/v${build.version}-build.${build.id}"

    /** The APK names a release of [build] may carry: the workflow's, and the hand-made 1048's. */
    fun apkNames(build: BuildLabel): List<String> =
        listOf("yaac-android-${build.version}-build.${build.id}.apk", "yaac-android-$build.apk")

    fun parseRelease(json: JsonElement?): Release? {
        val tag = json["tag_name"].text ?: return null
        if (json["draft"].flag()) return null
        val assets = json["assets"].objects.mapNotNull { a ->
            val name = a["name"].text ?: return@mapNotNull null
            val url = a["browser_download_url"].text ?: return@mapNotNull null
            ReleaseAsset(name, url, a["size"].long ?: 0)
        }
        return Release(tag, json["name"].text ?: "", assets)
    }

    fun parseRelease(text: String): Release? = runCatching { parseRelease(FrameJson.parseToJsonElement(text)) }.getOrNull()

    fun parseReleases(text: String): List<Release> =
        runCatching { FrameJson.parseToJsonElement(text).list.mapNotNull { parseRelease(it) } }.getOrDefault(emptyList())

    /** The APK of [release] for [build] (the release's own build when null); null when it has none. */
    fun apk(release: Release, build: BuildLabel? = release.build): ReleaseAsset? {
        val b = build ?: return null
        if (release.build != null && release.build != b) return null
        val names = apkNames(b)
        return names.firstNotNullOfOrNull { n -> release.assets.firstOrNull { it.name == n } }
    }

    /** The release of exactly [build] among [releases] that carries an APK. */
    fun releaseFor(releases: List<Release>, build: BuildLabel): Release? =
        releases.firstOrNull { it.build == build && apk(it, build) != null }

    /**
     * Whether [candidate] is a later release than [current]. A local build (no number) is
     * older than every release; two local builds are never ordered.
     */
    fun isNewer(candidate: BuildLabel, current: BuildLabel?): Boolean {
        val c = candidate.number ?: return false
        val now = current?.number ?: return true
        return c > now
    }

    /**
     * Android installs only a higher or equal versionCode (the build number) over the installed
     * app: going back to an older hub's build needs an uninstall first.
     */
    fun needsUninstall(target: BuildLabel, current: BuildLabel?): Boolean {
        val t = target.number ?: return false
        val now = current?.number ?: return false
        return t < now
    }

    /** The SHA-256 of [assetName] in a release's `manifest.json` (docs/install.md#the-manifest), if listed. */
    fun manifestSha256(manifest: String, assetName: String): String? = runCatching {
        FrameJson.parseToJsonElement(manifest)["artifacts"].objects
            .firstOrNull { it["name"].text == assetName }?.get("sha256").text
            ?.lowercase()?.takeIf { it.length == 64 && it.all { ch -> ch in '0'..'9' || ch in 'a'..'f' } }
    }.getOrNull()
}
