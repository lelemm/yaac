//! Request notifications. Dropping the withdrawal sender closes the native notification.
pub type Updates = tokio::sync::mpsc::UnboundedReceiver<Notification>;
use yaac_protocol::attach::{NotificationSettings, NotificationSound};

#[derive(Clone, Debug)]
pub struct Notification {
    pub revision: u64,
    pub play_sound: bool,
    pub title: String,
    pub body: String,
    pub decisions: bool,
    pub settings: NotificationSettings,
}

pub type Activate = Box<dyn Fn(u64, &str) + Send + Sync>;
pub trait Notifier: Send + Sync {
    /// Start the attached window after a user presses Open while it is closed.
    fn launch_window(&self);
    fn show(&self, notice: Notification, activate: Activate, withdraw: Updates);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn launch_window(&self) {
        std::thread::spawn(|| {
            let child =
                std::env::current_exe().and_then(|path| std::process::Command::new(path).spawn());
            match child {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(e) => eprintln!("YAAC: cannot open the request window: {e}"),
            }
        });
    }

    fn show(&self, notice: Notification, activate: Activate, withdraw: Updates) {
        #[cfg(target_os = "linux")]
        tokio::spawn(linux(notice, activate, withdraw));
        #[cfg(windows)]
        std::thread::spawn(move || {
            if let Err(e) = toast("YAAC.Desktop", notice, activate, withdraw) {
                eprintln!("YAAC notification: {e}");
            }
        });
    }
}

#[cfg(target_os = "linux")]
async fn linux(mut notice: Notification, activate: Activate, mut updates: Updates) {
    use notify_rust::{ActionResponse, Hint, Timeout, Urgency};
    let mut replace = None;
    loop {
        let mut n = notify_rust::Notification::new();
        n.appname("YAAC")
            .summary(&notice.title)
            .body(&escape(&notice.body))
            .urgency(Urgency::Critical)
            .timeout(Timeout::Never)
            .action("default", "Open")
            .action(&format!("{}/open", notice.revision), "Open");
        if notice.decisions {
            n.action(&format!("{}/allow", notice.revision), "Allow once")
                .action(&format!("{}/deny", notice.revision), "Deny");
        }
        if notice.play_sound {
            n.sound_name(match notice.settings.sound_choice {
                NotificationSound::Reminder => "dialog-warning",
                NotificationSound::Default => "message-new-instant",
            });
        } else {
            n.hint(Hint::SuppressSound(true));
        }
        if let Some(id) = replace {
            n.id(id);
        }
        let Ok(handle) = n.show_async().await else {
            return;
        };
        replace = Some(handle.id());
        let change = loop {
            let mut action = None;
            tokio::select! {
                next = updates.recv() => break next,
                _ = handle.wait_for_action_async(|response| match response {
                    ActionResponse::Custom("default") => action = Some("default".to_owned()),
                    ActionResponse::Custom(id) => {
                        if let Some((rev, button)) = id.split_once('/')
                            && rev.parse::<u64>().ok() == Some(notice.revision) {
                            action = Some(if button == "open" { "default" } else { button }.to_owned());
                        }
                    }
                    ActionResponse::Closed(_) => action = Some("dismiss".to_owned()),
                }) => {
                    if let Some(button) = action {
                        activate(notice.revision, &button);
                        break updates.recv().await;
                    }
                }
            }
        };
        let Some(next) = change else {
            handle.close_async().await;
            return;
        };
        notice = next;
    }
}

#[cfg(target_os = "linux")]
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(windows)]
fn toast(
    app_id: &str,
    n: Notification,
    activate: Activate,
    mut withdraw: Updates,
) -> Result<(), String> {
    use yaac_winsys::{RequestToast, ToastSound, request_toast};
    let render = |n: Notification| RequestToast {
        title: n.title,
        body: n.body,
        revision: n.revision,
        decisions: n.decisions,
        sound: n.play_sound.then_some(match n.settings.sound_choice {
            NotificationSound::Reminder => ToastSound::Reminder,
            NotificationSound::Default => ToastSound::Default,
        }),
    };
    // Keep this identity for every replacement; a later, independent lifetime can never
    // be erased by a slow withdrawal from the previous one.
    let tag = crate::settings::new_id("");
    request_toast(app_id, &tag[..16], render(n), activate, || {
        withdraw.blocking_recv().map(render)
    })
    .map_err(|e| e.to_string())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// Run only on a desktop in an isolated process with a separately registered test AUMID.
    #[test]
    #[ignore = "posts, updates, and withdraws real desktop reminders"]
    fn isolated_native_toast() {
        let id = std::env::var("YAAC_TEST_TOAST_ID").expect("set a registered test AppUserModelID");
        assert!(id.starts_with("YAAC.P355Test."));
        let notice = Notification {
            revision: 1,
            play_sound: false,
            title: "YAAC isolated approval".into(),
            body: "This temporary reminder closes automatically.".into(),
            decisions: true,
            settings: NotificationSettings {
                sound: false,
                ..Default::default()
            },
        };
        let (changes, receiver) = tokio::sync::mpsc::unbounded_channel();
        std::thread::scope(|threads| {
            let seed = notice.clone();
            threads.spawn(move || {
                for (i, count) in [2, 3, 2, 1].into_iter().enumerate() {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    changes
                        .send(Notification {
                            revision: i as u64 + 2,
                            decisions: count == 1,
                            title: if count == 1 {
                                "YAAC isolated approval".into()
                            } else {
                                format!("{count} requests need you")
                            },
                            ..seed.clone()
                        })
                        .unwrap();
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            });
            toast(&id, notice, Box::new(|_, _| {}), receiver).unwrap();
        });
    }
}
