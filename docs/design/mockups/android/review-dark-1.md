# Android mockups — dark review, batch 1 (A01–A10d)

Reviewer: Claude (Opus 5.5). Judged against `android-screens.md` (Frames), `mockups/android/STYLE.md`,
and `screens.md` → Visual direction / The fleet.

## Shared fixes (referenced below; expand into each prompt)

- **S1 — list backdrop** (every list frame, A01–A03): (a) the list shows only 4 of the 8 fleet threads.
  Draw the full fleet: **Pinned** = Migrate auth module — orchestrator. Under it, **api · local**
  (users-service, unseen done, with the cornflower outline). **api · devbox** (billing-service done,
  CI integration waiting). **search · gpu-box** (search-service running, index-rebuild needs-you
  with the red !). **yaac · devbox** (Fix frame decoder needs-you). The Release notes row goes
  under a project header with "staging" on the right and "offline" next to it. Two rows must
  show the red ! so the count matches "(!) 2". (b) Add the temporary project: an italic name,
  a dashed mark and its folder path. (c) Remove the outline around the search field: use the
  faint fill only, and the neutral focus ring only when focused. (d) Keep one PR badge number
  in every frame (the frames show #33342 and #33343). (e) Draw the hub status with the
  connection glyph, not a share/nodes icon.
- **S2 — orchestrator thread backdrop** (A06–A10): (a) For the running state in the header,
  use the primary-text **play triangle**, not a dotted spinner. (b) The control row must read
  `Codex ▸ gpt-6.2-sol ▾ · high ▾ · ask ▾ · agents: ask ▾ · 38% · Fork`. Some frames drop
  "Codex ▸" and others drop the mode. Put the row below the composer, not inside its box.
  (c) Remove the outlines from the composer, the pill, the peer pills, the cards and the
  code blocks. (d) Draw the → arrow neutral, not green. (e) Remove the left "—" dashes beside
  each activity line: timeline dashes mark prompts, and only when there are ≥ 2 prompts.
  (f) "Started" rows include the model: `Started [users-service] · opus-5.5 · done`.
  (g) In the pill, mark the search-service cluster with the red ! (index-rebuild below it needs
  you). Remove the unexplained branch icon at the top of the pill: the root has no parent
  cluster. (h) Only one filled button per screen. While a request card is on screen, Queue is
  plain text.
- **S3 — dialogs, sheets and forms**: secondary buttons (Cancel, Deny…) are plain text with no
  outline. Dropdowns and inputs have the faint fill and no outline. Remove the hairline rules
  between sheet items (use space, or the desktop's separators only where the desktop has them).
  No folder, server or branch icons inside fields.

## Frames

A01a — FIX: S1. Rate-limit group: the five-segment bars are fine. Show distinct real values per provider rather than two identical "55% · 2d" rows (optional).
A01b — FIX: S1. Order by is exclusive: draw radio marks with "Last message" selected, not empty checkboxes. Add "Show archived" (it opens the Archived section). Remove the hairline rule inside the sheet. Separate sections by space.
A01c — FIX: S1. Remove the outlined "Max" chip: write the plan as plain secondary text ("Claude Max"). "Fresh" must not be green (green means done only): use secondary text, e.g. "updated 1 min ago". Show the used % for each window (5 h and weekly), each with its local reset time. Remove the inner hairlines and the card outline: use the raised fill and a shadow for the floating card.
A01d — FIX: S1(d, e) only. The focused field may keep a ring, but it must be the neutral focus ring #C9CED6. The results and "Search older threads on devbox" are correct.
A02a — FIX: S1. Add the desktop separators: Copy thread ID, Rename | Fork, Move…, Pin, Mute…, Archive…, Create pull request… | Delete… (in the failed colour).
A02b — FIX: S1, S3. Remove the hairlines between New Thread / Archive all (1)… / Mute all….
A02c — FIX: S1. The row's badge must read "#33342 +1" (one PR number plus the +1), and the sheet lists the *other* PR, e.g. "#33343 · Fix frame decoder (follow-up)". Remove the branch icon on each sheet row.
A02d — FIX: S1, S3. Remove every outline: the three option pills, the date and time fields, and the Cancel button. Options are plain radio rows: Until unmuted · For [2] [hours ▾] · Until date and time. The selected day keeps its neutral fill. Use a date consistent with the fleet's "today" across frames.
A02e — FIX: S1, S3. The Delete button is a solid primary-text-coloured button whose label is in the failed colour (or a plain red text action). No red fill. The checkbox is monochrome, not red. Remove the dialog outline (use the raised fill and a shadow). Cancel is plain text.
A02f — FIX: S1 only (the sheet content itself is correct).
A03a — FIX: S1. The rows, 3 px bar, tint, handles and selection bar are correct.
A03b — FIX: S1, S3. While the dialog is open the list behind must show the selection state: the top bar is the selection bar "5 selected · Archive · Mute · Move · ⋮ · ×", and 5 rows have the tint and the bar. Remove the invented bottom bar "5 selected · Cancel".
A03c — FIX: S1. The dragged stack uses neutral raised fill, not blue/cornflower tint and glow (cornflower = unseen only). Drag rows that are *not* already in api: Fix frame decoder from yaac · devbox. The target api · devbox header has the neutral focus-colour outline (correct). The "Move to api" card has no outline.
A03d — FIX: S1. The refused target gets the same neutral focus outline (not a red dashed box, no red tint). The card is the neutral raised card with a not-allowed mark and "A thread cannot move to another machine" in primary text, no red fill. The dragged chips have no outlines.
A03e — FIX: S1. After the move, the list must show the moved threads under api · devbox (the yaac group loses them). "Undo" is a plain primary-text action, not a blue underlined link.
A03f — FIX: S1, S3. The machine is a fixed label ("devbox"), not a dropdown: a thread cannot change machine. Only Project and Worktree are dropdowns. Remove the server, folder and branch icons in the fields. Cancel is plain text.
A03g — FIX: S1, S3. Same machine-label and outline fixes as A03f. The kept path must be the moved threads' current folder (e.g. `/home/dev/yaac`), not `/srv/search`.
A04a — FIX: regenerate entirely. Draw an 800 × 412 dp landscape YAAC frame, 09:41 status row. Left: the strip, top to bottom: the YAAC mark, ＋, the back/forward history buttons, vertical project names (api, search, yaac), one harness logo per thread (Claude orange / Codex neutral) with its state badge, the rate-limit bars, and ⚙ at the bottom. Right: the Fix frame decoder thread exactly as A05a (header with the tag and breadcrumb, the transcript, the approval card with the amber edge, the composer, the control row). Remove the invented "YAAC" speaker and avatar, "Pending approval / Approve / Reject", "Type a message…", and the outlined card and composer.
A04b — FIX: regenerate entirely, the same layout as A04a. One strip icon (the Codex logo with the red ! for Fix frame decoder) is held, and beside it is its floating row card: "Fix frame decoder · PR #33342 · 3m / feature/protocol +2 ~1". Remove the invented code, branch and layers icons, the projects "core / protocol / ui", the avatars, the purple "Y", "Message YAAC…" and the search icon.
A05a — FIX: Composer placeholder "Message Codex…" on the faint fill. The control row starts with "Codex · work ▸". Remove the outlines on the code block and the approval card (faint fill + amber edge only). The amber stripe is #F2B24C (it renders too saturated). Status row: signal, Wi-Fi and battery glyphs (a square renders now).
A05b — FIX: Keep the command consistent with A05a ("Show command (3 lines)" → show 3 lines). The AI explanation must describe the shown command (e.g. "Removes the stale build directory, then rebuilds"). Keep `/home/dev/yaac` visible. Remove the outline on the command box (faint fill). The same composer and control-row fixes as A05a.
A06a — FIX: The backdrop must be **index-rebuild's** own thread (the header shows index-rebuild · Needs you — question · gpu-box · search · main), not the orchestrator. The sheet fields: an editable Title (the input has the faint fill and the focus ring), then origin, project, worktree and machine as plain text rows with no boxes. The Project row has no search icon. Add the links "Spawned by search-service" and "Forked from —", plus the ancestry orchestrator › search-service › index-rebuild as tappable text. Copy thread ID is a plain text action. S2.
A06b — FIX: S2 for the dimmed backdrop. The Back history sheet itself is correct.
A07a — FIX: S2. Add the missing agent-call kinds from the brief: `↗ [billing-service · devbox] "Tell me when merged" · delivered`, `⧗ Waiting for [search-service]`, `↙ [users-service] reported back`, and the mesh line `← billing-service · Migration merged…` in its muted single-line form. The pill shows the in-flight arrows on the contact cluster only while a message is in flight, and dims the finished workers (users-service, billing-service).
A07b — FIX: S2. Open a worker call (e.g. ⑂ Started users-service) or keep the CI message, but add the **Result** section. Draw "To" as peer pills (logo + name · machine), not bracketed text. Show the task in full on the faint fill, the receipt line, then **Result**, then **Raw call**. Remove the card outline.
A07c — FIX: regenerate. The pill must end with "more (23) · 4 alive", with dimmed finished marks. The header shows the Needs-you button "!2", not an invented red "4/6". The transcript shows many Started rows (≈30 agents). S2.
A08a — FIX: The half sheet shows the **simple** rows only (logo, name, last activity, age, ⋮), under Siblings (self marked), Workers, In contact with (if any) and Spawned by. The full docked rows belong to A08b. The area above is search-service's real transcript: no "You"/person avatar and no monospace prose (agent prose 16 sp sans), with the normal thread header (← · state mark · title · !2 · ⌘ · ⋮). Use the gesture bar, not the 3-button nav.
A08b — FIX (minor): Add the speed bolts on the rows that have them and a "› N finished" line consistent with the fleet. Drop the meaningless "+0 −0" on index-rebuild (show no counts). The running state mark is the neutral running mark, not a dotted spinner (S2a).
A08c — FIX: The sheet keeps all its rows (billing-service, search-service self, users-service, index-rebuild), with the held row (index-rebuild) highlighted in the selected-row tint. The preview above shows index-rebuild's transcript: its From task on the faint fill and its question card with the **amber** left edge (not a cornflower outline). Keep the search-service header so it is clear the current thread is unchanged.
A08d — FIX: S3. The Set/Extend sheet: +1 h, +4 h, +12 h, +24 h as plain text rows (no clock icon on each row). Then one row: an hours field (faint fill) with an **Add** button beside it. Then "Remove time limit". Remove the hairlines and the popup outline. Keep users-service's counts consistent with A08b (+6 −1, not +2).
A09a — FIX: S2. The mirrored copy must be **focused**: draw the neutral focus ring (#C9CED6) on the first option or the card. Remove the outline from the Submit answer button: it is the one filled primary. The "index-rebuild · gpu-box" pill has a quiet fill and no outline. The composer is still editable (correct).
A09b — FIX: Header: ← · the red ! state mark · index-rebuild. The second line is the tag "Needs you — question" (the words in amber) · gpu-box · search · main. Add the Needs-you button "!2" and ⌘ · ⋮. Show the ancestry links "Spawned by search-service ↗ · root: Migrate auth module — orchestrator". The **From search-service** task sits on the faint fill at the left, labelled "From search-service · Claude · gpu-box". Rebuild the composer: the placeholder "Message Claude…", the + and Queue on the button row, the hint "Queued until the turn ends", then the control row `Claude ▸ opus-5.5 ▾ · … · Fork`. Remove the "Queue" typed into the field and the card outline.
A09c — FIX: S2. The resolved copy loses the amber stripe (amber only on actionable cards) and shows the chosen answer as plain text. The Needs-you count drops to "!1" (only Fix frame decoder remains).
A10a — FIX: S2, S3. Use real options instead of "A / B": a single-choice group (Changed shards / All shards), a multi-select group (e.g. "Also rebuild: ☐ users ☐ billing"), a free-text field (faint fill, no outline), and a masked secret. Add an example of the "Answer this in Codex/Claude" fallback for an unrenderable form, or leave that to its own frame.
A10b — FIX: S2. Draw the five outcomes as five resolved question cards stacked: the question title, the answer, then the outcome line. They have no amber stripe and no outlines, and the outcome is written in muted text: "Submitted — waiting for confirmation", "Confirmed — answered by you" (with the done check), "Resolved elsewhere", "Expired", and "Outcome unknown" with a plain-text **Check latest status** action. No resend anywhere.
A10c — FIX: S2, S3. A permission card is a request card: give it the faint fill and the **amber left edge**, and remove the outline and the invented icon. Add the heading "Budget for this orchestration" above Agent turns [100] and Hours [12] (fields with the faint fill, no outline). Allow is the only filled button; Deny and Always allow in this chat are plain text, so Queue in the composer is plain text too. Add the Needs-you button to the header.
A10d — FIX: S2. The budget notice must be monochrome: a faint-fill banner with no amber/orange fill and no outline. Any warning mark is neutral, and **Continue** is the primary filled button (so Queue is plain text). Keep the copy "3 messages are waiting. Nothing was cancelled." The header state is paused: use a paused mark, not the running spinner. Stop is hidden while paused.

## Scores (this batch, dark)

| Criterion | Score | Why |
| --- | --- | --- |
| Usability | 5 | The core mechanics read well: selection bar, sheets, approval card, Related agents rows. They are undermined by outlines everywhere, several primary buttons per screen, and broken composers (A09b) and dialogs (A03b). |
| Journeys | 4 | The list shows half the fleet, so the needs-you journey to index-rebuild cannot be followed from it. A04a/b are unrelated invented screens. A06a shows the wrong thread, and A08c/A09c break continuity (lost rows, an unchanged count). |
| Dark | 6 | The palette and surfaces are mostly right and the harness logos are correct. The misuses: amber/red/green and cornflower carry non-state meanings (the A10d banner, the A03d refusal, "Fresh", the blue drag stack, the green →), plus a spinner instead of the play triangle and a too-saturated amber. |

**Result: 0 PASS · 37 FIX** (A02f, A03a and A06b need only the shared backdrop fixes; A08b's fix is minor; A04a, A04b and A07c need a full regeneration).
