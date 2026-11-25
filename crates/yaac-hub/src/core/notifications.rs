//! One native reminder tracks the pending, unseen requests across the entire fleet.
use super::*;
use crate::notify::Notification;
use attach::{NotificationSettings, RequestLocation};
use yaac_protocol::{DecisionKind, ItemId, RequestBody};

struct Card {
    item: ItemId,
    title: String,
    question: bool,
    decisions: Option<(String, String)>,
    kind: &'static str,
    created_ms: i64,
    order: u64,
    reviewed: bool,
    announced: bool,
}

struct Posted {
    updates: mpsc::UnboundedSender<Notification>,
    members: Vec<RequestLocation>,
    decisions: Option<(String, String)>,
    notice: Notification,
}

#[derive(Default)]
pub(super) struct Notifications {
    cards: BTreeMap<RequestLocation, Card>,
    pub(super) labels: HashMap<Key, String>,
    posted: Option<Posted>,
    order: u64,
    revision: u64,
    deadline: Option<Instant>,
    pub(super) open: std::collections::VecDeque<attach::NotificationOpen>,
    last_launch: Option<Instant>,
    answers: HashMap<RpcId, attach::NotificationOpen>,
}

/// Fit each worker or title on one preview line, even when the provider sends a long prompt.
fn preview(value: &str, limit: usize) -> String {
    let mut words = value.split_whitespace();
    let first = words.next().unwrap_or_default();
    let mut chars = first
        .chars()
        .chain(words.flat_map(|word| std::iter::once(' ').chain(word.chars())));
    let mut short: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        short.pop();
        short.push('…');
    }
    short
}

impl Notifications {
    pub(super) fn observe(&mut self, key: &Key, item: &Item, now: Instant) {
        let ItemBody::Request(r) = &item.body else {
            return;
        };
        let loc = RequestLocation {
            target: key.0.clone(),
            thread_id: key.1.clone(),
            request_id: r.request_id.clone(),
        };
        if r.state != RequestState::Pending {
            self.resolve(&loc);
            return;
        }
        let (kind, title, decisions) = match &r.body {
            RequestBody::Approval(a) => {
                let plain = |kind| {
                    a.decisions
                        .iter()
                        .find(|d| d.kind == kind && !d.accepts_reason)
                        .map(|d| d.id.clone())
                };
                (
                    "Approval",
                    if a.details.as_ref().and_then(|d| d["kind"].as_str()) == Some("writeStdin") {
                        "Send input to a running command".into()
                    } else if a.command.is_some() {
                        "Run a command".into()
                    } else {
                        a.title.clone()
                    },
                    plain(DecisionKind::Allow).zip(plain(DecisionKind::Deny)),
                )
            }
            RequestBody::Form(form)
                if form.schema.as_ref().is_some_and(|s| s["url"].is_string()) =>
            {
                ("Login", form.message.clone(), None)
            }
            RequestBody::Form(f) => ("Form", f.message.clone(), None),
            RequestBody::Question(q) => (
                "Question",
                q.questions
                    .first()
                    .map_or_else(|| "Agent question".into(), |q| q.text.clone()),
                None,
            ),
            _ => ("Request", "Input needed".into(), None),
        };
        let card = self.cards.entry(loc).or_insert_with(|| {
            self.order += 1;
            Card {
                item: item.id.clone(),
                title: String::new(),
                decisions: None,
                question: matches!(r.body, RequestBody::Question(_)),
                kind,
                created_ms: item
                    .started_at
                    .as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts.as_str()).ok())
                    .map_or_else(now_ms, |at| at.timestamp_millis()),
                order: self.order,
                reviewed: false,
                announced: false,
            }
        });
        card.title = preview(&title, 64);
        card.decisions = decisions;
        self.reconsider(now);
    }

    pub(super) fn reconsider(&mut self, now: Instant) {
        // Give the first viewport report a chance to suppress a newly arrived card.
        // An existing reminder reconciles on the next hub tick.
        self.deadline.get_or_insert(if self.posted.is_some() {
            now
        } else {
            now + Duration::from_millis(300)
        });
    }

    pub(super) fn remove_target(&mut self, target: &TargetId) {
        self.labels.retain(|(t, _), _| t != target);
        let gone: Vec<_> = self
            .cards
            .keys()
            .filter(|loc| &loc.target == target)
            .cloned()
            .collect();
        for loc in gone {
            self.resolve(&loc);
        }
    }

    pub(super) fn snapshot<'a>(&mut self, key: &Key, items: impl IntoIterator<Item = &'a Item>) {
        let pending: Vec<_> = items
            .into_iter()
            .filter(|i| matches!(i.body, ItemBody::Request(_)))
            .collect();
        let ids: HashSet<_> = pending
            .iter()
            .filter_map(|item| match &item.body {
                ItemBody::Request(r) if r.state == RequestState::Pending => Some(&r.request_id),
                _ => None,
            })
            .collect();
        let gone: Vec<_> = self
            .cards
            .keys()
            .filter(|loc| {
                loc.target == key.0 && loc.thread_id == key.1 && !ids.contains(&loc.request_id)
            })
            .cloned()
            .collect();
        for loc in gone {
            self.resolve(&loc);
        }
        for item in pending {
            self.observe(key, item, Instant::now());
        }
    }

    pub(super) fn resolve(&mut self, loc: &RequestLocation) {
        if self.cards.remove(loc).is_some() {
            self.deadline = Some(Instant::now());
        }
    }

    fn description(&self, loc: &RequestLocation) -> String {
        let card = &self.cards[loc];
        let place = self
            .labels
            .get(&(loc.target.clone(), loc.thread_id.clone()))
            .cloned()
            .unwrap_or_else(|| format!("{}/{}", loc.target, loc.thread_id));
        format!("{} · {} · {}", preview(&place, 48), card.kind, card.title)
    }

    pub(super) fn flush(
        &mut self,
        now: Instant,
        settings: NotificationSettings,
        visible: impl Fn(&RequestLocation) -> bool,
        notifier: &dyn crate::notify::Notifier,
        tx: &mpsc::UnboundedSender<CoreEvent>,
    ) {
        if !self.deadline.is_some_and(|at| now >= at) {
            return;
        }
        self.deadline = None;
        let mut unseen = Vec::new();
        for (loc, card) in &mut self.cards {
            if visible(loc) {
                // Once an announced request is seen, leaving the viewport does not re-alert.
                card.reviewed |= card.announced;
            } else if settings.enabled
                && !card.reviewed
                && (!card.question || settings.include_questions)
            {
                unseen.push(((card.created_ms, card.order), loc.clone()));
            }
        }
        unseen.sort_unstable_by_key(|(order, _)| *order);
        let members: Vec<_> = unseen.into_iter().map(|(_, loc)| loc).collect();
        let Some(first) = members.first() else {
            self.posted = None;
            return;
        };
        let count = members.len();
        let decisions = (count == 1)
            .then(|| self.cards[first].decisions.clone())
            .flatten();
        let title = if count == 1 {
            "YAAC needs you".into()
        } else {
            format!("{count} requests need you")
        };
        let body = members
            .iter()
            .take(3)
            .map(|loc| self.description(loc))
            .collect::<Vec<_>>()
            .join("\n");
        let previous_count = self.posted.as_ref().map_or(0, |p| p.members.len());
        if self.posted.as_ref().is_some_and(|p| {
            !p.updates.is_closed()
                && p.members == members
                && p.decisions == decisions
                && p.notice.title == title
                && p.notice.body == body
        }) {
            return;
        }

        self.revision += 1;
        let notice = Notification {
            title,
            body,
            settings,
            decisions: decisions.is_some(),
            revision: self.revision,
            play_sound: settings.sound && count > previous_count,
        };
        for loc in &members {
            self.cards.get_mut(loc).unwrap().announced = true;
        }
        if let Some(posted) = &mut self.posted
            && posted.updates.send(notice.clone()).is_ok()
        {
            posted.members = members;
            posted.decisions = decisions;
            posted.notice = notice;
            return;
        }
        let (updates, receiver) = mpsc::unbounded_channel();
        let tx = tx.clone();
        notifier.show(
            notice.clone(),
            Box::new(move |revision, action| {
                let _ = tx.send(CoreEvent::NotificationAction(revision, action.to_owned()));
            }),
            receiver,
        );
        self.posted = Some(Posted {
            updates,
            members,
            decisions,
            notice,
        });
    }
}

impl Core {
    pub(super) fn notification_action(&mut self, revision: u64, action: &str) {
        let state = &mut self.notifications;
        let Some(posted) = state
            .posted
            .as_ref()
            .filter(|p| p.notice.revision == revision)
        else {
            return;
        };
        if action == "dismiss" {
            for loc in &posted.members {
                if let Some(card) = state.cards.get_mut(loc) {
                    card.reviewed = true;
                }
            }
            state.posted = None;
            state.reconsider(Instant::now());
            return;
        }
        let loc = posted.members[0].clone();
        let Some(card) = state.cards.get_mut(&loc) else {
            return;
        };
        if card.reviewed {
            return;
        }
        let open = attach::NotificationOpen {
            location: loc.clone(),
            item_id: card.item.clone(),
            error: None,
        };
        let decision = match (action, &posted.decisions) {
            ("default", _) => None,
            ("allow" | "deny", Some((allow, deny))) if posted.decisions == card.decisions => {
                Some(if action == "allow" {
                    allow.clone()
                } else {
                    deny.clone()
                })
            }
            ("allow" | "deny", Some(_)) => None, // Decisions changed while the popup was visible.
            _ => return,
        };
        card.reviewed = true;
        state.deadline = Some(Instant::now());
        if let Some(decision) = decision {
            let request = RoutedRequest {
                target: loc.target.clone(),
                method: names::REQUEST_RESPOND.into(),
                params: json!({
                    "threadId":loc.thread_id,"requestId":loc.request_id,"opId":new_id("toast"),"response":{"requestKind":"approval","decision":decision}
                }),
            };
            // request/respond checks the current writer on the target, including a read-only
            // harness worker whose parent controls answers. Its failure reopens the card.
            let rid = RpcId::String(new_id("notification"));
            self.notifications.answers.insert(rid.clone(), open);
            if let Some(outcome) = self.on_routed(NOTIFICATION_CLIENT, &rid, request) {
                self.notification_answer(&rid, outcome.err());
            }
        } else {
            self.open_notification(open);
        }
    }
    pub(super) fn notification_answer(&mut self, rid: &RpcId, error: Option<ErrorObject>) {
        if let Some(mut open) = self.notifications.answers.remove(rid)
            && let Some(error) = error
        {
            open.error = Some(error.message);
            self.open_notification(open);
        }
    }

    fn open_notification(&mut self, open: attach::NotificationOpen) {
        let origin = (
            open.location.target.clone(),
            open.location.thread_id.clone(),
        );
        if self
            .clients
            .values()
            .filter(|c| c.visible_requests.is_some())
            .take(2)
            .count()
            < 2
        {
            self.show_notification_destination(open, &[origin]);
            return;
        }
        self.worker.submit(move |stores| {
            let mut lineage = vec![origin];
            while let Some((target, thread)) = lineage.last() {
                let Some(parent) = stores
                    .cache
                    .thread_summary(target, thread)
                    .ok()
                    .flatten()
                    .and_then(|s| s.spawned_by)
                else {
                    break;
                };
                let (Some(t), Some(th)) = (parent.target(), parent.thread()) else {
                    break;
                };
                let next = (t, th);
                if lineage.contains(&next) {
                    break;
                }
                lineage.push(next);
            }
            Some(CoreEvent::NotificationDestination(open, lineage))
        });
    }

    pub(super) fn show_notification_destination(
        &mut self,
        open: attach::NotificationOpen,
        lineage: &[Key],
    ) {
        if let Some((&client, _)) = self
            .clients
            .iter()
            .filter(|(_, c)| c.visible_requests.is_some())
            .min_by_key(|(id, c)| {
                (
                    lineage
                        .iter()
                        .position(|k| c.showing.contains(k))
                        .unwrap_or(usize::MAX),
                    **id,
                )
            })
        {
            self.send_to(
                client,
                Message::notification(attach::names::NOTIFICATIONS_OPEN, to_value(&open)),
            );
        } else {
            let retry_launch = self
                .notifications
                .last_launch
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(5));
            if self.notifications.open.is_empty() || retry_launch {
                if let Some(desktop) = &self.cfg.notifier {
                    desktop.launch_window();
                    self.notifications.last_launch = Some(Instant::now());
                }
            }
            self.notifications.open.push_back(open);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Desktop(Mutex<Vec<(Notification, crate::notify::Updates)>>);
    impl crate::notify::Notifier for Desktop {
        fn launch_window(&self) {
            panic!("unexpected Open");
        }
        fn show(
            &self,
            n: Notification,
            _: crate::notify::Activate,
            cancel: crate::notify::Updates,
        ) {
            self.0.lock().unwrap().push((n, cancel));
        }
    }

    #[test]
    fn hiding_a_previously_visible_request_posts_once() {
        let key = (TargetId::new("a"), ThreadId::new("worker"));
        let item = serde_json::from_value(
            json!({"kind":"request","id":"i","turnId":"turn","status":"completed",
            "requestId":"request","state":"pending","requestKind":"question","questions":[]}),
        )
        .unwrap();
        let now = Instant::now();
        let mut state = Notifications::default();
        let desktop = Desktop::default();
        let (tx, _) = mpsc::unbounded_channel();
        let delay = Duration::from_secs(2);
        state.observe(&key, &item, now);
        state.flush(now + delay, Default::default(), |_| true, &desktop, &tx);
        assert!(desktop.0.lock().unwrap().is_empty());
        state.reconsider(now + delay);
        state.flush(
            now + 2 * delay,
            Default::default(),
            |_| false,
            &desktop,
            &tx,
        );
        state.observe(&key, &item, now + 2 * delay);
        state.reconsider(now + 2 * delay);
        state.flush(
            now + 3 * delay,
            Default::default(),
            |_| false,
            &desktop,
            &tx,
        );
        assert_eq!(desktop.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn focused_cards_and_preferences_filter_the_summary() {
        let key = (TargetId::new("a"), ThreadId::new("t"));
        let desktop = Desktop::default();
        let now = Instant::now();
        let (tx, _) = mpsc::unbounded_channel();
        for (prefs, expected) in [
            (NotificationSettings::default(), 1),
            (
                NotificationSettings {
                    enabled: false,
                    ..Default::default()
                },
                0,
            ),
            (
                NotificationSettings {
                    include_questions: false,
                    ..Default::default()
                },
                0,
            ),
        ] {
            let mut state = Notifications::default();
            for i in 0..5 {
                let item = serde_json::from_value(json!({"kind":"request","id":format!("i{i}"),"turnId":"turn","status":"completed",
                    "requestId":i.to_string(),"state":"pending","requestKind":"question","questions":[]})).unwrap();
                state.observe(&key, &item, now);
                state.observe(&key, &item, now);
            }
            state.flush(
                now + Duration::from_secs(2),
                prefs,
                |loc| loc.request_id.as_str() == "0",
                &desktop,
                &tx,
            );
            let mut posted = desktop.0.lock().unwrap();
            assert_eq!(posted.len(), expected);
            if let Some((n, mut cancel)) = posted.pop() {
                assert_eq!(n.title, "4 requests need you");
                assert!(!n.decisions);
                state.snapshot(&key, &[]);
                state.flush(
                    now + Duration::from_secs(3),
                    prefs,
                    |_| false,
                    &desktop,
                    &tx,
                );
                while cancel.try_recv().is_ok() {}
                assert!(matches!(
                    cancel.try_recv(),
                    Err(mpsc::error::TryRecvError::Disconnected)
                ));
            }
        }
    }
    #[test]
    fn a_visible_copy_removes_its_worker_from_a_muted_summary() {
        let mut state = Notifications::default();
        let desktop = Desktop::default();
        let (tx, _) = mpsc::unbounded_channel();
        let now = Instant::now();
        let settings = NotificationSettings {
            sound: false,
            ..Default::default()
        };
        let late = (TargetId::new("t"), ThreadId::new("late"));
        let early = (TargetId::new("t"), ThreadId::new("early"));
        for (key, date) in [
            (&late, "2026-11-25T10:10:00Z"),
            (&early, "2026-11-25T10:00:00Z"),
        ] {
            let item = serde_json::from_value(json!({"kind":"request","id":"item","turnId":"turn","status":"completed",
                "startedAt":date,"requestId":"r","state":"pending","requestKind":"form","message":"Sign in","schema":{"url":"https://example.invalid"}})).unwrap();
            state.observe(key, &item, now);
            state.flush(
                now + Duration::from_secs(1),
                settings,
                |_| false,
                &desktop,
                &tx,
            );
        }
        {
            let mut posted = desktop.0.lock().unwrap();
            assert!(!posted[0].0.play_sound);
            let group = posted[0].1.try_recv().unwrap();
            assert!(!group.play_sound);
            assert!(group.body.starts_with("t/early · Login · Sign in"));
        }
        state.reconsider(now + Duration::from_secs(2));
        state.flush(
            now + Duration::from_secs(2),
            settings,
            |r| r.thread_id == early.1,
            &desktop,
            &tx,
        );
        let mut posted = desktop.0.lock().unwrap();
        let single = posted[0].1.try_recv().unwrap();
        assert!(single.body.contains("t/late"));
        assert!(!single.play_sound && !single.decisions);
        state.reconsider(now + Duration::from_secs(3));
        state.flush(
            now + Duration::from_secs(3),
            settings,
            |_| false,
            &desktop,
            &tx,
        );
        assert!(posted[0].1.try_recv().is_err());
    }

    #[test]
    fn six_approvals_ten_seconds_apart_update_one_reminder_and_sound_only_on_growth() {
        let mut state = Notifications::default();
        let desktop = Desktop::default();
        let (tx, _) = mpsc::unbounded_channel();
        let start = Instant::now();
        let mut places = Vec::new();
        for i in 0..6 {
            let at = start + Duration::from_secs(i * 10);
            // Reverse lexicographic order to distinguish chronological from id order.
            let key = (
                TargetId::new("remote"),
                ThreadId::new(format!("worker{}", 6 - i)),
            );
            let item = serde_json::from_value(json!({"kind":"request","id":"item","turnId":"turn","status":"completed",
                "requestId":"r","state":"pending","requestKind":"approval","approvalKind":"fileChange","title":format!("Edit file {i}"),
                "decisions":[{"kind":"allow","id":"yes","label":"Allow once"},{"kind":"deny","id":"no","label":"Deny"}]})).unwrap();
            places.push(RequestLocation {
                target: key.0.clone(),
                thread_id: key.1.clone(),
                request_id: "r".into(),
            });
            state.observe(&key, &item, at);
            state.flush(
                at + Duration::from_secs(1),
                Default::default(),
                |_| false,
                &desktop,
                &tx,
            );
            let mut posted = desktop.0.lock().unwrap();
            assert_eq!(posted.len(), 1);
            let n = if i == 0 {
                posted[0].0.clone()
            } else {
                posted[0].1.try_recv().unwrap()
            };
            assert!(n.play_sound);
            assert_eq!(n.decisions, i == 0);
            if i > 0 {
                assert_eq!(n.title, format!("{} requests need you", i + 1));
            }
            assert_eq!(n.body.lines().count(), (i + 1).min(3) as usize);
            assert!(
                n.body
                    .starts_with("remote/worker6 · Approval · Edit file 0")
            );
            assert_eq!(state.posted.as_ref().unwrap().members, places);
            state.observe(&key, &item, at);
            state.flush(
                at + Duration::from_secs(2),
                Default::default(),
                |_| false,
                &desktop,
                &tx,
            );
            assert!(
                posted[0].1.try_recv().is_err(),
                "replay must not sound or redisplay"
            );
        }
        // Resolve out of arrival order; the last remaining approval regains its buttons.
        for (count, index) in [2, 0, 4, 1, 3, 5].into_iter().enumerate() {
            state.resolve(&places[index]);
            state.flush(
                start + Duration::from_secs(60 + count as u64),
                Default::default(),
                |_| false,
                &desktop,
                &tx,
            );
            let mut posted = desktop.0.lock().unwrap();
            let updated = posted[0].1.try_recv();
            match 5 - count {
                0 => assert!(matches!(
                    updated,
                    Err(mpsc::error::TryRecvError::Disconnected)
                )),
                remaining => {
                    let n = updated.unwrap();
                    assert!(!n.play_sound);
                    assert_eq!(n.decisions, remaining == 1);
                    if remaining == 1 {
                        assert!(n.body.contains("Edit file 5"));
                        assert_eq!(n.title, "YAAC needs you");
                    } else {
                        assert_eq!(n.title, format!("{remaining} requests need you"));
                    }
                }
            }
        }
    }
}
