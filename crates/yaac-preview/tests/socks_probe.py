"""Live OSR tests with a target reachable only through a disposable SOCKS5 server."""

import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import sys
import socket
import tempfile

PAGE = b"""<body><script>
new WebSocket('ws://' + location.host + '/ws').onmessage = () =>
    document.body.style.background = '#112233';
</script>"""
WEBSOCKET_GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


async def target(reader, writer, seen, origin_port):
    try:
        version, count = await reader.readexactly(2)
        methods = await reader.readexactly(count)
        assert version == 5 and 0 in methods
        writer.write(b"\x05\x00")
        version, command, reserved, address_type = await reader.readexactly(4)
        assert (version, command, reserved) == (5, 1, 0)
        if address_type == 3:
            length = (await reader.readexactly(1))[0]
            host = (await reader.readexactly(length)).decode()
        elif address_type == 1:
            host = ".".join(map(str, await reader.readexactly(4)))
        else:
            return
        port = int.from_bytes(await reader.readexactly(2), "big")
        if host not in ("localhost", "127.0.0.1", "target-only.invalid") or port != origin_port:
            return
        writer.write(bytes([5, 0, 0, 1, 127, 0, 0, 1, 0, 0]))
        headers = await reader.readuntil(b"\r\n\r\n")
        if host == "target-only.invalid":
            writer.write(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 6\r\n\r\ntarget")
            seen.add("external")
        elif headers.startswith(b"GET /ws "):
            key = next(
                line.split(b":", 1)[1].strip() for line in headers.splitlines()
                if line.lower().startswith(b"sec-websocket-key:")
            )
            accept = base64.b64encode(hashlib.sha1(key + WEBSOCKET_GUID).digest())
            writer.write(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                b"Connection: Upgrade\r\nSec-WebSocket-Accept: " + accept
                + b"\r\n\r\n\x81\x02ok"
            )
            seen.add("websocket")
        else:
            writer.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n"
                + f"Content-Length: {len(PAGE)}\r\n\r\n".encode() + PAGE
            )
            seen.add("http")
        await writer.drain()
    except (EOFError, ConnectionError):
        pass
    finally:
        writer.close()


async def host_test(binary, profile, port, origin_port):
    proc = await asyncio.create_subprocess_exec(
        binary, "--yaac-browser-host",
        env=dict(os.environ, YAAC_PREVIEW_SESSION=profile),
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        limit=8 * 1024 * 1024,
    )

    async def send(action):
        proc.stdin.write(json.dumps({"tab": 1, "action": action}).encode() + b"\n")
        await proc.stdin.drain()

    async def receive(test):
        while line := await proc.stdout.readline():
            event = json.loads(line)["event"]
            if isinstance(event, dict) and "Error" in event:
                raise RuntimeError(event["Error"])
            if isinstance(event, dict) and "Paint" in event:
                frame = event["Paint"]
                surface = Path(profile) / ("1-%d.bgra" % frame["popup"])
                pixels = surface.read_bytes()
                assert len(pixels) == frame["size"][0] * frame["size"][1] * 4
                await send({"Consumed": {"popup": frame["popup"]}})
                if test(pixels):
                    return pixels
            elif test(event):
                return event
        raise RuntimeError(f"browser host exited with {await proc.wait()}")

    async def devtools(call, method, params):
        await send({"DevTools": {"id": call, "method": method, "params": params}})
        event = await receive(
            lambda e: isinstance(e, dict) and e.get("DevTools", {}).get("id") == call
        )
        return event["DevTools"]["result"]

    try:
        async with asyncio.timeout(30):
            await send({"Open": {"url": f"http://localhost:{origin_port}/", "socks": port}})
            marker = bytes([51, 34, 17, 255]) * 8
            await receive(lambda e: isinstance(e, bytes) and marker in e)
            print("HTTP and WebSocket target painted to shared memory", flush=True)
            result = await devtools(3, "Runtime.evaluate", {
                "expression": "document.body.style.backgroundColor", "returnByValue": True,
            })
            assert result["result"]["value"] == "rgb(17, 34, 51)"
            remote = await devtools(5, "Runtime.evaluate", {
                "expression": f"fetch('http://target-only.invalid:{origin_port}/').then(r=>r.text())",
                "returnByValue": True, "awaitPromise": True,
            })
            assert remote["result"]["value"] == "target"
            screenshot = await devtools(4, "Page.captureScreenshot", {})
            assert base64.b64decode(screenshot["data"]).startswith(b"\x89PNG")
            print("Proxied hostname, Runtime.evaluate and screenshot passed", flush=True)
            await send({"DevTools": {"id": 6, "method": "Runtime.evaluate", "params": {
                "expression": "prompt('Fixture prompt','initial')", "returnByValue": True,
            }}})
            prompt = await receive(lambda e: isinstance(e, dict) and "Dialog" in e)
            assert prompt["Dialog"]["kind"] == 2 and prompt["Dialog"]["text"] == "initial"
            await send({"Dialog": {"accept": True, "text": "confirmed"}})
            answer = await receive(lambda e: isinstance(e, dict) and e.get("DevTools", {}).get("id") == 6)
            assert answer["DevTools"]["result"]["result"]["value"] == "confirmed"

            for up in (False, True):
                await send({"Pointer": {"x": 20, "y": 20, "flags": 0 if up else 64,
                    "kind": {"Button": {"button": 2, "up": up, "count": 1}}}})
            menu = await receive(lambda e: isinstance(e, dict) and "Menu" in e)
            assert menu["Menu"]["items"]
            await send({"Menu": None})
            print("JS prompt and native context menu callbacks passed", flush=True)
            await send("Close")
            await receive(lambda e: e == "Closed")
            await send("Shutdown")
            proc.stdin.close()
            assert await proc.wait() == 0
    finally:
        if proc.returncode is None:
            proc.kill()
            await proc.wait()


async def main():
    # Reserve an unlistened port. A direct request cannot accidentally hit another server.
    with socket.socket() as guard:
        guard.bind(("127.0.0.1", 0))
        await run(guard.getsockname()[1])


async def run(origin_port):
    seen = set()
    server = await asyncio.start_server(
        lambda r, w: target(r, w, seen, origin_port), "127.0.0.1", 0
    )
    async with server:
        port = server.sockets[0].getsockname()[1]
        with tempfile.TemporaryDirectory() as profile:
            if sys.argv[1] == "--host":
                await host_test(sys.argv[2], profile, port, origin_port)
            else:
                env = dict(os.environ, PROBE_PORT=str(port), PROBE_PROFILE=profile, PROBE_ORIGIN=f"http://localhost:{origin_port}/", TMPDIR=profile)
                proc = await asyncio.create_subprocess_exec(*sys.argv[1:], env=env)
                try:
                    assert await asyncio.wait_for(proc.wait(), 60) == 0
                finally:
                    if proc.returncode is None:
                        proc.kill()
                        await proc.wait()
            assert not list(Path(profile).glob("yaac-browser-*")), "browser session cleanup incomplete"
            assert seen == ({"http", "websocket", "external"} if sys.argv[1] == "--host" else {"http", "websocket"}), seen


if __name__ == "__main__":
    asyncio.run(main())
