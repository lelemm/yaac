# Android parity inventory

Each checkbox is a Windows interaction or observable state for the Android screen brief to cover. **Desktop** marks a window-system, pointer, physical-keyboard or OS integration that needs an equivalent interaction. A Windows hub’s WSL controls still manage the hub’s distros. Screen IDs refer to the desktop review scenes. When an older scene conflicts with a newer decision, use the current UI behavior (for example D143 for PR badges and P354 for mirrored requests).


**Mockup frame** identifies the scene where a reviewer starts each check. A frame can demonstrate a visible state or provide the entry point for an interaction. One still image cannot verify replay, persistence or input behavior. S14 and S17 include open platform questions. S20 refers to the demo-only command surface.

## Shell and fleet

- [ ] **F01** First run with an empty index, recommended connected machine and ready providers, check installed providers, Refresh, Start a thread, Add server or Turn on WSL. Disable Start and show the reason until a provider is ready. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A15a
- [ ] **F02** Keep drafts out of the fleet. Start in a selected group’s project, target and checkout. Promote a draft to a real thread on first send. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A16a, A16b
- [ ] **F03** Flat sticky machine/project/worktree headers with independent grouping switches, remembered collapse, count, and header actions even when collapsed. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a, A01b
- [ ] **F04** Two-line rows show state, harness, relative time, title, branch, added/modified/deleted files, ahead/behind and direct-worker count. Preserve marks as text narrows. Non-git rows collapse to one line. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a
- [ ] **F05** Pinned projects, temporary discovered folders, muted and archived sections. Blocked muted threads resurface until answered. Archive clears pin and mute. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a, A02b
- [ ] **F06** Select the open thread separately from keyboard focus and bulk selection. Children normally live in their parent’s panel. Orphan workers can surface in the fleet. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a, A03a
- [ ] **F07** Group ordering by last message or natural title ascending/descending. Independently float needs-you and unseen. Hold reorder during interaction. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01b
- [ ] **F08** Search collapsed groups and archived items. Fuzzy title/native ID search, more results, older per-project pages and per-target older search. Offline cache search. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01d
- [ ] **F09** Needs-you button cycles unresolved requests by recency, including hidden workers. Focus the visible ancestor copy or open its origin. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A09a, A09b
- [ ] **F10** Unseen completed turn marks on row, strip and related panel. Clear on open and preserve across windows. Waiting on reporting workers survives the parent turn ending. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a, A04a, A08b
- [ ] **F11** Copy thread ID (YAAC ID for new chats, harness ID for older/discovered chats), rename, fork, take over, move, pin/unpin, mute/unmute, archive/unarchive, delete and dismiss/restore as applicable. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A02a
- [ ] **F12** Change mute or unmute. Mute indefinitely, for minutes/hours/days, or until local date/time with calendar. Show expiry and auto-unmute. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A02d
- [ ] **F13** Delete confirmation with optional delete in harness and unknown-outcome state. Archive confirmation. Dismiss discovered thread or entire temporary project. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A02e, A03b
- [ ] **F14** Create project from discovered folder or new draft. Choose target, name and directory, browse folders, use repository root, handle loading, empty and error. [dialogs.rs:261](../../crates/yaac-app/src/wpf/dialogs.rs#L261) · Mockup frame A16c
- [ ] **F15** **Desktop** Back/Forward, mouse side buttons, history menus, jump several visits, restore scroll and draft across threads/settings. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A06b
- [ ] **F16** **Desktop** Resize sidebar, collapse into scrollable project strip, inspect full row by hover, restore remembered width when space allows. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A04a, A04b
- [ ] **F17** Hub connection status, connected/offline target counts. Cached disconnected threads, disabled target, stopped distro with Start it. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a, A20b
- [ ] **F18** Daemon older than window, automatic update when idle, Update now confirmation with interrupted-turn count, updating progress. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A20c

## Selection and moving threads

- [ ] **SEL01** **Desktop** Add/remove individual rows, extend range across groups, select all visible, clear. Keep selection on reorder and prune filtered rows. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A03a
- [ ] **SEL02** Selection bar and context menu. Archive, mute, unmute, move or clear selected. Group and pinned header actions apply to listed rows. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A03a, A02b
- [ ] **SEL03** Bulk archive eligibility, one confirmation with skipped reasons, per-thread failures/unknown outcomes. Bulk mute uses one duration dialog. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A03b, A02d
- [ ] **SEL04** Pick same-machine project and worktree. Explain whether the underlying cwd changes next turn or only sidebar placement changes. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A03f, A03g
- [ ] **SEL05** **Desktop** Drag row or selection to header/row, highlight valid target, explain refused drop, cancel with Esc, confirm cwd changes, Undo moves and record in transcript. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A03c, A03d, A03e

## Conversation and transcript

- [ ] **T01** Header title/rename, state, target/project/worktree, linked parent, fork and ancestry. Budget state and reasons remain visible when title truncates. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A06a, A08b
- [ ] **T02** Open newest page, page backwards, preserve scroll anchor through reflow/folding, follow streaming at bottom, new-message count and Jump to bottom. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A12a, A11f
- [ ] **T03** Prompt timeline spans loaded and unloaded history. Highlight visible prompts, show first line and local time, select grouped prompts and jump directly. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11f
- [ ] **T04** Agent prose streams in markdown. Preserve typed line breaks, lists, tables, fenced code with line numbers/copy, inline code, literal HTML and explicitly opened links. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A05a, A11c
- [ ] **T05** Select across multiple items and expanded details. Copy selected range, single message, code or pasted block with visual decoration excluded. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11e
- [ ] **T06** User bubble versus attributed starting task. Expand pasted content, command chips, harness Context parts and context-only rows separately. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A09b, A11c
- [ ] **T07** Readable reasoning opens by default and can be collapsed. Compact tools, command, plan, edits, notices and agent calls. Compact repetitive runs. Hidden empty reasoning appears only for a meaningful elapsed pause. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11a, A11d
- [ ] **T08** Open command details, full input, stdout/stderr, duration and exit result. Failed commands stay visible. Incrementally reveal output and file diff hunks. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11b, A11d
- [ ] **T09** Changed-file counts and end-of-turn summary, show all changed files. Fold completed/interrupted/failed work with elapsed time, expand again, preserve final reply, errors and actionable requests. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11a, A11b
- [ ] **T10** **Desktop** Inspect message timestamps without shifting the transcript. Use the client’s local time. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11a
- [ ] **T11** View external-holder thread live/read-only with holder reason, Check latest, fork or eligible Take over. In-flight, failure and blocked takeover states. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A17a, A17b, A17c
- [ ] **T12** Fork destination identifies target instance/account/model. Native fork or handoff consequence, size/cutoff, preview, first message, Send and fork. Inherited history is read-only. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A18a, A18b, A18c
- [ ] **T13** Read-only harness subagent, Open parent, answer its requests where parent permits. Instance moved to another store shows reason and disables composition. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A17d, A20a
- [ ] **T14** Open inline image results, message attachments and thumbnails in viewer. Zoom controls, fit/reset, zoom around pointer, drag to pan, loading and error. [images.rs:377](../../crates/yaac-app/src/images.rs#L377) · Mockup frame A26a
- [ ] **T15** Render fenced Mermaid and artifact diagrams, source/error fallback, Save PNG, Save SVG and Save source. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A26c
- [ ] **T16** Published artifacts in worker and ancestor transcripts. Read origin, name, time, transfer progress/error. Download, open externally, open supported types in integrated browser, preview diagram or dismiss locally. [artifacts.rs:65](../../crates/yaac-app/src/artifacts.rs#L65) · Mockup frame A26b

## Requests, notices and reminders

- [ ] **R01** Approval names tool, exact command, reason and scope. Render only offered decisions including thread/session-scoped choices. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A05a, A05b
- [ ] **R02** AI explanation selector (main/fallback), loading and labeled explanation. Independently show dangerous patterns, expand raw command. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A05b, A27a
- [ ] **R03** Question cards allow choices, multi-select, other/free text, secret input and Submit answer. Forms and otherwise unsupported request kinds direct the user to answer in the harness. [transcript.rs:49](../../crates/yaac-app/src/wpf/transcript.rs#L49) · Mockup frame A10a
- [ ] **R04** Show pending, submitted, confirmed by you, resolved elsewhere, expired and unknown outcome. Check latest status for unknown. Keep externally held cards read-only. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A10b, A09c
- [ ] **R05** Automatic approval in an explicitly bypassing thread appears as expandable activity with reason/input. Questions, forms and logins still require answers. [07-ui.md § thread view](../07-ui.md#thread-view) · Mockup frame A11d, A14c
- [ ] **R06** Mirrored worker cards link to the originating request, allow action where permitted and reconcile resolution everywhere. Keep ancestor composer usable. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A09a, A09b, A09c
- [ ] **R07** Agent permission prompt with ask/allow and root turn/time budget. Continue held orchestration from the budget notice. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A10c, A10d
- [ ] **R08** Notices above composer, open the request or thread, dismiss notice and inspect unresolved delivery notices after reconnect. [screens.md § 19  notices](screens.md#19--notices) · Mockup frame A20a
- [ ] **R09** **Desktop** Sticky request reminder/sound when card is outside the focused viewport. One unseen request opens exact item, offered simple approval allows Allow once/Deny. Fleet summary previews the oldest three and opens oldest. Dismiss acknowledges displayed requests. Sound on count increase only. Withdraw/update on resolution and guard stale actions. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A21a, A21b

## Composer, attachments and controls

- [ ] **C01** Editable multiline Markdown, selection/undo, IME Enter, growing editor up to five lines, scroll within editor, configurable send key. Preserve caret after paste. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A16a, A12a
- [ ] **C02** **Desktop** Resize centered transcript/composer together. Stack actions below field at narrow widths. Preserve transcript position. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A12a, A29b
- [ ] **C03** Persist draft text/files per thread across restarts. Empty composer recalls previous ten window submissions across chats. Edit recalled entry or return to empty. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A13a
- [ ] **C04** At-word file picker from thread cwd, loading/filter/empty/errors, insert quoted native mention or path at caret. Works in draft with cwd. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A13b
- [ ] **C05** Slash/dollar skill picker, capability-based triggers and matching, loading/error/retry, keyboard/touch pick, dismiss without losing draft. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A13a
- [ ] **C06** Manual /compact on supported idle thread, progress, confirmed activity, refused command stays in draft, unknown outcome. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A13a
- [ ] **C07** Send echo with text and chips, Sending, acknowledgement/replacement, refused Edit/Retry, uncertain Edit/Dismiss without retry. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A12c
- [ ] **C08** Running-turn queue, Send now with steering/interrupt hint, Stop. Waiting on request allows queue only. Queue ordered entries, promote, edit/save/cancel or remove. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A12a, A05a
- [ ] **C09** **Desktop** Paperclip picker, clipboard text/image/file, drag/drop overlay. Large pasted text offers attach as file, insert text or cancel. [composer.rs:701](../../crates/yaac-app/src/wpf/composer.rs#L701) · Mockup frame A13e, A13f
- [ ] **C10** Attachment chip name, preview, progress, remove, retry, failed reason and awaiting-upload send gate. Historical attachment may be missing from target. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A13d
- [ ] **C11** ImageInput supported, unsupported (file with warning), unknown (choose image/file). New target invalidates target-specific uploads. [06-attachments.md § an upload belongs to one target](../06-attachments.md#an-upload-belongs-to-one-target) · Mockup frame A13d
- [ ] **C12** Model grouped by instance, fork-to-switch disabled reason in started thread, changing capability/default effort, off-model label and effective selection. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A14a, A16a
- [ ] **C13** Standard/Fast/Ultrafast control and bolt states, usage warning and confirmation on enabling Ultrafast, reset unsupported speed after model change. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A14b
- [ ] **C14** Mode defaults to harness configuration. Bypass confirmation names thread/machine/folder, default Cancel. Worker scope choice names affected running workers; active mode warning. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A10e, A10f, A14c
- [ ] **C15** Draft project/worktree and new project, root Agents ask/allow, fork on existing thread, context-usage meter when offered. [07-ui.md § composer](../07-ui.md#composer) · Mockup frame A16a, A14a, A18a
- [ ] **C16** Collapsible Running commands across thread and descendants, owner/target/age, live Logs, last output after exit and supported Stop, confirmed launch in transcript. [07-ui.md § running commands](../07-ui.md#running-commands) · Mockup frame A12a, A12b

## Agents and mesh

- [ ] **A01** Related agents appear for relatives or an orchestration budget. The pill clusters parent/workers/contacts with direction, state, unseen and in-flight marks. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A07a, A07c
- [ ] **A02** Expand card without navigating. Dock, undock, resize, move pill vertically, remember preferred position. **Desktop** hover/focus and drag affordances. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A08a, A08b, A29a
- [ ] **A03** Sibling list including selected self, parent pinned at dock foot, nested workers and contacts with last activity age, branch, provider/model/target, speed, git and PR. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A08b
- [ ] **A04** **Desktop** Hover preview a relative’s transcript without changing selection. Switch preview between rows, leave to restore. Open related thread or its context menu. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A08c
- [ ] **A05** Mark all as read including hidden agents, distinguish unread completions, collapse inactive workers. Overflow count, hidden live/needs-you, open at first hidden agent. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A07c, A08b
- [ ] **A06** Root turn/time budget, elapsed since human activity, Continue, set/extend/clear idle time limit with preset or custom hours, explain unsupported target. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A08b, A08d
- [ ] **A07** Running with exhausted budget versus idle agent blocked from starting a turn. Notice explains pending work and resume outcome. [07-ui.md § the agents panel](../07-ui.md#the-agents-panel) · Mockup frame A10d
- [ ] **A08** Sent and received messages in both transcripts, sender/recipient links. Mailbox acceptance, queued/boundary, failed, gone and uncertain delivery. [07-ui.md § agent messages in the transcript](../07-ui.md#agent-messages-in-the-transcript) · Mockup frame A07a, A07b, A20a
- [ ] **A09** Live spawn/wait progress; expandable task, result and raw call, resolved or unavailable peer pills. Read/stop/notify/resume actions shown as agent activity. [07-ui.md § agent calls d150](../07-ui.md#agent-calls-d150) · Mockup frame A07a, A07b

- [ ] **A10** Open the orchestration root from the panel when its thread is known. [agents.rs:123](../../crates/yaac-app/src/wpf/agents.rs#L123) · Mockup frame A08b

## Pull requests and rate limits

- [ ] **PR01** Real PR badge, additional PR list, draft, merged, closed, pending/failed checks. Open in default browser, copy link or create another PR. Disabled reasons. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A01a, A02c, A02f
- [ ] **PR02** Create PR with branch picker, editable generated title/description, Draft, Ask agent to write it (consumes a turn), push/uncommitted warnings and uncertain outcome. [07-ui.md § sidebar](../07-ui.md#sidebar) · Mockup frame A19a
- [ ] **PR03** Usage grouped per signed-in account across machines, provider mark and segmented used/reset windows, scoped and near-limit windows, stale reports and reset rollover. [07-ui.md § the rate limit panel](../07-ui.md#the-rate-limit-panel) · Mockup frame A01a, A01c
- [ ] **PR04** **Desktop** Account drilldown/hover shows exact share, local reset, plan, identity, targets and last report. Compact-strip usage bars and details. [07-ui.md § the rate limit panel](../07-ui.md#the-rate-limit-panel) · Mockup frame A01c, A04a

## Settings and window integration

- [ ] **S01** Full settings navigation with Back to threads. Targets, Providers, Appearance, Window, Terminal, Keyboard, Approvals, Notifications, Skills, Advanced, About. [settings.rs:150](../../crates/yaac-app/src/wpf/settings.rs#L150) · Mockup frame A27a
- [ ] **S02** Local enable/disable. Remote target add/edit/enable/disable/remove, retry connection; distinguish disable, remove, revoke. [07-ui.md § targets](../07-ui.md#targets) · Mockup frame A27b
- [ ] **S03** **Desktop hub** WSL master switch, refresh installed distros, loading/failure, default/running/already added, optional user, second-session warning and start stopped distro. [07-ui.md § targets](../07-ui.md#targets) · Mockup frame A27b, A20b
- [ ] **S04** SSH import config or enter host/user/port/name. Test existing access, verify fingerprint, password or key/passphrase, install key, verify or show SSH error. [07-ui.md § adding an ssh connection](../07-ui.md#adding-an-ssh-connection) · Mockup frame A28a, A28b, A28e
- [ ] **S05** SSH setup ownership/history. Revoke installed key, remove with/without revocation, manual removal line and confirmation when unreachable. [07-ui.md § adding an ssh connection](../07-ui.md#adding-an-ssh-connection) · Mockup frame A28c, A28d
- [ ] **S06** Per-target automatic discovery, offline cached providers, readiness/sign-in instructions, backend/version, instance and target refresh even with empty list. [07-ui.md § providers per target](../07-ui.md#providers-per-target) · Mockup frame A27c
- [ ] **S07** Provider add/name/endpoint/home, edit, enable/disable/remove, external sessions' YAAC tools and folder trust switches. Confirm changed store or keep current. [07-ui.md § providers per target](../07-ui.md#providers-per-target) · Mockup frame A27c, A20a
- [ ] **S08** Edit Best used as model guidance, turn model on/off, show disabled entries and retain existing thread’s effective model. [settings.rs:591](../../crates/yaac-app/src/wpf/settings.rs#L591) · Mockup frame A27c, A14a
- [ ] **S09** Approvals main/fallback live model choices including None. Notifications toggle, include questions, sound enable and sound choice. [settings.rs:171](../../crates/yaac-app/src/wpf/settings.rs#L171) · Mockup frame A27a, A05b, A21a
- [ ] **S10** Scale entire interface, larger/smaller/reset, normal/code fonts separately, installed fonts with hover sample, import font, errors and bundled fallback. [settings.rs:805](../../crates/yaac-app/src/wpf/settings.rs#L805) · Mockup frame A27d
- [ ] **S11** Follow system/dark/light/custom. Import JSON, choose real preview card, restore last selection, empty/duplicate/invalid library states. Agent theme changes appear in active UI. [07-ui.md § appearance  custom themes](../07-ui.md#appearance--custom-themes) · Mockup frame A27d
- [ ] **S12** Skill catalog create/edit/import folder with supporting files, id/description/instructions, manual/automatic, save/cancel/remove, bundled override/restore and validation. [catalog_settings.rs:355](../../crates/yaac-app/src/catalog_settings.rs#L355) · Mockup frame A27f
- [ ] **S13** Advanced send key, hub login/lifetime, clear cache while retaining drafts, per-target diagnostics and last error. [settings.rs:957](../../crates/yaac-app/src/wpf/settings.rs#L957) · Mockup frame A27a
- [ ] **S14** General spec also mentions cache size, default grouping and target daemon idle timeout. Verify the surface against the running Windows build during review. [07-ui.md § general](../07-ui.md#general) · Mockup frame A27a
- [ ] **S15** About version/build, release and source links, component status, Refresh including enabled offline targets. Symptom, Diagnose, chosen target, editable report and Copy. [07-ui.md § troubleshoot](../07-ui.md#troubleshoot) · Mockup frame A27g
- [ ] **S16** **Desktop** Update available, download/progress/error/try again, verified restart-to-update and version mismatch at attach. [updater.rs:360](../../crates/yaac-app/src/updater.rs#L360) · Mockup frame A27g
- [ ] **S17** **Desktop** Renderer choices, current GPU/backend/software/fallback reason, applies on restart and Try it again after failed start. [07-ui.md § display](../07-ui.md#display) · Mockup frame A27a
- [ ] **S18** **Desktop** Tray close preference, hide/reopen, needs-you badge/count, Open and Quit while hub runs, foreground existing window on relaunch. [07-ui.md § the tray](../07-ui.md#the-tray) · Mockup frame A21c
- [ ] **S19** **Desktop** Titlebar move, resize, minimize, maximize/restore, close and pane menu. Preserve workspace/attention during changes of available bounds. [titlebar.rs:1](../../crates/yaac-app/src/titlebar.rs#L1) · Mockup frame A04a, A29b
- [ ] **S20** **Desktop** Demo-only play the story and stream a reply for renderer comparison. [07-ui.md § display](../07-ui.md#display) · Mockup frame A22a

## Commander, keyboard and accessibility

- [ ] **K01** Commander recent/grouped catalog, search names/descriptions/aliases, disabled reasons, key caps, second-step pickers, Back, dismiss or launch existing dialog. [07-ui.md § keyboard](../07-ui.md#keyboard) · Mockup frame A22a, A22b, A22c
- [ ] **K02** Settings Keyboard record chord, conflict Replace/Cancel, clear/reset individual, reset all, fixed context-only keys, Works in terminal per command. [07-ui.md § keyboard](../07-ui.md#keyboard) · Mockup frame A27e
- [ ] **K03** **Desktop** Physical shortcuts and focus traversal; preserve editing shortcuts and IME. Announce new requests without stealing focus, approval keys only act on focused card. [07-ui.md § keyboard](../07-ui.md#keyboard) · Mockup frame A09a, A11e, A27e
- [ ] **K04** Command navigation restores composer focus where writable. Focused terminal captures unassigned shortcuts. Expose semantic names/states and both theme contrasts. [07-ui.md § keyboard](../07-ui.md#keyboard) · Mockup frame A22c, A24a

## Side pane and preview

- [ ] **B01** Per-thread side pane and tabs, split divider/remembered width, close last to return to chat. Add browser/files/terminal according to target/build. [11-preview.md § window](../11-preview.md#window) · Mockup frame A25a, A29b
- [ ] **B02** Tab icons, ellipsized titles, loading indicator, active/close, overflow at narrow widths, full-screen Chat tab, Restore and full-screen tab switcher. [12-files.md § tab strip](../12-files.md#tab-strip) · Mockup frame A25c
- [ ] **B03** Address field accepts URL/host/port, bare port or loopback. Back/forward/reload/stop; preserve editable address on error. [11-preview.md § window](../11-preview.md#window) · Mockup frame A25a
- [ ] **B04** New tab discovered servers with process and project precedence, no ports/offline state, refresh while visible. [12-files.md § new tab page](../12-files.md#new-tab-page) · Mockup frame A25b
- [ ] **B05** Interactive page input, scroll, forms, hot reload. Agent-driven mark and background tabs preserve user selection. Page traffic sees target network. [11-preview.md § window](../11-preview.md#window) · Mockup frame A25a
- [ ] **B06** **Desktop** External browser and modified-click links, forwarded loopback address. Missing embedded runtime disables Browser; external forward remains available. [11-preview.md § window](../11-preview.md#window) · Mockup frame A25a
- [ ] **B07** **Desktop** Inspect, Inspect element or F12 opens per-tab DevTools; focus existing, follow inspected element, close with tab. Page context menu follows theme. [11-preview.md § window](../11-preview.md#window) · Mockup frame A25d
- [ ] **B08** Agent browser activity: list/open/close/navigate, snapshot, screenshot, click/type/key, eval, console/network via the same thread tabs. [11-preview.md § agents](../11-preview.md#agents) · Mockup frame A25a, A25d

- [ ] **B09** The pinned Chat tab shows the harness and highlights new chat activity while another tab occupies full screen. [browser.rs:116](../../crates/yaac-app/src/wpf/browser.rs#L116) · Mockup frame A25c

- [ ] **B10** Handle web page popups, IME composition, JavaScript alert/confirm/prompt and browser-requested file chooser. [BrowserView.Page.cs:131](../../tools/wpf-preview/Views/BrowserView.Page.cs#L131) · Mockup frame A25e

## Files and diffs

- [ ] **D01** Lazy directory expand/collapse, folders first, worktree root, path/name and git M/A/D/R/U/! or changed-folder mark. Show ignored, refresh and Collapse all. [12-files.md § files tab](../12-files.md#files-tab) · Mockup frame A23a
- [ ] **D02** Open file by name, diff by git mark, reuse existing tab or **Desktop** middle-click for another; refresh on entry changes/turn end. [12-files.md § files tab](../12-files.md#files-tab) · Mockup frame A23a, A23b
- [ ] **D03** Read-only selectable syntax-highlighted File tab with line numbers, reload, relative path, binary/oversize notice. [12-files.md § file tab](../12-files.md#file-tab) · Mockup frame A23b
- [ ] **D04** Diff against HEAD: unified/side-by-side, added/deleted/renamed/untracked, line and intraline marks, folded unchanged regions, numbered change navigation/reload. [12-files.md § diff tab](../12-files.md#diff-tab) · Mockup frame A23c, A29b
- [ ] **D05** Add selected file/diff range at saved composer caret, removable excerpt chip, expand path and fenced content in position on queue/send. [12-files.md § add to composer](../12-files.md#add-to-composer) · Mockup frame A23d, A23e

## Terminal and shell commands

- [ ] **E01** Open default or selected available shell in thread worktree. Multiple attached sessions survive closing client. Shell exit code, Restart, close, reattach and replay. [13-terminal.md § the tab](../13-terminal.md#the-tab) · Mockup frame A24a
- [ ] **E02** Tab title/icon, folder, bell, unseen output. Inline Rename via menu/F2, save/cancel and clear custom title to restore shell title. [13-terminal.md § the tab](../13-terminal.md#the-tab) · Mockup frame A24a
- [ ] **E03** Toolbar Clear, Search, match navigation, Add to composer as fenced console output, Send commands to agent switch and disabled reason where integration unavailable. [13-terminal.md § the tab](../13-terminal.md#the-tab) · Mockup frame A24a, A24b
- [ ] **E04** Terminal panel below composer and side-pane terminal. Toggle, focus shell, tabs, new shell, switch, close, resize, maximize and restore per thread. [13-terminal.md § the terminal panel under the composer](../13-terminal.md#the-terminal-panel-under-the-composer) · Mockup frame A24d
- [ ] **E05** Interactive terminal input/resize, colors, wide glyphs, scrollback, full-screen applications and app mouse reporting. [13-terminal.md § emulator  yaac term](../13-terminal.md#emulator--yaac-term) · Mockup frame A24a
- [ ] **E06** **Desktop** Linear/word/line/block selection, extend and override captured mouse. Copy versus interrupt, copy on select, context-menu copy/search/add/paste. [13-terminal.md § clipboard and selection](../13-terminal.md#clipboard-and-selection) · Mockup frame A24a, A24b
- [ ] **E07** Paste shortcut versus shell literal key, bracketed paste, multiline execution warning and Don’t ask again. Target clipboard write notice, denied clipboard reads. [13-terminal.md § clipboard and selection](../13-terminal.md#clipboard-and-selection) · Mockup frame A24c
- [ ] **E08** Leading ! enters target/cwd command mode, monospace, remove prefix to return to draft, separate command history, Run/Queue run. Keep attachments for later. [13-terminal.md § how it looks p346](../13-terminal.md#how-it-looks-p346) · Mockup frame A13c
- [ ] **E09** Command transcript shows streaming output, Stop, exit code/duration, Show all lines and agent reply. Interrupted command and queued command behavior. [13-terminal.md § shell commands and the agent](../13-terminal.md#shell-commands-and-the-agent) · Mockup frame A24e
- [ ] **E10** Completed terminal commands optionally become context-only records, including interactive-program placeholder and deferred delivery during active turns. [13-terminal.md § shell commands and the agent](../13-terminal.md#shell-commands-and-the-agent) · Mockup frame A24a, A24e

- [ ] **E11** Terminal Settings exposes Copy on select, Ctrl+V goes to the shell and Ask before pasting several lines. [settings.rs:890](../../crates/yaac-app/src/wpf/settings.rs#L890) · Mockup frame A27a, A24c

## Desktop review scenes

These scene IDs give reviewers a visual cross-check. Entries 06 and 18 were retired. A suffix identifies a recorded variant of a numbered scene.

- [ ] **SC01** A thread that needs an approval. [screens.md:607](screens.md#01--a-thread-that-needs-an-approval) · Mockup frame A05a
- [ ] **SC02** The orchestrator: agents talking to agents. [screens.md:627](screens.md#02--the-orchestrator-agents-talking-to-agents) · Mockup frame A07a
- [ ] **SC03** The same conversation from a worker's side. [screens.md:663](screens.md#03--the-same-conversation-from-a-workers-side) · Mockup frame A09b
- [ ] **SC04** A blocked worker, two levels down. [screens.md:683](screens.md#04--a-blocked-worker-two-levels-down) · Mockup frame A09a, A09b
- [ ] **SC05** Every state a request can be in. [screens.md:701](screens.md#05--every-state-a-request-can-be-in) · Mockup frame A10b
- [ ] **SC07** First run. [screens.md:738](screens.md#07--first-run) · Mockup frame A15a
- [ ] **SC08** A new thread, before the first message. [screens.md:756](screens.md#08--a-new-thread-before-the-first-message) · Mockup frame A16a
- [ ] **SC09** The first message, sent. [screens.md:782](screens.md#09--the-first-message-sent) · Mockup frame A16b
- [ ] **SC10** In use elsewhere. [screens.md:797](screens.md#10--in-use-elsewhere) · Mockup frame A17a
- [ ] **SC11** Fork. [screens.md:814](screens.md#11--fork) · Mockup frame A18b
- [ ] **SC11n** fork native. [11n-fork-native-dark.png](screenshots/11n-fork-native-dark.png) · Mockup frame A18a
- [ ] **SC12** The forked thread. [screens.md:834](screens.md#12--the-forked-thread) · Mockup frame A18c
- [ ] **SC13** Settings: targets. [screens.md:848](screens.md#13--settings-targets) · Mockup frame A27b
- [ ] **SC13d** settings wsl distros. [13d-settings-wsl-distros-dark.png](screenshots/13d-settings-wsl-distros-dark.png) · Mockup frame A27b
- [ ] **SC14** SSH setup: confirm the server. [screens.md:872](screens.md#14--ssh-setup-confirm-the-server) · Mockup frame A28b
- [ ] **SC15** SSH setup: password, key install, verified. [screens.md:885](screens.md#15--ssh-setup-password-key-install-verified) · Mockup frame A28c
- [ ] **SC16** SSH setup: when it fails, and after a revoke. [screens.md:903](screens.md#16--ssh-setup-when-it-fails-and-after-a-revoke) · Mockup frame A28d
- [ ] **SC17** Settings: providers. [screens.md:928](screens.md#17--settings-providers) · Mockup frame A27c
- [ ] **SC19** Notices. [screens.md:972](screens.md#19--notices) · Mockup frame A20a
- [ ] **SC20** The budget ran out. [screens.md:991](screens.md#20--the-budget-ran-out) · Mockup frame A10d
- [ ] **SC21** A narrow window. [screens.md:1007](screens.md#21--a-narrow-window) · Mockup frame A04a
- [ ] **SC22** SSH setup: signing in. [screens.md:1022](screens.md#22--ssh-setup-signing-in) · Mockup frame A28e
- [ ] **SC23** The queued message arrives, and one that never will. [screens.md:1038](screens.md#23--the-queued-message-arrives-and-one-that-never-will) · Mockup frame A20a
- [ ] **SC24** The same message at its destination. [screens.md:1058](screens.md#24--the-same-message-at-its-destination) · Mockup frame A07b, A09b
- [ ] **SC25** An agent asks to work with other agents. [screens.md:1078](screens.md#25--an-agent-asks-to-work-with-other-agents) · Mockup frame A10c
- [ ] **SC26** Muting and archiving. [screens.md:1104](screens.md#26--muting-and-archiving) · Mockup frame A02d
- [ ] **SC27** The agents pill. [screens.md:1121](screens.md#27--the-agents-pill) · Mockup frame A07a
- [ ] **SC28** The agents card, on hover. [screens.md:1135](screens.md#28--the-agents-card-on-hover) · Mockup frame A08a
- [ ] **SC29** Creating a project from a discovered folder. [screens.md:1152](screens.md#29--creating-a-project-from-a-discovered-folder) · Mockup frame A16c
- [ ] **SC30** Confirming a mode that bypasses approvals. [screens.md:1170](screens.md#30--confirming-a-mode-that-bypasses-approvals) · Mockup frame A10e
- [ ] **SC31** history list. [31-history-list-dark.png](screenshots/31-history-list-dark.png) · Mockup frame A06b
- [ ] **SC32** ssh form. [32-ssh-form-dark.png](screenshots/32-ssh-form-dark.png) · Mockup frame A28a
- [ ] **SC33** mute dialog. [33-mute-dialog-dark.png](screenshots/33-mute-dialog-dark.png) · Mockup frame A02d
- [ ] **SC34** wsl stopped. [34-wsl-stopped-dark.png](screenshots/34-wsl-stopped-dark.png) · Mockup frame A20b
- [ ] **SC35** sidebar strip. [35-sidebar-strip-dark.png](screenshots/35-sidebar-strip-dark.png) · Mockup frame A04a
- [ ] **SC36** sidebar wide. [36-sidebar-wide-dark.png](screenshots/36-sidebar-wide-dark.png) · Mockup frame A29a
- [ ] **SC37** Upload in progress, image thumbnail and send gate. [screens.md:1187](screens.md#3740--attachments-in-the-composer) · Mockup frame A13d
- [ ] **SC38** Unsupported image and failed upload with Retry. [screens.md:1187](screens.md#3740--attachments-in-the-composer) · Mockup frame A13d
- [ ] **SC39** Unknown image support and image/file choice. [screens.md:1187](screens.md#3740--attachments-in-the-composer) · Mockup frame A13d
- [ ] **SC40** Drag-to-attach overlay. [screens.md:1187](screens.md#3740--attachments-in-the-composer) · Mockup frame A13f
- [ ] **SC41** A context menu. [screens.md:1203](screens.md#41--a-context-menu) · Mockup frame A02a
- [ ] **SC42** Deleting a thread. [screens.md:1217](screens.md#42--deleting-a-thread) · Mockup frame A02e
- [ ] **SC43** Archiving asks first. [screens.md:1227](screens.md#43--archiving-asks-first) · Mockup frame A03b
- [ ] **SC44** Turns that finished unseen. [screens.md:1233](screens.md#44--turns-that-finished-unseen) · Mockup frame A01a
- [ ] **SC45** Ordering the sidebar. [screens.md:1243](screens.md#45--ordering-the-sidebar) · Mockup frame A01b
- [ ] **SC46** The pill with all three relations. [screens.md:1250](screens.md#46--the-pill-with-all-three-relations) · Mockup frame A07c
- [ ] **SC47** The pill with workers only. [screens.md:1257](screens.md#47--the-pill-with-workers-only) · Mockup frame A07a
- [ ] **SC48** The docked panel at its narrowest. [screens.md:1262](screens.md#48--the-docked-panel-at-its-narrowest) · Mockup frame A08b
- [ ] **SC49** Settings: window. [screens.md:1297](screens.md#49--settings-window) · Mockup frame A27a
- [ ] **SC50** Fast mode. [screens.md:1315](screens.md#50--fast-mode) · Mockup frame A14b
- [ ] **SC60** Pull request badges. [screens.md:1267](screens.md#60--pull-request-badges) · Mockup frame A01a
- [ ] **SC61** The `+1` list. [screens.md:1280](screens.md#61--the-1-list) · Mockup frame A02c
- [ ] **SC62** The badge's menu. [screens.md:1285](screens.md#62--the-badges-menu) · Mockup frame A02f
- [ ] **SC63** Creating a pull request. [screens.md:1289](screens.md#63--creating-a-pull-request) · Mockup frame A19a
- [ ] **SC64** The Commander. [screens.md:1324](screens.md#64--the-commander) · Mockup frame A22a
- [ ] **SC65** The Commander, filtered. [screens.md:1337](screens.md#65--the-commander-filtered) · Mockup frame A22b
- [ ] **SC66** A step inside the Commander. [screens.md:1343](screens.md#66--a-step-inside-the-commander) · Mockup frame A22c
- [ ] **SC67** Settings: keyboard. [screens.md:1350](screens.md#67--settings-keyboard) · Mockup frame A27e
- [ ] **SC68** Settings: About. [screens.md:1365](screens.md#68--settings-about) · Mockup frame A27g
- [ ] **SC69** Several rows selected. [screens.md:1376](screens.md#69--several-rows-selected) · Mockup frame A03a
- [ ] **SC70** The selection's menu. [screens.md:1388](screens.md#70--the-selections-menu) · Mockup frame A03a
- [ ] **SC71** Archiving several asks once. [screens.md:1395](screens.md#71--archiving-several-asks-once) · Mockup frame A03b
- [ ] **SC72** A group header's menu. [screens.md:1404](screens.md#72--a-group-headers-menu) · Mockup frame A02b
- [ ] **SC73** After a bulk Archive. [screens.md:1411](screens.md#73--after-a-bulk-archive) · Mockup frame A03b
- [ ] **SC74** Take over is not possible now. [screens.md:1418](screens.md#74--take-over-is-not-possible-now) · Mockup frame A17b
- [ ] **SC75** Taken over. [screens.md:1427](screens.md#75--taken-over) · Mockup frame A17c
- [ ] **SC76** A pasted log, a command, and notices from Claude Code. [screens.md:1434](screens.md#76--a-pasted-log-a-command-and-notices-from-claude-code) · Mockup frame A11c
- [ ] **SC77** The same, opened. [screens.md:1450](screens.md#77--the-same-opened) · Mockup frame A11c
- [ ] **SC78** Moving a thread whose folder follows. [screens.md:1456](screens.md#78--moving-a-thread-whose-folder-follows) · Mockup frame A03f
- [ ] **SC79** Moving a thread whose folder stays. [screens.md:1470](screens.md#79--moving-a-thread-whose-folder-stays) · Mockup frame A03g
- [ ] **SC80** Dragging a row onto another project. [screens.md:1479](screens.md#80--dragging-a-row-onto-another-project) · Mockup frame A03c
- [ ] **SC81** A drop that is refused. [screens.md:1487](screens.md#81--a-drop-that-is-refused) · Mockup frame A03d
- [ ] **SC82** After a move: Undo. [screens.md:1495](screens.md#82--after-a-move-undo) · Mockup frame A03e
- [ ] **SC83** A table in an agent's message. [screens.md:1503](screens.md#83--a-table-in-an-agents-message) · Mockup frame A11c
- [ ] **SC84** An open tool call. [screens.md:1510](screens.md#84--an-open-tool-call) · Mockup frame A11d
- [ ] **SC85** Hidden reasoning between commands. [screens.md:1517](screens.md#85--hidden-reasoning-between-commands) · Mockup frame A11d
- [ ] **SC90** Agents talking to agents (the orchestrator). [screens.md:1524](screens.md#90--agents-talking-to-agents-the-orchestrator) · Mockup frame A07a
- [ ] **SC91** An agent call opened. [screens.md:1542](screens.md#91--an-agent-call-opened) · Mockup frame A07b
- [ ] **SC92** Claude Code's own subagents (users-service). [screens.md:1549](screens.md#92--claude-codes-own-subagents-users-service) · Mockup frame A07a, A17d
- [ ] **SC93** A subagent call opened, with its raw call. [screens.md:1560](screens.md#93--a-subagent-call-opened-with-its-raw-call) · Mockup frame A07b
- [ ] **SC95** A harness's own subagents under their parent (search-service). [screens.md:1566](screens.md#95--a-harnesss-own-subagents-under-their-parent-search-service) · Mockup frame A08b
- [ ] **SC96** A subagent opened: read-only, with the reason and its parent. [screens.md:1575](screens.md#96--a-subagent-opened-read-only-with-the-reason-and-its-parent) · Mockup frame A17d
- [ ] **SC100** The timeline beside a long thread. [screens.md:1583](screens.md#100--the-timeline-beside-a-long-thread) · Mockup frame A11f
- [ ] **SC101** Finished turns, folded. [screens.md:1593](screens.md#101--finished-turns-folded) · Mockup frame A11a
- [ ] **SC102** A finished turn opened again. [screens.md:1599](screens.md#102--a-finished-turn-opened-again) · Mockup frame A11a
- [ ] **SC103** A message's time, on hover. [screens.md:1606](screens.md#103--a-messages-time-on-hover) · Mockup frame A11a
- [ ] **SC104** A turn that failed. [screens.md:1612](screens.md#104--a-turn-that-failed) · Mockup frame A11b
- [ ] **SC110** Rate limits at the bottom of the sidebar. [screens.md:1618](screens.md#110--rate-limits-at-the-bottom-of-the-sidebar) · Mockup frame A01a
- [ ] **SC111** The account's tooltip. [screens.md:1633](screens.md#111--the-accounts-tooltip) · Mockup frame A01c
- [ ] **SC112** Rate limits in the collapsed strip. [screens.md:1639](screens.md#112--rate-limits-in-the-collapsed-strip) · Mockup frame A04a
- [ ] **SC120** Queue or send now while a turn runs. [screens.md:1645](screens.md#120--queue-or-send-now-while-a-turn-runs) · Mockup frame A12a
- [ ] **SC130** Thirty agents: the pill. [screens.md:1658](screens.md#130--thirty-agents-the-pill) · Mockup frame A07c
- [ ] **SC131** Thirty agents: the card, opened from "more". [screens.md:1670](screens.md#131--thirty-agents-the-card-opened-from-more) · Mockup frame A08a
- [ ] **SC132** Thirty agents, docked. [screens.md:1677](screens.md#132--thirty-agents-docked) · Mockup frame A08b
- [ ] **SC133** Thirty agents, more alive than the pill has room for. [screens.md:1685](screens.md#133--thirty-agents-more-alive-than-the-pill-has-room-for) · Mockup frame A07c

## Commander actions in the Windows controller

Each action uses the current selection, capabilities and focus rules. Default key chords are desktop keyboard affordances. Rows with no chord remain reachable through Commander.

- [ ] **CMD-commander** GoTo · Commander. Open this list. Default keys: Ctrl+Space. [commands.rs:267](../../crates/yaac-core/src/commands.rs#L267) · Mockup frame A22a
- [ ] **CMD-search-threads** GoTo · Search threads. Focus the sidebar's search field. Default keys: Ctrl+K. [commands.rs:268](../../crates/yaac-core/src/commands.rs#L268) · Mockup frame A01d
- [ ] **CMD-find-thread** GoTo · Find a thread…. Find any thread by title or id across every target, and open it. Default keys: Ctrl+P. [commands.rs:269](../../crates/yaac-core/src/commands.rs#L269) · Mockup frame A22b, A01d
- [ ] **CMD-needs-next** GoTo · Next thread that needs you. Open the next thread waiting on you, at its request. Default keys: Ctrl+J. [commands.rs:270](../../crates/yaac-core/src/commands.rs#L270) · Mockup frame A09a
- [ ] **CMD-jump-to-bottom** GoTo · Jump to the bottom. Scroll the open thread to its newest message. Default keys: Ctrl+End. [commands.rs:271](../../crates/yaac-core/src/commands.rs#L271) · Mockup frame A12a
- [ ] **CMD-prev-message** GoTo · Go to previous message. Scroll the open thread to the message you sent before the one in view. Default keys: Alt+PageUp. [commands.rs:272](../../crates/yaac-core/src/commands.rs#L272) · Mockup frame A11f
- [ ] **CMD-next-message** GoTo · Go to next message. Scroll the open thread to the next message you sent. Default keys: Alt+PageDown. [commands.rs:273](../../crates/yaac-core/src/commands.rs#L273) · Mockup frame A11f
- [ ] **CMD-next-unseen** GoTo · Next unseen finished thread. Open the next thread whose finished turn you have not seen. Default keys: Ctrl+U. [commands.rs:274](../../crates/yaac-core/src/commands.rs#L274) · Mockup frame A01a
- [ ] **CMD-next-thread** GoTo · Next thread in the list. Move the selection down the sidebar. Default keys: Ctrl+Down. [commands.rs:275](../../crates/yaac-core/src/commands.rs#L275) · Mockup frame A01a
- [ ] **CMD-prev-thread** GoTo · Previous thread in the list. Move the selection up the sidebar. Default keys: Ctrl+Up. [commands.rs:276](../../crates/yaac-core/src/commands.rs#L276) · Mockup frame A01a
- [ ] **CMD-back** GoTo · Back. Go back through visited threads. Default keys: Alt+Left, Mouse back. [commands.rs:277](../../crates/yaac-core/src/commands.rs#L277) · Mockup frame A06b
- [ ] **CMD-forward** GoTo · Forward. Go forward through visited threads. Default keys: Alt+Right, Mouse forward. [commands.rs:278](../../crates/yaac-core/src/commands.rs#L278) · Mockup frame A06b
- [ ] **CMD-history** GoTo · History…. Pick a place from the back and forward list. [commands.rs:279](../../crates/yaac-core/src/commands.rs#L279) · Mockup frame A06b
- [ ] **CMD-parent** GoTo · Go to the parent thread. Open the thread that spawned this one. Default keys: Alt+Up. [commands.rs:280](../../crates/yaac-core/src/commands.rs#L280) · Mockup frame A06a
- [ ] **CMD-related** GoTo · Go to a related agent…. Pick one of this thread's related agents and open it. Default keys: Alt+Down. [commands.rs:281](../../crates/yaac-core/src/commands.rs#L281) · Mockup frame A08a
- [ ] **CMD-settings** GoTo · Settings. Open Settings. Default keys: Ctrl+Comma. [commands.rs:282](../../crates/yaac-core/src/commands.rs#L282) · Mockup frame A27a
- [ ] **CMD-settings-page** GoTo · Settings: a page…. Open one Settings page (Targets, Providers, Keyboard, Window…). [commands.rs:283](../../crates/yaac-core/src/commands.rs#L283) · Mockup frame A27a
- [ ] **CMD-new-thread** Threads · New thread. Start a draft (choose target, project, model). Default keys: Ctrl+N. [commands.rs:285](../../crates/yaac-core/src/commands.rs#L285) · Mockup frame A16a
- [ ] **CMD-new-in-project** Threads · New thread in this project. Draft in this thread's project and target, same harness and model. Default keys: Ctrl+Shift+N. [commands.rs:286](../../crates/yaac-core/src/commands.rs#L286) · Mockup frame A02b, A16a
- [ ] **CMD-rename** Threads · Rename. Rename this thread. Default keys: F2. [commands.rs:287](../../crates/yaac-core/src/commands.rs#L287) · Mockup frame A06a
- [ ] **CMD-copy-id** Threads · Copy thread ID. Copy the conversation's shareable thread id. Default keys: Ctrl+Shift+C. [commands.rs:288](../../crates/yaac-core/src/commands.rs#L288) · Mockup frame A02a
- [ ] **CMD-pin** Threads · Pin / Unpin. Keep this thread in Pinned. Default keys: Ctrl+Shift+P. [commands.rs:289](../../crates/yaac-core/src/commands.rs#L289) · Mockup frame A02a
- [ ] **CMD-mute** Threads · Mute… / Unmute. Mute for a duration or until a date. Default keys: Ctrl+Shift+M. [commands.rs:290](../../crates/yaac-core/src/commands.rs#L290) · Mockup frame A02d
- [ ] **CMD-archive** Threads · Archive… / Unarchive. Move this thread to Archived, with confirmation. Default keys: Ctrl+Shift+E. [commands.rs:291](../../crates/yaac-core/src/commands.rs#L291) · Mockup frame A03b
- [ ] **CMD-delete** Threads · Delete with the optional harness-session deletion choice. Default keys: Ctrl+Delete. [commands.rs:292](../../crates/yaac-core/src/commands.rs#L292) · Mockup frame A02e
- [ ] **CMD-dismiss** Threads · Dismiss / Show again. Hide a discovered thread in the window. [commands.rs:293](../../crates/yaac-core/src/commands.rs#L293) · Mockup frame A02a
- [ ] **CMD-dismiss-all-temp** Threads · Dismiss all in this temporary project. Hide every thread of this folder in the window. [commands.rs:294](../../crates/yaac-core/src/commands.rs#L294) · Mockup frame A02b
- [ ] **CMD-fork** Threads · Fork…. Open the fork dialog (native or handoff). Default keys: Ctrl+Shift+F. [commands.rs:295](../../crates/yaac-core/src/commands.rs#L295) · Mockup frame A18b
- [ ] **CMD-take-over** Threads · Take over. Continue here a thread another program holds; that program is not stopped. [commands.rs:296](../../crates/yaac-core/src/commands.rs#L296) · Mockup frame A17a
- [ ] **CMD-move** Threads · Move to project…. List this thread under another project or worktree of its machine; its harness may move its folder too. [commands.rs:297](../../crates/yaac-core/src/commands.rs#L297) · Mockup frame A03f
- [ ] **CMD-move-selected** Threads · Move selected…. Move the rows selected in the sidebar to another project or worktree of their machine. [commands.rs:298](../../crates/yaac-core/src/commands.rs#L298) · Mockup frame A03g
- [ ] **CMD-check-latest** Threads · Check for the latest. Read this thread again from its target. [commands.rs:299](../../crates/yaac-core/src/commands.rs#L299) · Mockup frame A17a
- [ ] **CMD-continue-budget** Threads · Continue (budget). Resume an orchestration paused on its budget. [commands.rs:300](../../crates/yaac-core/src/commands.rs#L300) · Mockup frame A10d
- [ ] **CMD-archive-selected** Threads · Archive selected…. Archive the rows selected in the sidebar, with one confirmation. [commands.rs:301](../../crates/yaac-core/src/commands.rs#L301) · Mockup frame A03b
- [ ] **CMD-mute-selected** Threads · Mute selected…. Mute the rows selected in the sidebar, with one choice of how long. [commands.rs:302](../../crates/yaac-core/src/commands.rs#L302) · Mockup frame A02d
- [ ] **CMD-clear-selection** Threads · Clear selection. Unselect the rows selected in the sidebar. Default keys: Escape. [commands.rs:303](../../crates/yaac-core/src/commands.rs#L303) · Mockup frame A03a
- [ ] **CMD-focus-composer** Composer · Focus the composer. Put the caret in the message box. Default keys: Ctrl+L. [commands.rs:305](../../crates/yaac-core/src/commands.rs#L305) · Mockup frame A12a
- [ ] **CMD-send** Composer · Send / Queue message. Send; while a turn runs, queue it for the turn's end. Default keys: Enter. [commands.rs:306](../../crates/yaac-core/src/commands.rs#L306) · Mockup frame A12a
- [ ] **CMD-send-now** Composer · Send now. While a turn runs: steer it, or interrupt it and start again with the message. Default keys: Ctrl+Enter. [commands.rs:307](../../crates/yaac-core/src/commands.rs#L307) · Mockup frame A12a
- [ ] **CMD-new-line** Composer · New line. Start a new line in the message. Default keys: Shift+Enter. [commands.rs:308](../../crates/yaac-core/src/commands.rs#L308) · Mockup frame A16a
- [ ] **CMD-stop** Composer · Stop the turn. Stop the running turn. Default keys: Escape. [commands.rs:309](../../crates/yaac-core/src/commands.rs#L309) · Mockup frame A12a
- [ ] **CMD-attach** Composer · Attach files…. Open the file picker. Default keys: Ctrl+Shift+A. [commands.rs:310](../../crates/yaac-core/src/commands.rs#L310) · Mockup frame A13d
- [ ] **CMD-paste** Composer · Paste. Paste text, files or images. Default keys: Ctrl+V. [commands.rs:311](../../crates/yaac-core/src/commands.rs#L311) · Mockup frame A13e
- [ ] **CMD-model** Composer · Change model…. Pick a model of this harness. Default keys: Ctrl+M. [commands.rs:312](../../crates/yaac-core/src/commands.rs#L312) · Mockup frame A14a
- [ ] **CMD-effort** Composer · Change effort…. Pick an effort level. Default keys: Ctrl+E. [commands.rs:313](../../crates/yaac-core/src/commands.rs#L313) · Mockup frame A14a
- [ ] **CMD-mode** Composer · Change mode…. Pick the approval mode (ask, auto, …). [commands.rs:314](../../crates/yaac-core/src/commands.rs#L314) · Mockup frame A14c
- [ ] **CMD-agents** Composer · Agents: ask / allow…. Set whether agents may spawn and message. [commands.rs:315](../../crates/yaac-core/src/commands.rs#L315) · Mockup frame A10c
- [ ] **CMD-fast** Composer · Fast mode on / off. Toggle fast mode where the model offers it. Default keys: Ctrl+Shift+L. [commands.rs:316](../../crates/yaac-core/src/commands.rs#L316) · Mockup frame A14b
- [ ] **CMD-allow-once** Requests · Allow once. Allow the focused approval request this time. Default keys: Enter. [commands.rs:318](../../crates/yaac-core/src/commands.rs#L318) · Mockup frame A05a
- [ ] **CMD-allow-session** Requests · Allow for this thread. Allow the focused request for the rest of the thread. [commands.rs:319](../../crates/yaac-core/src/commands.rs#L319) · Mockup frame A05a
- [ ] **CMD-deny** Requests · Deny. Deny the focused approval request. Default keys: Escape. [commands.rs:320](../../crates/yaac-core/src/commands.rs#L320) · Mockup frame A05a
- [ ] **CMD-answer-question** Requests · Answer the question. Focus the open question's input. [commands.rs:321](../../crates/yaac-core/src/commands.rs#L321) · Mockup frame A10a
- [ ] **CMD-open-pr** PullRequests · Open this branch's PR. Open it in the browser. Default keys: Ctrl+Shift+O. [commands.rs:323](../../crates/yaac-core/src/commands.rs#L323) · Mockup frame A01a
- [ ] **CMD-create-pr** PullRequests · Create a pull request…. Open the create modal. [commands.rs:324](../../crates/yaac-core/src/commands.rs#L324) · Mockup frame A19a
- [ ] **CMD-copy-pr-link** PullRequests · Copy the PR link. Copy the pull request's address. [commands.rs:325](../../crates/yaac-core/src/commands.rs#L325) · Mockup frame A02f
- [ ] **CMD-toggle-sidebar** View · Toggle the sidebar. Collapse the sidebar to the strip and back. Default keys: Ctrl+B. [commands.rs:327](../../crates/yaac-core/src/commands.rs#L327) · Mockup frame A04a
- [ ] **CMD-cycle-fullscreen-tabs** View · Cycle full-screen tabs. Step through Chat and full-screen tabs. Release the shortcut modifier to switch. Default keys: Ctrl+Tab. [commands.rs:328](../../crates/yaac-core/src/commands.rs#L328) · Mockup frame A25c
- [ ] **CMD-dock** View · Dock / undock the agents panel. Dock the agents panel beside the thread, or float it. Default keys: Ctrl+Shift+B. [commands.rs:329](../../crates/yaac-core/src/commands.rs#L329) · Mockup frame A08b
- [ ] **CMD-grouping** View · Grouping: machine / project / worktree…. Toggle each grouping of the sidebar. [commands.rs:330](../../crates/yaac-core/src/commands.rs#L330) · Mockup frame A01b
- [ ] **CMD-order** View · Order: last message / A → Z / Z → A…. How threads are ordered in each group. [commands.rs:331](../../crates/yaac-core/src/commands.rs#L331) · Mockup frame A01b
- [ ] **CMD-unseen-first** View · Keep unseen at the top. Unseen finished threads stay at the top of their group. [commands.rs:332](../../crates/yaac-core/src/commands.rs#L332) · Mockup frame A01b
- [ ] **CMD-needs-first** View · Keep needs-you at the top. Threads that need you stay at the top of their group. [commands.rs:333](../../crates/yaac-core/src/commands.rs#L333) · Mockup frame A01b
- [ ] **CMD-running-only** View · Show only running. List only running threads (and those kept at the top). Default keys: Ctrl+Shift+R. [commands.rs:334](../../crates/yaac-core/src/commands.rs#L334) · Mockup frame A01b
- [ ] **CMD-show-dismissed** View · Show dismissed. List dismissed threads too. [commands.rs:335](../../crates/yaac-core/src/commands.rs#L335) · Mockup frame A01b
- [ ] **CMD-show-archived** View · Show archived. Open the Archived section. [commands.rs:336](../../crates/yaac-core/src/commands.rs#L336) · Mockup frame A01a
- [ ] **CMD-theme** View · Theme: system / light / dark…. Choose the colours. [commands.rs:337](../../crates/yaac-core/src/commands.rs#L337) · Mockup frame A27d
- [ ] **CMD-zoom-in** View · Zoom in. Make the interface larger. Default keys: Ctrl+Equals, Ctrl+Plus. [commands.rs:338](../../crates/yaac-core/src/commands.rs#L338) · Mockup frame A27d
- [ ] **CMD-zoom-out** View · Zoom out. Make the interface smaller. Default keys: Ctrl+Minus. [commands.rs:339](../../crates/yaac-core/src/commands.rs#L339) · Mockup frame A27d
- [ ] **CMD-zoom-reset** View · Reset zoom. Return the interface to its normal size. Default keys: Ctrl+0. [commands.rs:340](../../crates/yaac-core/src/commands.rs#L340) · Mockup frame A27d
- [ ] **CMD-expand-all** View · Expand all tool lines. Open every activity line in this thread. [commands.rs:341](../../crates/yaac-core/src/commands.rs#L341) · Mockup frame A11a
- [ ] **CMD-collapse-all** View · Collapse all tool lines. Close every activity line in this thread. [commands.rs:342](../../crates/yaac-core/src/commands.rs#L342) · Mockup frame A11a
- [ ] **CMD-copy-selection** View · Copy the selection. Copy the selected text. Default keys: Ctrl+C. [commands.rs:343](../../crates/yaac-core/src/commands.rs#L343) · Mockup frame A11e
- [ ] **CMD-copy-message** View · Copy message. Copy the agent's last message in this thread. [commands.rs:344](../../crates/yaac-core/src/commands.rs#L344) · Mockup frame A11c
- [ ] **CMD-retry** Targets · Retry connecting…. Retry an offline target now. [commands.rs:346](../../crates/yaac-core/src/commands.rs#L346) · Mockup frame A27b
- [ ] **CMD-start-wsl** Targets · Start a WSL distribution…. Start a stopped distribution and connect to it. [commands.rs:347](../../crates/yaac-core/src/commands.rs#L347) · Mockup frame A20b
- [ ] **CMD-target-enable** Targets · Enable / disable a target…. Turn a target's connection on or off. [commands.rs:348](../../crates/yaac-core/src/commands.rs#L348) · Mockup frame A27b
- [ ] **CMD-add-ssh** Targets · Add an SSH target…. Open the add form. [commands.rs:349](../../crates/yaac-core/src/commands.rs#L349) · Mockup frame A28a
- [ ] **CMD-add-wsl** Targets · Add WSL…. Open the WSL form. [commands.rs:350](../../crates/yaac-core/src/commands.rs#L350) · Mockup frame A27b
- [ ] **CMD-refresh-providers** Targets · Refresh providers and models…. Read a target's harnesses again. [commands.rs:351](../../crates/yaac-core/src/commands.rs#L351) · Mockup frame A27c
- [ ] **CMD-toggle-terminal-panel** Terminal · Toggle terminal panel. Open or close the terminal panel under the composer. Default keys: Ctrl+`. [commands.rs:353](../../crates/yaac-core/src/commands.rs#L353) · Mockup frame A24d
- [ ] **CMD-new-terminal** Terminal · New terminal. Add a terminal with the default shell to the panel. Default keys: Ctrl+Shift+`. [commands.rs:354](../../crates/yaac-core/src/commands.rs#L354) · Mockup frame A24a
- [ ] **CMD-new-terminal-with-shell** Terminal · New terminal with shell…. Choose one of the machine's shells; adds it to the panel. [commands.rs:355](../../crates/yaac-core/src/commands.rs#L355) · Mockup frame A24a
- [ ] **CMD-next-terminal** Terminal · Next terminal. Switch to the panel's next terminal tab. Default keys: Ctrl+PageDown. [commands.rs:356](../../crates/yaac-core/src/commands.rs#L356) · Mockup frame A24d
- [ ] **CMD-prev-terminal** Terminal · Previous terminal. Switch to the panel's previous terminal tab. Default keys: Ctrl+PageUp. [commands.rs:357](../../crates/yaac-core/src/commands.rs#L357) · Mockup frame A24d
- [ ] **CMD-maximise-terminal-panel** Terminal · Maximise terminal panel. The panel takes the chat column's height, or gives it back. [commands.rs:358](../../crates/yaac-core/src/commands.rs#L358) · Mockup frame A24d
- [ ] **CMD-hide-to-tray** Window · Hide to the tray. Hide the window; the tray icon brings it back. Default keys: Ctrl+W. [commands.rs:360](../../crates/yaac-core/src/commands.rs#L360) · Mockup frame A21c
- [ ] **CMD-quit** Window · Quit YAAC. Close the window (the hub keeps running). Default keys: Ctrl+Q. [commands.rs:361](../../crates/yaac-core/src/commands.rs#L361) · Mockup frame A21c
- [ ] **CMD-renderer** Window · Renderer…. Choose the renderer in Settings → Window. [commands.rs:362](../../crates/yaac-core/src/commands.rs#L362) · Mockup frame A27a
