//! Request cards: what a card shows in each request state (docs/03-protocol.md#request-states).

use yaac_protocol::{
    AnsweredBy, ApprovalDecision, ClientId, DecisionKind, ItemBody, RequestBody, RequestItem, RequestResponse,
    RequestState, TurnId, TurnOrigin,
};

use crate::time;
use crate::transcript::Transcript;

/// The mask shown for a secret answer, everywhere.
pub const SECRET_MASK: &str = "••••••••";

/// What a request card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardState {
    /// Waiting for an answer. Not actionable when the thread is held elsewhere: then the card
    /// says "Waiting for an answer in the other program." and has no buttons.
    Pending {
        /// Buttons are shown.
        actionable: bool,
    },
    /// This window answered; the harness has not confirmed yet.
    Submitted {
        /// "Patch the decoder · yaac-protocol" (masked when secret).
        summary: String,
    },
    /// Answered by this window and confirmed — the normal end.
    Confirmed {
        /// The answer, masked when secret.
        summary: String,
        /// "confirmed 12s ago".
        when: Option<String>,
    },
    /// Answered somewhere else: another program, or another YAAC client.
    ResolvedElsewhere {
        /// "Allowed in another program on devbox · 2m ago".
        text: String,
    },
    /// No longer answerable. Not an error.
    Expired,
    /// The answer may or may not have arrived. Never re-sent; one action: check the status.
    OutcomeUnknown,
    /// A state this build does not know: shown as text, no buttons.
    Other(String),
}

/// Whether a question in this request is secret.
fn any_secret(body: &RequestBody) -> bool {
    matches!(body, RequestBody::Question(q) if q.questions.iter().any(|q| q.secret))
}

/// A one-line, masked summary of an answer.
pub fn answer_summary(body: &RequestBody, answer: &RequestResponse) -> String {
    match (body, answer) {
        (RequestBody::Approval(a), RequestResponse::Approval(ans)) => a
            .decisions
            .iter()
            .find(|d| d.id == ans.decision)
            .map_or_else(|| ans.decision.clone(), |d| d.label.clone()),
        (RequestBody::Question(q), RequestResponse::Question(ans)) => {
            let mut parts = Vec::new();
            for question in &q.questions {
                let Some(a) = ans.answers.get(&question.id) else { continue };
                if question.secret || a.redacted {
                    parts.push(SECRET_MASK.to_owned());
                    continue;
                }
                parts.extend(a.selected.iter().cloned());
                if let Some(other) = &a.other
                    && !other.is_empty()
                {
                    parts.push(other.clone());
                }
            }
            parts.join(" · ")
        }
        (_, RequestResponse::Unknown(_)) if crate::permission::summary(answer).is_some() => {
            crate::permission::summary(answer).unwrap_or_default()
        }
        (_, _) if any_secret(body) => SECRET_MASK.to_owned(),
        _ => "answered".to_owned(),
    }
}

/// The verb for a resolution: "Allowed", "Denied", "Answered".
fn verb(body: &RequestBody, answer: Option<&RequestResponse>) -> &'static str {
    match (body, answer) {
        (RequestBody::Approval(a), Some(RequestResponse::Approval(ans))) => {
            match a.decisions.iter().find(|d| d.id == ans.decision).map(|d| &d.kind) {
                Some(DecisionKind::Deny | DecisionKind::Abort) => "Denied",
                Some(DecisionKind::Allow | DecisionKind::AllowSession | DecisionKind::AllowAlways) => "Allowed",
                _ => "Answered",
            }
        }
        _ => "Answered",
    }
}

/// The card state of a request item, from this window's point of view.
pub fn card_state(
    request: &RequestItem,
    may_act: bool,
    me: Option<&ClientId>,
    local_summary: Option<&str>,
    target_name: &str,
    now: i64,
) -> CardState {
    match &request.state {
        RequestState::Pending => CardState::Pending { actionable: may_act },
        RequestState::Submitted => CardState::Submitted {
            summary: local_summary.map(str::to_owned).or_else(|| {
                request.resolution.as_ref().and_then(|r| r.answer.as_ref()).map(|a| answer_summary(&request.body, a))
            }).unwrap_or_else(|| if any_secret(&request.body) { SECRET_MASK.to_owned() } else { "your answer".to_owned() }),
        },
        RequestState::Resolved => {
            let res = request.resolution.as_ref();
            let answer = res.and_then(|r| r.answer.as_ref());
            let when = res.and_then(|r| r.at.as_ref()).and_then(|at| time::ago(now, at));
            let summary = answer
                .map(|a| answer_summary(&request.body, a))
                .or_else(|| local_summary.map(str::to_owned))
                .unwrap_or_default();
            let mine = res.is_some_and(|r| r.by == AnsweredBy::Client && r.client_id.is_some() && r.client_id.as_ref() == me);
            if mine {
                return CardState::Confirmed { summary, when: when.map(|w| format!("confirmed {w}")) };
            }
            let where_ = match res {
                Some(r) if r.by == AnsweredBy::Client => {
                    format!("on {}", r.client_host.clone().unwrap_or_else(|| "another YAAC window".to_owned()))
                }
                _ => format!("in another program on {target_name}"),
            };
            let mut text = format!("{} {where_}", verb(&request.body, answer));
            if let Some(w) = when {
                text.push_str(" · ");
                text.push_str(&w);
            }
            CardState::ResolvedElsewhere { text }
        }
        RequestState::Expired => CardState::Expired,
        RequestState::Unknown => CardState::OutcomeUnknown,
        RequestState::Other(s) => CardState::Other(s.clone()),
    }
}

/// The key shown on each decision: Enter on the first plain "allow", Esc on the first deny.
/// Keys act only while the card has focus.
pub fn decision_keys(decisions: &[ApprovalDecision]) -> Vec<Option<&'static str>> {
    let enter = decisions
        .iter()
        .position(|d| d.kind == DecisionKind::Allow)
        .or_else(|| decisions.iter().position(|d| !matches!(d.kind, DecisionKind::Deny | DecisionKind::Abort)));
    let esc = decisions.iter().position(|d| d.kind == DecisionKind::Deny).or_else(|| decisions.iter().position(|d| d.kind == DecisionKind::Abort));
    (0..decisions.len())
        .map(|i| if Some(i) == enter { Some("⏎") } else if Some(i) == esc { Some("Esc") } else { None })
        .collect()
}

/// Whether a turn was started by another agent rather than by the user: the turn's origin
/// when it was seen live, otherwise whether the turn contains a received agent message.
pub fn started_by_agent(transcript: &Transcript, turn: &TurnId) -> bool {
    if let Some(origin) = transcript.turn_origins.get(turn) {
        return *origin == TurnOrigin::Mesh;
    }
    transcript.items().iter().any(|i| &i.turn_id == turn && matches!(i.body, ItemBody::MeshReceived(_)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use yaac_protocol::{
        ApprovalAnswer, ApprovalKind, ApprovalRequest, Question, QuestionAnswer, QuestionAnswers, QuestionId,
        QuestionRequest, RequestId, RequestResolution, Timestamp,
    };

    use super::*;

    fn approval() -> RequestItem {
        RequestItem {
            request_id: RequestId::new("r1"),
            blocking: true,
            state: RequestState::Pending,
            resolution: None,
            body: RequestBody::Approval(ApprovalRequest {
                approval_kind: ApprovalKind::Command,
                title: "Run command".into(),
                command: Some("cargo fix".into()),
                cwd: None,
                changes: vec![],
                scope: None,
                reason: None,
                decisions: vec![
                    ApprovalDecision { id: "a".into(), label: "Allow once".into(), kind: DecisionKind::Allow, description: None, accepts_reason: false },
                    ApprovalDecision { id: "s".into(), label: "Allow for this thread".into(), kind: DecisionKind::AllowSession, description: None, accepts_reason: false },
                    ApprovalDecision { id: "d".into(), label: "Deny".into(), kind: DecisionKind::Deny, description: None, accepts_reason: false },
                ],
                details: None,
            }),
        }
    }

    fn secret_question() -> RequestItem {
        RequestItem {
            request_id: RequestId::new("r2"),
            blocking: true,
            state: RequestState::Pending,
            resolution: None,
            body: RequestBody::Question(QuestionRequest {
                questions: vec![Question {
                    id: QuestionId::new("token"),
                    header: None,
                    text: "Token?".into(),
                    options: vec![],
                    multi_select: false,
                    allow_other: true,
                    secret: true,
                }],
            }),
        }
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn transitions_cover_every_state() {
        let me = ClientId::new("me");
        let mut r = approval();
        assert_eq!(card_state(&r, true, Some(&me), None, "devbox", NOW), CardState::Pending { actionable: true });
        assert_eq!(card_state(&r, false, Some(&me), None, "devbox", NOW), CardState::Pending { actionable: false });

        r.state = RequestState::Submitted;
        assert_eq!(
            card_state(&r, true, Some(&me), Some("Allow once"), "devbox", NOW),
            CardState::Submitted { summary: "Allow once".into() }
        );

        r.state = RequestState::Resolved;
        r.resolution = Some(RequestResolution {
            by: AnsweredBy::Client,
            client_id: Some(me.clone()),
            client_host: None,
            answer: Some(RequestResponse::Approval(ApprovalAnswer { decision: "a".into(), reason: None })),
            at: Some(Timestamp::new(crate::time::format_rfc3339(NOW - 12))),
        });
        assert_eq!(
            card_state(&r, true, Some(&me), None, "devbox", NOW),
            CardState::Confirmed { summary: "Allow once".into(), when: Some("confirmed 12s ago".into()) }
        );

        r.resolution = Some(RequestResolution {
            by: AnsweredBy::Elsewhere,
            client_id: None,
            client_host: None,
            answer: Some(RequestResponse::Approval(ApprovalAnswer { decision: "a".into(), reason: None })),
            at: Some(Timestamp::new(crate::time::format_rfc3339(NOW - 120))),
        });
        assert_eq!(
            card_state(&r, true, Some(&me), None, "devbox", NOW),
            CardState::ResolvedElsewhere { text: "Allowed in another program on devbox · 2m ago".into() }
        );

        // Another YAAC client is "elsewhere" too, never "you".
        r.resolution = Some(RequestResolution {
            by: AnsweredBy::Client,
            client_id: Some(ClientId::new("laptop")),
            client_host: Some("laptop".into()),
            answer: Some(RequestResponse::Approval(ApprovalAnswer { decision: "d".into(), reason: None })),
            at: None,
        });
        assert_eq!(card_state(&r, true, Some(&me), None, "devbox", NOW), CardState::ResolvedElsewhere { text: "Denied on laptop".into() });

        r.state = RequestState::Expired;
        assert_eq!(card_state(&r, true, Some(&me), None, "devbox", NOW), CardState::Expired);
        r.state = RequestState::Unknown;
        assert_eq!(card_state(&r, true, Some(&me), None, "devbox", NOW), CardState::OutcomeUnknown);
    }

    #[test]
    fn secrets_are_masked_in_every_summary() {
        let me = ClientId::new("me");
        let mut r = secret_question();
        let mut answers = BTreeMap::new();
        answers.insert(QuestionId::new("token"), QuestionAnswer { selected: vec![], other: Some("hunter2".into()), redacted: false });
        let answer = RequestResponse::Question(QuestionAnswers { answers });
        assert_eq!(answer_summary(&r.body, &answer), SECRET_MASK);
        r.state = RequestState::Submitted;
        assert_eq!(card_state(&r, true, Some(&me), None, "x", NOW), CardState::Submitted { summary: SECRET_MASK.into() });
        r.state = RequestState::Resolved;
        r.resolution = Some(RequestResolution { by: AnsweredBy::Client, client_id: Some(me.clone()), client_host: None, answer: Some(answer), at: None });
        assert_eq!(card_state(&r, true, Some(&me), None, "x", NOW), CardState::Confirmed { summary: SECRET_MASK.into(), when: None });
    }

    #[test]
    fn keys_follow_the_offered_decisions() {
        let r = approval();
        let RequestBody::Approval(a) = &r.body else { unreachable!() };
        assert_eq!(decision_keys(&a.decisions), vec![Some("⏎"), None, Some("Esc")]);
        assert_eq!(decision_keys(&a.decisions[1..]), vec![Some("⏎"), Some("Esc")]);
    }
}
