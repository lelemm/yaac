//! Walking a runtime dialog layout ([`crate::DialogNode`], docs/16-external-dialogs.md): its
//! inputs, its buttons, its secret inputs and the parts this build does not know. Shared by the
//! hub (redacting secrets before anything reaches the cache) and the window controller (drafts,
//! validation, rendering).

use crate::{DialogInput, DialogNode};

/// Every node of the tree, parents before children, in document order.
pub fn nodes(root: &DialogNode) -> Vec<&DialogNode> {
    let mut out = Vec::new();
    push(root, &mut out);
    out
}

fn push<'a>(node: &'a DialogNode, out: &mut Vec<&'a DialogNode>) {
    out.push(node);
    for child in children(node) {
        push(child, out);
    }
}

/// A container's children; empty for every other node.
pub fn children(node: &DialogNode) -> &[DialogNode] {
    match node {
        DialogNode::Column(c) | DialogNode::Row(c) => &c.children,
        DialogNode::Group(g) => &g.children,
        _ => &[],
    }
}

/// The common input properties of an input node; `None` for containers, content and buttons.
pub fn input(node: &DialogNode) -> Option<&DialogInput> {
    Some(match node {
        DialogNode::TextInput(n) => &n.input,
        DialogNode::Number(n) => &n.input,
        DialogNode::Checkbox(n) | DialogNode::Toggle(n) => &n.input,
        DialogNode::Select(n) | DialogNode::Radio(n) => &n.input,
        DialogNode::MultiSelect(n) => &n.input,
        DialogNode::Date(n) => &n.input,
        DialogNode::Slider(n) => &n.input,
        _ => return None,
    })
}

/// Whether the node is a secret text input: its value is masked while typed, never shown in a
/// summary, and never written to the host.
pub fn is_secret(node: &DialogNode) -> bool {
    matches!(node, DialogNode::TextInput(t) if t.secret == Some(true))
}

/// The ids of every secret input.
pub fn secret_ids(root: &DialogNode) -> Vec<String> {
    nodes(root).into_iter().filter(|n| is_secret(n)).filter_map(|n| input(n).map(|i| i.id.clone())).collect()
}

/// The `type` of a node this build does not know (a newer integrator), when it is one.
pub fn unknown_kind(node: &DialogNode) -> Option<&str> {
    match node {
        DialogNode::Unknown(u) => Some(u.tag("type")),
        _ => None,
    }
}

/// Whether an unknown node is itself a required input: then the dialog cannot be answered here
/// with any button that validates (an unknown optional part is only shown as a muted line).
pub fn unknown_required(node: &DialogNode) -> bool {
    match node {
        DialogNode::Unknown(u) => u.fields.get("required").and_then(serde_json::Value::as_bool) == Some(true),
        _ => false,
    }
}

/// Removes the values of secret inputs from an answer's values (by id).
pub fn redact(root: &DialogNode, values: &mut serde_json::Map<String, serde_json::Value>) {
    for id in secret_ids(root) {
        values.remove(&id);
    }
}
