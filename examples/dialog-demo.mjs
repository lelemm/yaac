// Run on a target where yaacd is already serving.
// Pass a socket path explicitly to use a non-default daemon home on Unix.
import { connect } from "node:net";
import { homedir, userInfo } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

const address = process.argv[2] ?? (process.platform === "win32"
  ? String.raw`\\.\pipe\yaac-channel-${userInfo().username}`
  : join(homedir(), ".yaac", "run", "channel.sock"));
const channel = connect(address);
const lines = createInterface({ input: channel });
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

function request(id, method, params) {
  channel.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
}

let completed = false;
try {
  request(1, "channel/register", { name: "Dialog demo" });
  for await (const line of lines) {
    const message = JSON.parse(line);
    if (message.error) throw new Error(message.error.message);
    if (message.id === 1) request(2, "dialog/open", { requestId, title: "Send the report", layout });
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
  channel.destroy();
}
