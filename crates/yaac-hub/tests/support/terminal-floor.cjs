// Time the PTY alone using the same native binding, raw mode, file and cat as the relay.
const pty = require(require.resolve("@lydell/node-pty", { paths: [process.cwd()] }));
const size = Number(process.argv[3]);
const terminal = pty.spawn("/bin/sh", ["-c", "stty raw -echo; printf '\\036'; dd bs=1 count=1 of=/dev/null 2>/dev/null; exec cat payload"], {
  cwd: process.argv[2], cols: 80, rows: 24, encoding: null
});
// The native child-exit callback alone does not keep libuv alive after EOF.
const timeout = setTimeout(() => {
  terminal.kill();
  throw new Error(`timed out after ${bytes} of ${size} PTY bytes`);
}, 15000);
let start, elapsed;
let bytes = 0;
terminal.onData(chunk => {
  if (start === undefined) {
    if (chunk.length !== 1 || chunk[0] !== 30) throw new Error("missing PTY ready signal");
    start = process.hrtime.bigint();
    terminal.write(Buffer.from([1]));
    return;
  }
  bytes += chunk.length;
  if (bytes === size) elapsed = process.hrtime.bigint() - start;
});
terminal.onExit(({ exitCode }) => {
  clearTimeout(timeout);
  if (exitCode !== 0 || bytes !== size) throw new Error(`received ${bytes} of ${size} bytes`);
  console.log(elapsed.toString());
});
