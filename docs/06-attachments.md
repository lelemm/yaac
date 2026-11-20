# 06 — Attachments

Files and images reach a thread by being pasted (Ctrl+V) or dropped into the composer.

## The flow

```
paste / drop
   │
   ▼
┌─ host ────────────────────────────┐        ┌─ target ───────────────────────────────┐
│ 1. read clipboard: files, image,  │        │                                        │
│    or text                        │        │                                        │
│ 2. add a chip to the composer     │        │                                        │
│ 3. upload/begin (name, size, hash)├───────►│ 4. content already here? → skip bytes  │
│    data frames …                  ├───────►│    else write into the upload folder   │
│    upload/end                     ├───────►│ 5. verify hash → uploadId              │
│                                   │        │                                        │
│ 6. user presses send              │        │                                        │
│    turn/start { parts: [text,     ├───────►│ 7. pin uploads to the thread           │
│      image(uploadId), file(…)] }  │        │ 8. hand each part to the provider in   │
│                                   │        │    the form that provider wants        │
└───────────────────────────────────┘        └────────────────────────────────────────┘
```

Uploads start **when the attachment is added**, not when the message is sent, so sending is
instant and a large file shows its progress on the chip. A message cannot be sent while one of
its uploads is unfinished; the send button says why.

Uploads share the single connection with everything else, in small chunks behind control
traffic ([protocol](03-protocol.md#scheduling-and-flow-control)), so a big file neither freezes
a streaming answer in another thread nor delays a Stop.

### An upload belongs to one target

An `uploadId` is only meaningful on the target that issued it. In a **draft**, the target can
still be changed — and then the attachments must follow:

- The host keeps the original bytes of a draft's attachments (in the durable draft store) until
  the message is sent.
- If the draft's target changes, its uploads are invalidated and re-sent to the new target.
  The chips show the upload again.
- An upload that was never attached to a sent message is simply left for the system's temp
  cleaning.

## The upload folder

**Decided:** pasted files go to a folder in the target's **system temp directory**, and the
message references them there.

```
<temp>/yaac-<user>/uploads/
  blobs/<full-sha256>                      the bytes, stored once
  threads/<thread>/<n>-<sanitised-name>    a link to the blob, under the name the agent sees
```

`<temp>` is `$TMPDIR` or `/tmp` on Linux and `%TEMP%` on Windows.

- **Private, even though temp is shared.** `yaacd` creates the folder itself with user-only
  permissions and refuses to use one it does not own or that is a link to somewhere else. On a
  multi-user machine other users cannot read or plant files in it.
- **Content-addressed by the full digest.** The same bytes are stored once, however many
  threads use them. Pasting the same screenshot twice costs nothing.
- **Original file name kept** (sanitised), so the agent sees `error-log.txt`, not a hash.
- **Local targets copy too.** On the local machine the file is still taken into the folder —
  linked rather than duplicated when the filesystem allows. A message must keep referring to
  the bytes that were attached, even if the original file is edited or deleted a minute later.
- **Cleanup is mostly the operating system's job.** `yaacd` removes a thread's files when the
  thread is deleted, and removes partial uploads from a dropped connection. Everything else is
  left to the system's normal temp cleaning.
- **Limits.** Per-file size, total size, and concurrent uploads are bounded, with a clear
  message when a limit is hit.

### Temp means temporary

The trade-off of this location is accepted and made visible: a temp directory can be emptied —
typically at reboot — while the conversation that referenced a file lives on.

- When a thread is opened, `yaacd` reports which of its attachments still exist. A chip whose
  file is gone is shown as *no longer on the target*; the message text is unaffected.
- If the user resumes such a thread, the agent may be told about a file it can no longer open.
  The chip state makes that understandable, and the file can simply be attached again.
- A draft's attachments are safe: the host keeps the original bytes until the message is sent,
  and re-uploads them if the target's copy has vanished.
- Things that must not vanish are not stored here. Fork handoff transcripts live in `yaacd`'s
  own state directory ([05 Threads](05-threads.md#handoff-fork--everything-else)).

### The agent must be able to read it

A path is useless if the harness's sandbox cannot read it. `yaacd` makes the upload folder
readable to each harness explicitly, in the way that harness provides (for Claude Code, it is
passed as an additional directory), and it does so narrowly — the thread's own attachment
folder, not a broad grant. Where readability cannot be established, the upload is rejected with
a reason instead of sending the agent a path it cannot open.

## Images are not ordinary attachments

A model with vision can *see* an image, but only if the image is given to it as image input. If
the same picture is passed as a file path, the agent has a path to a binary it cannot read as
text — it may try a tool, or it may guess. So the two must never be confused.

The decision is made per message from the thread's **effective image support**, which `yaacd`
computes for the selected model and which has three values, not two
([04 Providers](04-providers.md#effective-capabilities-are-computed-on-the-target)):

| Pasted item | Image support | Sent as | Result |
| --- | --- | --- | --- |
| Image | **Supported** | `image` part | The model sees it. Codex gets `localImage { path }`; Claude gets an image content block that `yaacd` builds from the uploaded file. |
| Image | **Unsupported** | `file` part, with a warning | The chip is marked "this model cannot see images — sent as a file". The user can still send, switch model, or remove it. |
| Image | **Unknown** | the user chooses | The harness gave no information about this model's inputs. The chip asks: send as an image, or as a file. YAAC does not guess. |
| Any other file | — | `file` part | The path is listed for the agent, which reads it with its own tools. |

Details that matter:

- **The check uses the model selected at send time.** Switching from a vision model to a
  text-only model re-evaluates every image chip in the composer.
- **`yaacd` re-validates.** An `image` part for a model that is `Unsupported` is rejected with
  `capability_missing` rather than silently downgraded, so the UI and the daemon can never
  disagree about what the model received.
- **How to detect an image.** By content sniffing (magic bytes), not by file extension.
- **Formats and size.** What a harness accepts differs and changes over time; accepted media
  types come from the thread's effective capabilities when the harness reports them. The MVP
  sends PNG, JPEG, GIF, and WebP as-is and rejects oversized images — by bytes and by pixel
  count — with a clear message. Downscaling on the host before upload is a later optimisation.
- **Thumbnails.** The transcript shows a thumbnail decoded at display size on the host. The
  original pixels are not kept in memory. A host that did not upload the image fetches a
  bounded read of it over the same connection (`upload/read`).

### How file parts are presented to the agent

A `file` part becomes a short, fixed block appended to the user's text:

```
Attached files (on this machine):
- /tmp/yaac-me/uploads/threads/t_01J…/1-error-log.txt
- /tmp/yaac-me/uploads/threads/t_01J…/2-schema.sql
```

The transcript in the UI shows chips, not this block. `yaacd` records the original input parts
of every message it sends, alongside what was actually given to the harness, so the UI on any
host can render chips, and re-rendering and forking stay clean. For messages sent from outside
YAAC there is no such record; they are shown as the harness stored them.

Later, `DocumentInput` lets a PDF go to a capable model as a native document instead of a path,
under the same rule as images.

## Reading the clipboard

egui's built-in paste event delivers text only. Files and images need direct clipboard access:

| Clipboard content | Becomes |
| --- | --- |
| A list of files (copied in a file manager) | One chip per file. |
| Bitmap data (a screenshot) | One image chip, encoded to PNG, named by timestamp. |
| Text | Inserted into the composer as text. Very large pastes are offered as a file attachment instead. |

Hashing a large file and encoding a screenshot happen on worker threads, never on the UI or I/O
thread.

**Verify (spike S4):** reading a *file list* from the clipboard on Windows (`CF_HDROP`) and on
Linux under both X11 and Wayland (`text/uri-list`). Image and text are well supported by the
`arboard` crate; file lists may need platform code. Drag-and-drop of files is already supported
by egui and uses the same pipeline.
