//! The core's side of rate limits (D161): asks each target whose providers have the
//! `rateLimits` capability, keeps their reports in a [`RateLimitBook`], and tells the windows
//! (`hub/rateLimits/changed`). A window that shows the panel asks with `refresh`; each target is
//! asked to read its harnesses again at most every [`REFRESH_EVERY`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::Value;
use yaac_protocol::attach::{self, RateLimitsGetParams};
use yaac_protocol::methods::ProviderRateLimits;
use yaac_protocol::rpc::ErrorObject;
use yaac_protocol::{Capability, InstanceRateLimits, ProviderDescriptor, ProviderRateLimitsParams, ProviderRateLimitsResult, TargetId};

use super::{Core, CoreEvent, Pending, to_value};
use crate::limits::{RateLimitBook, STORE_KEY};

/// The least time between two refreshes asked of one target.
pub(crate) const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);

/// What the core keeps about rate limits.
#[derive(Debug, Default)]
pub(crate) struct LimitsState {
    pub(crate) book: RateLimitBook,
    /// Targets with at least one `rateLimits` instance (from their provider list).
    capable: HashSet<TargetId>,
    /// When each target was last asked to refresh.
    asked: HashMap<TargetId, Instant>,
}

impl LimitsState {
    pub(crate) fn new(book: RateLimitBook) -> Self {
        Self { book, ..Self::default() }
    }
}

impl Core {
    fn limits_online(&self, id: &TargetId) -> bool {
        self.targets.get(id).is_some_and(|t| t.conn.as_ref().is_some_and(|c| c.ready))
    }

    fn limits_result(&self) -> Value {
        to_value(&self.limits.book.accounts(|id| self.limits_online(id)))
    }

    /// Tells the windows; `store`: the book changed (not only who is online).
    pub(super) fn limits_changed(&mut self, store: bool) {
        let result = self.limits_result();
        self.notify_all(attach::names::RATE_LIMITS_CHANGED, result);
        if store {
            let stored = self.limits.book.to_stored();
            self.worker.submit(move |s| s.cache.set_ui_state(STORE_KEY, &stored).err().map(|e| CoreEvent::StoreError(e.to_string())));
        }
    }

    /// A target's provider list arrived: ask for its rate limits when it has any.
    pub(super) fn limits_providers(&mut self, id: &TargetId, providers: &[ProviderDescriptor]) {
        if providers.iter().any(|p| p.capabilities.contains(&Capability::RateLimits)) {
            self.limits.capable.insert(id.clone());
            self.limits_ask(id, false);
        } else {
            self.limits.capable.remove(id);
        }
    }

    fn limits_ask(&mut self, id: &TargetId, refresh: bool) {
        if self.send::<ProviderRateLimits>(id, &ProviderRateLimitsParams { refresh }, Pending::RateLimits) {
            self.flush(id);
        }
    }

    /// The target's answer to `provider/rateLimits`.
    pub(super) fn limits_answered(&mut self, id: &TargetId, outcome: Result<Value, ErrorObject>) {
        let Ok(result) = outcome.and_then(|w| self.decode_typed::<ProviderRateLimits>(id, w)) else { return };
        let ProviderRateLimitsResult { instances } = result;
        if self.limits.book.replace_target(id, instances) {
            self.limits_changed(true);
        }
    }

    /// `provider/rateLimits/updated` from a target.
    pub(super) fn limits_updated(&mut self, id: &TargetId, params: Value) {
        let Ok(report) = serde_json::from_value::<InstanceRateLimits>(params) else { return };
        if self.limits.book.update(id, report) {
            self.limits_changed(true);
        }
    }

    /// A target connected or went away: the stale marks may have changed.
    pub(super) fn limits_presence(&mut self, id: &TargetId) {
        if self.limits.book.has_target(id) {
            self.limits_changed(false);
        }
    }

    /// A target was removed.
    pub(super) fn limits_forget(&mut self, id: &TargetId) {
        self.limits.capable.remove(id);
        self.limits.asked.remove(id);
        if self.limits.book.remove_target(id) {
            self.limits_changed(true);
        }
    }

    /// `hub/rateLimits/get`.
    pub(super) fn limits_get(&mut self, p: &RateLimitsGetParams) -> Result<Value, ErrorObject> {
        if p.refresh {
            let now = Instant::now();
            let due: Vec<TargetId> = self
                .limits
                .capable
                .iter()
                .filter(|id| self.limits_online(id) && self.limits.asked.get(*id).is_none_or(|at| now.duration_since(*at) >= REFRESH_EVERY))
                .cloned()
                .collect();
            for id in due {
                self.limits.asked.insert(id.clone(), now);
                self.limits_ask(&id, true);
            }
        }
        Ok(self.limits_result())
    }
}
