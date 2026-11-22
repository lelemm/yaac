//! The thread header, its banners, and the composer's state.

use yaac_protocol::attach::{ConnectionState, ConnectionStatus, DaemonUpdatePhase};
use yaac_protocol::{Capability, Control, ItemBody, ReceiptReason, Support, TargetId, has_capability};

use crate::attention::{Attention, AttentionMap, Mark, ThreadDisplay, display};
use crate::hints::RequestKindWord;
use crate::key::ThreadKey;
use crate::state::{AppState, Draft};

/// The tone of the header tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Needs the user.
    NeedsYou,
    /// Running.
    Running,
    /// Failed.
    Failed,
    /// Anything else.
    Neutral,
}

/// One link in a link line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPart {
    /// Shown text.
    pub title: String,
    /// The thread, when it can be opened.
    pub key: Option<ThreadKey>,
}

/// The line of links under the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkLine {
    /// No relatives.
    None,
    /// "Spawned by X ↗ on local".
    SpawnedBy {
        /// The parent.
        parent: LinkPart,
        /// Its target.
        target: String,
    },
    /// "A › B › **C**" for a worker several levels down (the last part is this thread).
    Ancestry(Vec<LinkPart>),
    /// "Forked from X ↗ at turn 12 · handoff".
    ForkedFrom {
        /// The source.
        source: LinkPart,
        /// "at turn 12 · handoff".
        detail: String,
    },
}

/// A banner under the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Banner {
    /// The thread is held elsewhere: fork to continue here, and — for a program on the target
    /// whose harness allows it — **Take over** (D140).
    InUseElsewhere {
        /// The sentence.
        text: String,
        /// **Take over**, when offered.
        take_over: Option<crate::thread_menu::TakeOver>,
    },
    /// The target's daemon is older, or being updated; one action, when the hub can update
    /// it: **Update now**.
    OlderDaemon {
        /// The sentence.
        text: String,
        /// The target **Update now** acts on; `None`: no button (nothing newer to install,
        /// or the update is under way).
        update_now: Option<TargetId>,
    },
}

/// The header.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderView {
    /// Mark and attention.
    pub display: ThreadDisplay,
    /// Title.
    pub title: String,
    /// Tag text and tone.
    pub tag: Option<(String, Tone)>,
    /// target · project · worktree.
    pub breadcrumb: Vec<String>,
    /// Relatives.
    pub links: LinkLine,
    /// Banners.
    pub banners: Vec<Banner>,
}

/// The header of a thread.
pub fn header(state: &AppState, map: &AttentionMap, key: &ThreadKey) -> Option<HeaderView> {
    let s = state.summary(key)?;
    let d = display(state, map, key);
    let tag = if d.read_only {
        Some(("Read-only".to_owned(), Tone::Neutral))
    } else {
        match (d.attention, d.mark) {
            (Some(Attention::BudgetReached), _) => Some(("Paused · budget reached".to_owned(), Tone::NeedsYou)),
            (Some(a), _) => Some((format!("Needs you · {}", a.word()), Tone::NeedsYou)),
            (None, Mark::Running) => Some(("Running".to_owned(), Tone::Running)),
            (None, Mark::Waiting) => Some(("Waiting".to_owned(), Tone::Running)),
            (None, Mark::Failed) => Some(("Failed".to_owned(), Tone::Failed)),
            _ => None,
        }
    };
    let mut breadcrumb = vec![state.target_name(&key.target)];
    if let Some(p) = &s.project {
        breadcrumb.push(p.display_name.clone());
    }
    if let Some(b) = s.worktree.as_ref().and_then(|w| w.branch.clone()).or_else(|| s.branch.clone()) {
        breadcrumb.push(b);
    }

    // Ancestry.
    let mut ancestors: Vec<LinkPart> = Vec::new();
    let mut parent = s.spawned_by.clone();
    let mut guard = 0;
    while let Some(addr) = parent {
        guard += 1;
        if guard > 16 {
            break;
        }
        let k = state.key_of(&addr);
        let title = k.as_ref().and_then(|k| state.summary(k)).map_or_else(|| addr.to_string(), |s| s.title.clone());
        parent = k.as_ref().and_then(|k| state.summary(k)).and_then(|s| s.spawned_by.clone());
        ancestors.push(LinkPart { title, key: k });
    }
    ancestors.reverse();
    let links = if ancestors.len() >= 2 {
        let mut parts = ancestors;
        parts.push(LinkPart { title: s.title.clone(), key: None });
        LinkLine::Ancestry(parts)
    } else if let Some(p) = ancestors.pop() {
        let target = s.spawned_by.as_ref().and_then(|a| a.target()).map(|t| state.target_name(&t)).unwrap_or_default();
        LinkLine::SpawnedBy { parent: p, target }
    } else if let Some(src) = &s.forked_from {
        let k = ThreadKey::new(key.target.clone(), src.clone());
        let title = state.summary(&k).map_or_else(|| src.to_string(), |s| s.title.clone());
        let mut detail = String::new();
        if let Some(t) = state.transcripts.get(key) {
            for item in t.items() {
                if let ItemBody::Handoff(h) = &item.body {
                    // Turn ids are opaque: name the turn by its position in the source, when
                    // the source's transcript is at hand.
                    let position = h.cutoff_turn.as_ref().and_then(|turn| {
                        let mut seen: Vec<&str> = Vec::new();
                        for it in state.transcripts.get(&k)?.items() {
                            if !it.turn_id.as_str().is_empty() && !seen.contains(&it.turn_id.as_str()) {
                                seen.push(it.turn_id.as_str());
                            }
                        }
                        seen.iter().position(|t| *t == turn.as_str()).map(|i| i + 1)
                    });
                    if let Some(n) = position {
                        detail = format!("at turn {n} · ");
                    }
                    detail.push_str(h.fork_kind.as_str());
                    break;
                }
            }
        }
        LinkLine::ForkedFrom { source: LinkPart { title, key: state.threads.contains_key(&k).then_some(k) }, detail }
    } else {
        LinkLine::None
    };

    let mut banners = Vec::new();
    let target_name = state.target_name(&key.target);
    if d.read_only {
        let text = match state.control(key) {
            Control::OtherClient => format!(
                "This thread is in use in YAAC on {}. You can watch, but not send.",
                state.control_holder(key).unwrap_or_else(|| "another computer".to_owned())
            ),
            _ => format!("This thread is open in another program on {target_name}. You can watch, but not send."),
        };
        banners.push(Banner::InUseElsewhere { text, take_over: crate::thread_menu::take_over(state, key) });
    }
    if let Some(t) = state.target(&key.target)
        && let Some(b) = daemon_banner(&key.target, &t.entry.state, &target_name)
    {
        banners.push(b);
    }
    Some(HeaderView { display: d, title: s.title.clone(), tag, breadcrumb, links, banners })
}

/// The banner about a target's `yaacd`: older (updated when idle, or **Update now**), being
/// updated, or not updated after a hand-over (docs/02-architecture.md#one-serving-daemon-per-user).
pub fn daemon_banner(target: &TargetId, state: &ConnectionState, name: &str) -> Option<Banner> {
    let daemon = state.daemon.as_ref().filter(|_| state.status == ConnectionStatus::Connected);
    let button = daemon.is_some_and(|d| d.can_update).then(|| target.clone());
    match state.update.as_ref().map(|u| (u, &u.phase)) {
        Some((_, DaemonUpdatePhase::Updating)) => Some(Banner::OlderDaemon { text: format!("Updating yaacd on {name}…"), update_now: None }),
        Some((u, DaemonUpdatePhase::Stuck)) => Some(Banner::OlderDaemon {
            text: u.detail.clone().unwrap_or_else(|| format!("yaacd on {name} was not updated: {} still serves.", u.from)),
            update_now: button,
        }),
        _ => {
            let d = daemon.filter(|d| d.older)?;
            let text = if d.can_update {
                format!("{name} is running an older YAAC daemon ({}). It will be updated when idle.", d.build)
            } else {
                format!("{name} is running an older YAAC daemon ({}).", d.build)
            };
            Some(Banner::OlderDaemon { text, update_now: button })
        }
    }
}

pub use crate::attachments::{ChipStatus, ChipView};

/// What the composer shows.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposerView {
    /// Read-only: no input, no Send, no Stop — one quiet line instead.
    Absent {
        /// "Held by a terminal session on devbox."
        holder_line: String,
    },
    /// The composer.
    Shown {
        /// "Message Codex…".
        placeholder: String,
        /// Dimmed with this hint while a request on this thread waits for the user.
        dimmed_hint: Option<String>,
        /// A turn is running and can be stopped: Send becomes Stop.
        stop: bool,
        /// Input can be added to the running turn.
        steer: bool,
        /// Attachment chips.
        chips: Vec<ChipView>,
        /// Every chip is ready to send.
        ready: bool,
        /// Why the message cannot be sent yet (the send button says it).
        blocked: Option<String>,
    },
}

/// The provider's name without the instance suffix, for "Message Codex…".
pub fn short_provider(display_name: &str, instance_name: Option<&str>) -> String {
    match instance_name {
        Some(n) => display_name.strip_suffix(&format!(" · {n}")).unwrap_or(display_name).to_owned(),
        None => display_name.to_owned(),
    }
}

/// Chips routed by the image support of the model selected now, for a draft on `target`.
pub fn chips(draft: Option<&Draft>, support: &Support, target: Option<&yaac_protocol::TargetId>) -> Vec<ChipView> {
    draft.map(|d| crate::attachments::chips(&d.attachments, support, target)).unwrap_or_default()
}

/// The composer of a started thread.
pub fn composer(state: &AppState, map: &AttentionMap, key: &ThreadKey) -> ComposerView {
    if !state.may_act(key) {
        let holder = state.control_holder(key).unwrap_or_else(|| "another program".to_owned());
        return ComposerView::Absent { holder_line: format!("Held by {holder} on {}.", state.target_name(&key.target)) };
    }
    let provider = state.provider_of(key);
    let name = provider.map_or_else(|| "the agent".to_owned(), |p| short_provider(&p.display_name, p.name.as_deref()));
    let transcript = state.transcripts.get(key);
    let effective = transcript.and_then(|t| t.effective.as_ref());
    let caps = effective.map_or(&[][..], |e| e.capabilities.as_slice());
    let running = transcript.is_some_and(|t| t.active_turn.is_some());
    let dimmed_hint = match map.of(key) {
        Some(Attention::Request(RequestKindWord::Permission)) => Some("Answer above to continue".to_owned()),
        Some(Attention::Request(kind)) => Some(format!("Answer the {} above to continue", kind.word())),
        Some(Attention::Unspecified) => Some("Answer the request above to continue".to_owned()),
        _ => None,
    };
    let support = crate::attachments::thread_image_support(state, key);
    let chips = chips(state.drafts.get(key), &support, Some(&key.target));
    let blocked = crate::attachments::blocked(&chips);
    let ready = blocked.is_none();
    ComposerView::Shown {
        placeholder: format!("Message {name}…"),
        dimmed_hint,
        stop: running && has_capability(caps, &Capability::Interrupt),
        steer: running && has_capability(caps, &Capability::Steer),
        chips,
        ready,
        blocked,
    }
}

/// A receipt in words (docs/07-ui.md: "A receipt says what it means").
pub fn receipt_text(
    status: &yaac_protocol::ReceiptStatus,
    reason: Option<&ReceiptReason>,
    detail: Option<&str>,
    target_name: &str,
) -> String {
    use yaac_protocol::ReceiptStatus as S;
    let why = |r: Option<&ReceiptReason>| -> String {
        match r {
            Some(ReceiptReason::TargetOffline) => {
                format!("{target_name} is offline. It will be delivered when {target_name} reconnects.")
            }
            Some(ReceiptReason::HubUnavailable) => "no hub is running. It will be sent when the hub returns.".to_owned(),
            Some(ReceiptReason::RecipientBusy) => "the recipient is running a turn.".to_owned(),
            Some(ReceiptReason::AwaitingUser) => "the recipient is waiting for you.".to_owned(),
            Some(ReceiptReason::InUseElsewhere) => "another program holds the recipient.".to_owned(),
            Some(ReceiptReason::BudgetExhausted) => "the orchestration's budget ran out.".to_owned(),
            Some(ReceiptReason::UnknownAddress) => "there is no such thread.".to_owned(),
            Some(ReceiptReason::ThreadDeleted) => format!("that thread was deleted on {target_name}."),
            Some(ReceiptReason::LimitExceeded) => "a mesh limit was exceeded.".to_owned(),
            Some(ReceiptReason::PolicyDenied) => "the mesh policy refused it.".to_owned(),
            Some(ReceiptReason::RemoteDisallowed) => format!("{target_name} does not accept messages from other machines."),
            Some(ReceiptReason::Stopped) => "The thread was stopped; it waits until its stopper or the user messages it.".to_owned(),
            Some(ReceiptReason::NotBeneathSender) => "Sent as queued: `now` is only for threads beneath the sender.".to_owned(),
            Some(ReceiptReason::Unknown(s)) => s.clone(),
            None => String::new(),
        }
    };
    match status {
        S::Delivered => match detail {
            Some(d) => format!("Delivered · stored in its mailbox · {d}"),
            None => "Delivered · stored in its mailbox".to_owned(),
        },
        S::Queued => format!("Queued · {}", detail.map_or_else(|| why(reason), str::to_owned)),
        S::Failed => {
            let w = detail.map_or_else(|| why(reason), str::to_owned);
            format!("Failed · {w} The message was not delivered.")
        }
        S::Unknown(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use yaac_protocol::TargetId;
    use yaac_protocol::attach::ConnectionState;

    use super::{Banner, daemon_banner};

    fn banner(state: serde_json::Value) -> Option<Banner> {
        let state: ConnectionState = serde_json::from_value(state).unwrap();
        daemon_banner(&TargetId::new("t1"), &state, "devbox")
    }

    #[test]
    fn the_older_daemon_banner_offers_update_now_only_when_the_hub_can_update() {
        let older = |can: bool| json!({ "status": "connected", "daemon": { "build": "0.1.0+40", "older": true, "schemaHashMatches": true, "canUpdate": can } });
        assert_eq!(
            banner(older(true)),
            Some(Banner::OlderDaemon {
                text: "devbox is running an older YAAC daemon (0.1.0+40). It will be updated when idle.".into(),
                update_now: Some(TargetId::new("t1")),
            })
        );
        assert_eq!(
            banner(older(false)),
            Some(Banner::OlderDaemon { text: "devbox is running an older YAAC daemon (0.1.0+40).".into(), update_now: None })
        );
        assert_eq!(banner(json!({ "status": "connected", "daemon": { "build": "0.1.0+42", "older": false, "schemaHashMatches": true } })), None);
    }

    #[test]
    fn an_update_under_way_shows_while_offline_and_a_stuck_one_keeps_the_button() {
        let updating = json!({ "status": "offline", "update": { "from": "0.1.0+40", "to": "0.1.0+42-ab", "phase": "updating" } });
        assert_eq!(banner(updating), Some(Banner::OlderDaemon { text: "Updating yaacd on devbox…".into(), update_now: None }));
        let stuck = json!({
            "status": "connected",
            "daemon": { "build": "0.1.0+40", "older": true, "schemaHashMatches": true, "canUpdate": true },
            "update": { "from": "0.1.0+40", "to": "0.1.0+42-ab", "phase": "stuck", "detail": "yaacd 0.1.0+40 started again." }
        });
        assert_eq!(banner(stuck), Some(Banner::OlderDaemon { text: "yaacd 0.1.0+40 started again.".into(), update_now: Some(TargetId::new("t1")) }));
    }
}
