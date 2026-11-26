package dev.yaac.android.core

import android.util.Log

/**
 * The YAAC window controller compiled into the app (`crates/yaac-android`, `libyaac_android.so`).
 * The same controller the Windows renderer drives through pipes; here frames and commands cross
 * JNI. The library is optional while it is being built: [available] says whether it loaded.
 */
object NativeController {
    /**
     * Whether the library is packaged, without loading it: loading 28 MB of native code is for
     * the frame thread, not the UI thread (invariant 7).
     */
    fun packaged(): Boolean =
        (NativeController::class.java.classLoader as? dalvik.system.BaseDexClassLoader)?.findLibrary("yaac_android") != null

    val available: Boolean = try {
        System.loadLibrary("yaac_android")
        true
    } catch (e: UnsatisfiedLinkError) {
        Log.i("yaac", "libyaac_android is not packaged; using the dev transport")
        false
    }

    /** Starts a controller with a JSON config; returns its handle. */
    external fun start(configJson: String): Long

    /** Queues one JSON command (`{"command": …, "payload": …}`). Never blocks on the controller. */
    external fun command(handle: Long, json: String)

    /** The next view frame, or null when none is ready. */
    external fun pollFrame(handle: Long): String?

    /** Waits up to [timeoutMs] for the next view frame. Called on a background thread only. */
    external fun waitFrame(handle: Long, timeoutMs: Long): String?

    external fun stop(handle: Long)
}
