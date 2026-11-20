//! Identifying a thread across targets.

use std::fmt;

use yaac_protocol::{Address, TargetId, ThreadId};

/// A thread, anywhere: thread ids are unique per target, so the pair is the identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadKey {
    /// The target the thread lives on.
    pub target: TargetId,
    /// The thread on that target.
    pub thread: ThreadId,
}

impl ThreadKey {
    /// A key from its parts.
    pub fn new(target: impl Into<TargetId>, thread: impl Into<ThreadId>) -> Self {
        Self { target: target.into(), thread: thread.into() }
    }

    /// The mesh address of this thread.
    pub fn address(&self) -> Address {
        Address::new(&self.target, &self.thread)
    }

    /// The key a mesh address names, when it is well formed.
    pub fn from_address(address: &Address) -> Option<Self> {
        Some(Self { target: address.target()?, thread: address.thread()? })
    }
}

impl fmt::Display for ThreadKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.target, self.thread)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_round_trips() {
        let key = ThreadKey::new("devbox", "t1");
        assert_eq!(key.address().as_str(), "devbox/t1");
        assert_eq!(ThreadKey::from_address(&key.address()), Some(key));
        assert_eq!(ThreadKey::from_address(&Address("nothing".into())), None);
    }
}
