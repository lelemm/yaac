# 06 — Attachments

Files and images reach a thread in three ways (decided):

- **The attachment button** in the composer opens the system's file picker; several files can
  be chosen at once.
- **Drag and drop**: files dropped anywhere on the thread view — transcript or composer — are
  attached. While dragging, the view shows a quiet drop target, "Drop to attach".
- **Paste (Ctrl+V)**: files copied in a file manager, or an image on the clipboard such as a
  screenshot.

## Who reads the file

The **hub** reads, sniffs, and hashes a file the user attaches — the window only names it
(`hub/upload/start` with a path). An image that exists only on the clipboard is handed to the
hub in 512 KiB chunks and kept as a durable draft blob first, so every upload is from a file.
Progress and the result come back as `hub/upload/progress` and `hub/upload/done`; a dropped
connection fails the upload and the window asks again (content addressing makes that cheap).
The hub hands a bulk frame to the writer only after the previous one is written, so a Stop
never waits behind more than one chunk — measured at about 1 ms during a 20 MB upload.

Limits: 512 MiB per file, 2 GiB of unfinished uploads, 4 sending to one target at once,
64 clipboard MiB per image.

**On Android** (docs/14-android.md) the file is on the phone and the hub on another machine,
so the hub cannot read it by path. The window reads each picked or pasted file itself, on a
worker, and sends its bytes like a clipboard image: a draft blob first, then the upload. The
limit there is 64 MiB per file. Images are recognised by content as on the desktop, and the
thread's `ImageInput` decides how they are sent, unchanged. The same applies to any window
attached to a hub by address (`AttachConfig::remote`).

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
- **A thread's folder exists before anything is attached.** `yaacd` creates `threads/<thread>`
  for every turn's input and hands it to the provider (`uploadsDir`), so a harness that fixes
  its readable folders when a conversation opens (Claude Code) can read it from the start: a
  screenshot pasted later never forces the harness's process to be re-opened, which would kill
  its background work (D160, P222).
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

- The daemon recreates missing upload directories when preparing an upload or a turn, even
  if the cleanup happened while it was running.
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
- **Thumbnails.** The composer decodes its draft images at display size. For uploads shown in
  the transcript, the window calls `hub/upload/preview {target, uploadId, maxEdge}` (P396).
  The hub reads the image in 512 KiB chunks over the existing target connection and uses its
  worker pool to decode and resize it. It returns `{pngBase64}` in the bulk queue, capped at
  a 220-pixel long edge and 72 KiB before base64. With a hub that lacks this method, the window
  reads and decodes the original as before. Opening the viewer reads the original in bounded
  chunks (`upload/read`). Codex's **Generate image** tool uses the same viewer in egui
  and WPF. `yaacd` snapshots the image when Codex supplies a saved path or inline image bytes,
  and keeps the reference in the transcript. Older recorded calls can recover their preview
  from the saved file or inline result that is still available.

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

**How the window does it (P26, P27).** Ctrl+V is delivered to the window as a key (eframe's
clipboard shortcuts are off; Ctrl+C and Ctrl+X are turned back into egui's copy and cut), and a
worker thread reads the clipboard with `arboard` in this order:

1. **A file list** — only into a composer: one chip per file, uploaded by path.
2. **Text** — pasted into the focused field. Into a composer, 64 KiB or more is offered as a
   file (`pasted-text.txt`) or as text.
3. **A bitmap** — only into a composer: encoded to PNG on the worker, named
   `screenshot-<local date and time>.png`, stored in the hub as a draft blob, then uploaded.

If the worker cannot open the clipboard at all, egui's own text paste is used.

**Spike S4, verified (2026-11-20).** `arboard` 3.7 reads file lists itself — `CF_HDROP` on
Windows, `text/uri-list` on X11, and on Wayland through the data-control protocol — so no
platform code was needed.

| Platform | Verified |
| --- | --- |
| Windows (the WSL host) | A file list put on the clipboard by another program (PowerShell `Set-Clipboard -Path`, which writes `CF_HDROP` as Explorer does) is read as two absolute paths, one with a space; text and a bitmap round-trip (the cross-compiled test run on Windows). |
| Linux, X11 | File list (`text/uri-list`, percent-decoded), text, and bitmap, under Xvfb. |
| Linux, Wayland | Not verified: the only compositor available (WSLg's Weston) has no data-control protocol, so `arboard` falls back to X11 (XWayland). Without XWayland, files and images cannot be pasted and text falls back to egui's paste. |

On Android the system answers instead (`PlatformBridge`, P378). **Attach** asks *Photos*
(the photo picker, images only) or *Files* (the document picker). **Paste** takes a copied
picture or document (a clip with a `content://` URI) as a file, else text. Each file is copied
into the app's cache first, because the controller reads by path.

Drag and drop comes from egui (winit): Windows and X11 report dropped files; winit does not
report dropped files on Wayland.

## In the window

- **The attachment button** (a paper clip beside Send) opens the system's file picker on a
  worker thread (`rfd`: the common item dialog on Windows, the XDG desktop portal on Linux).
- **Dropping files** anywhere in the window attaches them to the open composer (P28).
- **Chips** sit above the text: a thumbnail for an image (decoded at display size on a worker;
  only the thumbnail's pixels are kept) or a file mark, the name, then the upload's progress
  ("62%") or phase ("reading…", "waiting…", "checking…"), or the hub's error as-is with
  **Retry**; for an image, "this model cannot see images; it will be sent as a file" or, when
  support is unknown, "send as image · file". An × removes the chip and cancels its upload.
- **Send** is plain, not filled, while anything holds the message back, and says why: "Waiting
  for 1 upload", "trace.log was not uploaded — remove it or retry", "Choose how to send
  sketch.png".

### Viewing images

Click a draft image's thumbnail or name to open YAAC's image viewer in egui or WPF.
Expanded tool calls show a clickable preview when the harness supplies an image result.
A sent image attachment can also open in the viewer while its bytes remain on the target.
The viewer opens fitted to the available space. Use the zoom controls or mouse wheel to zoom,
drag to pan, select **Reset** to restore the fitted view, or use **Close** or Escape to leave.

Image results use the target's private temporary upload store. The transcript keeps references
and the window requests thumbnails as they come into view. Codex's native
image-view calls supply a target path, so an older call can show the file only if it is still
readable on that target. YAAC does not fetch external image URLs. Images are decoded on a
worker thread. The viewer bounds decoding and scales very large images to a 4,096-pixel
long edge.

### Drafts across a restart

A composer's text and chips are kept in the hub's durable draft store (P29), so a window
restart brings them back: finished uploads keep their `uploadId`, an upload that was running
is checked against `hub/upload/list` and asked again if the hub no longer has it, and clipboard
bytes come back from their draft blob. Sending deletes the message's blobs and the stored
draft. Moving a new-thread draft to a project on another target uploads every chip again there.

**Drafts on the device (P393, Proposed).** A client attached to a hub on another machine (the
phone over Tailscale) keeps its drafts on the device, in `drafts.json` in its data directory,
and sends no `hub/drafts/put`, `hub/drafts/delete` or `hub/drafts/list`: the draft store is
not shared between clients, and no client shows another's draft. The text and chips are back
the moment the controller starts or reattaches, before the hub answers; only the uploads wait
for the hub (restarted, or checked against `hub/upload/list`, as above). Attachments still go
to the target when attached; a stored chip keeps its `uploadId` or blob id, so its bytes are
never sent again. Pasted or picked bytes still go to the hub's blob store first (the hub
uploads them to the target), under the draft key `device/<key>`, so a desktop window sending
or clearing its own draft for the same thread never deletes them; sending or removing the
chip deletes the blob as before. The hub sweeps device blobs older than 14 days, keeping
any that are feeding a target upload until the upload ends (P395). An expired attachment
that still needs its blob must be attached again. The file is written off the controller's thread, newest
snapshot first, and once more in place when the controller stops.

### Generated documents and images (P358, Proposed)

A published artifact card shows a snapshot from the agent's working directory. The thread
record keeps its name and upload ID. Download and Open read the snapshot from the originating
target's private upload store. **Dismiss** hides the card in that conversation on the host.
Removing or editing the working file leaves the snapshot intact.
If the target's temporary store was cleaned, the card reports the missing bytes on the next
attempt. The agent can publish the same file again to restore that snapshot. See the
[supported formats and worker card behavior](10-agent-mesh.md#presenting-generated-files-p358-proposed).
