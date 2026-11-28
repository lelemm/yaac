//! External dialogs (docs/16-external-dialogs.md): what every window needs to show and answer a
//! dialog an integrator opened — the input draft keyed by node id, validation before submit, the
//! answer, its masked summary, the source line, and the target inboxes (dialogs raised without a
//! thread) with this window's answers in flight.
//!
//! The layout is built at runtime by the integrator; renderers walk it generically and ask this
//! module for every decision, so egui, WPF and Android agree. Nothing here knows a provider; it
//! branches on the request kind only (invariant 4). Secret values live only in a draft in memory
//! and in the answer sent; summaries mask them and nothing caches them.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Number, Value};
use yaac_protocol::attach::{HubChannel, TargetInbox};
use yaac_protocol::dialog_tree::{self, input, is_secret};
use yaac_protocol::{
    DialogAnswer, DialogButton, DialogButtonKind, DialogNode, DialogRequest, RequestBody, RequestId, RequestItem, RequestResponse,
    RequestState, TargetId,
};

use crate::requests::SECRET_MASK;

/// The line an unknown node shows.
pub const NEWER_YAAC: &str = "This part of the dialog needs a newer YAAC";

/// What the user has entered so far in one dialog, by input id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogDraft {
    pub values: BTreeMap<String, DraftValue>,
    /// The problems of the last submit attempt, by input id (`""` for the dialog as a whole).
    pub problems: BTreeMap<String, String>,
}

/// One input's value while it is edited. Numbers, sliders and dates are kept as typed text and
/// checked when the dialog is answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftValue {
    Text(String),
    Bool(bool),
    Choice(Option<String>),
    Many(BTreeSet<String>),
}

fn number_text(n: &Number) -> String {
    n.to_string()
}

impl DialogDraft {
    /// A draft holding every input's default.
    pub fn new(layout: &DialogNode) -> Self {
        let mut values = BTreeMap::new();
        for node in dialog_tree::nodes(layout) {
            let Some(i) = input(node) else { continue };
            let v = match node {
                DialogNode::TextInput(t) => DraftValue::Text(if is_secret(node) { String::new() } else { t.default.clone().unwrap_or_default() }),
                DialogNode::Number(n) => DraftValue::Text(n.default.as_ref().map(number_text).unwrap_or_default()),
                DialogNode::Slider(s) => DraftValue::Text(number_text(s.default.as_ref().unwrap_or(&s.min))),
                DialogNode::Date(d) => DraftValue::Text(d.default.clone().unwrap_or_default()),
                DialogNode::Checkbox(c) | DialogNode::Toggle(c) => DraftValue::Bool(c.default.unwrap_or(false)),
                DialogNode::Select(s) | DialogNode::Radio(s) => DraftValue::Choice(s.default.clone().filter(|d| s.options.iter().any(|o| &o.value == d))),
                DialogNode::MultiSelect(m) => DraftValue::Many(
                    m.default.iter().flatten().filter(|d| m.options.iter().any(|o| &o.value == *d)).cloned().collect(),
                ),
                _ => continue,
            };
            values.insert(i.id.clone(), v);
        }
        Self { values, problems: BTreeMap::new() }
    }

    /// The text of a text, number, slider or date input.
    pub fn text(&self, id: &str) -> &str {
        match self.values.get(id) {
            Some(DraftValue::Text(t)) => t,
            _ => "",
        }
    }

    pub fn flag(&self, id: &str) -> bool {
        matches!(self.values.get(id), Some(DraftValue::Bool(true)))
    }

    pub fn choice(&self, id: &str) -> Option<&str> {
        match self.values.get(id) {
            Some(DraftValue::Choice(c)) => c.as_deref(),
            _ => None,
        }
    }

    pub fn chosen(&self, id: &str, value: &str) -> bool {
        matches!(self.values.get(id), Some(DraftValue::Many(m)) if m.contains(value))
    }

    pub fn set(&mut self, id: &str, value: DraftValue) {
        self.problems.remove(id);
        self.problems.remove("");
        self.values.insert(id.to_owned(), value);
    }

    /// Ticks or unticks one choice of a multi-select.
    pub fn toggle_choice(&mut self, id: &str, value: &str) {
        let mut set = match self.values.remove(id) {
            Some(DraftValue::Many(m)) => m,
            _ => BTreeSet::new(),
        };
        if !set.remove(value) {
            set.insert(value.to_owned());
        }
        self.set(id, DraftValue::Many(set));
    }

    /// Takes the values a window sent with an action (WPF, Android): `{ id: value }` in wire
    /// shape. Unknown ids and ill-typed values are ignored; the answer checks the rest.
    pub fn from_wire(layout: &DialogNode, values: &Value) -> Self {
        let mut draft = Self::new(layout);
        let Some(map) = values.as_object() else { return draft };
        for node in dialog_tree::nodes(layout) {
            let Some(i) = input(node) else { continue };
            let Some(v) = map.get(&i.id) else { continue };
            let value = match (node, v) {
                (DialogNode::TextInput(_) | DialogNode::Date(_), Value::String(s)) => DraftValue::Text(s.clone()),
                (DialogNode::Number(_) | DialogNode::Slider(_), Value::Number(n)) => DraftValue::Text(n.to_string()),
                (DialogNode::Number(_) | DialogNode::Slider(_), Value::String(s)) => DraftValue::Text(s.clone()),
                (DialogNode::Checkbox(_) | DialogNode::Toggle(_), Value::Bool(b)) => DraftValue::Bool(*b),
                (DialogNode::Select(_) | DialogNode::Radio(_), Value::String(s)) => DraftValue::Choice(Some(s.clone())),
                (DialogNode::Select(_) | DialogNode::Radio(_), Value::Null) => DraftValue::Choice(None),
                (DialogNode::MultiSelect(_), Value::Array(a)) => DraftValue::Many(a.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
                _ => continue,
            };
            draft.values.insert(i.id.clone(), value);
        }
        draft
    }
}

/// Every button of the layout, in document order.
pub fn buttons(layout: &DialogNode) -> Vec<&DialogButton> {
    dialog_tree::nodes(layout)
        .into_iter()
        .filter_map(|n| match n {
            DialogNode::Button(b) => Some(b),
            _ => None,
        })
        .collect()
}

/// How a button is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Danger,
    Normal,
}

pub fn button_style(b: &DialogButton) -> ButtonStyle {
    match b.kind {
        Some(DialogButtonKind::Primary) => ButtonStyle::Primary,
        Some(DialogButtonKind::Danger) => ButtonStyle::Danger,
        _ => ButtonStyle::Normal,
    }
}

/// The wire name of a button's style, for the view frames.
pub fn button_style_name(b: &DialogButton) -> &'static str {
    match button_style(b) {
        ButtonStyle::Primary => "primary",
        ButtonStyle::Danger => "danger",
        ButtonStyle::Normal => "normal",
    }
}

/// "From deployer on devbox". The source is stamped by `yaacd` from the channel; it is never an
/// agent or the user (invariant 13).
pub fn source_line(d: &DialogRequest, target_name: &str) -> String {
    format!("From {} on {target_name}", d.source.name)
}

/// "deployer is not connected · your answer will be delivered when it reconnects", while the
/// integrator's channel is closed. The dialog stays answerable: `yaacd` records the answer and
/// delivers it when the integrator resumes its channel.
pub fn gone_line(d: &DialogRequest) -> Option<String> {
    (!d.channel_open).then(|| format!("{} is not connected · your answer will be delivered when it reconnects", d.source.name))
}

/// Whether the dialog's buttons may be pressed. Always, while it is open — also while its
/// integrator is away (the answer waits for it). Kept as one place for the renderers to ask.
pub fn answerable(_d: &DialogRequest) -> bool {
    true
}

fn label_of(node: &DialogNode) -> String {
    input(node).map(|i| i.label.clone().unwrap_or_else(|| i.id.clone())).unwrap_or_default()
}

fn present(v: &Value) -> bool {
    !matches!(v, Value::Null) && v.as_str() != Some("") && v.as_array().is_none_or(|a| !a.is_empty())
}

fn parse_number(s: &str) -> Option<Number> {
    let t = s.trim();
    if let Ok(i) = t.parse::<i64>() {
        return Some(Number::from(i));
    }
    t.parse::<f64>().ok().filter(|f| f.is_finite()).and_then(Number::from_f64)
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !s.chars().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()) {
        return false;
    }
    let (y, m, d) = (s[..4].parse::<u32>().unwrap_or(0), s[5..7].parse::<u32>().unwrap_or(0), s[8..].parse::<u32>().unwrap_or(0));
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

fn in_range(n: &Number, min: Option<&Number>, max: Option<&Number>) -> bool {
    let v = n.as_f64().unwrap_or(f64::NAN);
    min.is_none_or(|m| v >= m.as_f64().unwrap_or(f64::MIN)) && max.is_none_or(|m| v <= m.as_f64().unwrap_or(f64::MAX))
}

/// The value of one input in wire shape, or a problem. `None`: nothing entered.
fn wire_value(node: &DialogNode, draft: &DialogDraft) -> Result<Option<Value>, String> {
    let Some(i) = input(node) else { return Ok(None) };
    let id = i.id.as_str();
    Ok(match node {
        DialogNode::TextInput(t) => {
            let s = draft.text(id);
            if t.max_length.is_some_and(|m| s.chars().count() > m as usize) {
                return Err(format!("At most {} characters", t.max_length.unwrap_or(0)));
            }
            (!s.is_empty()).then(|| Value::String(s.to_owned()))
        }
        DialogNode::Number(n) => match draft.text(id).trim() {
            "" => None,
            s => {
                let v = parse_number(s).ok_or("Enter a number")?;
                if !in_range(&v, n.min.as_ref(), n.max.as_ref()) {
                    return Err(range_text(n.min.as_ref(), n.max.as_ref()));
                }
                Some(Value::Number(v))
            }
        },
        DialogNode::Slider(s) => {
            let v = parse_number(draft.text(id)).unwrap_or_else(|| s.min.clone());
            if !in_range(&v, Some(&s.min), Some(&s.max)) {
                return Err(range_text(Some(&s.min), Some(&s.max)));
            }
            Some(Value::Number(v))
        }
        DialogNode::Date(_) => match draft.text(id).trim() {
            "" => None,
            s if valid_date(s) => Some(Value::String(s.to_owned())),
            _ => return Err("Enter a date as YYYY-MM-DD".into()),
        },
        DialogNode::Checkbox(_) | DialogNode::Toggle(_) => Some(Value::Bool(draft.flag(id))),
        DialogNode::Select(s) | DialogNode::Radio(s) => draft.choice(id).filter(|c| s.options.iter().any(|o| o.value == *c)).map(|c| Value::String(c.to_owned())),
        DialogNode::MultiSelect(m) => Some(Value::Array(
            m.options.iter().filter(|o| draft.chosen(id, &o.value)).map(|o| Value::String(o.value.clone())).collect(),
        )),
        _ => None,
    })
}

fn range_text(min: Option<&Number>, max: Option<&Number>) -> String {
    match (min, max) {
        (Some(a), Some(b)) => format!("Between {a} and {b}"),
        (Some(a), None) => format!("At least {a}"),
        (None, Some(b)) => format!("At most {b}"),
        (None, None) => "Out of range".into(),
    }
}

/// The answer for pressing `button`, or the problems to show (by input id, `""` for the
/// dialog). A submit button checks every enabled required input; any button checks its own
/// `requires` and every value entered. An enabled required part this build does not know makes
/// a submitting button unusable here.
pub fn answer(layout: &DialogNode, draft: &DialogDraft, button: &str) -> Result<DialogAnswer, BTreeMap<String, String>> {
    let mut problems = BTreeMap::new();
    let Some(b) = buttons(layout).into_iter().find(|b| b.id == button) else {
        problems.insert(String::new(), "This button is not in the dialog".to_owned());
        return Err(problems);
    };
    let submit = b.submit != Some(false);
    let requires: BTreeSet<&str> = b.requires.iter().flatten().map(String::as_str).collect();
    let mut values = Map::new();
    for node in dialog_tree::nodes(layout) {
        if dialog_tree::unknown_required(node) && node_enabled(node) && submit {
            problems.insert(String::new(), format!("{NEWER_YAAC} to be answered"));
            continue;
        }
        let Some(i) = input(node) else { continue };
        if i.disabled == Some(true) {
            continue;
        }
        match wire_value(node, draft) {
            Err(p) => {
                problems.insert(i.id.clone(), p);
            }
            Ok(v) => {
                let needed = (submit && i.required == Some(true)) || requires.contains(i.id.as_str());
                if needed && !v.as_ref().is_some_and(present) {
                    problems.insert(i.id.clone(), format!("{} is required", label_of(node)));
                }
                if let Some(v) = v {
                    values.insert(i.id.clone(), v);
                }
            }
        }
    }
    if problems.is_empty() { Ok(DialogAnswer { action: Some(b.id.clone()), values, cancelled: false }) } else { Err(problems) }
}

fn node_enabled(node: &DialogNode) -> bool {
    match node {
        DialogNode::Unknown(u) => u.fields.get("disabled").and_then(Value::as_bool) != Some(true),
        _ => true,
    }
}

/// Closing the dialog: a cancellation (the integrator gets `cancelled`).
pub fn cancel() -> DialogAnswer {
    DialogAnswer { action: None, values: Map::new(), cancelled: true }
}

/// The response to send for an answer.
pub fn response(answer: DialogAnswer) -> RequestResponse {
    RequestResponse::Dialog(answer)
}

fn short(s: &str) -> String {
    yaac_protocol::attach::one_line(s, 40)
}

/// One line for an answer: the button, then each value shown (secrets masked, never shown).
pub fn summary(d: &DialogRequest, a: &DialogAnswer) -> String {
    if a.cancelled {
        return "Dismissed".to_owned();
    }
    let mut parts = Vec::new();
    let label = a.action.as_ref().and_then(|id| buttons(&d.layout).into_iter().find(|b| &b.id == id).map(|b| b.label.clone()));
    parts.push(label.or_else(|| a.action.clone()).unwrap_or_else(|| "Answered".to_owned()));
    for node in dialog_tree::nodes(&d.layout) {
        let Some(i) = input(node) else { continue };
        let name = i.label.clone().unwrap_or_else(|| i.id.clone());
        if is_secret(node) {
            if a.values.contains_key(&i.id) {
                parts.push(format!("{name}: {SECRET_MASK}"));
            }
            continue;
        }
        let Some(v) = a.values.get(&i.id) else { continue };
        let shown = match (node, v) {
            (DialogNode::Select(s) | DialogNode::Radio(s), Value::String(c)) => s.options.iter().find(|o| &o.value == c).map_or_else(|| c.clone(), |o| o.label.clone()),
            (DialogNode::MultiSelect(m), Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .map(|c| m.options.iter().find(|o| o.value == c).map_or(c, |o| o.label.as_str()).to_owned())
                .collect::<Vec<_>>()
                .join(", "),
            (_, Value::Bool(b)) => if *b { "yes" } else { "no" }.to_owned(),
            (_, Value::String(s)) => s.clone(),
            (_, other) => other.to_string(),
        };
        parts.push(format!("{name}: {}", short(&shown)));
    }
    parts.join(" · ")
}

/// The dialog of a request, when it is one.
pub fn of(r: &RequestItem) -> Option<&DialogRequest> {
    match &r.body {
        RequestBody::Dialog(d) => Some(d),
        _ => None,
    }
}

// ── Target inboxes ──────────────────────────────────────────────────────────────────────────

/// This window's answer to an inbox dialog, while the target has not confirmed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxAnswer {
    /// `Submitted` while in flight; `Unknown` when the connection dropped before the answer was
    /// confirmed (never re-sent, invariant 10).
    pub state: RequestState,
    /// The masked summary shown meanwhile.
    pub summary: String,
}

/// Every target's inbox, as the hub last told this window, with this window's answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inboxes {
    pub targets: BTreeMap<TargetId, TargetInbox>,
    pub answers: BTreeMap<(TargetId, RequestId), InboxAnswer>,
}

/// One open dialog of a target inbox, as a window lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxEntry<'a> {
    pub target: &'a TargetId,
    pub request: RequestItem,
    pub dialog: &'a DialogRequest,
}

impl Inboxes {
    /// Replaces one target's inbox; answers to requests it no longer holds are dropped (the
    /// target decided them), and an unknown outcome stays until the target says.
    pub fn replace(&mut self, inbox: TargetInbox) {
        let target = inbox.target.clone();
        self.answers.retain(|(t, id), _| t != &target || inbox.requests.iter().any(|r| &r.request_id == id));
        if inbox.requests.is_empty() && inbox.channels.is_empty() {
            self.targets.remove(&target);
        } else {
            self.targets.insert(target, inbox);
        }
    }

    /// The request as this window shows it: the target's state, overridden by this window's
    /// answer in flight (submitted, or unknown).
    pub fn request(&self, target: &TargetId, id: &RequestId) -> Option<RequestItem> {
        let mut r = self.targets.get(target)?.requests.iter().find(|r| &r.request_id == id)?.clone();
        if let Some(a) = self.answers.get(&(target.clone(), id.clone()))
            && r.state == RequestState::Pending
        {
            r.state = a.state.clone();
        }
        Some(r)
    }

    /// Every dialog that needs the user, oldest first per target: open, or with an unknown
    /// outcome (the user must check it). A submitted one is waiting on the target, not the user.
    pub fn needing_you(&self) -> Vec<InboxEntry<'_>> {
        let mut out = Vec::new();
        for (target, inbox) in &self.targets {
            for raw in &inbox.requests {
                let Some(dialog) = of(raw) else { continue };
                let Some(request) = self.request(target, &raw.request_id) else { continue };
                if matches!(request.state, RequestState::Pending | RequestState::Unknown) {
                    out.push(InboxEntry { target, request, dialog });
                }
            }
        }
        out
    }

    /// A channel of a target, by id.
    pub fn channel(&self, target: &TargetId, channel: &str) -> Option<&HubChannel> {
        self.targets.get(target)?.channels.iter().find(|c| c.channel_id == channel)
    }

    /// The local answer summary of an inbox request, when this window answered it.
    pub fn local_summary(&self, target: &TargetId, id: &RequestId) -> Option<&str> {
        self.answers.get(&(target.clone(), id.clone())).map(|a| a.summary.as_str())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use yaac_protocol::DialogSource;

    use super::*;

    fn layout() -> DialogNode {
        serde_json::from_value(json!({ "type": "column", "children": [
            { "type": "text", "text": "Pick a region" },
            { "type": "select", "id": "region", "label": "Region", "required": true,
              "options": [{ "value": "eu", "label": "Europe" }, { "value": "us", "label": "United States" }] },
            { "type": "textInput", "id": "token", "label": "Token", "secret": true },
            { "type": "number", "id": "count", "label": "Count", "min": 1, "max": 5, "default": 2 },
            { "type": "checkbox", "id": "dry", "label": "Dry run", "default": true },
            { "type": "multiSelect", "id": "tags", "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] },
            { "type": "date", "id": "when" },
            { "type": "sparkles", "id": "future" },
            { "type": "row", "children": [
                { "type": "button", "id": "go", "label": "Deploy", "kind": "primary" },
                { "type": "button", "id": "later", "label": "Later", "submit": false },
            ]},
        ]}))
        .unwrap()
    }

    fn request(layout: DialogNode, open: bool) -> DialogRequest {
        DialogRequest { title: "Deploy".into(), layout, source: DialogSource { channel_id: "c1".into(), name: "deployer".into() }, channel_open: open }
    }

    #[test]
    fn defaults_fill_the_draft_and_secrets_start_empty() {
        let d = DialogDraft::new(&layout());
        assert_eq!(d.text("count"), "2");
        assert!(d.flag("dry"));
        assert_eq!(d.choice("region"), None);
        assert_eq!(d.text("token"), "");
    }

    #[test]
    fn a_submit_button_checks_required_inputs_and_values() {
        let l = layout();
        let mut d = DialogDraft::new(&l);
        let problems = answer(&l, &d, "go").unwrap_err();
        assert_eq!(problems.get("region").map(String::as_str), Some("Region is required"));
        d.set("region", DraftValue::Choice(Some("eu".into())));
        d.set("count", DraftValue::Text("9".into()));
        d.set("when", DraftValue::Text("2026-02-30".into()));
        let problems = answer(&l, &d, "go").unwrap_err();
        assert_eq!(problems.get("count").map(String::as_str), Some("Between 1 and 5"));
        assert!(problems.contains_key("when"));
        d.set("count", DraftValue::Text("3".into()));
        d.set("when", DraftValue::Text("2026-02-28".into()));
        d.set("token", DraftValue::Text("hunter2".into()));
        d.toggle_choice("tags", "b");
        let a = answer(&l, &d, "go").unwrap();
        assert_eq!(a.action.as_deref(), Some("go"));
        assert_eq!(a.values["region"], json!("eu"));
        assert_eq!(a.values["count"], json!(3));
        assert_eq!(a.values["dry"], json!(true));
        assert_eq!(a.values["tags"], json!(["b"]));
        assert_eq!(a.values["token"], json!("hunter2"));
        assert!(!a.values.contains_key("future"), "unknown parts send nothing");
        // The summary names the button and masks the secret.
        let s = summary(&request(l, true), &a);
        assert!(s.starts_with("Deploy · Region: Europe"), "{s}");
        assert!(s.contains(&format!("Token: {SECRET_MASK}")));
        assert!(!s.contains("hunter2"));
    }

    #[test]
    fn a_non_submit_button_skips_requirements() {
        let l = layout();
        let a = answer(&l, &DialogDraft::new(&l), "later").unwrap();
        assert_eq!(a.action.as_deref(), Some("later"));
    }

    #[test]
    fn an_unknown_required_part_blocks_submitting_buttons_only() {
        let l: DialogNode = serde_json::from_value(json!({ "type": "column", "children": [
            { "type": "sparkles", "id": "future", "required": true },
            { "type": "button", "id": "go", "label": "Go" },
            { "type": "button", "id": "skip", "label": "Skip", "submit": false },
        ]}))
        .unwrap();
        let d = DialogDraft::new(&l);
        assert!(answer(&l, &d, "go").unwrap_err().contains_key(""));
        assert!(answer(&l, &d, "skip").is_ok());
    }

    #[test]
    fn values_from_a_window_are_typed_by_the_layout() {
        let l = layout();
        let d = DialogDraft::from_wire(&l, &json!({ "region": "us", "count": 4, "dry": false, "tags": ["a"], "token": "x", "bogus": 1 }));
        let a = answer(&l, &d, "go").unwrap();
        assert_eq!(a.values["region"], json!("us"));
        assert_eq!(a.values["count"], json!(4));
        assert_eq!(a.values["dry"], json!(false));
        assert!(!a.values.contains_key("bogus"));
    }

    #[test]
    fn the_source_line_and_a_gone_integrator() {
        let r = request(layout(), false);
        assert_eq!(source_line(&r, "devbox"), "From deployer on devbox");
        assert_eq!(gone_line(&r).as_deref(), Some("deployer is not connected · your answer will be delivered when it reconnects"));
        assert!(answerable(&r), "the answer waits for the integrator");
        assert!(gone_line(&request(layout(), true)).is_none());
        assert_eq!(summary(&r, &cancel()), "Dismissed");
    }
}
