# YAAC web preview

`BrowserHost` runs CEF on demand in a window-owned process. Keep one host in the Rust controller.
Each `Tab` owns a browser and an in-memory request context. Drop a tab to close it and its
DevTools. Reopening the last tab starts a fresh helper. Check `runtime_available()` on a worker
before offering preview. Runtime startup errors arrive as `Update::Event(Event::Error)`.

## egui

Construct `Preview::new(&mut host, url, Some(target_socks_port), ctx)` or wrap a
controller-created tab with `Preview::from_tab(tab, url, ctx)`. Call `preview.ui(ui)` in the
available page area. The returned egui response supports layout and focus integration.
Call `set_hidden(true)` when the page is covered by another tab or its window is hidden.
Drawing the widget restores rendering. Set `preview.devtools_title` before `inspect(None)`.
Use `navigate`, `back`, `forward`, `reload`, `stop` and `state` for the toolbar.

## Controller and WPF

`BrowserHost::open(url, Some(target_socks_port), wake)` returns immediately. Use `None` for a
direct, standalone demo. The wake callback runs on a worker thread. Drain `Tab::try_update()`
after a wake. Send navigation, input and geometry with `Tab::send(wire::Action)`.
`Update::Frame` leases a complete premultiplied BGRA surface. Its dirty rectangles and dimensions
are physical pixels; popup placement, pointer coordinates and view dimensions are DIPs.

For WPF, pass `frame.path`, size and dirty rectangles over the controller's existing pipe.
Open a read-only file mapping with `FileShare.ReadWrite | FileShare.Delete`. Copy the changed
rows into a `WriteableBitmap` with `PixelFormats.Pbgra32` (stride = width × 4), close the
mapping and acknowledge the copy before dropping the Rust frame. Hold one lease per view or
popup. A renderer switching to an existing tab can send `Action::Invalidate` to refresh it.

`Tab::devtools_command(method, params)` sends a CDP command to that browser and returns an id.
The result or protocol error arrives in `Event::DevTools`. `Runtime.evaluate`,
`Input.dispatchMouseEvent`, `Input.insertText` and `Page.captureScreenshot` are available
through the same API. Read `Tab::diagnostics_since(cursor)` on a worker to get console,
exception and network history in order. Each of the two logs (console and network) retains
up to 1,000 entries and 4 MiB of serialized payloads. A cursor starts at zero.

## Dev run on Linux x64

From the repository root, with Cargo on PATH and a graphical display:

```sh
export CARGO_BUILD_JOBS=6
scripts/build-preview-runtime.sh linux64 .build/cef/dev-linux
export YAAC_CEF_DIR="$PWD/.build/cef/dev-linux"
cargo run -p yaac-app --features preview -- --demo preview http://localhost:5173/
```

Add `--socks <port>` after the URL to use a target proxy. A packaged executable looks for a
sibling `cef/` directory. The staging script verifies the pinned archive digest and builds the
matching native adapter. For Windows x64, run it from Linux/WSL with `windows64` and the MinGW
cross toolchain. Installer staging does this automatically. The Windows host starts with CEF's
sandbox bootstrap, which supplies sandbox information to the adapter's `RunWinMain`.

## Tests

The Python fixture serves HTTP and a WebSocket after accepting SOCKS CONNECT. The page changes
to a specific color only after receiving a WebSocket message. The live tests run in an isolated
temporary profile and require Python 3.11+, Linux x64 and a display. Set `YAAC_CEF_DIR` as above.

```sh
cargo test -p yaac-preview
cargo build -p yaac-preview --example controller_probe
xvfb-run -a python3 crates/yaac-preview/tests/socks_probe.py target/debug/examples/controller_probe
cargo build -p yaac-app --no-default-features --features glow,preview
LD_LIBRARY_PATH="$YAAC_CEF_DIR" xvfb-run -a python3 crates/yaac-preview/tests/socks_probe.py --host target/debug/yaac
```

For the isolated CEF binding and native DevTools window probe:

```sh
export CEF_DIR="$YAAC_CEF_DIR"
export CARGO_TARGET_DIR="$PWD/.build/cef/probe-target"
cargo build --locked --manifest-path crates/yaac-preview/native/Cargo.toml --example cef_probe
LD_LIBRARY_PATH="$CEF_DIR" xvfb-run -a python3 crates/yaac-preview/tests/socks_probe.py "$CARGO_TARGET_DIR/debug/examples/cef_probe"
```

On Windows, exercise the staged sandbox bootstrap, shared BGRA mapping and graceful shutdown
with `crates/yaac-preview/tests/windows-paint.ps1 -Stage C:\path\to\cef`.
