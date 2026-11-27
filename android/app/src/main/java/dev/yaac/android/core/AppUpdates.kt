package dev.yaac.android.core

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.FileProvider
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.Executors

/**
 * Finding, downloading and installing another build of this app (docs/14-android.md, Updates).
 * Network work runs on one worker thread; state changes are posted to the main thread, so
 * Compose reads [state] without locks (invariant 7). Nothing is downloaded or installed without
 * a tap. The APK goes to the cache (`cache/updates/`, covered by `yaac_bridge_paths.xml`) and is
 * handed to the system installer through the app's FileProvider.
 */
object AppUpdates {
    sealed interface State {
        data object Idle : State
        data class Checking(val what: String) : State
        /** [release]'s APK for [build]; [forHub] when it is the hub's build. */
        data class Found(val build: BuildLabel, val release: Release, val apk: ReleaseAsset, val forHub: Boolean) : State
        data class None(val message: String) : State
        data class Downloading(val build: BuildLabel, val done: Long, val total: Long) : State
        data class Ready(val build: BuildLabel, val file: File) : State
        data class Failed(val message: String) : State
    }

    var state: State by mutableStateOf(State.Idle)
        private set
    /** The hub's build and this controller's, from the shell frame (`hub.build`, `build`). */
    var hubBuild: BuildLabel? by mutableStateOf(null)
        private set
    var ownBuild: BuildLabel? by mutableStateOf(null)
        private set
    var hubMismatch: Boolean by mutableStateOf(false)
        private set
    /** The newest release seen by Check for updates, shown beside the hub's. */
    var latest: Release? by mutableStateOf(null)
        private set

    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "yaac-updates").apply { isDaemon = true } }
    private val main = Handler(Looper.getMainLooper())
    private fun post(s: State) = main.post { state = s }

    /** From each shell frame: what this phone and the hub run. */
    fun observe(shell: kotlinx.serialization.json.JsonElement?, fallbackOwn: String) {
        val own = BuildLabel.parse(shell["build"].text) ?: BuildLabel.parse(fallbackOwn)
        val hub = BuildLabel.parse(shell["hub"]["build"].text)
        val mismatch = shell["hub"]["mismatch"].flag() || (hub != null && own != null && hub != own)
        if (own != ownBuild) ownBuild = own
        if (hub != hubBuild) hubBuild = hub
        if (mismatch != hubMismatch) hubMismatch = mismatch
    }

    /** Finds the release of the hub's build (when the hub is known), else the latest. */
    fun check(forHub: Boolean) {
        val hub = hubBuild.takeIf { forHub }
        post(State.Checking(if (hub != null) "Looking for YAAC $hub…" else "Checking for updates…"))
        worker.execute {
            // The latest release is shown beside the hub's, with its warning.
            if (hub != null) runCatching { Updates.parseRelease(get(Updates.LATEST)) }.getOrNull()?.let { r -> main.post { latest = r } }
            post(runCatching {
                if (hub != null) findFor(hub) else findLatest()
            }.getOrElse { State.Failed("Could not reach GitHub: ${it.message ?: it.javaClass.simpleName}") })
        }
    }

    private fun findLatest(): State {
        val release = Updates.parseRelease(get(Updates.LATEST)) ?: return State.Failed("GitHub answered with no release.")
        main.post { latest = release }
        val build = release.build ?: return State.None("The latest release (${release.tag}) has no build number.")
        val apk = Updates.apk(release, build) ?: return State.None("The latest release (YAAC $build) has no Android app.")
        return State.Found(build, release, apk, forHub = false)
    }

    private fun findFor(build: BuildLabel): State {
        // Releases made by Run workflow carry their own tag; a tag push (`v<version>`) does not,
        // so fall back to the list.
        val byTag = runCatching { Updates.parseRelease(get(Updates.tagUrl(build))) }.getOrNull()
        val release = byTag?.takeIf { Updates.apk(it, build) != null }
            ?: Updates.releaseFor(Updates.parseReleases(get(Updates.LIST)), build)
            ?: return State.None("No release of YAAC $build has an Android app. Install it from the PC's build, or update both to the latest release.")
        return State.Found(build, release, Updates.apk(release, build)!!, forHub = true)
    }

    /** Downloads [found]'s APK into the cache, checking its size and, when listed, its SHA-256. */
    fun download(context: Context, found: State.Found) {
        val dir = File(context.cacheDir, "updates")
        post(State.Downloading(found.build, 0, found.apk.size))
        worker.execute {
            post(runCatching { fetch(dir, found) }.getOrElse {
                State.Failed("The download failed: ${it.message ?: it.javaClass.simpleName}")
            })
        }
    }

    private fun fetch(dir: File, found: State.Found): State {
        dir.mkdirs()
        dir.listFiles()?.forEach { it.delete() }
        val expected = found.release.assets.firstOrNull { it.name == "manifest.json" }
            ?.let { m -> runCatching { Updates.manifestSha256(get(m.url), found.apk.name) }.getOrNull() }
        val file = File(dir, "yaac-${found.build.version}-build.${found.build.id}.apk")
        val part = File(dir, file.name + ".part")
        val digest = MessageDigest.getInstance("SHA-256")
        val conn = open(found.apk.url)
        try {
            if (conn.responseCode !in 200..299) error("HTTP ${conn.responseCode}")
            val total = conn.contentLengthLong.takeIf { it > 0 } ?: found.apk.size
            var done = 0L
            var shown = -1L
            conn.inputStream.use { input ->
                part.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        digest.update(buf, 0, n)
                        done += n
                        // Repaint only when the bar moves a visible step (1 %).
                        val step = if (total > 0) done * 100 / total else done / (1 shl 20)
                        if (step != shown) { shown = step; post(State.Downloading(found.build, done, total)) }
                    }
                }
            }
            if (found.apk.size > 0 && done != found.apk.size) error("got $done bytes of ${found.apk.size}")
        } catch (e: Throwable) {
            part.delete()
            throw e
        } finally {
            conn.disconnect()
        }
        val sha = digest.digest().joinToString("") { "%02x".format(it) }
        if (expected != null && sha != expected) {
            part.delete()
            error("its SHA-256 does not match the release's manifest")
        }
        if (!part.renameTo(file)) error("could not save it")
        return State.Ready(found.build, file)
    }

    /**
     * Hands the APK to the system installer. Without "Install unknown apps" for YAAC, opens that
     * setting first; the user comes back and taps Install again.
     */
    fun install(activity: Activity, ready: State.Ready) {
        val pm = activity.packageManager
        if (!pm.canRequestPackageInstalls()) {
            activity.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}")))
            return
        }
        val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.yaac.files", ready.file)
        val intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { activity.startActivity(intent) }
            .onFailure { state = State.Failed("No installer opened the app: ${it.message}") }
    }

    /** Whether "Install unknown apps" is allowed for YAAC. */
    fun canInstall(context: Context): Boolean = context.packageManager.canRequestPackageInstalls()

    fun dismiss() { state = State.Idle }

    private fun open(url: String): HttpURLConnection = (URL(url).openConnection() as HttpURLConnection).apply {
        connectTimeout = 10_000
        readTimeout = 30_000
        instanceFollowRedirects = true
        // The generic agent the desktop updater uses (docs/install.md#updating); no token.
        setRequestProperty("User-Agent", "yaac-updater")
        setRequestProperty("Accept", if (url.startsWith(Updates.API)) "application/vnd.github+json" else "application/octet-stream")
    }

    private fun get(url: String): String {
        val conn = open(url)
        try {
            val code = conn.responseCode
            if (code == 404) error("not found")
            if (code == 403 || code == 429) error("GitHub's rate limit; try again later")
            if (code !in 200..299) error("HTTP $code")
            return conn.inputStream.bufferedReader().use { it.readText() }
        } finally {
            conn.disconnect()
        }
    }
}
