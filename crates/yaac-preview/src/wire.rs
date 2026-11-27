//! Private controller/browser-host pipe protocol. Coordinates are CEF device-independent pixels.
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub tab: u64,
    pub action: Action,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Action {
    Open {
        url: String,
        socks: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot: Option<std::path::PathBuf>,
    },
    Close,
    Shutdown,
    OpenCall,
    CallAnswer(String),
    CallMute(bool),
    Navigate(String),
    Back,
    Forward,
    Reload,
    Stop,
    Bounds {
        width: i32,
        height: i32,
        scale: f32,
    },
    Hidden(bool),
    Focus(bool),
    Inspect {
        title: String,
        at: Option<[i32; 2]>,
    },
    Pointer {
        x: i32,
        y: i32,
        flags: u32,
        kind: Pointer,
    },
    Key {
        vk: i32,
        flags: u32,
        pressed: bool,
    },
    Text(String),
    Compose(String),
    Commit(String),
    CancelComposition,
    Invalidate,
    FileDialog(Option<Vec<String>>),
    Consumed {
        popup: bool,
    },
    Menu(Option<i32>),
    Dialog {
        accept: bool,
        text: String,
    },
    DevTools {
        id: i32,
        method: String,
        params: serde_json::Value,
    },
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Pointer {
    Move,
    Leave,
    Button { button: u8, up: bool, count: i32 },
    Wheel(i32, i32),
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Notice {
    pub tab: u64,
    pub event: Event,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Event {
    Paint {
        popup: bool,
        size: [usize; 2],
        dirty: Vec<[usize; 4]>,
    },
    Popup(Option<[i32; 4]>),
    Favicon(Vec<u8>),
    FileDialog {
        mode: u32,
        title: String,
        default_path: String,
        filters: Vec<String>,
    },
    Url(String),
    ExternalLink(String),
    Title(String),
    Loading {
        busy: bool,
        back: bool,
        forward: bool,
    },
    Cursor(u32),
    CustomCursor {
        rgba: Vec<u8>,
        size: [u16; 2],
        hotspot: [u16; 2],
    },
    CompositionCaret([i32; 4]),
    DismissDialog,
    DismissMenu,
    Menu {
        at: [i32; 2],
        items: Vec<(i32, String, bool)>,
    },
    Dialog {
        kind: u32,
        message: String,
        text: String,
    },
    DevTools(serde_json::Value),
    Call(CallEvent),
    Error(String),
    Closed,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CallEvent {
    Offer(String),
    Ready,
    Failed(String),
}

/// Proxy preference for a tab's request context (P323): every host, loopback included, goes
/// through the hub's SOCKS proxy for the thread's target, which resolves names there.
/// `<-loopback>` removes Chromium's implicit loopback bypass; no other entry, so nothing is
/// fetched DIRECT from the host.
pub fn proxy_settings(socks: u16) -> [(&'static str, String); 3] {
    [
        ("mode", "fixed_servers".into()),
        ("server", format!("socks5://127.0.0.1:{socks}")),
        ("bypass_list", "<-loopback>".into()),
    ]
}
/// WebRTC must not open UDP sockets that bypass the proxy from the host (P323).
pub const WEBRTC_POLICY: (&str, &str) = ("webrtc.ip_handling_policy", "disable_non_proxied_udp");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_host_goes_through_the_target_proxy() {
        let settings = proxy_settings(4321);
        assert_eq!(
            settings,
            [
                ("mode", "fixed_servers".to_owned()),
                ("server", "socks5://127.0.0.1:4321".to_owned()),
                ("bypass_list", "<-loopback>".to_owned())
            ]
        );
        // No bypass entry other than removing the implicit loopback bypass.
        let bypass = &settings[2].1;
        assert!(
            bypass
                .split([';', ','])
                .all(|rule| rule.trim() == "<-loopback>"),
            "{bypass}"
        );
        assert_eq!(WEBRTC_POLICY.1, "disable_non_proxied_udp");
    }
}
