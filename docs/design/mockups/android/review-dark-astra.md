# Android dark review · Astra

## Round 1

I inspected all 110 dark frames against the [Android screen brief](../../android-screens.md). The 106 portrait, two medium-width and two tablet PNGs decode and have distinct image digests. A frame passes here when it demonstrates the named interaction clearly. Repeated styling defects also count in the overall usability and dark scores.

| Usability | Journeys | Dark legibility | Dark/light parity |
| --- | --- | --- | --- |
| 6/10 | 6/10 | 7/10 | Pending light frames |

Approval and notification actions read clearly. Files and Terminal repeatedly lose the open thread’s identity. Fork introduces providers outside the fixture, and the tablet drops the real sidebar. The dark theme usually separates prose from activity, but some active controls depend on colour or use ambiguous state glyphs.

**22 PASS · 88 FIX.** Each finding identifies the visible behavior to keep or change.

| Frame | Verdict | Finding |
| --- | --- | --- |
| [A01a](A01a-dark.png) | FIX | Show the pinned thread with a play mark and add the temporary project to the list. |
| [A01b](A01b-dark.png) | FIX | Give Order by a single checked radio and include Show archived. |
| [A01c](A01c-dark.png) | FIX | Show each quota window with its own percentage and local reset. |
| [A01d](A01d-dark.png) | FIX | Keep the single search result and put a neutral focus ring around the active field only. |
| [A02a](A02a-dark.png) | FIX | Use spacing to separate the identity, thread and destructive command groups. |
| [A02b](A02b-dark.png) | FIX | Remove the rule between New Thread and bulk actions. |
| [A02c](A02c-dark.png) | FIX | Keep PR# 33342 in the row and show a distinct additional PR in the sheet. |
| [A02d](A02d-dark.png) | FIX | Use vertically stacked radio rows for the three mute choices, with larger calendar targets. |
| [A02e](A02e-dark.png) | FIX | Default the harness deletion checkbox to unchecked and keep destructive confirmation explicit. |
| [A02f](A02f-dark.png) | PASS | The PR context menu offers both Create a new PR and Copy link. |
| [A03a](A03a-dark.png) | PASS | Three tinted rows, three handles and a selection count agree. |
| [A03b](A03b-dark.png) | FIX | Keep five visibly selected rows and the selection toolbar behind the dialog. Source eligible/skipped counts from that selection. |
| [A03c](A03c-dark.png) | FIX | Keep the selection toolbar during the drag and put the card beside the destination. |
| [A03d](A03d-dark.png) | FIX | Keep the not-allowed symbol and reason on a neutral surface with failed-colour text. |
| [A03e](A03e-dark.png) | FIX | Show the moved rows under api and retain the Undo action above the footer. |
| [A03f](A03f-dark.png) | FIX | Remove borders from the Project and Worktree fields. Leave the folder-following explanation visible. |
| [A03g](A03g-dark.png) | FIX | Keep the actual working folder in the explanation and use borderless controls. |
| [A04a](A04a-dark.png) | FIX | Replace the miniature generic approval app with YAAC’s medium-width strip and real thread. |
| [A04b](A04b-dark.png) | FIX | Anchor the two-line held-row card to the harness icon and retain the real landscape thread. |
| [A05a](A05a-dark.png) | PASS | Approval choices, reason, Queue and the waiting hint read together. |
| [A05b](A05b-dark.png) | FIX | Differentiate the expanded command and its deterministic danger label. Make the explanation agree with the actual command. |
| [A06a](A06a-dark.png) | FIX | The sheet edits index-rebuild while the header still names the orchestrator. Show the worker as the open thread. |
| [A06b](A06b-dark.png) | PASS | The history lists distinct thread and Settings destinations. |
| [A07a](A07a-dark.png) | FIX | Use a solid triangle, neutral mesh arrows and a prompt-indexed timeline. |
| [A07b](A07b-dark.png) | FIX | Expand the worker-start call itself with To, task, receipt, Result and Raw call. The current frame opens a contact message. |
| [A07c](A07c-dark.png) | FIX | The 30-agent case must put “more (23) · 4 alive” in the right-edge pill. |
| [A08a](A08a-dark.png) | FIX | Put the sheet grabber halfway down and leave enough real search-service transcript visible. |
| [A08b](A08b-dark.png) | PASS | Sibling, worker and origin sections are distinct and the fleet is identifiable. |
| [A08c](A08c-dark.png) | FIX | Show all three named siblings and the single held worker. Its question preview is read-only and the request stripe is amber. |
| [A08d](A08d-dark.png) | FIX | Make the time-limit menu belong to the budget header and keep its chosen duration legible. |
| [A09a](A09a-dark.png) | FIX | Focus the question copy itself and show the timeline for the parent’s multiple prompts. |
| [A09b](A09b-dark.png) | FIX | Replace the field containing the literal word “Queue” with a real composer and separate Queue action. |
| [A09c](A09c-dark.png) | PASS | The copy records the answer and who answered it. |
| [A10a](A10a-dark.png) | FIX | Use meaningful option labels, visible multi-select controls and a neutral focus ring. |
| [A10b](A10b-dark.png) | FIX | Make Check latest status an actionable affordance on Outcome unknown. |
| [A10c](A10c-dark.png) | FIX | Add “Budget for this orchestration” above the two numeric controls and the request stripe. |
| [A10d](A10d-dark.png) | FIX | Show the wait count and Continue on a faint notice surface with one clear primary action. |
| [A10e](A10e-dark.png) | PASS | The named scope and Cancel default communicate the bypass decision. |
| [A10f](A10f-dark.png) | FIX | Name both affected descendants and keep Only this thread visibly selected. |
| [A11a](A11a-dark.png) | FIX | Render the final agent response directly on the canvas. Keep fold indicators compact. |
| [A11b](A11b-dark.png) | FIX | Show the failed command, exit code and expanded error. Idle composer offers Send. |
| [A11c](A11c-dark.png) | PASS | The pasted text, command and expanded IDE context are distinguishable. |
| [A11d](A11d-dark.png) | FIX | Keep reasoning visually collapsed and show the failed command result in its expanded body. |
| [A11e](A11e-dark.png) | FIX | Only Copy belongs in the cross-message selection toolbar. Remove model and policy selectors. |
| [A11f](A11f-dark.png) | FIX | Align timeline marks to user prompts and show a preview of the held prompt. |
| [A12a](A12a-dark.png) | PASS | Queue, Stop, queued-message actions and running command controls are distinct. |
| [A12b](A12b-dark.png) | PASS | The live log opens directly below the command whose Logs control was tapped. |
| [A12c](A12c-dark.png) | PASS | Sending, Not sent and Outcome unknown have different recovery actions. |
| [A13a](A13a-dark.png) | FIX | Keep the attachment and action row visible above the keyboard while the skill picker is open. |
| [A13b](A13b-dark.png) | FIX | Show repo-relative paths in the @ picker and remove the history button from the nonempty composer. |
| [A13c](A13c-dark.png) | FIX | Use neutral command-mode text and keep Queue run on the normal action row. |
| [A13d](A13d-dark.png) | FIX | Clearly disable sending during upload. Give separate choices for unknown image support and explain file fallback. |
| [A13e](A13e-dark.png) | PASS | All three large-paste actions are visible. |
| [A13f](A13f-dark.png) | FIX | Replace repeated launcher icons and YAAC labels with thread titles, targets, state and provider marks in the destination list. |
| [A14a](A14a-dark.png) | FIX | Show the instance and account, available effort range and unavailable reason in the model list. |
| [A14b](A14b-dark.png) | FIX | Remove the fabricated 38% price estimate and show the actual Ultrafast confirmation. |
| [A14c](A14c-dark.png) | FIX | Keep high as effort and replace ask with Full access only in the mode position. |
| [A15a](A15a-dark.png) | FIX | Add the disabled search and Windows-only note. Distinguish harness states by shape and text. |
| [A16a](A16a-dark.png) | FIX | Group each model under its instance and show a selected model in the blank draft. |
| [A16b](A16b-dark.png) | FIX | Keep the project from A16a and the first message in a neutral user bubble. |
| [A16c](A16c-dark.png) | FIX | Show the temporary folder that initiated Create project and its prefilled path. |
| [A17a](A17a-dark.png) | FIX | Move the read-only notice to a neutral surface; retain holder, Take over and Fork. |
| [A17b](A17b-dark.png) | FIX | Keep the reason next to the disabled takeover action and remove the red banner fill. |
| [A17c](A17c-dark.png) | FIX | Remove invented GPT-4o, Medium and Chat controls; restore the actual thread capability row. |
| [A17d](A17d-dark.png) | PASS | A harness-managed worker is read-only and offers Open parent. |
| [A18a](A18a-dark.png) | FIX | Remove invented network providers, sites and models. Use fleet instance/account destinations and an explicit Native selection. |
| [A18b](A18b-dark.png) | FIX | Label the Handoff destination. Explain what the destination receives; retain size, cutoff, preview and input. |
| [A18c](A18c-dark.png) | FIX | Carry through the chosen destination, title, breadcrumb and full YAAC composer after the fork. |
| [A19a](A19a-dark.png) | FIX | Keep the source branch and destination clear; add the outcome-unknown recovery specified by the brief where applicable. |
| [A20a](A20a-dark.png) | FIX | Remove the invented mesh receipt statistics. Show queue delivery and failure as quiet notices. |
| [A20b](A20b-dark.png) | FIX | Use Ubuntu as the WSL target, leave the cached transcript readable and replace composer with Start it. |
| [A20c](A20c-dark.png) | FIX | Keep Update now as the action and remove the prominent amber border around the banner. |
| [A21a](A21a-dark.png) | PASS | The single notification offers Open, Allow once and Deny. |
| [A21b](A21b-dark.png) | PASS | The summary shows three pending requests and a single Open. |
| [A21c](A21c-dark.png) | PASS | The ongoing notification has Quit and the launcher uses the YAAC mark. |
| [A22a](A22a-dark.png) | FIX | Use the Commander search and catalogue hierarchy without decorative leading icons. |
| [A22b](A22b-dark.png) | FIX | The filtered result offers one Fork command. Remove the extra overflow menu. |
| [A22c](A22c-dark.png) | FIX | List the available model for the current harness at this Commander step. |
| [A23a](A23a-dark.png) | FIX | Restore the thread header and show correct changed-folder and per-file git marks. |
| [A23b](A23b-dark.png) | FIX | Retain Fix frame decoder identity while showing the path, line numbers and Reload. |
| [A23c](A23c-dark.png) | FIX | Add intraline changes and the real thread/tab header above unified diff navigation. |
| [A23d](A23d-dark.png) | FIX | Remove the invented Composer tab; keep Add to composer in the text-selection toolbar. |
| [A23e](A23e-dark.png) | FIX | Keep the inserted chip between the existing words and use Queue while approval is pending. |
| [A24a](A24a-dark.png) | FIX | Restore search-service’s thread header and Add to composer beside Search, keeping the terminal key row. |
| [A24b](A24b-dark.png) | FIX | Use the gpu-box prompt, neutral search highlight and the thread’s terminal tab. |
| [A24c](A24c-dark.png) | FIX | Remove the printed clipboard dump. Show a quiet shell behind the explicit paste confirmation. |
| [A24d](A24d-dark.png) | FIX | Give the panel a clear Maximise control and keep the thread’s capability row above the panel. |
| [A24e](A24e-dark.png) | FIX | Remove the earlier running line once the command result shows exit 0. |
| [A25a](A25a-dark.png) | FIX | Keep the thread header and make the selected browser tab distinct from its web content. |
| [A25b](A25b-dark.png) | FIX | Show the process/project for each discovered port inside the normal tab shell. |
| [A25c](A25c-dark.png) | FIX | Give the pinned Chat tab a clear unread mark and active browser tab a Restore action. |
| [A25d](A25d-dark.png) | FIX | Restyle DevTools with neutral YAAC selection, while keeping the element picker and Styles. |
| [A25e](A25e-dark.png) | FIX | Draw the JS prompt in YAAC’s theme. Preserve origin, input, Cancel and OK. |
| [A26a](A26a-dark.png) | PASS | The viewer shows the image, zoom level, Reset and Close. |
| [A26b](A26b-dark.png) | FIX | Use the actual harness logo in its attribution; remove the literal words “Claude mark”. |
| [A26c](A26c-dark.png) | FIX | Keep diagram export actions above a rendered diagram, using YAAC neutral controls. |
| [A27a](A27a-dark.png) | PASS | All eleven Settings pages are present and Back to threads is clear. |
| [A27b](A27b-dark.png) | FIX | Make add/edit/remove and the WSL second-session note discoverable alongside target state. |
| [A27c](A27c-dark.png) | FIX | Display the instance version, source and availability, and a route to add/edit/remove. |
| [A27d](A27d-dark.png) | FIX | Label the sizing group Interface scale and show what each preview card selects. |
| [A27e](A27e-dark.png) | FIX | The conflict must name both affected commands; display the actual bound and incoming chord. |
| [A27f](A27f-dark.png) | FIX | Show Remove for the selected user skill and Restore for the bundled override. |
| [A27g](A27g-dark.png) | FIX | Use a neutral update action, clearly named target and a selectable editable report. |
| [A28a](A28a-dark.png) | PASS | The form includes Import from SSH config, host fields and Setup. |
| [A28b](A28b-dark.png) | PASS | Server identity, algorithm and fingerprint precede the confirmation. |
| [A28c](A28c-dark.png) | PASS | The setup log ends in a verified state and offers Revoke. |
| [A28d](A28d-dark.png) | FIX | Show the full copyable public-key removal line and acknowledge manual removal separately. |
| [A28e](A28e-dark.png) | PASS | The key-file alternative gives a picker, masked passphrase and Connect. |
| [A29a](A29a-dark.png) | FIX | Build the actual desktop-style sidebar with fleet rows and footer. Remove Chat/Fleet/Agents/Files navigation and redundant pill. |
| [A29b](A29b-dark.png) | FIX | Keep fleet, chat and docked related agents. Add the diff beside the existing chat. |

## Round 2

| Usability | Journeys | Dark legibility | Dark/light parity |
| --- | --- | --- | --- |
| 7/10 | 7.5/10 | 8/10 | Pending light frames |

**54 PASS · 56 FIX** after reviewing all 110 dark images. PASS means the pictured interaction works as a design reference. The overall scores also account for state-mark and control inconsistencies between successive frames. The links in both rounds open the latest PNG for each ID.

The fork now has three distinct stages. The pre-submit screens identify the destination, and the resulting thread gives inherited messages a read-only boundary. Files, Terminal and Browser mostly preserve the open thread identity across full-screen tabs. The persistent-permission and worker-scope confirmations name what changes before the user commits.

Selection still loses commands during multi-select. The in-place group sheet invents Settings, and several relationship screens show a waiting composer when the parent can still send. The tablet fits its four regions, but its narrow agents column cannot carry the same row information as the docked view. These issues keep the set below the review target.

The generation pass ran for 106 IDs. Four images stayed in place. For A01d I restored the previous PNG after the replacement lost its Search older threads action.

| Frame | Verdict | Finding |
| --- | --- | --- |
| [A01a](A01a-dark.png) | FIX | The main fleet is visible, but the temporary folder lacks its dashed mark and the footer glyph still reads like Share. |
| [A01b](A01b-dark.png) | FIX | Keep the selected sort radio. Restore both float options and the running/dismissed filters. Remove Apply. |
| [A01c](A01c-dark.png) | PASS | The held quota group ties each local reset to its window and reports staleness. |
| [A01d](A01d-dark.png) | PASS | The filter yields one frame-related thread and a separate search-older action. |
| [A02a](A02a-dark.png) | FIX | Open the row actions in the specified bottom sheet. The image uses a desktop popover. |
| [A02b](A02b-dark.png) | FIX | Remove the large Settings action inside the group menu. Settings belongs to the list footer. |
| [A02c](A02c-dark.png) | PASS | The additional PR has its own number and target. |
| [A02d](A02d-dark.png) | FIX | Restore Cancel beside Mute and show the six-row date chooser. |
| [A02e](A02e-dark.png) | PASS | The harness-delete checkbox defaults to off. Cancel and destructive Delete are distinct. |
| [A02f](A02f-dark.png) | PASS | The bottom sheet offers exactly Create a new PR and Copy link. |
| [A03a](A03a-dark.png) | FIX | Replace the whole regular header with the selection toolbar. Archive and Mute are missing. |
| [A03b](A03b-dark.png) | FIX | The title says Archive 5 although only 3 are eligible. Use ‘Archive 3 threads?’ and remove the duplicate orchestrator row. |
| [A03c](A03c-dark.png) | FIX | Keep one selection toolbar. Place ‘Move to api’ at the hover destination, separate from the normal commands. |
| [A03d](A03d-dark.png) | FIX | Highlight the destination on another machine. The current outline is around api on the same devbox. |
| [A03e](A03e-dark.png) | FIX | Name the destination and the number of moved threads in the Undo bar. |
| [A03f](A03f-dark.png) | PASS | The move confirmation says which folder the agent will use on its next turn. |
| [A03g](A03g-dark.png) | FIX | Use faint-fill dropdowns to match the same Move sheet in A03f. |
| [A04a](A04a-dark.png) | FIX | The strip now fits, but the same harness mark is rendered beside both the user's and agent's messages. Place the user bubble at the right. |
| [A04b](A04b-dark.png) | PASS | The held strip icon opens a row card anchored beside it while the approval and composer stay visible. |
| [A05a](A05a-dark.png) | PASS | The waiting thread retains the decision card and Queue. |
| [A05b](A05b-dark.png) | PASS | The disclosed command, danger mark and explanation describe the same operation. |
| [A06a](A06a-dark.png) | FIX | The question is in the right worker thread, but the sheet grows a second composer and prints generic capability labels. |
| [A06b](A06b-dark.png) | FIX | History destinations are readable; the dimmed background still uses a spinner and green contact arrow. |
| [A07a](A07a-dark.png) | FIX | Restore model and state on Started rows, attach dashes to prompts, and show the running actions Send now and Stop. |
| [A07b](A07b-dark.png) | FIX | The expanded worker call is readable, but the root’s composer has gained an approval-wait hint. |
| [A07c](A07c-dark.png) | FIX | Keep the overflow count but remove invented Plan, Approve and Regenerate composer actions. Use the fleet root as the header. |
| [A08a](A08a-dark.png) | FIX | Restore full sibling/worker metadata and remove the extra invented contact ‘cache-service’. |
| [A08b](A08b-dark.png) | FIX | The docked sheet now shows the right family and no composer. Its budget header dropped the elapsed time and 12-hour limit. |
| [A08c](A08c-dark.png) | FIX | The relationships and read-only question preview are correct. Remove the standalone composer below the sheet and show the correct worker/target header. |
| [A08d](A08d-dark.png) | FIX | Keep the time-limit menu anchored to budget and remove the composer from the full-height sheet. |
| [A09a](A09a-dark.png) | FIX | Draw one dash for each user prompt and keep the parent’s normal running composer actions while its worker waits. |
| [A09b](A09b-dark.png) | FIX | Pair the visible Queue action with the waiting hint and the complete capability row for the open worker. |
| [A09c](A09c-dark.png) | PASS | The answer stays at the mirrored location and the confirmation names who responded. |
| [A10a](A10a-dark.png) | PASS | Single choice, multiselect, free text and masked input are visible in one question. |
| [A10b](A10b-dark.png) | PASS | Each of the five outcomes has a distinct text label. Outcome unknown offers Check latest status. |
| [A10c](A10c-dark.png) | PASS | The permission request clearly exposes both limits and all three decisions. |
| [A10d](A10d-dark.png) | PASS | The notice reports the exhausted allowance and waiting count, with Continue. |
| [A10e](A10e-dark.png) | PASS | The mode dialog names the thread and folder, describes the persistent permission, and defaults to Cancel. |
| [A10f](A10f-dark.png) | PASS | Only this thread is selected; the second scope names both affected workers. |
| [A11a](A11a-dark.png) | FIX | Render the folded turn states without contradictory ‘complete’ prose and restore normal running controls. |
| [A11b](A11b-dark.png) | PASS | The failed command has an exit code and the interrupted sequence ends at an idle composer. |
| [A11c](A11c-dark.png) | PASS | The expanded paste, command pill and context disclosure can be read separately. |
| [A11d](A11d-dark.png) | FIX | Remove the unrelated approval-wait hint. Keep the command expansion and collapsed reasoning row. |
| [A11e](A11e-dark.png) | FIX | Selection now spans both messages; remove the new Share action and keep the expected Copy toolbar. |
| [A11f](A11f-dark.png) | FIX | The held marker and tooltip must refer to the same user prompt. Remove dashes beside activity lines. |
| [A12a](A12a-dark.png) | PASS | Queue, Send now and Stop are available while work continues. |
| [A12b](A12b-dark.png) | PASS | The log belongs to the running command and the thread still accepts input. |
| [A12c](A12c-dark.png) | PASS | Outgoing bubbles distinguish pending, failed and uncertain delivery with the appropriate actions. |
| [A13a](A13a-dark.png) | FIX | The picker mixes slash commands and skills, and the control row prints generic labels in place of values. |
| [A13b](A13b-dark.png) | FIX | Preserve the repo-relative file list. Replace ‘model/effort/mode/root agents policy’ placeholders with the actual thread controls. |
| [A13c](A13c-dark.png) | FIX | Show the typed command in the monospace field, alongside the prompt, and keep the mode exit affordance. |
| [A13d](A13d-dark.png) | FIX | The image/file choice and disabled Queue are clear. Put an image thumbnail on the in-progress upload itself. |
| [A13e](A13e-dark.png) | PASS | Attaching the paste, inserting text and cancelling are distinct choices. |
| [A13f](A13f-dark.png) | FIX | Fix harness and state for each destination. users-service and billing-service are done; index-rebuild needs its owner. |
| [A14a](A14a-dark.png) | PASS | The picker uses fixture models, labels instances, and explains two unavailable choices. |
| [A14b](A14b-dark.png) | FIX | Put the confirmation beside the real composer. The render prints invented Balanced, CI and Main agents selectors. |
| [A14c](A14c-dark.png) | FIX | Mode placement is corrected. Remove the approval-wait hint from a running root that has no pending request. |
| [A15a](A15a-dark.png) | PASS | First run presents local readiness, sign-in, adding a server and WSL separately from a thread draft. |
| [A16a](A16a-dark.png) | FIX | The blank draft and its two valid models now agree; wake the selected worktree value and model trigger on the draft row. |
| [A16b](A16b-dark.png) | FIX | The project and title now match the draft; restore the thread header’s running mark, attention and Commander. |
| [A16c](A16c-dark.png) | FIX | Keep the prefilled create sheet; remove the invented formatting toolbar behind it. |
| [A17a](A17a-dark.png) | FIX | Use the read-only message in the specified banner and give the holder line once. The render duplicates ownership information. |
| [A17b](A17b-dark.png) | FIX | Keep the disabled takeover reason and restore the same real thread header and holder line as A17a. |
| [A17c](A17c-dark.png) | PASS | Takeover returns the user to an editable thread with the correct Codex model. |
| [A17d](A17d-dark.png) | PASS | The harness-owned thread now offers Open parent without Take over or a live composer. |
| [A18a](A18a-dark.png) | PASS | The native fork names its source, compatible instance/account, inherited data and Fork action. |
| [A18b](A18b-dark.png) | PASS | Handoff identifies the recipient provider, transcript boundary, preview and first message before sending. |
| [A18c](A18c-dark.png) | PASS | The new thread marks the inherited history boundary and provides an idle composer. |
| [A19a](A19a-dark.png) | FIX | Do not show ‘Outcome isn’t known yet’ before Create has been pressed. Put that recovery message in the post-submit state. |
| [A20a](A20a-dark.png) | FIX | Separate each actual delivery state. The same screen currently says a target is offline and updating and introduces Legacy Chat. |
| [A20b](A20b-dark.png) | FIX | The WSL target and Start it are correct. Remove the duplicate red down banner and fabricated model controls. |
| [A20c](A20c-dark.png) | FIX | Keep the older-daemon notice; restore the editable waiting composer and Queue under the approval. |
| [A21a](A21a-dark.png) | PASS | One pending approval has Open, Allow once and Deny. |
| [A21b](A21b-dark.png) | PASS | The lock screen shows three specific requests and one Open action. |
| [A21c](A21c-dark.png) | PASS | The persistent notification offers Quit and the launcher carries the notification badge. |
| [A22a](A22a-dark.png) | PASS | Search and the grouped command catalogue are reachable without keyboard-only controls. |
| [A22b](A22b-dark.png) | PASS | Filtering to fork leaves one actionable result and a clear-search affordance. |
| [A22c](A22c-dark.png) | PASS | The model step shows the currently available harness model and Back. |
| [A23a](A23a-dark.png) | PASS | Files retains the thread header, tree status and refresh controls. |
| [A23b](A23b-dark.png) | PASS | The file viewer keeps the owning thread, path, line numbers, syntax and Reload. |
| [A23c](A23c-dark.png) | FIX | Show intraline highlight and a clean plus/minus gutter for the changed lines. Thread identity and change navigation are now preserved. |
| [A23d](A23d-dark.png) | PASS | The file selection spans lines and offers Add to composer at the selection. |
| [A23e](A23e-dark.png) | FIX | The inline chip is positioned correctly, but the approval is reduced to a banner without Allow/Deny. |
| [A24a](A24a-dark.png) | PASS | The terminal exposes the shell-specific key row above the IME. |
| [A24b](A24b-dark.png) | PASS | Search stays in the right terminal and identifies the active match. |
| [A24c](A24c-dark.png) | PASS | The warning interrupts the paste before execution and offers Cancel or Paste. |
| [A24d](A24d-dark.png) | FIX | Make the lower terminal panel large enough to use and replace generic effort/mode field names with the actual values. |
| [A24e](A24e-dark.png) | FIX | Remove the contradictory ‘running’ activity above the exit-0 card and restore Claude · opus-5.5. |
| [A25a](A25a-dark.png) | PASS | The browser keeps search-service, its tab, address and navigation together. |
| [A25b](A25b-dark.png) | PASS | The target’s ports, processes and project paths appear in the new tab. |
| [A25c](A25c-dark.png) | FIX | Restore the thread shell above the pinned Chat and keep full-screen tabs inside it, without a hamburger or generic YAAC bar. |
| [A25d](A25d-dark.png) | PASS | Full-height DevTools retains the tab identity and uses a neutral DOM selection. |
| [A25e](A25e-dark.png) | FIX | The dialog is now dark. Restore the owning browser tab and thread identity behind it. |
| [A26a](A26a-dark.png) | PASS | The full-screen image exposes zoom, Reset and Close. |
| [A26b](A26b-dark.png) | FIX | Offer Preview diagram only on the .mmd file, not also on the .png. Keep the sending worker identifiable. |
| [A26c](A26c-dark.png) | PASS | The diagram preview exposes PNG, SVG and source export. |
| [A27a](A27a-dark.png) | PASS | All Settings destinations can be reached from the page list. |
| [A27b](A27b-dark.png) | FIX | Restore the Local target and Add WSL target controls alongside the improved SSH and distro rows. |
| [A27c](A27c-dark.png) | FIX | The fixture model list is corrected. Make Best used as editable and present the real another-store confirmation instead of a switch. |
| [A27d](A27d-dark.png) | PASS | The appearance page distinguishes theme selection, interface scale and font samples. |
| [A27e](A27e-dark.png) | PASS | The keyboard conflict names both commands and offers Replace or Cancel. |
| [A27f](A27f-dark.png) | PASS | The catalogue distinguishes user removal from restoring an overridden bundled skill. |
| [A27g](A27g-dark.png) | FIX | Keep the update action neutral, and make the editable report and the selected target unambiguous. |
| [A28a](A28a-dark.png) | PASS | SSH setup accepts host, port and user, and offers SSH-config import. |
| [A28b](A28b-dark.png) | PASS | The remote identity and fingerprint appear before the trust decision. |
| [A28c](A28c-dark.png) | PASS | The step log reaches verified access and exposes Revoke. |
| [A28d](A28d-dark.png) | FIX | Show a copyable complete public-key line and avoid presenting the acknowledgment twice. |
| [A28e](A28e-dark.png) | PASS | The setup offers password or key file with a masked passphrase. |
| [A29a](A29a-dark.png) | FIX | The docked panel now names the correct workers. Restore the project sections in the sidebar and the running controls below the chat. |
| [A29b](A29b-dark.png) | FIX | The four panes now coexist, but the agents pane breaks names into very short fragments and its visible cells are too narrow to scan or tap comfortably. |
