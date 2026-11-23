use super::*;
use yaac_core::pr::{self, BadgeAction, BadgeClick, BadgeHint};
use yaac_core::state::AppState;
use yaac_protocol::ChecksState;

pub(super) fn badge(state: &AppState, key: &ThreadKey) -> Option<Value> {
    let badge = pr::badge_of(state, key)?;
    let primary = badge.primary.as_ref()?;
    let hint = match badge.hint {
        Some(BadgeHint::Draft) => "draft",
        Some(BadgeHint::Merged) => "merged",
        Some(BadgeHint::Closed) => "closed",
        None => "",
    };
    let checks = match badge.checks {
        Some(ChecksState::Failing) => "failing",
        Some(ChecksState::Pending) => "pending",
        _ => "",
    };
    let others: Vec<_> = badge
        .others
        .iter()
        .map(|pr| {
            json!({ "number": pr.number, "label": format!("{} · {}", pr::describe(pr), pr.title),
            "enabled": !pr.url.is_empty() })
        })
        .collect();
    let actions: Vec<_> = pr::badge_menu(&badge)
        .into_iter()
        .map(|item| {
            let id = match item.action {
                BadgeAction::Create => "create",
                BadgeAction::CopyLink => "copy",
            };
            json!({ "id": id, "label": item.label, "enabled": item.enabled, "detail": item.reason })
        })
        .collect();
    Some(json!({
        "number": primary.number, "hint": hint, "checks": checks, "detail": badge.tooltip(),
        "enabled": matches!(pr::click(&badge), BadgeClick::Open(_)),
        "more": badge.more_text(), "others": others, "actions": actions,
    }))
}

pub(super) fn action(app: &YaacApp, ctx: &egui::Context, payload: &Value) -> Option<Action> {
    let key = key(app, string(payload, "id"))?;
    let badge = pr::badge_of(&app.client.state, &key)?;
    let number = payload["number"].as_u64();
    let action = string(payload, "action");
    if action == "openOther" && badge.enabled {
        let pr = badge.others.iter().find(|pr| Some(pr.number) == number)?;
        return (!pr.url.is_empty()).then(|| Action::OpenUrl(pr.url.clone()));
    }
    let primary = badge.primary.as_ref()?;
    if Some(primary.number) != number {
        return None;
    }
    match action {
        "open" => match pr::click(&badge) {
            BadgeClick::Open(url) => Some(Action::OpenUrl(url)),
            _ => None,
        },
        "refresh" => Some(Action::PrRefresh(key)),
        "create" | "copy" => {
            let command = if action == "create" {
                BadgeAction::Create
            } else {
                BadgeAction::CopyLink
            };
            pr::badge_menu(&badge)
                .into_iter()
                .find(|item| item.action == command && item.enabled)?;
            if command == BadgeAction::Create {
                Some(Action::OpenCreatePr(key))
            } else {
                ctx.copy_text(primary.url.clone());
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
#[test]
fn wpf_pr_badge_shows_state_and_targets_the_selected_number() {
    use yaac_core::demo::threads;

    let ctx = egui::Context::default();
    let mut app = crate::app::tests::demo_app(&ctx);
    let billing = threads::billing();
    let users = threads::users();
    let now = app.now();
    app.client
        .forge_visible(&[billing.clone(), users.clone()], now);
    app.pump_demo();

    let state = badge(&app.client.state, &billing).unwrap();
    assert_eq!(state["number"], 33343);
    assert_eq!(state["hint"], "draft");
    assert_eq!(state["checks"], "pending");
    assert_eq!(state["more"], "+1");
    assert_eq!(state["others"][0]["number"], 33301);
    assert!(
        state["others"][0]["label"]
            .as_str()
            .unwrap()
            .starts_with("#33301 · closed")
    );
    assert_eq!(state["actions"][0]["id"], "create");
    assert_eq!(state["actions"][1]["id"], "copy");
    assert_eq!(
        badge(&app.client.state, &users).unwrap()["checks"],
        "failing"
    );

    let invoke = |name, number| {
        action(
            &app,
            &ctx,
            &json!({
                "id": billing.address(), "action": name, "number": number,
            }),
        )
    };
    assert!(matches!(invoke("open", 33343), Some(Action::OpenUrl(url)) if url.ends_with("/33343")));
    assert!(
        matches!(invoke("openOther", 33301), Some(Action::OpenUrl(url)) if url.ends_with("/33301"))
    );
    assert!(matches!(invoke("create", 33343), Some(Action::OpenCreatePr(key)) if key == billing));
    assert!(invoke("open", 33301).is_none());
    assert!(invoke("openOther", 33342).is_none());
    assert!(invoke("copy", 33343).is_none());
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
    assert!(output.platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(url) if url.ends_with("/33343"))
    ));
}
