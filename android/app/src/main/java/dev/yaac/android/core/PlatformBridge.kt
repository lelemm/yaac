package dev.yaac.android.core

import android.app.Activity
import android.app.AlertDialog
import android.app.Application
import android.content.ActivityNotFoundException
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.system.Os
import android.util.Log
import android.webkit.MimeTypeMap
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContract
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.lang.ref.WeakReference
import java.util.UUID
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * What the Rust controller asks of the device: file pickers, saving, the clipboard, opening
 * links and documents (`yaac_app::platform::PlatformHooks`, answered from
 * `crates/yaac-android/src/bridge.rs` through JNI).
 *
 * The `@JvmStatic` methods are called on the controller's worker threads and block until the
 * user is done; the system UI runs on the main thread and completes a future. They never run
 * on the main thread (a call from it answers "cancelled" at once instead of deadlocking).
 * Every path returned is a local copy in the app's cache, because the controller reads files
 * by path and `content://` URIs are not paths. The controller decides what an image is and how
 * it is sent (invariant 6); this only delivers files.
 *
 * Call [init] once, in the activity's `onCreate`, before the controller starts.
 * [openDocument] needs the FileProvider declared in the manifest (authority
 * `${applicationId}.yaac.files`, paths `@xml/yaac_bridge_paths`).
 */
object PlatformBridge {
    private const val TAG = "yaac-bridge"
    /** The controller refuses larger files; a copy stops one byte past it. */
    private const val MAX_COPY = 64L * 1024 * 1024 + 1
    /** A folder import (a skill) copies at most this many files. */
    private const val MAX_FOLDER_FILES = 2000

    private val main = Handler(Looper.getMainLooper())
    private val sequence = AtomicInteger()
    private val pending = mutableSetOf<CompletableFuture<*>>()
    @Volatile private var app: Context? = null
    @Volatile private var current: WeakReference<ComponentActivity>? = null

    /** The current activity (for views and dialogs), else the application. */
    fun context(): Context? = current?.get() ?: app

        /** Where the controller keeps its own files (`dataDir` for hub mode, fonts). */
    val dataDir: String get() = (app ?: error("PlatformBridge.init was not called")).filesDir.absolutePath

    /**
     * Points the controller's folders at the app's storage (the system temporary folder is not
     * writable by apps) and follows the resumed activity for pickers and dialogs.
     */
    fun init(activity: ComponentActivity) {
        val context = activity.applicationContext
        if (app == null) {
            app = context
            val cache = context.cacheDir
            Os.setenv("TMPDIR", cache.absolutePath, true)
            Os.setenv("YAAC_DATA_DIR", context.filesDir.absolutePath, true)
            Os.setenv("YAAC_CONFIG_DIR", File(context.filesDir, "config").absolutePath, true)
            Os.setenv("YAAC_RUNTIME_DIR", File(cache, "run").absolutePath, true)
            File(cache, "picked").deleteRecursively()
            (context as Application).registerActivityLifecycleCallbacks(Tracker)
        }
        current = WeakReference(activity)
    }

    private object Tracker : Application.ActivityLifecycleCallbacks {
        override fun onActivityResumed(activity: Activity) {
            if (activity is ComponentActivity) current = WeakReference(activity)
        }
        override fun onActivityDestroyed(activity: Activity) {
            if (current?.get() === activity) {
                current = null
                // A picker's answer would go to the destroyed activity: give up on it.
                synchronized(pending) { pending.toList() }.forEach { it.complete(null) }
            }
        }
        override fun onActivityCreated(activity: Activity, state: Bundle?) {}
        override fun onActivityStarted(activity: Activity) {}
        override fun onActivityPaused(activity: Activity) {}
        override fun onActivityStopped(activity: Activity) {}
        override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) {}
    }

    // ---- Called from Rust -------------------------------------------------------------------

    /** Attach: photos through the photo picker, anything else through the document picker. */
    @JvmStatic
    fun pickFiles(title: String): Array<String>? {
        val choice = onMain<Int> { activity, done ->
            AlertDialog.Builder(activity)
                .setTitle(title)
                .setItems(arrayOf("Photos", "Files")) { _, which -> done.complete(which) }
                .setOnCancelListener { done.complete(null) }
                .show()
        } ?: return null
        val uris: List<Uri> = when (choice) {
            0 -> launch(ActivityResultContracts.PickMultipleVisualMedia(),
                PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
            else -> launch(ActivityResultContracts.OpenMultipleDocuments(), arrayOf("*/*"))
        } ?: return null
        return uris.mapNotNull(::copyIn).toTypedArray().takeIf { it.isNotEmpty() }
    }

    @JvmStatic
    fun pickFile(title: String, filterName: String?, extensions: Array<String>): String? {
        val uri = launch(ActivityResultContracts.OpenDocument(), mimeTypes(extensions)) ?: return null
        return copyIn(uri)
    }

    /** A folder (a skill to import), copied with what is under it. */
    @JvmStatic
    fun pickFolder(title: String): String? {
        val tree = launch(ActivityResultContracts.OpenDocumentTree(), null) ?: return null
        val context = app ?: return null
        val root = DocumentsContract.getTreeDocumentId(tree)
        val name = safeName(queryName(DocumentsContract.buildDocumentUriUsingTree(tree, root)) ?: "folder")
        val dest = File(freshDir(context), name)
        return try {
            copyTree(context, tree, root, dest, intArrayOf(0))
            dest.absolutePath
        } catch (e: Exception) {
            Log.w(TAG, "could not copy the folder", e)
            null
        }
    }

    /** Save `staged` (already written by the controller) where the user chooses. */
    @JvmStatic
    fun saveFile(name: String, filterName: String?, extensions: Array<String>, staged: String): Boolean {
        val mime = mimeOf(name.substringAfterLast('.', ""))
        val uri = launch(ActivityResultContracts.CreateDocument(mime), name) ?: return false
        val context = app ?: return false
        val out: OutputStream = context.contentResolver.openOutputStream(uri, "wt") ?: return false
        out.use { o -> File(staged).inputStream().use { it.copyTo(o) } }
        return true
    }

    @JvmStatic
    fun message(title: String, text: String) {
        main.post {
            val activity = current?.get() ?: return@post
            AlertDialog.Builder(activity).setTitle(title).setMessage(text).setPositiveButton("OK", null).show()
        }
    }

    @JvmStatic
    fun clipboardText(): String? = onMain<String> { activity, done ->
        val clip = clipboard(activity)?.primaryClip
        done.complete(clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(activity)?.toString())
    }

    /** A copied picture or document (a clip item with a `content://` URI), as local copies. */
    @JvmStatic
    fun clipboardFiles(): Array<String>? {
        val uris = onMain<List<Uri>> { activity, done ->
            val clip = clipboard(activity)?.primaryClip
            done.complete((0 until (clip?.itemCount ?: 0)).mapNotNull { clip?.getItemAt(it)?.uri }
                .filter { it.scheme == "content" })
        } ?: return null
        return uris.mapNotNull(::copyIn).toTypedArray().takeIf { it.isNotEmpty() }
    }

    @JvmStatic
    fun openUrl(url: String): Boolean = start(Intent(Intent.ACTION_VIEW, Uri.parse(url)))

    @JvmStatic
    fun openDocument(path: String): Boolean {
        val context = app ?: return false
        val uri = try {
            FileProvider.getUriForFile(context, "${context.packageName}.yaac.files", File(path))
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "the FileProvider is not declared or does not cover $path", e)
            return false
        }
        val intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, mimeOf(path.substringAfterLast('.', "")))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        return start(intent)
    }

    // ---- Helpers ----------------------------------------------------------------------------

    private fun start(intent: Intent): Boolean {
        val context = current?.get() ?: app ?: return false
        if (context !is Activity) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            context.startActivity(intent)
            true
        } catch (e: ActivityNotFoundException) {
            false
        }
    }

    /** Runs `body` on the main thread with the current activity and waits for its answer. */
    private fun <T> onMain(body: (ComponentActivity, CompletableFuture<T?>) -> Unit): T? {
        if (Looper.myLooper() == Looper.getMainLooper()) return null
        val done = CompletableFuture<T?>()
        synchronized(pending) { pending += done }
        main.post {
            val activity = current?.get()
            if (activity == null) done.complete(null)
            else try { body(activity, done) } catch (e: Exception) {
                Log.w(TAG, "the system UI failed", e)
                done.complete(null)
            }
        }
        return try {
            done.get(30, TimeUnit.MINUTES)
        } catch (e: Exception) {
            null
        } finally {
            synchronized(pending) { pending -= done }
        }
    }

    /** Launches a system activity for a result, registered for this one call. */
    private fun <I, O> launch(contract: ActivityResultContract<I, O>, input: I): O? = onMain { activity, done ->
        var launcher: ActivityResultLauncher<I>? = null
        launcher = activity.activityResultRegistry.register("yaac-bridge-${sequence.incrementAndGet()}", contract) {
            launcher?.unregister()
            done.complete(it)
        }
        try {
            launcher.launch(input)
        } catch (e: ActivityNotFoundException) {
            launcher.unregister()
            done.complete(null)
        }
    }

    private fun clipboard(context: Context) = context.getSystemService(ClipboardManager::class.java)

    private fun freshDir(context: Context) = File(context.cacheDir, "picked/${UUID.randomUUID()}").apply { mkdirs() }

    private fun safeName(name: String) = name.replace('/', '_').replace('\u0000', '_').ifBlank { "file" }

    private fun queryName(uri: Uri): String? = app?.contentResolver?.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }

    /**
     * The name a chip shows. The photo picker hides a photo's file name by design (its
     * DISPLAY_NAME is the media id, "19.png"), so a picked photo is named the way a pasted
     * screenshot is on the desktop: `photo-<date and time taken>.<ext>`.
     */
    private fun nameOf(context: Context, uri: Uri): String {
        val name = queryName(uri) ?: "file"
        val fromPicker = uri.authority?.contains("photopicker") == true || uri.pathSegments.contains("picker")
        if (!fromPicker) return name
        val taken = try {
            context.contentResolver.query(uri, arrayOf(MediaStore.MediaColumns.DATE_TAKEN), null, null, null)
                ?.use { c -> if (c.moveToFirst() && !c.isNull(0)) c.getLong(0) else null }
        } catch (e: Exception) {
            null
        } ?: System.currentTimeMillis()
        val ext = name.substringAfterLast('.', "").ifEmpty {
            MimeTypeMap.getSingleton().getExtensionFromMimeType(context.contentResolver.getType(uri)) ?: "jpg"
        }
        val stamp = java.text.SimpleDateFormat("yyyy-MM-dd-HHmm", java.util.Locale.ROOT).format(java.util.Date(taken))
        return "photo-$stamp.$ext"
    }

    /** A picked document as a local file, its name kept (the controller shows it on the chip). */
    private fun copyIn(uri: Uri): String? {
        val context = app ?: return null
        return try {
            val out = File(freshDir(context), safeName(nameOf(context, uri)))
            val input: InputStream = context.contentResolver.openInputStream(uri) ?: return null
            input.use { i -> out.outputStream().use { o -> copyLimited(i, o) } }
            out.absolutePath
        } catch (e: Exception) {
            Log.w(TAG, "could not copy $uri", e)
            null
        }
    }

    private fun copyLimited(input: InputStream, output: OutputStream) {
        val buffer = ByteArray(64 * 1024)
        var left = MAX_COPY
        while (left > 0) {
            val n = input.read(buffer, 0, minOf(buffer.size.toLong(), left).toInt())
            if (n < 0) break
            output.write(buffer, 0, n)
            left -= n
        }
    }

    private fun copyTree(context: Context, tree: Uri, id: String, dest: File, count: IntArray) {
        dest.mkdirs()
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, id)
        val columns = arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_MIME_TYPE)
        context.contentResolver.query(children, columns, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val childId = c.getString(0)
                val name = safeName(c.getString(1) ?: continue)
                if (c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    copyTree(context, tree, childId, File(dest, name), count)
                } else {
                    if (++count[0] > MAX_FOLDER_FILES) throw IllegalStateException("the folder has too many files")
                    val uri = DocumentsContract.buildDocumentUriUsingTree(tree, childId)
                    context.contentResolver.openInputStream(uri)?.use { i -> File(dest, name).outputStream().use { o -> copyLimited(i, o) } }
                }
            }
        }
    }

    private fun mimeOf(extension: String): String =
        MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension.lowercase()) ?: "application/octet-stream"

    /** The document picker's types for these extensions, or everything if one is unknown. */
    private fun mimeTypes(extensions: Array<String>): Array<String> {
        val types = extensions.map { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it.lowercase()) }
        return if (types.isEmpty() || types.any { it == null }) arrayOf("*/*") else types.filterNotNull().distinct().toTypedArray()
    }
}
