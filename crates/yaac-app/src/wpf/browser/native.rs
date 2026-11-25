use std::collections::HashMap;
use std::io::{self, Write};

use serde_json::{Value, json};
use yaac_core::ThreadKey;
use yaac_preview::wire::{Action, Event};
use yaac_preview::{Frame, Tab, Update};

use super::super::write_frame;
use crate::preview_controller::Browsers;

struct Surface {
    thread: ThreadKey,
    tab: Tab,
    cursor: u32,
    popup: Option<[i32; 4]>,
    favicon: Option<Vec<u8>>,
    prompt: Option<Event>,
    needs_full: [bool; 2],
    read_failed: [bool; 2],
}

struct Lease {
    tab: u64,
    frame: Frame,
}

#[derive(Default)]
pub(super) struct Surfaces {
    tabs: HashMap<u64, Surface>,
    visible: Option<u64>,
    leases: HashMap<u64, Lease>,
    next_frame: u64,
    /// DevTools replies for agent calls (docs/11-preview.md#agents).
    replies: Vec<(u64, Value)>,
    /// Tabs kept rendering, though not shown, because an agent drives them.
    awake: std::collections::HashSet<u64>,
}

pub(super) enum Output {
    Event(u64, Event),
    Frame(u64, Frame, bool),
}

impl Surfaces {
    pub(super) fn contains(&self, id: u64) -> bool {
        self.tabs.contains_key(&id)
    }

    pub(super) fn send(&self, id: u64, action: Action) {
        if let Some(surface) = self.tabs.get(&id) {
            surface.tab.send(action);
        }
    }

    pub(super) fn input(
        &self,
        thread: &ThreadKey,
        id: u64,
        data: &Value,
        shown: bool,
    ) -> Result<(), String> {
        let Some(surface) = self.tabs.get(&id).filter(|tab| &tab.thread == thread) else {
            return Ok(());
        };
        let action = frontend_action(data)?;
        let completes_request = matches!(
            action,
            Action::Focus(false)
                | Action::CancelComposition
                | Action::Menu(_)
                | Action::Dialog { .. }
                | Action::FileDialog(_)
        );
        if (shown && self.visible == Some(id)) || completes_request {
            surface.tab.send(action);
        }
        Ok(())
    }

    pub(super) fn ack(&mut self, tab: u64, id: u64, retry: bool) {
        if !self.leases.get(&id).is_some_and(|lease| lease.tab == tab) {
            return;
        }
        let lease = self.leases.remove(&id).expect("validated browser lease");
        let popup = usize::from(lease.frame.popup);
        drop(lease);
        if let Some(surface) = self.tabs.get_mut(&tab) {
            let repeated_failure = std::mem::replace(&mut surface.read_failed[popup], retry);
            if retry && !repeated_failure && self.visible == Some(tab) {
                // Request one fresh paint if the mapping disappeared during the read.
                surface.needs_full[popup] = true;
                surface.tab.send(Action::Invalidate);
            }
        }
    }

    pub(super) fn pump(
        &mut self,
        browsers: &mut Browsers,
        visible: Option<&ThreadKey>,
        wake: impl Fn() + Clone + Send + Sync + 'static,
    ) -> Vec<Output> {
        let active = visible
            .and_then(|key| browsers.areas.get(key))
            .filter(|area| !area.chat)
            .map(|area| area.active);
        self.tabs.retain(|id, surface| {
            browsers
                .areas
                .get(&surface.thread)
                .is_some_and(|area| area.tabs.iter().any(|tab| tab.id == *id))
        });

        let ready = browsers
            .areas
            .iter()
            .flat_map(|(key, area)| area.tabs.iter().map(move |tab| (key.clone(), tab.id)))
            .collect::<Vec<_>>();
        for (key, id) in ready {
            if let Some(tab) = browsers.open_ready(&key, id, wake.clone()) {
                if active != Some(id) {
                    tab.send(Action::Hidden(true));
                }
                self.tabs.insert(
                    id,
                    Surface {
                        thread: key,
                        tab,
                        cursor: 0,
                        popup: None,
                        favicon: None,
                        prompt: None,
                        needs_full: [true; 2],
                        read_failed: [false; 2],
                    },
                );
            }
        }

        let mut updates = Vec::new();
        if active != self.visible {
            if let Some(id) = self.visible {
                self.awake.remove(&id);
            }
            if let Some(surface) = self.visible.and_then(|id| self.tabs.get(&id)) {
                surface.tab.send(Action::CancelComposition);
                surface.tab.send(Action::Focus(false));
                surface.tab.send(Action::Hidden(true));
            }
            self.visible = active;
            if let Some(id) = active
                && let Some(surface) = self.tabs.get_mut(&id)
            {
                surface.needs_full = [true; 2];
                surface.read_failed = [false; 2];
                surface.tab.send(Action::Hidden(false));
                updates.push(Output::Event(id, Event::Cursor(surface.cursor)));
                updates.push(Output::Event(id, Event::Popup(surface.popup)));
                if let Some(icon) = surface.favicon.clone() {
                    updates.push(Output::Event(id, Event::Favicon(icon)));
                }
                if let Some(prompt) = surface.prompt.take() {
                    updates.push(Output::Event(id, prompt));
                }
            }
        }

        for (&id, surface) in &self.tabs {
            let agent = browsers
                .areas
                .get(&surface.thread)
                .and_then(|area| area.tabs.iter().find(|tab| tab.id == id))
                .is_some_and(|tab| tab.agent.is_some());
            let wake = agent && active != Some(id);
            if wake && self.awake.insert(id) {
                surface.tab.send(Action::Hidden(false));
            } else if !wake && self.awake.remove(&id) && active != Some(id) {
                surface.tab.send(Action::Hidden(true));
            }
        }
        self.awake.retain(|id| self.tabs.contains_key(id));

        let mut closed = Vec::new();
        for (&id, surface) in &mut self.tabs {
            let Some(tab) = browsers.tab_mut(&surface.thread, id) else {
                continue;
            };
            let foreground = active == Some(id);
            while let Some(update) = surface.tab.try_update() {
                match update {
                    Update::Frame(frame) if foreground => {
                        let full =
                            std::mem::take(&mut surface.needs_full[usize::from(frame.popup)]);
                        updates.push(Output::Frame(id, frame, full));
                    }
                    Update::Frame(_) => {}
                    Update::Event(Event::Url(url)) => tab.url = url,
                    Update::Event(Event::Title(title)) => tab.title = title,
                    Update::Event(Event::Loading {
                        busy,
                        back,
                        forward,
                    }) => {
                        tab.loading = busy;
                        tab.back = back;
                        tab.forward = forward;
                    }
                    Update::Event(Event::Error(error)) => {
                        tab.error = Some(error.clone());
                        tab.loading = false;
                        tab.back = false;
                        tab.forward = false;
                        browsers.host_error = Some(error);
                        closed.push(id);
                        break;
                    }
                    Update::Event(Event::Closed) => {
                        tab.error = Some("Browser tab closed".into());
                        tab.loading = false;
                        tab.back = false;
                        tab.forward = false;
                        closed.push(id);
                        break;
                    }
                    Update::Event(event) => {
                        match &event {
                            Event::Cursor(cursor) => surface.cursor = *cursor,
                            Event::Popup(popup) => {
                                surface.popup = *popup;
                                surface.needs_full[1] = true;
                            }
                            Event::Favicon(png) => surface.favicon = Some(png.clone()),
                            Event::Menu { .. } if !foreground => {
                                surface.tab.send(Action::Menu(None))
                            }
                            Event::FileDialog { .. } if !foreground => {
                                surface.tab.send(Action::FileDialog(None))
                            }
                            Event::Dialog { .. } if !foreground => {
                                surface.prompt = Some(event);
                                continue;
                            }
                            Event::DevTools(reply) => {
                                if reply.get("id").is_some() {
                                    self.replies.push((id, reply.clone()));
                                }
                                continue;
                            }
                            Event::Paint { .. } => continue,
                            _ => {}
                        }
                        if foreground {
                            updates.push(Output::Event(id, event));
                        }
                    }
                }
            }
        }
        for id in closed {
            self.tabs.remove(&id);
        }
        updates
    }

    pub(super) fn write(
        &mut self,
        updates: Vec<Output>,
        stdout: &mut impl Write,
    ) -> io::Result<()> {
        for update in updates {
            match update {
                Output::Event(tab, event) => write_frame(stdout, &event_message(tab, &event))?,
                Output::Frame(tab, frame, full) => {
                    self.next_frame += 1;
                    let id = self.next_frame;
                    let image = frame_message(
                        id,
                        tab,
                        frame.popup,
                        &frame.path,
                        frame.size,
                        &frame.dirty,
                        full,
                    );
                    self.leases.insert(id, Lease { tab, frame });
                    write_frame(stdout, &image)?;
                }
            }
        }
        Ok(())
    }
}

fn frontend_action(data: &Value) -> Result<Action, String> {
    let action = serde_json::from_value(data.clone()).map_err(|_| "Invalid browser input")?;
    match action {
        Action::Back
        | Action::Forward
        | Action::Reload
        | Action::Stop
        | Action::Bounds { .. }
        | Action::Focus(_)
        | Action::Pointer { .. }
        | Action::Key { .. }
        | Action::Text(_)
        | Action::Compose(_)
        | Action::Commit(_)
        | Action::CancelComposition
        | Action::Menu(_)
        | Action::Dialog { .. }
        | Action::FileDialog(_) => Ok(action),
        _ => Err("Invalid browser input".into()),
    }
}

fn event_message(tab: u64, event: &Event) -> Value {
    json!({ "browserEvent": { "tab": tab, "event": event } })
}

fn frame_message(
    id: u64,
    tab: u64,
    popup: bool,
    path: &std::path::Path,
    size: [usize; 2],
    dirty: &[[usize; 4]],
    full: bool,
) -> Value {
    let entire = [[0, 0, size[0], size[1]]];
    json!({
        "browserFrame": { "id": id, "tab": tab, "popup": popup,
            "path": path, "size": size, "dirty": if full { &entire[..] } else { dirty } },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaac_preview::wire::Pointer;

    #[test]
    fn browser_frame_uses_mmap_metadata_and_expands_the_first_dirty_patch() {
        let path = std::path::Path::new(r"C:\Users\dev\preview\7-1.bgra");
        for (full, dirty) in [
            (false, json!([[1, 2, 3, 4]])),
            (true, json!([[0, 0, 64, 48]])),
        ] {
            let value = frame_message(25, 7, true, path, [64, 48], &[[1, 2, 3, 4]], full);
            assert_eq!(
                value["browserFrame"],
                json!({
                    "id": 25, "tab": 7, "popup": true, "path": path,
                    "size": [64, 48], "dirty": dirty,
                })
            );
        }
    }

    #[test]
    fn wpf_actions_keep_device_independent_coordinates_and_ime_text() {
        let pointer = frontend_action(&json!({ "Pointer": {
            "x": 39, "y": 17, "flags": 4, "kind": { "Wheel": [0, -120] },
        }}))
        .unwrap();
        assert!(matches!(
            pointer,
            Action::Pointer {
                x: 39,
                y: 17,
                flags: 4,
                kind: Pointer::Wheel(0, -120)
            }
        ));
        assert!(
            matches!(frontend_action(&json!({"Commit": "日本語"})), Ok(Action::Commit(text)) if text == "日本語")
        );
        assert!(matches!(
            frontend_action(&json!({"Menu": null})),
            Ok(Action::Menu(None))
        ));
        assert!(frontend_action(&json!({"Navigate": "file:///not-allowed"})).is_err());
        assert!(frontend_action(&json!({"Consumed": {"popup": false}})).is_err());
    }

    #[test]
    fn serializes_browser_ui_dismissal_and_ime_caret() {
        assert_eq!(
            event_message(41, &Event::DismissMenu)["browserEvent"]["event"],
            "DismissMenu"
        );
        assert_eq!(
            event_message(41, &Event::DismissDialog)["browserEvent"]["event"],
            "DismissDialog"
        );
        assert_eq!(
            event_message(41, &Event::CompositionCaret([46, 31, 2, 18]))["browserEvent"]["event"],
            json!({ "CompositionCaret": [46, 31, 2, 18] }),
        );
    }

    #[test]
    fn delivers_menu_coordinates_and_dialog_type_with_logical_tab() {
        assert_eq!(
            event_message(
                41,
                &Event::Menu {
                    at: [13, 42],
                    items: vec![(100, "Copy".into(), true)]
                }
            ),
            json!({"browserEvent": {"tab": 41, "event": {"Menu": {"at": [13, 42], "items": [[100, "Copy", true]]}}}}),
        );
        assert_eq!(
            event_message(
                41,
                &Event::Dialog {
                    kind: 2,
                    message: "Name?".into(),
                    text: "Draft".into()
                }
            ),
            json!({"browserEvent": {"tab": 41, "event": {"Dialog": {"kind": 2, "message": "Name?", "text": "Draft"}}}}),
        );
    }
}

impl crate::preview_agent::Driver for Surfaces {
    fn engine(&mut self, id: u64) -> Option<&mut dyn crate::preview_agent::Engine> {
        self.tabs
            .get_mut(&id)
            .map(|surface| &mut surface.tab as &mut dyn crate::preview_agent::Engine)
    }

    fn take_replies(&mut self) -> Vec<(u64, Value)> {
        std::mem::take(&mut self.replies)
    }
}
