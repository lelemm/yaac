use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};

#[derive(Default)]
struct Log {
    entries: VecDeque<(u64, Arc<Value>, usize)>,
    bytes: usize,
}
#[derive(Default)]
pub(crate) struct Diagnostics {
    sequence: u64,
    console: Log,
    network: Log,
}
impl Diagnostics {
    pub fn push(&mut self, mut value: Value, mut size: usize) {
        self.sequence += 1;
        let log = if value["method"]
            .as_str()
            .is_some_and(|s| s.starts_with("Network."))
        {
            &mut self.network
        } else {
            &mut self.console
        };
        if size > 256 * 1024 {
            value["params"] = json!({"truncated":true,"originalBytes":size});
            size = value.to_string().len();
        }
        while log.entries.len() >= 1000 || log.bytes + size > 4 * 1024 * 1024 {
            if let Some((_, _, n)) = log.entries.pop_front() {
                log.bytes -= n
            } else {
                break;
            }
        }
        log.bytes += size;
        log.entries
            .push_back((self.sequence, Arc::new(value), size));
    }
    pub fn since(&self, cursor: u64) -> Vec<(u64, Arc<Value>)> {
        let mut out: Vec<_> = self
            .console
            .entries
            .iter()
            .chain(&self.network.entries)
            .filter(|(n, _, _)| *n > cursor)
            .map(|(n, v, _)| (*n, v.clone()))
            .collect();
        out.sort_unstable_by_key(|(n, _)| *n);
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_noise_preserves_network_history() {
        let mut d = Diagnostics::default();
        d.push(json!({"method":"Network.loadingFailed"}), 42);
        for _ in 0..1010 {
            d.push(json!({"method":"Runtime.consoleAPICalled"}), 64);
        }
        assert_eq!(d.since(0).len(), 1001);
        assert_eq!(d.since(0)[0].0, 1);
        assert_eq!(d.since(1010).len(), 1);
    }
}
