//! The client's side of the rate-limit panel (D161): the hub's list of accounts, read once on
//! attaching and kept current by `hub/rateLimits/changed`; while the panel is drawn, a refresh
//! is asked at most every [`limits::REFRESH_SECS`] (the hub and `yaacd` hold their own limits
//! too). Never a poll of its own: the window calls [`Client::rate_limits_shown`] when it draws.

use serde_json::{Value, json};
use yaac_protocol::attach::{self, RateLimitsResult};

use super::{Call, Client};
use crate::limits;

impl Client {
    /// Reads the accounts (after attaching).
    pub(super) fn load_rate_limits(&mut self) {
        self.request(attach::names::RATE_LIMITS_GET, json!({ "refresh": false }), Call::RateLimits);
    }

    /// The hub's answer or notification.
    pub(super) fn on_rate_limits(&mut self, value: Value) {
        if let Ok(r) = serde_json::from_value::<RateLimitsResult>(value) {
            self.state.rate_limits.accounts = r.accounts;
        }
    }

    /// The panel is drawn at `now` (seconds): asks for fresh values when the last ask is
    /// [`limits::REFRESH_SECS`] old. Cheap to call every frame.
    pub fn rate_limits_shown(&mut self, now: i64) {
        if !self.state.hub.attached {
            return;
        }
        let asked = *self.state.rate_limits.asked.get_or_insert(now);
        if now - asked >= limits::REFRESH_SECS {
            self.state.rate_limits.asked = Some(now);
            self.request(attach::names::RATE_LIMITS_GET, json!({ "refresh": true }), Call::RateLimits);
        }
    }
}
