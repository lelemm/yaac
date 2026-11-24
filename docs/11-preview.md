# 11 — Web preview

A web server running on a target — usually a dev server with hot reload (Vite, webpack, Next.js)
that an agent is changing — can be seen live inside YAAC. The page and its hot-reload socket
travel over the target's one connection (invariant 1); nothing new is opened to the target.

Design status: **Proposed** (P318–P323). The owner asked for the feature on 2026-11-24 and chose CEF
in off-screen mode, with DevTools in a separate window.

```
preview (CEF request context, fixed SOCKS proxy) ──SOCKS5──▶ hub 127.0.0.1:<proxy> ──tunnel streams──▶ yaacd ──TCP──▶ target 127.0.0.1:<port>
system browser (Open in browser)   ──TCP────▶ hub 127.0.0.1:<forward> ──┘
```

The embedded preview loads the server's **own URL** (`http://localhost:5173/`): its request
context sends **every** request to a SOCKS5 proxy the hub runs for the thread's target (P323), so
the page sees the network exactly as the target does and keeps its real origin,
no local port has to match, and two targets serving the same port never collide. The system
browser cannot use that proxy, so **Open in browser** uses a plain forwarded port instead.
Both end in the same `tunnel/open`.

## Tunnel (YAAC protocol)

A tunnel is a raw TCP connection from the hub to a port on the target, forwarded byte for byte.
HTTP, WebSockets and server-sent events all pass through without YAAC understanding them.

| Method | Direction | Purpose |
| --- | --- | --- |
| `tunnel/open` | host → daemon | `{ port, host?, initialCredit }` → `{ stream, initialCredit }`. `yaacd` connects to `host:port` **on the target** (default `127.0.0.1`; any host name or address, resolved on the target — the tunnel reaches what the target user's own shell reaches, no more; P323). Answers once the TCP connection is up; `connection_refused` (no server on that port) and `invalid_params` are ordinary errors. The one stream number carries data both ways. |
| *(data frames)* | both | The bytes, on `stream`, under credit. Each direction has its own credit, granted by its receiver with `stream/credit`, exactly as for uploads. |
| `tunnel/close` | notification, both | `{ stream, reason? }`. Either end closing its socket closes the tunnel; the other end closes its socket and forgets the stream. |
| `preview/ports` | host → daemon | → `{ ports: [ { port, host, pid?, command?, cwd? } ] }`: TCP ports listening on loopback or any address on the target, owned by the target user. `cwd` is the listening process's working directory when it can be read, so the window can match a port to a project folder. Bulk response. |

- Tunnel data is **bulk** class and never delays control traffic ([03](03-protocol.md#scheduling-and-flow-control)).
- Tunnels are not journaled, not replayed, and never retried (invariants 8 and 10 do not apply:
  a tunnel changes no state). When the connection drops, every tunnel on it closes; the browser
  reconnects by itself (hot-reload clients do) once the target is back.
- `yaacd` caps open tunnels per connection (256) and closes any stream whose socket errors.
  It accepts any host name or address (an IPv6 literal with or without brackets) and resolves
  names itself, on the target; `localhost` is pinned to `127.0.0.1` and `::1`. It rejects with
  `invalid_params` only what cannot name a TCP host: empty, over 253 characters, a path, `@`,
  whitespace or control characters. A name that does not resolve is `connection_failed`.
- The optional `features` list in the `initialize` result advertises `portForward`; an absent
  entry disables previews without probing the target. A daemon without these methods answers `method_not_found`; the dialect folds that into a
  missing **`portForward`** target feature, and the window then shows no preview controls for
  that target. Nothing outside the dialect looks at a version.
- Port discovery: `/proc/net/tcp` and `/proc/net/tcp6` (state `0A` LISTEN) with inode → pid via
  `/proc/<pid>/fd` on Linux and WSL; `lsof -nP -iTCP -sTCP:LISTEN` on macOS; `netstat -ano`
  (or `Get-NetTCPConnection`) on Windows. It runs only when asked. Ports of `yaacd`, of the Codex
  daemon and other YAAC-owned processes are left out.

## Hub (attach protocol)

The hub owns every listener (invariant 14). The window never talks to the target directly.

| Method | Purpose |
| --- | --- |
| `hub/preview/proxy` | `{ target }` → `{ socksPort }`. A SOCKS5 server (no authentication; `CONNECT` only) on `127.0.0.1`, bound to one target. Each `CONNECT` (IPv4, IPv6, or a host name, which is passed on unresolved — SOCKS5 address type 3 — and resolved on the target) becomes one `tunnel/open` to that host and port on the target. Asking again for the same target returns the same server. |
| `hub/preview/proxyClose` | `{ target }`. Stops it and closes its connections. |
| `hub/preview/open` | `{ target, port, host? }` → `{ url, localPort }`. A plain forward for the system browser: binds `127.0.0.1` on a free port (the same number as the target's when it is free, else any) and returns `http://127.0.0.1:<localPort>/`. Opening the same target and port again returns the same listener. |
| `hub/preview/close` | `{ target, port }`. Stops the listener and closes its connections. |
| `hub/preview/list` | → an array of open proxies and forwards: `{ target, kind: "proxy" \| "forward", port?, localPort, connections }`. |

- Every connection accepted on a forward, and every `CONNECT` on a proxy, becomes one
  `tunnel/open` on that target's connection. While the target is not connected, an accepted connection is closed at once.
- The target's connection state exposes support as `daemon.portForward`. Creating a listener
  requires a connected target with this feature. The hub reports `target_offline` or
  `capability_missing` otherwise. Closing and listing listeners also work while their targets
  are offline, as does requesting the port of an existing listener. A SOCKS `CONNECT` takes
  any destination (P323); the hub refuses only a name that cannot be a host (empty, over 255
  bytes, whitespace or control characters) with `0x02`, as it does when the target rejects
  the destination (`invalid_params`). Refused by the target server: `0x05`. Other tunnel-open
  failures (including a name the target cannot resolve): `0x01`. `hub/preview/open` takes the
  same hosts.
- A listener lives until it is closed, its target is removed, or the hub exits. It is not
  persisted.
- One forward per port, so in the system browser each server is its own origin.
- Everything binds loopback only. Chromium cannot authenticate to a SOCKS proxy, so anyone on
  the host can use a proxy or forward while it is open, as with `ssh -D` and `ssh -L`; that is
  accepted and stated here. A proxy reaches what the target reaches; it never reaches the host's
  own network.

## Window

Available in **both front ends** — the egui window and the WPF front end. Tabs, their state and
the browser host are managed once, in the Rust controller; a front end only draws a tab's frames
and sends input back (see [The browser host](#the-browser-host)).

The Rust window controller keeps the tab list, active tab, split ratio, and full-screen flag in memory under the complete target/thread key. It obtains a proxy asynchronously on the first navigation. The new-tab page needs only port discovery. A failed proxy request leaves the address editable.

### Opening the browser

- A **+** button in the title bar, just left of **Minimize**, opens a small menu. For now it has
  one item, **Browser**; later items (other panes) go in the same menu.
- **Browser** splits the main area: the chat on the left, the browser on the right, a draggable
  divider between them. If the thread already has a browser area, it is shown and focused, and a
  new tab is added.
- The browser area belongs to **the thread** (the chat instance) that is open: each thread has
  its own tabs. Switching to another thread shows that thread's browser area, or none; switching
  back shows its tabs as they were. Tabs are not kept across a restart of the window.

### The browser area

- **Tabs** along its top: favicon, title (ellipsised), close; a **+** after the last tab opens
  another; there is no limit. Middle-click closes a tab. Closing the last tab closes the area
  and the chat takes the whole width again.
- **Toolbar** under the tabs: back, forward, reload (stop while loading), the address field
  (shows the page's own URL, e.g. `http://localhost:5173/`; editable; Enter navigates; a bare
  `5173` or `:5173` means `http://localhost:5173/`), **Open in browser**, **Inspect**, and
  **Full screen**.
- **Full screen** hides the chat: the browser takes the whole container (the sidebar and title
  bar stay). The button becomes **Restore**, which brings the split back at the width it had.
  Esc does not leave full screen — the page may need Esc; Restore does.
- A new tab with no address shows the thread target's **discovered ports** (below) as a list to
  pick from, and the address field.
- `localhost`, `127.0.0.1` and `[::1]` in a tab mean **the thread's target**, never the host:
  they go through the hub's proxy for that target. So does every other address: a tab sees the
  network from the target, never from the host (P323), so an agent driving a tab reaches nothing
  its shell on that target cannot already reach.
- A tab driven by an agent shows a small agent mark (the harness logo) beside its title while
  the agent acts on it.

### Ports

While a thread's browser area is visible (or the new-tab page is open), the window asks
`preview/ports` on its target about every 5 s (never while hidden, and never when the target
lacks `portForward`). Ports whose `cwd` is inside the thread's project folder are listed first.

### DevTools

DevTools open in their own top-level window: F12 in a tab, **Inspect** in the toolbar, or
**Inspect element** in the page's right-click menu (opened on that element). One per tab, titled
"DevTools — <tab title> · <machine>"; opening again focuses it; closing the tab closes it.

### Without the browser host

In a build without the `preview` feature, or when the browser host cannot start (no CEF runtime,
a crash), **Browser** in the + menu is shown disabled with the reason, and **Open in browser**
— a forward in the host's default browser — is the way to see a target's server.

## Agents

Every tool below is one `browser/call { threadId, action }`. The daemon derives the thread id
from the authenticated attachment, never from the tool's arguments. The hub adds its
connection’s `target` when forwarding `hub/browser/call { target, threadId, action }` to the
window, which looks each tab up only inside that target and thread's browser area. Requests
and results use the same `kind` tag; a tab id is an opaque string from `list` or `open`. An
element is named by `ref` (from the latest snapshot) or `selector` (CSS, first match), not both.

| Kind | Action fields | Result fields |
| --- | --- | --- |
| `list` | — | `tabs: [{ tab, url, title, loading }]` |
| `open` | `url` (a URL, `host:port` or bare port) | `{ tab, url, title, loading }` after the load (bounded) |
| `close` | `tab` | — |
| `navigate` | `tab`, and `url` or `history: "back" \| "forward" \| "reload"` | `{ tab, url, title, loading }` after the load (bounded) |
| `snapshot` | `tab` | `tree` (accessible text) |
| `screenshot` | `tab, selector?` | `data` (base64 PNG) |
| `click` | `tab`, and `ref`, `selector`, or `x, y` (viewport CSS pixels) | — |
| `type` | `tab, text, ref?, selector?, clear?` (no element: the focused one) | — |
| `press` | `tab, key, ref?, selector?` (`Enter`, `Tab`, `Escape`, `ArrowDown`, `F5`, `Control+a`, …) | — |
| `eval` | `tab, expression` (a promise is awaited) | `value` (JSON; `null` for `undefined`) |
| `console` | `tab, cursor?` | `entries: [{ level, text }], cursor` |
| `network` | `tab, cursor?` | `entries: [{ url, method, status?, durationMs?, error? }], cursor` |

The window carries the calls out in its controller (`crates/yaac-app/src/preview_agent.rs`),
for both front ends, through the DevTools protocol of each tab inside the browser host:

- `open` adds the tab after the thread's tabs without changing the active tab, so the user's
  view does not switch (a thread's first tab does open its browser area). A tab an agent drives
  is started and kept rendering while hidden, so screenshots and input work, but it is not shown.
- `open` and `navigate` wait for the load: done when the page has loaded, or after 1.5 s idle;
  at most 20 s, after which the tab is returned with `loading: true`.
- `snapshot` is an outline of the page (roles, names, values, `[ref=eN]` on interactive
  elements, at most about 200 kB). A ref stays the same for the element's life; a ref to an
  element no longer on the page is an error asking for a new snapshot.
- `click` scrolls the element into view and dispatches mouse events at its centre; `type`
  focuses it and inserts text (`clear` replaces the current value); `press` dispatches the key
  down and up. These are real input events, as from the user.
- Errors in the page (no element matches, an exception in `eval`) and a refused DevTools command
  are returned to the agent as tool errors; nothing is retried. A call is answered within 25 s;
  the hub gives up at 30 s.
- While an agent acts on a tab, and for 3 s after, the tab shows the agent's harness logo
  (`BrowserTab.agent`, the provider kind; in the WPF view `agentLogo`/`agentLogoColor`).
- A window built without the browser host answers `method_not_found`, so the hub offers the
  call to another window.
- A page dialog (`alert`, `confirm`) blocks the page until the user answers it; an agent's
  call on that tab waits and then times out. Agents never answer page dialogs.

The daemon immediately reports `no_window` if no host is attached. The hub can return
the same error when it has no window. Successful results use bulk priority and must fit
one protocol message frame, including JSON and base64 overhead. Screenshots require confirmed
effective image support and reach the harness as MCP `image` content with MIME type `image/png`.

An agent can use its own thread's browser through tools `yaacd` serves to the harness, like the
mesh tools ([10](10-agent-mesh.md)):

| Tool | Does |
| --- | --- |
| `browser_open` | Opens a tab in the calling thread's browser area (opening the area if needed), at a URL or a port; returns the tab id. Does not switch the user's view or take focus. |
| `browser_list` | The thread's tabs: id, URL, title, loading. |
| `browser_close` | Closes a tab. |
| `browser_navigate` | URL, back, forward, reload; waits for the load to finish (bounded). |
| `browser_screenshot` | A PNG of a tab (or of an element), returned as an image to harnesses that take images. |
| `browser_snapshot` | The page's accessible text tree, with references the actions below can use. |
| `browser_click` · `browser_type` · `browser_press` | Act on an element (by snapshot reference or CSS selector) or at coordinates. |
| `browser_eval` | Run JavaScript in the page and return its JSON result. |
| `browser_console` | Console messages and uncaught exceptions since a cursor. |
| `browser_network` | Failed and recent requests (URL, method, status, timing) since a cursor. |

- An agent only ever sees and drives **its own thread's** tabs. The user's tabs and the agent's
  are the same tabs: the user can watch, take over with the mouse, or close them.
- The calls travel yaacd → hub → window as host requests (`browser/call`, `hub/browser/call`); the window's controller
  carries them out on the browser host. When no window process is running (the user quit it), a
  tool answers "No YAAC window is open; the browser needs one" — it does not start a window.
- The hub prefers a window showing the calling thread and offers the call to another attached
  window if the first reports `method_not_found`.
- Actions are recorded in the transcript as the tool calls they are; nothing is hidden from the
  user, and no tool answers an approval or a question (invariant 13).
- Console, exception and network buffers are kept per tab, bounded (about 1 000 entries each),
  from the moment the tab opens, whether DevTools is open or not.

## The browser host

CEF runs in a separate **browser host** process started by the window controller on the first tab and exited when the last tab closes (CEF must be initialised on a
process's main thread and cannot be started again after shutdown, so it cannot live inside the
window process). Each tab's frames are written to **shared memory** (BGRA, with dirty
rectangles); a pipe carries commands, input, and notifications (URL, title, loading, cursor,
popups, context menus, dialogs, console). The egui window draws a frame as a texture; WPF draws
it into a bitmap. Automation uses the DevTools protocol of each tab inside the host.

### Windows WPF

The WPF presentation receives the selected thread's browser view and separate browser event and
frame messages over the existing anonymous pipes. A frame message carries a tab id, paint id,
pixel size, dirty rectangles, and the path to the BGRA mapping. WPF reads the dirty regions with
`MemoryMappedFile` on its pipe reader, then applies them to a premultiplied `WriteableBitmap` on
the dispatcher. The controller holds the frame until WPF acknowledges that paint id, including
when the user switches away before the copy can be displayed. A newly visible tab receives a
full frame before incremental updates resume. Select popups use a second bitmap at the given
rectangle. Bounds and pointer coordinates are expressed in CEF device-independent pixels with a
separate display scale for physical pixels.

The split is a WPF `GridSplitter`. Its ratio and full-screen state belong to the shared Rust
controller. WPF sends browser input and text composition through the pipe and uses the host's
caret coordinates to position the IME input control. WPF shows the browser host's context menus,
script prompts, and file choosers and dismisses menus and prompts when the host cancels them. When the host is unavailable
but the thread's target supports port forwarding, the thread header offers **Open in browser**
with an address or port field.

### CEF details

- Off-screen rendering: `OnPaint` buffers (BGRA) are copied, dirty rectangles only, into an egui
  texture, and only the window showing that preview is asked to repaint. Works with every
  renderer (glow, wgpu, software). The GPU path (`OnAcceleratedPaint`) is a later optimisation.
- Input is forwarded from egui: mouse move, buttons, wheel, keys and characters, IME composition,
  focus; CEF's cursor changes set the egui cursor. `<select>` popups are drawn over the page at
  the rectangle CEF gives. The page's context menu, `alert`/`confirm`/`prompt` and file choosers
  are drawn by YAAC.
- **Each tab has its own request context** (in-memory, so cookies and storage of two
  tabs never mix). The host keeps its own process data in a disposable session directory. Its proxy is set at runtime to `socks5://127.0.0.1:<socksPort>` for **all** hosts
  (fixed servers, bypass list `<-loopback>` so Chromium's implicit loopback bypass is removed
  and nothing goes direct). DNS for every host is resolved by the proxy, on the target. The
  same context sets `webrtc.ip_handling_policy` to `disable_non_proxied_udp`, so WebRTC cannot
  open UDP sockets from the host; a tab whose context refuses either preference is not opened.
  The values are `wire::proxy_settings` and `wire::WEBRTC_POLICY`, unit-tested in
  `yaac-preview`.
- The Linux integration probe verifies an HTTP page and its WebSocket through a test SOCKS5
  endpoint, checks the pixels changed by a WebSocket message, and opens windowed DevTools
  from the windowless browser. CEF 156 accepts the manual proxy preference in
  `OnRequestContextInitialized`. Chromium applies its implicit localhost bypass to PAC URLs
  regardless of the PAC contents. [Chromium proxy documentation](https://chromium.googlesource.com/chromium/src/+/HEAD/net/docs/proxy.md)
- Linux starts `yaac --yaac-browser-host`. The Chromium subprocesses re-enter the same binary
  with `--type=…`. Windows uses the CEF distribution's sandbox bootstrap executable and its
  matching YAAC adapter DLL. Both platforms run the native CEF message loop on the **helper**
  main thread. A control-pipe reader posts commands to that thread. The egui thread has no CEF
  loop, IPC reads or shutdown waits.
- Each view and select popup has its own file-backed shared BGRA mapping. The host sends dirty
  rectangles and pauses writes to that mapping until the controller releases its frame. If a
  paint occurs in the meantime the next available frame refreshes the entire surface. A hidden
  view calls `WasHidden`; showing it requests a fresh paint.
- Rust's `BrowserHost::open` and `Tab::try_update` also serve WPF. Keep a frame alive while
  WPF opens the backing file with read/write/delete sharing and copies its dirty rectangles
  to a premultiplied BGRA bitmap. Release it when WPF acknowledges that copy. Popup rectangles
  and pointer coordinates use device-independent pixels. Surface dimensions use physical pixels.
- The helper enables the Runtime and Network DevTools domains per tab. CDP responses retain the
  caller's id. Console messages, exceptions and network events have sequence cursors and bounded
  buffers per tab. No remote debugging listener is needed.
- The Rust binding is Tauri's `cef` 156.7.0 with CEF 156.0.31. The native adapter is built
  separately, so a normal YAAC build does not require the CEF SDK. Binary distributions and
  reproducible smoke tests are described in the [crate guide](../crates/yaac-preview/README.md).

## Later

- More items in the title bar's **+** menu.
