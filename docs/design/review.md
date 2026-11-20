# Design review

The mockups are reviewed by two reviewers independently — Claude (Fable 5.2) and Codex
(`gpt-6.2-astra`) — against three criteria. A round ends when both have scored; the brief in
[screens.md](screens.md) is revised and the mockups regenerated until **both reviewers score
all three criteria at 9 or above**.

**Result: reached in round 4.** The reviewed set is [`mockups/final/`](mockups/final/) — 24
screens, each in dark and light — against revision 4 of the brief.

| Criterion | What it means |
| --- | --- |
| **Usability** | A developer can tell at a glance what each agent is doing and what needs them; controls are where expected; states are unambiguous; nothing is shown that should not be there. |
| **User journeys** | Each real task can be walked end to end with no missing screen or dead end. |
| **Dark and light** | The two modes are equals in hierarchy, contrast, and state legibility; nothing is conveyed by colour alone. |

Scores judge the design the mockups represent, not image-generation artefacts such as a
misspelt word.

## Scores

| Round | Mockups | Reviewer | Usability | Journeys | Dark / light | Lowest |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | [v1](mockups/v1/) | Claude | 7 | 6.5 | 8.5 | **6.5** |
| 1 | [v1](mockups/v1/) | Codex | 6 | 4 | 7 | **4** |
| 2 | [v2](mockups/v2/) | Claude | 9 | 9 | 9 | **9** |
| 2 | [v2](mockups/v2/) | Codex | 8 | 8 | 8.5 | **8** |
| 3 | v2 + [v3](mockups/v3/) | Claude | 9 | 9.5 | 9 | **9** |
| 3 | v2 + [v3](mockups/v3/) | Codex | 8.5 | 8.5 | 8.5 | **8.5** |
| 4 | [final](mockups/final/) | Claude | 9 | 9.5 | 9.5 | **9** |
| 4 | [final](mockups/final/) | Codex | 9 | 9 | 9 | **9** |

## Round 1 — Claude

**What works.** One coherent visual language: dense, flat, legible, with agent prose as the
most readable thing on screen. The orchestrator screen (02) does what it must — the spawn card,
the *From* and *To* message blocks, and the agents panel make it clear who is talking to whom.
Where a screen exists in both modes with the same layout, the two are genuinely equal.

**What does not.**

| # | Problem | Screens | Change to the brief |
| --- | --- | --- | --- |
| 1 | **Three screens are a different app.** Settings and first run were drawn with a title bar, a "Pinned / Active / Recent" sidebar, an "Agents (4) · Add agent" panel, and "+ Agent / + Tool / Auto" composer controls. None of that exists. The brief never said what the window looks like when settings are open. | 07, 08, 10 | Define the settings shell explicitly: a full-window page with its own left navigation and a "Back to threads" control; no thread sidebar, no agents panel, no composer. Define first run as the main shell with an empty sidebar and no composer. |
| 2 | **Nothing answers "what happened while I was away?"** Agents run unattended, across machines. Returning after hours, the user has to hunt the tree for amber dots. | all | Add a **Needs you** section at the top of the sidebar — every thread waiting on the user, across all origins, regardless of grouping — and a "while you were away" summary when the window re-attaches. |
| 3 | **The agents panel shows invented content** where the thread has no relationships ("Test Runner", "Linter", "Resources"). | 04, 09 | State the rule: hidden when the thread has no parent, workers, or contacts; a rail when collapsed; never anything but those three sections. |
| 4 | **A draft shows controls that cannot apply**: Fork and a context meter on a thread that does not exist yet; the picker's ticked model disagrees with the control row. | 03 | In a draft: no Fork, no context meter; the picker's selection and the control row agree; the picker is anchored to its trigger. |
| 5 | **Read-only still looks actionable**: a disabled Stop button, live-looking model and effort selectors, and two Fork buttons. | 05 | In use elsewhere: no send or stop control at all; the control row is replaced by one line saying who holds the thread; one Fork, in the banner. |
| 6 | **The sidebar does not reflect the open thread**, and gives no sign of which threads have workers or were spawned. | 02, 03 | The open thread is the selected row. Rows show a worker count and a "spawned by" mark. A parent shows when something beneath it needs attention. |
| 7 | **State by colour alone**: "needs attention" in the sidebar is an amber dot. | 01, 02, 05 | Every state mark is a distinct shape as well as a colour. |
| 8 | **The orchestrator repeats itself**: "Started 3 workers" and "Waiting for 3 workers" list the same three rows. | 02 | One live workers card that is created by the spawn and updates in place; a wait is one line beneath it. |
| 9 | **No sign of the orchestration budget**, or of a worker that is blocked. | 02 | The agents panel header shows budget used; one worker is shown needing attention, bubbling up. Add a screen for "budget reached · Continue". |
| 10 | **Native fork looks selectable** though it is not available. | 06 | Disabled, with the reason beside it. |
| 11 | **The hub is invisible.** Nothing says agents keep running when the window closes, or whether the hub is up. | all | A quiet hub status line at the foot of the sidebar. |

## Round 1 — Codex

Scores: usability 6, user journeys 4, dark and light 7. Its ranked problems, condensed:

| # | Problem | Screens |
| --- | --- | --- |
| 1 | **A blocked worker, and returning after an absence.** A pending approval coexists with "Running" in the header and tree; a question is obvious only once the right thread is open. Needing the user must take precedence everywhere, climb through every ancestor, and show on the collapsed agents rail. There is no "since you left" view. Requests need their other states — submitted, resolved elsewhere, expired, outcome unknown — and an uncertain one must never invite a blind retry. | 01, 02, 04, 09 |
| 2 | **Following an orchestration.** The header names one thread while the tree selects another; the wait card contradicts the outgoing message. One coherent timeline; "2 of 3 finished"; worker rows and *To/From* names visibly links; the same exchange shown from the recipient's side; a queued and a failed delivery; a receipt that says what "delivered" means. | 02 |
| 3 | **One thread tree, one relationship panel.** Some screens substitute a roster of tools or roles for the panel, and settings invents its own navigation and an "Add agent" button. | 01–05, 07–10 |
| 4 | **First run to first message.** Onboarding offers two ways to start; the draft highlights an old thread, offers Fork, and claims 38 % context. Missing: a not-signed-in state, and the moment the draft becomes a thread. | 10 → 03 |
| 5 | **SSH setup and revoke.** Only one step is drawn. Missing: fingerprint confirmation, the fallback to a password, key install and verification, a recoverable failure, the result of a revoke, and the difference between Disable, Remove, and Revoke. | 07 |
| 6 | **In use elsewhere, and handoff.** Model, effort, and mode still look editable. Native versus handoff should be a derived result, not a choice. The destination instance and account must be named before confirming, and the resulting forked thread shown. | 05 → 06 |
| 7 | **Theme parity.** A state shown by an amber dot alone; a barely dimmed modal in light; an action present in only one theme. | 01, 05, 06, 09 |

Its bar for 9: every journey prototyped **through its outcome**, in both themes; a developer
returning to a collapsed fleet can find and answer a blocked grandchild without searching
transcripts, trace a delayed message from either end, and see the destination account before a
handoff; all of it legible in greyscale and usable at the minimum window width.

## What changed for round 2

Both reviewers agreed on the causes, so [the brief](screens.md) was rewritten rather than
patched:

- **Rules that hold on every screen**: state is a shape and a word; needing the user outranks
  every other state and climbs to every ancestor; one shell; the selected row is the open
  thread; the agents panel has three fixed sections or is hidden; the control row shows only
  what can be changed.
- **A "Needs you" section** at the top of the sidebar, across all origins.
- **One story** — a single fleet used on every screen, so screens agree with each other.
- **Journeys shown through to their outcome**, which took the set from 10 screens to 21:
  the worker's side of a conversation; a blocked grandchild; every request state; "since you
  left"; the first message after a draft; the forked thread; the SSH flow in four steps
  including failure and revoke; a budget pause; a narrow window.
- **Settings** defined as its own full-window shell, with a separate **Agents** tab.
- The orchestrator's two redundant cards became one live workers card; receipts say what
  "delivered" means; a queued message to an offline target is shown.

## Round 2 — Claude

Scores: 9 / 9 / 9. The rewrite fixed what round 1 found: one shell throughout; the blocked
grandchild is one click from *Needs you*, with its ancestry in the header; the same
conversation reads from both ends; a request's states are distinct and none offers a retry;
"since you left" puts what needs the user first; the SSH flow shows each step, a failure that
cleans up after itself, and a revoke.

Left as polish: a way to reopen *Since you left* after dismissing it; the narrow rail's origin
marks need labels; "spawned by" and "root" are worded confusingly in the agents panel of a
worker two levels down.

## Round 2 — Codex

Scores: usability 8, user journeys 8, dark and light 8.5. Journeys (a), (e), (f), and (g) were
judged complete. Six changes were asked for to reach 9:

| # | Change | Screens |
| --- | --- | --- |
| 1 | A **Refresh** beside the sign-in hint. The first-ever draft must inherit the empty tree from first run; the populated fleet is the returning-user case. | 07–09 |
| 2 | The narrow Needs-you badge opens a keyboard-accessible list; choosing an entry scrolls to and focuses the request. An arriving request announces itself without taking focus. Enter and Esc act on an approval only while its card is focused. A request held by another program is labelled as such wherever it is listed. | 01, 04–06, 10, 21 |
| 3 | After the agent acknowledges an answer, show it as **confirmed, answered by you**, restore the composer, recompute attention up the ancestry, and mask secret answers. An unknown outcome offers **Check latest status**, never a resend. | 05 |
| 4 | Between confirming the server and "Ready", show the password-less probe and the masked password or key-file step with **Connect**. **Try again** returns to the failed step. Claim cleanup only once confirmed. If a revoke cannot reach the host, keep the local key and show the exact line to remove by hand. | 13–16 |
| 5 | *To* and *From* names are visible links to each other. Show the queued message's outcome: the receipt updated to delivered on reconnect, the matching *From* at the destination, and a failed delivery that keeps the sent item with its reason. | 02–03, 06 |
| 6 | Reasons the user must read use secondary text, not disabled text: the disabled token measured about 2.5 : 1 in dark and 2.3 : 1 in light. Require 4.5 : 1 for action and state labels in both themes. | tokens, 08, 11 |

## What changed for round 3

All six were applied to [the brief](screens.md) (revision 3):

- Colour tokens were **recomputed** so every text colour measures at least 4.5 : 1 on all
  three backgrounds of its theme; light-mode *needs you*, *done*, and *running* were darkened;
  the label on an accent-filled button is dark in dark mode (white on that accent measured
  3.1 : 1). A rule now says reasons are never set in faint text.
- New rules: links between agents; keys act only on a focused card; arriving requests do not
  take focus; **Ctrl+J** opens the Needs-you list; requests held by another program are
  labelled.
- Screen 05 gained the **confirmed** state and "Check latest status".
- Screens 07–09 now tell one first-run story with an empty tree.
- Screen 16 gained "could not revoke", with the exact line to remove by hand.
- Screen 21 shows the Needs-you list open, and the rail's origins are labelled.
- Three new screens: **22** signing in during SSH setup; **23** the queued message arriving,
  and one that failed; **24** the same message at its destination.

## Round 3 — Claude

Scores: 9 / 9.5 / 9. Every journey now reaches its outcome. One polish note: the generator
underlined every row of the thread tree; underlines belong on cross-thread links only.

## Round 3 — Codex

Scores: 8.5 on all three. Journeys (a), (c), (e), (f), and (g) complete; (b) and (d) each
missing one thing. It judged the two modes equivalent across all 24 pairs, and said the
remaining issues were "instructions an implementer could follow literally". Four edits:

| # | Edit | Screens |
| --- | --- | --- |
| 1 | Mask a secret answer while it is typed and in every later summary, including *submitted*. On acknowledgment, recompute the thread, ancestor, and rail marks from the requests still open. | 05 |
| 2 | Keep **Revoke** available for an offline server that YAAC set up. If removal cannot be verified, keep the local key and show — and copy — the complete installed line, not an abbreviated one. "I removed it myself" records that and returns the row to *Not set up*. | 13, 16 |
| 3 | Jump to the matching item only once delivery is recorded; while queued, keep the receipt; if the recipient was deleted, show its name as plain text beside the permanent reason. | 23 |
| 4 | Use secondary text for the native-fork explanation. Specify the selected row's fill and require 4.5 : 1 against it — link colour on a solid accent row measured about 1.3 : 1. | 11, all |

## What changed for round 4

All four were applied (revision 4 of the brief). The selected row became a **tint** with its
own token (`#263052` dark, `#E4EAFD` light) and every row colour was measured against it;
dark-mode *failed* was lightened to pass on that tint. The link rule now distinguishes
delivered, queued, and failed, and says tree rows are rows, not underlined links.

## Round 4 — the bar is met

**Codex: 9 / 9 / 9.** "I would build revision 4 as specified." All four round-3 edits resolved;
all seven journeys judged complete. It measured the contrast itself: the lowest text contrast
on the three base surfaces is 4.77 : 1 in dark and 4.78 : 1 in light; on the selected-row tint
the minima are 4.53 : 1 (running, dark) and 4.51 : 1 (failed, light); links on the tint are
5.66 and 6.11.

**Claude: 9 / 9.5 / 9.5.** The regenerated screens match the brief: the selected row is a tint
with readable text; tree rows are plain while cross-thread names are links; "could not revoke"
shows the complete key line with Copy and "I removed it myself".

Polish from this round, applied to the brief with no new mockups needed:

- The sidebar's worker count is the thread's **direct** workers, so it agrees with the agents
  panel (the mockups show "4 workers" beside a panel reading "Workers (3)").
- **Since you left** can be reopened from the hub status line.
- The contrast note names the true light-theme minimum.

### What the review changed, overall

Ten screens became twenty-four, and the design gained things neither the original brief nor
the behaviour spec had:

- A **Needs you** list above the thread tree, and **Since you left** on return — because the
  agents run for hours unattended, and "what needs me?" has to have one answer.
- **Needing the user outranks every other state**, and climbs to every ancestor.
- Every journey drawn **to its outcome**: the answer confirmed, the queued message delivered
  (and one that failed), the fork landed in, the server revoked (and one that could not be).
- **Measured contrast** for every text colour, on every surface including the selected row.
- Rules for focus: keys act only on a focused card; an arriving request never steals the caret.

These are reflected in [07 UI](../07-ui.md). The mockups are generated images — a reference for
layout, density, and hierarchy, not a pixel specification. Where a mockup and the brief
disagree, the brief wins.

## After the review — the owner's revision

The owner then looked at the reviewed mockups and changed the design. **Revision 5 of the
brief has not been through the two-reviewer scoring**; the scores above are for revision 4.

What the owner said, and what changed:

| The owner | Change |
| --- | --- |
| "It feels very polluted. Probably it's the tree style… a list with sticky position headers would probably look better." | The sidebar is a flat list of one-line rows under quiet sticky headers. No indentation, chevrons, folder icons, toggle chips, state words, or "spawned" arrows. |
| "Every control has a border; it increases the density of information without any reason." | No outlines anywhere. Space first, then a faint fill; one hairline between regions; one filled button per screen. |
| "On chat, the interaction of the user and the agent is what should stand out most. Tool calling, conversation between workers… should be more muted, by default collapsed into a small description." | Only user messages and agent prose are full size. All activity is one muted line, collapsed by default, expandable. Only things that need the user are cards. |
| Kept only the *Needs you* list of the review's additions. | Removed: the "Since you left" page, the warning marks on parent threads, the Agents settings tab. |
| Asked for a way to focus on a few agents among many. | Added **Mute** and **Archive**. |
| Wanted agent permissions in the chat. | Added a per-chat **agents: ask / allow** control and a permission prompt that sets the budget. |

The lesson for the reviewers: both scored *usability* on whether the information was present
and unambiguous, and neither weighed how much of it there was. A design can be complete and
still too loud. Revision 5's mockups are in [`mockups/v5/`](mockups/v5/).
