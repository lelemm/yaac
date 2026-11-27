//! Desktop WebRTC media lives in the dedicated browser host. The app owns the session and
//! consent, and uses the same signalling state as renderers that supply their own peer.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_core::key::ThreadKey;
use yaac_core::voice_call::CallPhase;

use crate::app::YaacApp;

const OFFER_LIMIT: Duration = Duration::from_secs(25);
const CONNECT_LIMIT: Duration = Duration::from_secs(30);
pub(crate) const CONSENT: &str =
    "The agent can start work in this thread while you talk. The call uses your plan’s quota.";

#[derive(Default)]
pub(crate) struct DesktopCall {
    consent: Option<ThreadKey>,
    session: Option<Session>,
    failure: Option<(ThreadKey, String)>,
}

struct Session {
    key: ThreadKey,
    peer: Box<dyn Peer>,
    phase: Phase,
    deadline: Instant,
    muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Preparing,
    AwaitingAnswer,
    Connecting,
    Active(Instant),
}

#[cfg_attr(not(all(windows, feature = "preview")), allow(dead_code))]
enum Event {
    Offer(String),
    Ready,
    Failed(String),
}

trait Peer {
    fn poll(&self) -> Option<Event>;
    fn answer(&self, sdp: String);
    fn mute(&self, muted: bool);
}

#[cfg(all(windows, feature = "preview"))]
impl Peer for yaac_preview::VoiceCall {
    fn poll(&self) -> Option<Event> {
        use yaac_preview::wire::CallEvent;
        Some(match self.try_update()? {
            CallEvent::Offer(sdp) => Event::Offer(sdp),
            CallEvent::Ready => Event::Ready,
            CallEvent::Failed(error) => Event::Failed(error),
        })
    }

    fn answer(&self, sdp: String) {
        yaac_preview::VoiceCall::answer(self, sdp);
    }

    fn mute(&self, muted: bool) {
        yaac_preview::VoiceCall::mute(self, muted);
    }
}

impl Session {
    fn new(key: ThreadKey, peer: Box<dyn Peer>, now: Instant) -> Self {
        Self {
            key,
            peer,
            phase: Phase::Preparing,
            deadline: now + OFFER_LIMIT,
            muted: false,
        }
    }

    fn offer(&mut self, now: Instant) -> bool {
        if self.phase != Phase::Preparing {
            return false;
        }
        self.phase = Phase::AwaitingAnswer;
        self.deadline = now + CONNECT_LIMIT;
        true
    }

    fn answer(&mut self, sdp: &str) {
        if self.phase == Phase::AwaitingAnswer {
            self.peer.answer(sdp.to_owned());
            self.phase = Phase::Connecting;
        }
    }

    fn ready(&mut self, now: Instant) {
        if self.phase == Phase::Connecting {
            self.phase = Phase::Active(now);
        }
    }

    fn toggle_mute(&mut self) {
        if matches!(self.phase, Phase::Active(_)) {
            self.muted = !self.muted;
            self.peer.mute(self.muted);
        }
    }
}

pub(crate) fn busy(app: &YaacApp) -> bool {
    app.desktop_call.session.is_some() || app.client.voice_call().is_some()
}

fn unavailable(app: &YaacApp) -> Option<&str> {
    if app.demo.is_some() {
        return Some("Desktop voice calls require a live thread.");
    }
    #[cfg(all(windows, feature = "preview"))]
    {
        app.browsers.runtime_reason()
    }
    #[cfg(not(all(windows, feature = "preview")))]
    {
        let _ = app;
        Some("Voice calls require the Windows browser host.")
    }
}

fn can_start(app: &YaacApp, key: &ThreadKey) -> bool {
    crate::voice_call::offered(app, key)
        && unavailable(app).is_none()
        && app.client.state.may_act(key)
        && app.client.dictation().is_none()
        && !busy(app)
}

pub(crate) fn ask(app: &mut YaacApp, key: &ThreadKey) {
    if !can_start(app, key) {
        return;
    }
    if app.ui.desktop_call_consent {
        start(app, key);
    } else {
        app.desktop_call.consent = Some(key.clone());
    }
}

pub(crate) fn consent(app: &mut YaacApp, key: &ThreadKey, accepted: bool) {
    if app.desktop_call.consent.as_ref() != Some(key) {
        return;
    }
    app.desktop_call.consent = None;
    if accepted && can_start(app, key) {
        app.ui.desktop_call_consent = true;
        start(app, key);
    }
}

fn open_peer(ctx: &egui::Context) -> Option<Box<dyn Peer>> {
    #[cfg(all(windows, feature = "preview"))]
    {
        let ctx = ctx.clone();
        Some(Box::new(yaac_preview::VoiceCall::open(move || {
            ctx.request_repaint()
        })))
    }
    #[cfg(not(all(windows, feature = "preview")))]
    {
        let _ = ctx;
        None
    }
}

fn start(app: &mut YaacApp, key: &ThreadKey) {
    if let Some(peer) = open_peer(&app.ctx) {
        app.desktop_call.failure = None;
        app.desktop_call.session = Some(Session::new(key.clone(), peer, Instant::now()));
        app.ctx.request_repaint();
    }
}

pub(crate) fn end(app: &mut YaacApp, key: &ThreadKey) {
    if app
        .desktop_call
        .session
        .as_ref()
        .is_some_and(|s| &s.key == key)
    {
        app.desktop_call.session = None;
    }
    crate::voice_call::end(app, key);
}

pub(crate) fn mute(app: &mut YaacApp, key: &ThreadKey) {
    if let Some(session) = app.desktop_call.session.as_mut().filter(|s| &s.key == key) {
        session.toggle_mute();
    }
}

fn fail(app: &mut YaacApp, key: ThreadKey, error: String) {
    crate::voice_call::end(app, &key);
    app.desktop_call.failure = Some((key, error));
}

pub(crate) fn upkeep(app: &mut YaacApp) {
    if let Some(key) = &app.desktop_call.consent
        && app.open_composer() != Some(yaac_core::attachments::DraftRef::Thread(key.clone()))
    {
        app.desktop_call.consent = None;
    }
    let Some(mut session) = app.desktop_call.session.take() else {
        return;
    };
    let now = Instant::now();
    if session.phase != Phase::Preparing {
        let Some(call) = app
            .client
            .voice_call()
            .filter(|c| c.key == session.key && c.phase != CallPhase::Ending)
        else {
            return;
        };
        if let Some(answer) = &call.answer {
            session.answer(answer);
        }
    }
    while let Some(event) = session.peer.poll() {
        match event {
            Event::Offer(sdp) if session.offer(now) => {
                if !app.client.state.may_act(&session.key) {
                    fail(
                        app,
                        session.key.clone(),
                        "The thread is no longer available for a call.".into(),
                    );
                    return;
                }
                if let Err(error) = crate::voice_call::start(app, &session.key, sdp, None) {
                    fail(app, session.key.clone(), error);
                    return;
                }
            }
            Event::Offer(_) => {}
            Event::Ready => session.ready(now),
            Event::Failed(error) => {
                fail(app, session.key.clone(), error);
                return;
            }
        }
    }
    if let Phase::Active(_) = session.phase {
        if app.client.state.selected_thread() == Some(&session.key)
            && !app.ui.settings
            && !app.shell.hidden
        {
            app.ctx.request_repaint_after(Duration::from_secs(1));
        }
    } else if now >= session.deadline {
        let error = if session.phase == Phase::Preparing {
            "The browser host did not prepare the call."
        } else {
            "The voice connection timed out."
        };
        fail(app, session.key.clone(), error.into());
        return;
    } else {
        app.ctx.request_repaint_after(session.deadline - now);
    }
    app.desktop_call.session = Some(session);
}

pub(crate) fn view(app: &YaacApp, key: &ThreadKey) -> Option<Value> {
    let signal = crate::voice_call::view(app, key);
    let session = app.desktop_call.session.as_ref().filter(|s| &s.key == key);
    let error = app
        .desktop_call
        .failure
        .as_ref()
        .filter(|(k, _)| k == key)
        .map(|(_, error)| error.as_str());
    let consent = app.desktop_call.consent.as_ref() == Some(key);
    if signal.is_none() && session.is_none() && error.is_none() {
        return None;
    }
    let mut view = signal.unwrap_or_else(|| json!({ "state": "idle" }));
    let reason = unavailable(app);
    let state = view["state"].as_str().unwrap_or("idle");
    let (status, elapsed) = match session.map(|session| session.phase) {
        Some(Phase::Preparing) => ("Preparing call…", None),
        Some(Phase::AwaitingAnswer | Phase::Connecting) => ("Connecting…", None),
        Some(Phase::Active(at)) => ("In call", Some(at.elapsed().as_secs())),
        None if error.is_none() && state == "ending" => ("Ending call…", None),
        None => ("", None),
    };
    let clock = elapsed.map(|s| format!("{}:{:02}", s / 60, s % 60));
    let error = error.or_else(|| view["error"].as_str().filter(|_| session.is_none()));
    let mute = session
        .filter(|_| elapsed.is_some())
        .map(|session| if session.muted { "Unmute" } else { "Mute" });
    view["desktop"] = json!({
        "phone": phone_offered(app, key),
        "canStart": can_start(app, key) && !consent,
        "reason": reason,
        "consent": consent.then_some(CONSENT),
        "status": status,
        "clock": clock,
        "error": error,
        "mute": mute,
        "end": session.is_some(),
    });
    Some(view)
}

pub(crate) fn bar(ui: &mut egui::Ui, app: &mut YaacApp, key: &ThreadKey) {
    let Some(view) = view(app, key) else { return };
    let desktop = &view["desktop"];
    let p = app.p;
    use crate::widgets::{plain, primary, secondary};
    if let Some(consent_text) = desktop["consent"].as_str() {
        ui.label(secondary(consent_text, &p));
        ui.horizontal(|ui| {
            if primary(ui, &p, "Start call", None).clicked() {
                consent(app, key, true);
            }
            if plain(ui, &p, "Cancel", None).clicked() {
                consent(app, key, false);
            }
        });
        return;
    }
    if let Some(error) = desktop["error"]
        .as_str()
        .or_else(|| desktop["reason"].as_str())
    {
        ui.label(secondary(error, &p));
    }
    let status = desktop["status"].as_str().unwrap_or_default();
    if !status.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(secondary(status, &p));
            if let Some(clock) = desktop["clock"].as_str() {
                ui.label(secondary(clock, &p));
            }
            if let Some(label) = desktop["mute"].as_str()
                && plain(ui, &p, label, None).clicked()
            {
                mute(app, key);
            }
            if desktop["end"] == true && plain(ui, &p, "End call", None).clicked() {
                end(app, key);
            }
        });
    }
    if desktop["end"] == true
        && let Some(lines) = view["transcript"].as_array()
    {
        for line in &lines[lines.len().saturating_sub(3)..] {
            let role = if line["role"] == "user" {
                "You"
            } else {
                "Agent"
            };
            if let Some(text) = line["text"].as_str() {
                ui.label(secondary(format!("{role} · {text}"), &p));
            }
        }
    }
}

pub(crate) fn phone(ui: &mut egui::Ui, app: &YaacApp, key: &ThreadKey, enabled: bool) -> bool {
    if !phone_offered(app, key) {
        return false;
    }
    ui.add_enabled_ui(
        enabled && can_start(app, key) && app.desktop_call.consent.as_ref() != Some(key),
        |ui| crate::widgets::icon_button(ui, &app.p, 16.0, "Start voice call", crate::icons::phone),
    )
    .inner
    .clicked()
}

pub(crate) fn phone_offered(app: &YaacApp, key: &ThreadKey) -> bool {
    crate::voice_call::offered(app, key)
        && unavailable(app).is_none()
        && app
            .desktop_call
            .session
            .as_ref()
            .is_none_or(|s| &s.key != key)
        && app.client.voice_call().is_none_or(|call| &call.key != key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct FakePeer {
        answers: RefCell<Vec<String>>,
        mutes: RefCell<Vec<bool>>,
    }

    impl Peer for Rc<FakePeer> {
        fn poll(&self) -> Option<Event> {
            None
        }
        fn answer(&self, sdp: String) {
            self.answers.borrow_mut().push(sdp);
        }
        fn mute(&self, muted: bool) {
            self.mutes.borrow_mut().push(muted);
        }
    }

    #[test]
    fn one_offer_and_answer_and_ready_only_after_answer() {
        let now = Instant::now();
        let key = ThreadKey::new(
            yaac_protocol::TargetId::new("test"),
            yaac_protocol::ThreadId::new("thread"),
        );
        let peer = Rc::new(FakePeer::default());
        let mut session = Session::new(key, Box::new(peer.clone()), now);
        session.ready(now);
        session.answer("early");
        assert_eq!(session.phase, Phase::Preparing);
        assert!(session.offer(now));
        assert!(!session.offer(now));
        assert_eq!(session.deadline, now + CONNECT_LIMIT);
        session.ready(now);
        assert_eq!(session.phase, Phase::AwaitingAnswer);
        session.answer("first");
        session.answer("duplicate");
        assert_eq!(&*peer.answers.borrow(), &["first"]);
        session.toggle_mute();
        assert!(peer.mutes.borrow().is_empty());
        session.ready(now);
        session.ready(now + Duration::from_secs(1));
        assert_eq!(session.phase, Phase::Active(now));
        session.toggle_mute();
        session.toggle_mute();
        assert_eq!(&*peer.mutes.borrow(), &[true, false]);
    }
}
