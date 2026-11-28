// Run on a target that your YAAC hub is connected to.
// Set YAACD to the bundled .mjs file or executable if yaacd is outside PATH.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const daemon = process.env.YAACD || "yaacd";
const bundled = daemon.endsWith(".mjs");
const args = bundled ? [daemon] : [];
const channel = spawn(bundled ? process.execPath : daemon, [...args, "channel", "--name", "Dialog demo"], {
  stdio: ["pipe", "pipe", "inherit"],
});
const lines = createInterface({ input: channel.stdout });
const requestId = "pick-a-team";
const layout = {
  type: "column",
  children: [
    { type: "text", text: "Choose a team to receive the report." },
    {
      type: "select",
      id: "team",
      label: "Team",
      required: true,
      options: [
        { value: "engineering", label: "Engineering" },
        { value: "design", label: "Design" },
      ],
    },
    { type: "button", id: "send", label: "Send report", kind: "primary" },
  ],
};

let completed = false;
channel.once("error", (error) => {
  completed = true;
  console.error(error.message);
  process.exitCode = 1;
});
try {
  for await (const line of lines) {
    const message = JSON.parse(line);
    if (message.error) throw new Error(message.error.message);
    if (message.id === "yaacd-channel-register") {
      channel.stdin.write(`${JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "dialog/open",
        params: { requestId, title: "Send the report", layout },
      })}\n`);
    }
    if (message.method === "dialog/result" && message.params.requestId === requestId) {
      console.log(JSON.stringify(message.params));
      completed = true;
      break;
    }
  }
  if (!completed) throw new Error("The channel disconnected before a result arrived.");
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
} finally {
  channel.stdin.end();
}
