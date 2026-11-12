use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use codexmanager_core::storage::now_ts;

const KEY_GATE_TTL_SECS: i64 = 30 * 60;
const KEY_GATE_CLEANUP_INTERVAL_SECS: i64 = 60;

struct KeyGateEntry {
    lock: Arc<KeyGateLock>,
    last_seen_at: i64,
}

#[derive(Default)]
struct KeyGateTable {
    entries: HashMap<String, KeyGateEntry>,
    last_cleanup_at: i64,
}

static KEY_GATES: OnceLock<Mutex<KeyGateTable>> = OnceLock::new();

#[derive(Default)]
struct KeyGateState {
    active: usize,
    mode: u8,
}

struct KeyGateLock {
    state: Mutex<KeyGateState>,
    available: Condvar,
}

impl KeyGateLock {
    fn new() -> Self {
        Self {
            state: Mutex::new(KeyGateState::default()),
            available: Condvar::new(),
        }
    }
}

pub(crate) struct KeyConcurrencyGuard {
    lock: Arc<KeyGateLock>,
}

impl Drop for KeyConcurrencyGuard {
    fn drop(&mut self) {
        let mut state = match self.lock.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.active = state.active.saturating_sub(1);
        if state.active == 0 {
            state.mode = 0;
        }
        drop(state);
        self.lock.available.notify_all();
    }
}

fn key_gate(key_id: &str) -> Arc<KeyGateLock> {
    let table = KEY_GATES.get_or_init(|| Mutex::new(KeyGateTable::default()));
    let mut table = crate::lock_utils::lock_recover(table, "key_gates");
    let now = now_ts();
    if table.last_cleanup_at == 0
        || now.saturating_sub(table.last_cleanup_at) >= KEY_GATE_CLEANUP_INTERVAL_SECS
    {
        table.last_cleanup_at = now;
        table.entries.retain(|_, entry| {
            now.saturating_sub(entry.last_seen_at) <= KEY_GATE_TTL_SECS
                || Arc::strong_count(&entry.lock) > 1
        });
    }
    let entry = table
        .entries
        .entry(key_id.trim().to_string())
        .or_insert_with(|| KeyGateEntry {
            lock: Arc::new(KeyGateLock::new()),
            last_seen_at: now,
        });
    entry.last_seen_at = now;
    entry.lock.clone()
}

/// Acquire one key-level concurrency slot. `max_active` is the total number of
/// in-flight requests allowed for this key at the moment of admission:
/// 1 for a normal request, 2 for an Astra Ultra request (parent + one child).
pub(crate) fn acquire_key_concurrency(
    key_id: &str,
    max_active: usize,
    timeout: Duration,
) -> Option<KeyConcurrencyGuard> {
    if key_id.trim().is_empty() {
        return None;
    }
    let max_active = max_active.max(1);
    let mode = max_active.min(2) as u8;
    let lock = key_gate(key_id);
    let state = match lock.state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let can_enter = |state: &KeyGateState| {
        state.active == 0 || (state.mode == mode && state.active < max_active)
    };
    if can_enter(&state) {
        let mut state = state;
        if state.active == 0 {
            state.mode = mode;
        }
        state.active += 1;
        drop(state);
        return Some(KeyConcurrencyGuard { lock });
    }
    let (mut state, _) = match lock
        .available
        .wait_timeout_while(state, timeout, |state| !can_enter(state))
    {
        Ok(result) => result,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !can_enter(&state) {
        return None;
    }
    if state.active == 0 {
        state.mode = mode;
    }
    state.active += 1;
    drop(state);
    Some(KeyConcurrencyGuard { lock })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn normal_request_is_exclusive_per_key() {
        let first = acquire_key_concurrency("key-a", 1, Duration::from_millis(50))
            .expect("first slot");
        assert!(acquire_key_concurrency("key-a", 1, Duration::from_millis(10)).is_none());
        drop(first);
        assert!(acquire_key_concurrency("key-a", 1, Duration::from_millis(10)).is_some());
    }

    #[test]
    fn ultra_request_can_use_two_slots() {
        let first = acquire_key_concurrency("key-b", 2, Duration::from_millis(50))
            .expect("first slot");
        let second = acquire_key_concurrency("key-b", 2, Duration::from_millis(50))
            .expect("second slot");
        assert!(acquire_key_concurrency("key-b", 2, Duration::from_millis(10)).is_none());
        drop(first);
        drop(second);
    }

    #[test]
    fn normal_and_ultra_do_not_share_a_key() {
        let normal = acquire_key_concurrency("key-d", 1, Duration::from_millis(50))
            .expect("normal slot");
        assert!(acquire_key_concurrency("key-d", 2, Duration::from_millis(10)).is_none());
        drop(normal);
        assert!(acquire_key_concurrency("key-d", 2, Duration::from_millis(10)).is_some());
    }

    #[test]
    fn waiting_request_wakes_when_slot_is_released() {
        let first = acquire_key_concurrency("key-c", 1, Duration::from_millis(50))
            .expect("first slot");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let guard = acquire_key_concurrency("key-c", 1, Duration::from_secs(1));
            tx.send(guard.is_some()).unwrap();
            drop(guard);
        });
        thread::sleep(Duration::from_millis(20));
        drop(first);
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
    }
}
