package dev.yaac.android.core

import android.util.Log
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue

/**
 * Carries newline-free JSON frames from a controller and JSON commands to it. The UI does not
 * know which transport is in use. Callbacks arrive on a background thread.
 */
interface FrameTransport {
    fun start(onFrame: (String) -> Unit, onStatus: (String?) -> Unit)
    fun send(json: String)
    fun stop()
}

/**
 * In-process: the controller in `libyaac_android.so`. Starting it can take a while, so it starts
 * on the frame thread, never the UI thread (invariant 7); commands sent before then wait, in
 * order, and are handed over once the handle exists.
 */
class NativeTransport(private val config: String) : FrameTransport {
    @Volatile private var handle = 0L
    @Volatile private var running = false
    private val early = ArrayList<String>()
    private var reader: Thread? = null

    override fun start(onFrame: (String) -> Unit, onStatus: (String?) -> Unit) {
        running = true
        reader = Thread({
            val h = try {
                if (!NativeController.available) error("libyaac_android could not be loaded")
                NativeController.start(config)
            } catch (e: Throwable) {
                // Reported as the library reports its own end, so the UI has one failure path.
                onFrame(jsonOf("stopped" to mapOf("error" to (e.message ?: e.javaClass.simpleName))).toString()); return@Thread
            }
            synchronized(early) {
                handle = h
                early.forEach { command(h, it) }
                early.clear()
            }
            var first = true
            while (running) {
                val frame = try { NativeController.waitFrame(h, 500) } catch (e: Throwable) {
                    onStatus("The controller stopped: ${e.message}"); null
                } ?: continue
                if (first) { onStatus(null); first = false }
                onFrame(frame)
            }
        }, "yaac-frames").apply { isDaemon = true; start() }
    }

    override fun send(json: String) {
        val h = handle
        if (h != 0L) { command(h, json); return }
        synchronized(early) { if (handle == 0L) early += json else command(handle, json) }
    }

    private fun command(h: Long, json: String) {
        try { NativeController.command(h, json) } catch (e: IllegalArgumentException) { Log.w("yaac", "command refused: ${e.message}") }
    }

    override fun stop() {
        running = false
        val h = handle
        handle = 0
        // stop() flushes drafts to the hub and waits; keep it off the UI thread too.
        if (h != 0L) Thread({ NativeController.stop(h) }, "yaac-stop").start()
    }
}

/**
 * Development: a desktop-built controller (`yaac --wpf-controller fleet dark`) bridged to TCP and
 * reached through `adb reverse`. Each connection is one controller. It reconnects after a drop.
 */
class TcpTransport(private val host: String, private val port: Int) : FrameTransport {
    @Volatile private var running = false
    @Volatile private var socket: Socket? = null
    private val outbox = LinkedBlockingQueue<String>()
    private val writer = Executors.newSingleThreadExecutor { r -> Thread(r, "yaac-commands").apply { isDaemon = true } }

    override fun start(onFrame: (String) -> Unit, onStatus: (String?) -> Unit) {
        running = true
        Thread({
            var delay = 500L
            while (running) {
                try {
                    onStatus("Connecting to the controller at $host:$port…")
                    val s = Socket()
                    s.connect(InetSocketAddress(host, port), 3000)
                    s.tcpNoDelay = true
                    socket = s
                    outbox.clear() // never replay commands meant for an earlier controller
                    val reader = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.UTF_8), 1 shl 16)
                    val out = OutputStreamWriter(s.getOutputStream(), Charsets.UTF_8)
                    writer.execute { drain(out) }
                    var first = true
                    while (running) {
                        val line = reader.readLine() ?: break
                        if (first) { onStatus(null); first = false; delay = 500 }
                        if (line.isNotBlank()) onFrame(line)
                    }
                } catch (e: Exception) {
                    Log.w("yaac", "dev transport: ${e.message}")
                }
                socket?.runCatching { close() }
                socket = null
                if (!running) break
                onStatus("No controller at $host:$port. Retrying…")
                Thread.sleep(delay)
                delay = (delay * 2).coerceAtMost(5000)
            }
        }, "yaac-frames").apply { isDaemon = true; start() }
    }

    private fun drain(out: OutputStreamWriter) {
        try {
            while (running && socket != null) {
                val next = outbox.poll(250, java.util.concurrent.TimeUnit.MILLISECONDS) ?: continue
                out.write(next); out.write("\n"); out.flush()
            }
        } catch (e: Exception) {
            Log.w("yaac", "dev transport write: ${e.message}")
        }
    }

    override fun send(json: String) { outbox.offer(json) }

    override fun stop() {
        running = false
        socket?.runCatching { close() }
        writer.shutdownNow()
    }
}
