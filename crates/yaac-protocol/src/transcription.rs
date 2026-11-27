//! Which instance transcribes a dictation (docs/plan/voice.md, P392).
//!
//! Dictation only fills a composer; it never touches a thread. So it is not bound to the
//! thread's harness: any instance that can transcribe now, on any connected target, may serve
//! any composer (a thread's or a new-thread draft's). The hub makes the choice when a window
//! starts a dictation, and the window makes the same choice beforehand to show the microphone
//! and say who will transcribe. Both call [`choose`], so they agree; it is pure and depends
//! only on what the targets reported.
//!
//! The order, first match wins:
//! 1. the composer's own instance, on its own target, when it can transcribe;
//! 2. another ready instance on the composer's target;
//! 3. a ready instance on another connected target (its audio then travels over that
//!    target's one connection; nothing new is opened).
//!
//! Ties are broken by target id, then instance id, so the choice never depends on the order
//! the descriptors arrived in. Nothing here looks at a provider's kind (invariant 4).

use crate::{Capability, InstanceId, ProviderDescriptor, ProviderStatus, TargetId, VoiceFormat};

/// One target as the chooser sees it.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    /// The target.
    pub target: &'a TargetId,
    /// Its display name ("wsl: Ubuntu").
    pub name: &'a str,
    /// Its connection is up now (offline descriptors from a cache never transcribe).
    pub connected: bool,
    /// Its instances, capabilities already folded by the connection's features.
    pub providers: &'a [ProviderDescriptor],
}

/// The composer a dictation is for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context<'a> {
    /// The thread's or draft's target, when it has one.
    pub target: Option<&'a TargetId>,
    /// The thread's or draft's instance, when it has one.
    pub instance: Option<&'a InstanceId>,
}

/// The chosen transcriber.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcriber {
    pub target: TargetId,
    pub instance: InstanceId,
    /// What to record.
    pub format: VoiceFormat,
    /// "Claude on wsl: Ubuntu": the instance's display name and the target's.
    pub label: String,
}

/// What [`choose`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Ready(Transcriber),
    /// No instance can transcribe now; why, in words for the user.
    Unavailable(String),
}

/// The format an instance transcribes in, when it can now.
pub fn ready_format(p: &ProviderDescriptor) -> Option<&VoiceFormat> {
    if p.status != ProviderStatus::Ready || !p.capabilities.contains(&Capability::Transcription) {
        return None;
    }
    p.transcription.as_ref()?.format.as_ref()
}

/// The transcriber for `context` among `candidates` (see the module docs for the order).
pub fn choose(context: Context<'_>, candidates: &[Candidate<'_>]) -> Choice {
    let rank = |c: &Candidate<'_>, p: &ProviderDescriptor| {
        let own_target = context.target == Some(c.target);
        let own_instance = own_target && context.instance == Some(&p.instance);
        (if own_instance { 0 } else if own_target { 1 } else { 2 }, c.target.clone(), p.instance.clone())
    };
    let connected = candidates.iter().filter(|c| c.connected);
    let ready = connected.clone().flat_map(|c| c.providers.iter().filter_map(move |p| Some((c, p, ready_format(p)?))));
    if let Some((c, p, format)) = ready.min_by_key(|(c, p, _)| rank(c, p)) {
        return Choice::Ready(Transcriber {
            target: c.target.clone(),
            instance: p.instance.clone(),
            format: format.clone(),
            label: label(c, p),
        });
    }
    // Why not: the nearest instance that has a voice surface and said why it is not ready.
    let why = connected
        .flat_map(|c| c.providers.iter().map(move |p| (c, p)))
        .filter_map(|(c, p)| {
            let t = p.transcription.as_ref()?;
            let reason = t.unavailable.clone().or_else(|| match p.status {
                ProviderStatus::Ready => None,
                _ => Some(p.status_message.clone().unwrap_or_else(|| "it is not ready".into())),
            })?;
            Some((rank(c, p), format!("{}: {reason}", label(c, p))))
        })
        .min_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, why)| why);
    Choice::Unavailable(match why {
        Some(why) => format!("No transcriber available. {why}"),
        None if candidates.iter().any(|c| c.connected) => "No transcriber available on the connected machines.".into(),
        None => "No transcriber available: no machine is connected.".into(),
    })
}

fn label(c: &Candidate<'_>, p: &ProviderDescriptor) -> String {
    format!("{} on {}", p.display_name, c.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioEncoding, BackendKind, ProviderKind, StoreIdentity, TranscriptionOffer};

    fn format() -> VoiceFormat {
        VoiceFormat { format: AudioEncoding::Pcm16le, sample_rate: 16_000, channels: 1, max_seconds: Some(120) }
    }

    fn provider(instance: &str, name: &str, offer: Option<TranscriptionOffer>) -> ProviderDescriptor {
        let ready = offer.as_ref().is_some_and(|o| o.format.is_some());
        ProviderDescriptor {
            instance: InstanceId::from(instance),
            kind: ProviderKind::from("fake"),
            name: None,
            display_name: name.into(),
            backend: BackendKind::from_wire("fake"),
            harness_version: None,
            store: StoreIdentity::default(),
            status: ProviderStatus::Ready,
            status_message: None,
            capabilities: if ready { vec![Capability::Transcription] } else { vec![] },
            models: vec![],
            disabled_models: vec![],
            approval_explanation_model: None,
            models_complete: true,
            modes: vec![],
            revision: 1,
            backend_origin: None,
            models_read_at: None,
            transcription: offer,
        }
    }

    fn ready() -> Option<TranscriptionOffer> {
        Some(TranscriptionOffer { format: Some(format()), unavailable: None })
    }

    fn refused(why: &str) -> Option<TranscriptionOffer> {
        Some(TranscriptionOffer { format: None, unavailable: Some(why.into()) })
    }

    fn pick(choice: Choice) -> (String, String, String) {
        match choice {
            Choice::Ready(t) => (t.target.to_string(), t.instance.to_string(), t.label),
            Choice::Unavailable(why) => panic!("unavailable: {why}"),
        }
    }

    #[test]
    fn the_composers_own_instance_comes_first() {
        let (here, there) = (TargetId::from("here"), TargetId::from("there"));
        let local = [provider("a", "Alpha", ready()), provider("b", "Beta", ready())];
        let remote = [provider("c", "Gamma", ready())];
        let candidates = [
            Candidate { target: &there, name: "There", connected: true, providers: &remote },
            Candidate { target: &here, name: "Here", connected: true, providers: &local },
        ];
        let own = InstanceId::from("b");
        let got = pick(choose(Context { target: Some(&here), instance: Some(&own) }, &candidates));
        assert_eq!(got, ("here".into(), "b".into(), "Beta on Here".into()));
    }

    #[test]
    fn another_instance_on_the_same_target_comes_before_another_target() {
        let (here, there) = (TargetId::from("here"), TargetId::from("there"));
        // The thread's own instance cannot transcribe (no voice surface), its neighbour can.
        let local = [provider("codex", "Own", None), provider("z", "Neighbour", ready())];
        let remote = [provider("a", "Far", ready())];
        let candidates = [
            Candidate { target: &here, name: "Here", connected: true, providers: &local },
            Candidate { target: &there, name: "There", connected: true, providers: &remote },
        ];
        let own = InstanceId::from("codex");
        let got = pick(choose(Context { target: Some(&here), instance: Some(&own) }, &candidates));
        assert_eq!(got.1, "z");
    }

    #[test]
    fn another_connected_target_serves_when_the_own_one_cannot() {
        let (here, there, off) = (TargetId::from("here"), TargetId::from("there"), TargetId::from("a-offline"));
        let local = [provider("codex", "Own", refused("needs an API key"))];
        let remote = [provider("claude", "Claude", ready())];
        let offline = [provider("x", "Cached", ready())];
        let candidates = [
            Candidate { target: &off, name: "Off", connected: false, providers: &offline },
            Candidate { target: &here, name: "Here", connected: true, providers: &local },
            Candidate { target: &there, name: "wsl: Ubuntu", connected: true, providers: &remote },
        ];
        let got = pick(choose(Context { target: Some(&here), instance: None }, &candidates));
        assert_eq!(got, ("there".into(), "claude".into(), "Claude on wsl: Ubuntu".into()));
    }

    #[test]
    fn a_draft_without_a_target_gets_the_first_ready_one_by_id() {
        let (a, b) = (TargetId::from("a"), TargetId::from("b"));
        let pa = [provider("y", "Y", ready())];
        let pb = [provider("x", "X", ready())];
        let candidates = [
            Candidate { target: &b, name: "B", connected: true, providers: &pb },
            Candidate { target: &a, name: "A", connected: true, providers: &pa },
        ];
        assert_eq!(pick(choose(Context::default(), &candidates)).0, "a");
    }

    #[test]
    fn no_transcriber_says_why_nearest_first() {
        let (here, there) = (TargetId::from("here"), TargetId::from("there"));
        let local = [provider("claude", "Claude", refused("Claude Code needs a claude.ai sign-in."))];
        let remote = [provider("codex", "Codex", refused("needs an API key"))];
        let candidates = [
            Candidate { target: &there, name: "There", connected: true, providers: &remote },
            Candidate { target: &here, name: "Here", connected: true, providers: &local },
        ];
        let Choice::Unavailable(why) = choose(Context { target: Some(&here), instance: None }, &candidates) else { panic!() };
        assert_eq!(why, "No transcriber available. Claude on Here: Claude Code needs a claude.ai sign-in.");
        let none: [Candidate<'_>; 0] = [];
        assert!(matches!(choose(Context::default(), &none), Choice::Unavailable(w) if w.contains("no machine is connected")));
    }

    #[test]
    fn a_folded_capability_is_not_ready() {
        // A connection without the `transcription` feature folds the capability away.
        let here = TargetId::from("here");
        let mut p = provider("a", "A", ready());
        p.capabilities.clear();
        let list = [p];
        let candidates = [Candidate { target: &here, name: "Here", connected: true, providers: &list }];
        assert!(matches!(choose(Context::default(), &candidates), Choice::Unavailable(_)));
    }
}
