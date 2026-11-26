package dev.yaac.android.core

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Base64
import android.util.Log
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.webkit.ConsoleMessage
import android.webkit.CookieManager
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.util.UUID
import java.util.concurrent.Executor

/**
 * The Android browser host (P380, docs/11-preview.md "Android"): one WebView per browser tab
 * of the Rust controller, which keeps the tabs, the agent calls and the network.
 *
 * Wiring (A3):
 * - [send] is how the host talks to the controller: set it to `NativeController.command(h, it)`.
 * - Every view frame goes through [onFrame] first (it consumes `browserHost` and
 *   `browserHostCall`; other frames are ignored here).
 * - The active browser tab is shown with `AndroidView(factory = { BrowserHost.viewFor(tabId)!! })`
 *   after detaching it from any previous parent ([viewFor] does that). [visibleTab] is the tab the
 *   controller says is on screen.
 *
 * All traffic goes through the controller's SOCKS endpoint (the process-wide proxy override,
 * no bypass, loopback included), so `localhost` is the thread's target, never the phone.
 * WebViews exist only for the live target's tabs; a target change destroys them and clears
 * cookies and storage. Everything here runs on the main thread.
 */
object BrowserHost {
    private const val TAG = "yaac-browser"
    private const val LOAD_LIMIT_MS = 20_000L
    private const val LOAD_GRACE_MS = 1_500L
    private const val MAX_ENTRIES = 1000
    /** Off-screen size for a tab an agent drives while it is not shown. */
    private const val HIDDEN_W = 1080
    private const val HIDDEN_H = 1920

    @Volatile var send: ((String) -> Unit)? = null
    /** The tab the controller says is on screen, or null. */
    @Volatile var visibleTab: Long? = null
        private set

    private val main = Handler(Looper.getMainLooper())
    private val mainExecutor = Executor { main.post(it) }
    private val tabs = HashMap<Long, Tab>()
    private var proxy: String? = null
    private var target: String? = null
    /** Results posted by pages, by one-time token (a page cannot answer a call it was not asked). */
    private val waiting = HashMap<String, (JSONObject) -> Unit>()

    private class Tab(val id: Long, val view: WebView) {
        var nav = 0L
        var loading = false
        var lastActivity = SystemClock.uptimeMillis()
        val console = ArrayList<Pair<Long, JSONObject>>()
        val network = ArrayList<Pair<Long, JSONObject>>()
        var seq = 0L
        val loadWaiters = ArrayList<() -> Unit>()
    }

    /** The WebView of a tab, detached from any parent, or null if it does not run now. */
    fun viewFor(tab: Long): View? {
        val v = tabs[tab]?.view ?: return null
        (v.parent as? ViewGroup)?.removeView(v)
        return v
    }

    /** A view frame from the controller (any thread). */
    fun onFrame(frame: String) {
        if (!frame.contains("\"browserHost")) return
        val json = try { JSONObject(frame) } catch (e: Exception) { return }
        json.optJSONObject("browserHost")?.let { s -> main.post { applyState(s) } }
        json.optJSONObject("browserHostCall")?.let { c -> main.post { onCall(c) } }
    }

    private fun command(name: String, payload: JSONObject) {
        val s = send ?: return
        s(JSONObject().put("command", name).put("payload", payload).toString())
    }

    private fun event(tab: Tab, extra: JSONObject.() -> Unit) {
        val p = JSONObject().put("tab", tab.id)
        p.extra()
        command("browser.hostEvent", p)
    }

    // ---- State ----------------------------------------------------------------------------

    private fun applyState(s: JSONObject) {
        val newProxy = s.optString("proxy").takeIf { s.has("proxy") && !s.isNull("proxy") }
        val newTarget = s.optString("target").takeIf { s.has("target") && !s.isNull("target") }
        if (newTarget != target) {
            // Another machine: nothing of the old one may run or keep state.
            tabs.values.forEach { it.view.destroy() }
            tabs.clear()
            CookieManager.getInstance().removeAllCookies(null)
            WebStorage.getInstance().deleteAllData()
            target = newTarget
        }
        if (newProxy != proxy) {
            proxy = newProxy
            setProxy(newProxy)
        }
        val listed = HashSet<Long>()
        val entries = s.optJSONArray("tabs") ?: JSONArray()
        for (i in 0 until entries.length()) {
            val e = entries.getJSONObject(i)
            val id = e.getLong("tab")
            listed += id
            val nav = e.optLong("nav")
            val tab = tabs[id] ?: newTab(id) ?: continue
            if (nav > tab.nav) {
                tab.nav = nav
                when (e.optString("history").takeIf { !e.isNull("history") }) {
                    "back" -> if (tab.view.canGoBack()) tab.view.goBack()
                    "forward" -> if (tab.view.canGoForward()) tab.view.goForward()
                    "reload" -> tab.view.reload()
                    "stop" -> tab.view.stopLoading()
                    else -> tab.view.loadUrl(e.getString("url"))
                }
            }
        }
        (tabs.keys - listed).forEach { tabs.remove(it)?.view?.destroy() }
        visibleTab = if (s.isNull("visible")) null else s.optLong("visible")
    }

    private fun setProxy(url: String?) {
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.PROXY_OVERRIDE)) {
            Log.w(TAG, "this WebView cannot use a proxy; browser tabs stay empty")
            return
        }
        val controller = ProxyController.getInstance()
        if (url == null) {
            controller.clearProxyOverride(mainExecutor) {}
            return
        }
        // Every scheme to the controller's endpoint; the implicit loopback bypass removed, so
        // `localhost` is the target's.
        val config = ProxyConfig.Builder().addProxyRule(url).removeImplicitRules().build()
        controller.setProxyOverride(config, mainExecutor) { Log.i(TAG, "proxy set") }
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun newTab(id: Long): Tab? {
        if (proxy == null) return null
        val context: Context = PlatformBridge.context() ?: return null
        if (dev.yaac.android.BuildConfig.DEBUG) WebView.setWebContentsDebuggingEnabled(true)
        val view = WebView(context)
        val tab = Tab(id, view)
        tabs[id] = tab
        view.settings.javaScriptEnabled = true
        view.settings.domStorageEnabled = true
        if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
            WebViewCompat.addDocumentStartJavaScript(view, START_SCRIPT, setOf("*"))
        }
        if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            WebViewCompat.addWebMessageListener(view, "yaacHost", setOf("*")) { _: WebView, message: WebMessageCompat, _: Uri, _: Boolean, _: JavaScriptReplyProxy ->
                val m = try { JSONObject(message.data ?: return@addWebMessageListener) } catch (e: Exception) { return@addWebMessageListener }
                when (m.optString("type")) {
                    "result" -> waiting.remove(m.optString("token"))?.invoke(m)
                    "net" -> record(tab.network, tab, m.optJSONObject("entry") ?: return@addWebMessageListener)
                }
            }
        }
        view.webViewClient = object : WebViewClient() {
            override fun onPageStarted(v: WebView, url: String, favicon: Bitmap?) {
                tab.loading = true
                tab.lastActivity = SystemClock.uptimeMillis()
                event(tab) { put("url", url).put("loading", true).put("error", JSONObject.NULL) }
            }
            override fun onPageFinished(v: WebView, url: String) {
                tab.loading = false
                tab.lastActivity = SystemClock.uptimeMillis()
                event(tab) { put("url", url).put("title", v.title ?: "").put("loading", false).put("back", v.canGoBack()).put("forward", v.canGoForward()) }
                val w = ArrayList(tab.loadWaiters)
                tab.loadWaiters.clear()
                w.forEach { it() }
            }
            override fun onReceivedError(v: WebView, request: WebResourceRequest, error: WebResourceError) {
                val entry = JSONObject().put("url", request.url.toString()).put("method", request.method).put("error", error.description.toString())
                record(tab.network, tab, entry)
                if (request.isForMainFrame) event(tab) { put("error", error.description.toString()) }
            }
            override fun shouldOverrideUrlLoading(v: WebView, request: WebResourceRequest) = false
        }
        view.webChromeClient = object : WebChromeClient() {
            override fun onReceivedTitle(v: WebView, title: String) {
                event(tab) { put("title", title) }
            }
            override fun onConsoleMessage(m: ConsoleMessage): Boolean {
                val level = when (m.messageLevel()) {
                    ConsoleMessage.MessageLevel.ERROR -> "error"
                    ConsoleMessage.MessageLevel.WARNING -> "warning"
                    ConsoleMessage.MessageLevel.DEBUG -> "debug"
                    else -> "log"
                }
                record(tab.console, tab, JSONObject().put("level", level).put("text", m.message()))
                return true
            }
        }
        return tab
    }

    private fun record(list: ArrayList<Pair<Long, JSONObject>>, tab: Tab, entry: JSONObject) {
        tab.seq += 1
        list += tab.seq to entry
        if (list.size > MAX_ENTRIES) list.subList(0, list.size - MAX_ENTRIES).clear()
    }

    // ---- Agent calls ----------------------------------------------------------------------

    private fun onCall(c: JSONObject) {
        val call = c.getLong("call")
        val action = c.getJSONObject("action")
        val kind = action.getString("kind")
        val tab = tabs[c.getLong("tab")]
        fun ok(result: JSONObject) = command("browser.hostResult", JSONObject().put("call", call).put("result", result.put("kind", kind)))
        fun fail(message: String) = command("browser.hostResult", JSONObject().put("call", call).put("error", message))
        if (tab == null) return fail("This tab is not running on the phone now")
        tab.lastActivity = SystemClock.uptimeMillis()
        val script = c.optString("script").takeIf { !c.isNull("script") && it.isNotEmpty() }
        try {
            when (kind) {
                "open" -> afterLoad(tab) { ok(tabState(tab)) }
                "navigate" -> {
                    val url = action.optString("url").takeIf { action.has("url") && !action.isNull("url") }
                    when {
                        url != null -> tab.view.loadUrl(url)
                        action.optString("history") == "back" -> tab.view.goBack()
                        action.optString("history") == "forward" -> tab.view.goForward()
                        else -> tab.view.reload()
                    }
                    afterLoad(tab) { ok(tabState(tab)) }
                }
                "snapshot" -> run(tab, "($script)()", action, ::fail) { ok(JSONObject().put("tree", it.optString("value"))) }
                "eval" -> run(tab, action.getString("expression"), action, ::fail) { ok(JSONObject().put("value", it.opt("value") ?: JSONObject.NULL)) }
                "click" -> {
                    val at = { x: Double, y: Double -> tap(tab, x, y); ok(JSONObject()) }
                    if (script != null) run(tab, "($script)(A)", action, ::fail) { val p = it.getJSONObject("value"); at(p.getDouble("x"), p.getDouble("y")) }
                    else at(action.getDouble("x"), action.getDouble("y"))
                }
                "type" -> {
                    val focus = JSONObject(action.toString()).put("empty", action.optString("text").isEmpty())
                    run(tab, "($script)(A)", focus, ::fail) {
                        val text = JSONObject.quote(action.optString("text"))
                        run(tab, "document.execCommand('insertText', false, $text)", action, ::fail) { ok(JSONObject()) }
                    }
                }
                "press" -> {
                    val press = { if (key(tab, action.getString("key"))) ok(JSONObject()) else fail("Unknown key ${action.getString("key")}") }
                    if (action.has("ref") || action.has("selector")) {
                        run(tab, "(function(a){const el=(${RESOLVE})(a);el.focus();return true})(A)", action, ::fail) { press() }
                    } else press()
                }
                "screenshot" -> {
                    if (script != null) run(tab, "(function(a){const r=($script)(a);return {x:r.x-scrollX,y:r.y-scrollY,width:r.width,height:r.height}})(A)", action, ::fail) {
                        ok(JSONObject().put("data", screenshot(tab, it.getJSONObject("value"))))
                    } else ok(JSONObject().put("data", screenshot(tab, null)))
                }
                "console", "network" -> {
                    val cursor = action.optLong("cursor", 0)
                    val list = if (kind == "console") tab.console else tab.network
                    val entries = JSONArray()
                    list.filter { it.first > cursor }.forEach { entries.put(it.second) }
                    ok(JSONObject().put("entries", entries).put("cursor", list.lastOrNull()?.first ?: cursor))
                }
                else -> fail("This browser cannot do $kind")
            }
        } catch (e: Exception) {
            fail(e.message ?: e.toString())
        }
    }

    private fun tabState(tab: Tab) = JSONObject()
        .put("tab", tab.id.toString()).put("url", tab.view.url ?: "").put("title", tab.view.title ?: "").put("loading", tab.loading)

    /** When the page has loaded and been idle a moment, or at the limit (then `loading` stays true). */
    private fun afterLoad(tab: Tab, done: () -> Unit) {
        var finished = false
        val finish = { if (!finished) { finished = true; done() } }
        main.postDelayed({ finish() }, LOAD_LIMIT_MS)
        main.postDelayed({
            if (!tab.loading) finish() else tab.loadWaiters += { main.postDelayed({ finish() }, LOAD_GRACE_MS) }
        }, 300)
    }

    /**
     * Evaluates `expression` (a promise is awaited) with the action as `A`, and hands back
     * `{value}` or the page's error.
     */
    private fun run(tab: Tab, expression: String, args: JSONObject, fail: (String) -> Unit, then: (JSONObject) -> Unit) {
        val token = UUID.randomUUID().toString()
        waiting[token] = { m -> if (m.has("error")) fail(m.getString("error")) else then(m) }
        main.postDelayed({ waiting.remove(token)?.let { fail("The page did not answer in time") } }, 25_000)
        val js = """(async () => { const A = ${args}; const post = (m) => yaacHost.postMessage(JSON.stringify(Object.assign({type: "result", token: ${JSONObject.quote(token)}}, m)));
            try { const v = await ($expression); post({ value: v === undefined ? null : v }); } catch (e) { post({ error: String(e && e.message || e) }); } })(); void 0;"""
        tab.view.evaluateJavascript(js, null)
    }

    private fun density(tab: Tab) = tab.view.resources.displayMetrics.density

    private fun tap(tab: Tab, x: Double, y: Double) {
        val d = density(tab)
        val now = SystemClock.uptimeMillis()
        val px = (x * d).toFloat()
        val py = (y * d).toFloat()
        val down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, px, py, 0)
        val up = MotionEvent.obtain(now, now + 50, MotionEvent.ACTION_UP, px, py, 0)
        tab.view.dispatchTouchEvent(down)
        tab.view.dispatchTouchEvent(up)
        down.recycle()
        up.recycle()
    }

    /** `Enter`, `Tab`, `ArrowDown`, `F5`, `Control+a`, … as key down and up on the view. */
    private fun key(tab: Tab, spec: String): Boolean {
        val parts = if (spec.endsWith("++")) spec.dropLast(2).split('+').filter { it.isNotEmpty() } + "+" else spec.split('+')
        var meta = 0
        for (m in parts.dropLast(1)) {
            meta = meta or when (m) {
                "Control", "Ctrl" -> KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
                "Shift" -> KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON
                "Alt" -> KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON
                "Meta" -> KeyEvent.META_META_ON or KeyEvent.META_META_LEFT_ON
                else -> return false
            }
        }
        val name = parts.last()
        val code = NAMED[name] ?: when {
            name.length == 1 && name[0].isLetter() -> KeyEvent.KEYCODE_A + (name[0].lowercaseChar() - 'a')
            name.length == 1 && name[0].isDigit() -> KeyEvent.KEYCODE_0 + (name[0] - '0')
            name.matches(Regex("F([1-9]|1[0-2])")) -> KeyEvent.KEYCODE_F1 + name.drop(1).toInt() - 1
            else -> return false
        }
        val now = SystemClock.uptimeMillis()
        tab.view.dispatchKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_DOWN, code, 0, meta))
        tab.view.dispatchKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, code, 0, meta))
        return true
    }

    private val NAMED = mapOf(
        "Enter" to KeyEvent.KEYCODE_ENTER, "Tab" to KeyEvent.KEYCODE_TAB, "Escape" to KeyEvent.KEYCODE_ESCAPE,
        "Backspace" to KeyEvent.KEYCODE_DEL, "Delete" to KeyEvent.KEYCODE_FORWARD_DEL, "Space" to KeyEvent.KEYCODE_SPACE, " " to KeyEvent.KEYCODE_SPACE,
        "ArrowUp" to KeyEvent.KEYCODE_DPAD_UP, "ArrowDown" to KeyEvent.KEYCODE_DPAD_DOWN,
        "ArrowLeft" to KeyEvent.KEYCODE_DPAD_LEFT, "ArrowRight" to KeyEvent.KEYCODE_DPAD_RIGHT,
        "Home" to KeyEvent.KEYCODE_MOVE_HOME, "End" to KeyEvent.KEYCODE_MOVE_END,
        "PageUp" to KeyEvent.KEYCODE_PAGE_UP, "PageDown" to KeyEvent.KEYCODE_PAGE_DOWN,
        "+" to KeyEvent.KEYCODE_PLUS, "-" to KeyEvent.KEYCODE_MINUS,
    )

    /** The tab as a PNG (base64), or the part `clip` names (CSS pixels of the viewport). */
    private fun screenshot(tab: Tab, clip: JSONObject?): String {
        val v = tab.view
        if (v.width == 0 || v.height == 0) {
            // Not on screen (an agent's tab): lay it out off screen.
            v.measure(View.MeasureSpec.makeMeasureSpec(HIDDEN_W, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(HIDDEN_H, View.MeasureSpec.EXACTLY))
            v.layout(0, 0, HIDDEN_W, HIDDEN_H)
        }
        val full = Bitmap.createBitmap(v.width, v.height, Bitmap.Config.ARGB_8888)
        v.draw(Canvas(full))
        val shot = clip?.let {
            val d = density(tab)
            val x = (it.getDouble("x") * d).toInt().coerceIn(0, full.width - 1)
            val y = (it.getDouble("y") * d).toInt().coerceIn(0, full.height - 1)
            val w = (it.getDouble("width") * d).toInt().coerceIn(1, full.width - x)
            val h = (it.getDouble("height") * d).toInt().coerceIn(1, full.height - y)
            Bitmap.createBitmap(full, x, y, w, h)
        } ?: full
        val out = ByteArrayOutputStream()
        shot.compress(Bitmap.CompressFormat.PNG, 100, out)
        return Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
    }

    /** Resolves `{ref, selector}` the way the snapshot hands out refs. */
    private const val RESOLVE = """function(a){const st=globalThis[Symbol.for("yaac.refs")];if(a.ref){const el=st&&st.els.get(a.ref)&&st.els.get(a.ref).deref();if(!el||!el.isConnected)throw new Error("Reference "+a.ref+" is not on the page; take a new browser_snapshot.");return el}const el=document.querySelector(a.selector);if(!el)throw new Error("No element matches "+a.selector);return el}"""

    /**
     * Before any page script: WebRTC cannot be kept on the proxy, so it is removed; resource
     * timings go to the host for `browser_network`.
     */
    private const val START_SCRIPT = """(() => {
  for (const k of ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel"]) { try { delete window[k]; Object.defineProperty(window, k, { value: undefined, configurable: false }); } catch (e) {} }
  if (typeof yaacHost === "undefined" || typeof PerformanceObserver === "undefined") return;
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        const entry = { url: e.name, method: "GET", durationMs: Math.round(e.duration) };
        if (e.responseStatus) entry.status = e.responseStatus;
        yaacHost.postMessage(JSON.stringify({ type: "net", entry }));
      }
    }).observe({ type: "resource", buffered: true });
  } catch (e) {}
})();"""
}
