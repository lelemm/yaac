//! Native reminders for an unpackaged desktop application with a registered Start menu AUMID.

#[derive(Clone, Copy, Debug)]
pub enum ToastSound {
    Reminder,
    Default,
}

#[derive(Debug)]
pub struct RequestToast {
    pub title: String,
    pub body: String,
    pub revision: u64,
    pub sound: Option<ToastSound>,
    pub decisions: bool,
}

/// Keeps the same toast tag and group for every replacement. Each set of native buttons
/// carries the revision it displayed, including callbacks already in flight during Show.
pub fn request_toast(
    app_id: &str,
    tag: &str,
    mut n: RequestToast,
    activate: impl Fn(u64, &str) + Send + Sync + 'static,
    mut next_change: impl FnMut() -> Option<RequestToast>,
) -> windows::core::Result<()> {
    use windows::{
        Data::Xml::Dom::XmlDocument,
        Foundation::TypedEventHandler,
        UI::Notifications::{
            NotificationData, ToastActivatedEventArgs, ToastDismissalReason,
            ToastDismissedEventArgs, ToastNotification, ToastNotificationManager,
        },
        core::{HSTRING, IInspectable, Interface},
    };
    let tag = HSTRING::from(tag);
    let group = HSTRING::from("Requests");
    let app_id = HSTRING::from(app_id);
    // Windows creates some notification database rows lazily on first Show for an
    // AUMID. Concurrent first posts can otherwise fail with SQLITE_CONSTRAINT_UNIQUE.
    static POST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let context = |stage: &'static str| {
        move |e: windows::core::Error| windows::core::Error::new(e.code(), format!("{stage}: {e}"))
    };
    let activate = std::sync::Arc::new(activate);
    let data = |title: &str, body: &str| -> windows::core::Result<NotificationData> {
        let d = NotificationData::new()?;
        let values = d.Values()?;
        values.Insert(&HSTRING::from("heading"), &HSTRING::from(title))?;
        values.Insert(&HSTRING::from("detail"), &HSTRING::from(body))?;
        Ok(d)
    };
    loop {
        let audio = match n.sound {
            None => r#"<audio silent="true"/>"#,
            Some(ToastSound::Reminder) => {
                r#"<audio src="ms-winsoundevent:Notification.Reminder" loop="false"/>"#
            }
            Some(ToastSound::Default) => {
                r#"<audio src="ms-winsoundevent:Notification.Default" loop="false"/>"#
            }
        };
        let buttons = if n.decisions {
            r#"<action content="Allow once" arguments="allow"/><action content="Deny" arguments="deny"/>"#
        } else {
            ""
        };
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(format!(r#"<toast scenario="reminder" launch="default"><visual><binding template="ToastGeneric"><text>{{heading}}</text><text>{{detail}}</text></binding></visual>{audio}<actions><action content="Open" arguments="default"/>{buttons}</actions></toast>"#)))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        toast.SetTag(&tag)?;
        toast.SetGroup(&group)?;
        toast.SetData(&data(&n.title, &n.body)?)?;
        let clicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let revision = n.revision;
        let on_dismiss = activate.clone();
        let dismissed = clicked.clone();
        let dismiss_handler =
            TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(move |_, args| {
                if let Some(args) = args.as_ref()
                    && args.Reason()? == ToastDismissalReason::UserCanceled
                    && !dismissed.swap(true, std::sync::atomic::Ordering::AcqRel)
                {
                    on_dismiss(revision, "dismiss");
                }
                Ok(())
            });
        let dismiss_token = toast.Dismissed(&dismiss_handler)?;
        let activate = activate.clone();
        let handler = TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let action = args
                    .cast::<ToastActivatedEventArgs>()?
                    .Arguments()?
                    .to_string_lossy();
                if !clicked.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    if action == "default" {
                        crate::win::allow_window_foreground();
                    }
                    activate(revision, &action);
                }
            }
            Ok(())
        });
        let token = toast.Activated(&handler)?;
        let notifier = {
            let _posting = POST.lock().unwrap_or_else(|e| e.into_inner());
            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app_id)
                .map_err(context("create toast notifier"))?;
            notifier.Show(&toast).map_err(context("show reminder"))?;
            notifier
        };
        let next = next_change();
        toast.RemoveActivated(token)?;
        toast.RemoveDismissed(dismiss_token)?;
        if let Some(next) = next {
            n = next;
        } else {
            let hidden = notifier.Hide(&toast);
            ToastNotificationManager::History()?
                .RemoveGroupedTagWithId(&tag, &group, &app_id)
                .map_err(context("remove reminder from history"))?;
            return hidden.map_err(context("hide reminder"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{UI::Notifications::ToastNotificationManager, core::HSTRING};

    #[test]
    #[ignore = "requires an isolated, registered test AppUserModelID on Windows"]
    fn same_native_reminder_switches_between_single_and_summary() {
        let id = std::env::var("YAAC_TEST_TOAST_ID").unwrap();
        assert!(id.starts_with("YAAC.P355Test."));
        let app = HSTRING::from(&id);
        let tag = "p355-test";
        let history = ToastNotificationManager::History().unwrap();
        let make = |count| RequestToast {
            revision: count,
            title: format!("{count} pending"),
            body: "Isolated check".into(),
            decisions: count == 1,
            sound: None,
        };
        let mut counts = [2, 3, 2, 1].into_iter();
        let mut current = 1;
        request_toast(
            &id,
            tag,
            make(current),
            |_, _| {},
            || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
                loop {
                    let entries = history.GetHistoryWithId(&app).unwrap();
                    assert!(entries.Size().unwrap() <= 1, "native reminders accumulated");
                    if let Ok(entry) = entries.GetAt(0) {
                        assert_eq!(entry.Tag().unwrap(), tag);
                        assert_eq!(entry.Group().unwrap(), "Requests");
                        let actions = entry
                            .Content()
                            .unwrap()
                            .GetElementsByTagName(&HSTRING::from("action"))
                            .unwrap();
                        let title = entry
                            .Data()
                            .unwrap()
                            .Values()
                            .unwrap()
                            .Lookup(&HSTRING::from("heading"))
                            .unwrap();
                        if title == format!("{current} pending").as_str() {
                            assert_eq!(actions.Length().unwrap(), if current == 1 { 3 } else { 1 });
                            break;
                        }
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "native history did not update"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                let count = counts.next()?;
                current = count;
                Some(make(count))
            },
        )
        .unwrap();
        assert_eq!(history.GetHistoryWithId(&app).unwrap().Size().unwrap(), 0);
    }
}
