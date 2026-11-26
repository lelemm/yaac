//! Window-local support form shared by both renderers.

use std::time::{Duration, Instant};

use yaac_core::{client::Client, state::AppState};
use yaac_protocol::rpc::RpcId;
use yaac_protocol::{Capability, ProviderStatus, TargetId, TroubleshootParams};

pub(crate) const GUIDANCE: &str = "Describe the problem and how to reproduce it. YAAC sends the filtered symptom and public source excerpts to the selected harness. Review the report before sharing.";
pub(crate) const UNAVAILABLE: &str =
    "Connect a ready harness offering GPT-6.2 Sol or Opus 5.5 with medium effort.";
pub(crate) const MAX_SYMPTOM: usize = 2000;

pub(crate) struct Choice {
    target: TargetId,
    params: TroubleshootParams,
    pub label: String,
}

pub(crate) fn choice(state: &AppState) -> Option<Choice> {
    let mut selected = None;
    for target in state.targets.iter().filter(|t| t.connected()) {
        for provider in target.providers.iter().filter(|p| {
            p.status == ProviderStatus::Ready && p.capabilities.contains(&Capability::Troubleshoot)
        }) {
            for model in provider.models.iter().filter(|m| !m.hidden) {
                let Some(effort) = model.efforts.iter().find(|e| {
                    e.id.as_str().eq_ignore_ascii_case("medium")
                        || e.display_name
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case("medium"))
                }) else {
                    continue;
                };
                let Some(priority) = [
                    Some(model.display_name.as_str()),
                    Some(model.id.as_str()),
                    model.resolved_model.as_ref().map(|id| id.as_str()),
                ]
                .into_iter()
                .flatten()
                .filter_map(preference)
                .min() else {
                    continue;
                };
                if selected
                    .as_ref()
                    .is_some_and(|(current, _)| *current <= priority)
                {
                    continue;
                }
                selected = Some((
                    priority,
                    Choice {
                        target: target.entry.id.clone(),
                        label: format!(
                            "{} · {} · {}",
                            model.display_name,
                            effort.display_name.as_deref().unwrap_or(effort.id.as_str()),
                            target.entry.name
                        ),
                        params: TroubleshootParams {
                            instance: provider.instance.clone(),
                            model: model.id.clone(),
                            effort: effort.id.clone(),
                            symptom: String::new(),
                            environment: String::new(),
                            build: crate::BUILD.to_owned(),
                        },
                    },
                ));
            }
        }
    }
    selected.map(|(_, choice)| choice)
}

// P368 uses the requested names to rank this feature's candidates. Requests use the
// model and effort IDs actually advertised by the chosen harness.
fn preference(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase().replace([' ', '_'], "-");
    if name == "gpt-6.2-sol" || name.starts_with("gpt-6.2-sol-") {
        Some(0)
    } else if name.split('-').any(|part| part == "opus")
        && (name.contains("5.5") || name.contains("5-5"))
    {
        Some(1)
    } else {
        None
    }
}

#[derive(Default)]
pub(crate) struct Troubleshoot {
    pub open: bool,
    pub symptom: String,
    pub report: String,
    pub report_version: u64,
    pub error: Option<String>,
    pending: Option<(RpcId, Instant, String)>,
}

impl Troubleshoot {
    pub(crate) fn pending_label(&self) -> Option<&str> {
        self.pending.as_ref().map(|(_, _, label)| label.as_str())
    }

    pub(crate) fn start(&mut self, client: &mut Client, renderer: &str, host_windows: bool) {
        if self.pending.is_some() {
            return;
        }
        let symptom = self.symptom.trim();
        if symptom.is_empty() || symptom.chars().count() > MAX_SYMPTOM {
            self.error = Some(format!(
                "Describe the symptom in up to {MAX_SYMPTOM} characters."
            ));
            return;
        }
        let Some(mut selection) = choice(&client.state) else {
            self.error = Some(UNAVAILABLE.into());
            return;
        };
        let os = if host_windows {
            "Windows"
        } else {
            std::env::consts::OS
        };
        selection.params.symptom = symptom.to_owned();
        selection.params.environment = format!("{os} {renderer}");
        let id = client.troubleshoot(&selection.target, selection.params);
        self.pending = Some((
            id,
            Instant::now() + Duration::from_secs(165),
            selection.label,
        ));
        self.error = None;
    }

    pub(crate) fn pump(&mut self, client: &mut Client, ctx: &eframe::egui::Context) {
        if let Some((id, response)) = client.take_troubleshoot_reply()
            && self
                .pending
                .as_ref()
                .is_some_and(|(expected, _, _)| *expected == id)
        {
            self.pending = None;
            match response {
                Ok(result) => {
                    self.report = format!(
                        "YAAC {}\nSource revision {}\n\n{}",
                        crate::BUILD,
                        result.revision,
                        result.report
                    );
                    self.report_version += 1;
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some((_, deadline, _)) = &self.pending {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.pending = None;
                self.error = Some(
                    "The diagnosis timed out. Check the target connection and try again.".into(),
                );
            } else {
                ctx.request_repaint_after(left);
            }
        }
    }
}
