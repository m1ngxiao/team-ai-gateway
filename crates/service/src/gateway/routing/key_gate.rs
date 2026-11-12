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

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyGateRole {
    Normal,
    UltraParent,
    UltraChild,
}

#[derive(Default)]
struct KeyGateState {
    active: usize,
    /// 0 = idle, 1 = normal, 2 = ultra.
    mode: u8,
    parent_active: bool,
    child_active: bool,
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
    role: KeyGateRole,
}

impl Drop for KeyConcurrencyGuard {
    fn drop(&mut self) {
        let mut state = match self.lock.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.active = state.active.saturating_sub(1);
        match self.role {
            KeyGateRole::UltraParent => state.parent_active = false,
            KeyGateRole::UltraChild => state.child_active = false,
            KeyGateRole::Normal => {}
        }
        if state.active == 0 {
            *state = KeyGateState::default();
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

fn can_enter(state: &KeyGateState, max_active: usize, is_subagent: bool) -> bool {
    if max_active <= 1 {
        return state.active == 0;
    }
    if state.active == 0 {
        return true;
    }
    if state.mode != 2 {
        return false;
    }
    if is_subagent {
        state.parent_active && !state.child_active
    } else {
        false
    }
}

/// Acquire one key-level concurrency slot.
///
/// Normal requests are exclusive (`max_active = 1`). Astra Ultra requests use
/// `max_active = 2`, but only one parent and one child may overlap; a second
/// parent or a second child waits. This keeps a single Ultra turn from turning
/// into two independent parent turns while still allowing one subagent to run
/// alongside its parent.
pub(crate) fn acquire_key_concurrency(
    key_id: &str,
    max_active: usize,
    is_subagent: bool,
    timeout: Duration,
) -> Option<KeyConcurrencyGuard> {
    if key_id.trim().is_empty() {
        return None;
    }
    let max_active = max_active.max(1);
    let role = if max_active <= 1 {
        KeyGateRole::Normal
    } else if is_subagent {
        KeyGateRole::UltraChild
    } else {
        KeyGateRole::UltraParent
    };
    let lock = key_gate(key_id);
    let mut state = match lock.state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !can_enter(&state, max_active, is_subagent) {
        let (next, _) = match lock
            .available
            .wait_timeout_while(state, timeout, |state| {
                !can_enter(state, max_active, is_subagent)
            }) {
            Ok(result) => result,
            Err(poisoned) => poisoned.into_inner(),
        };
        state = next;
    }
    if !can_enter(&state, max_active, is_subagent) {
        return None;
    }
    if state.active == 0 {
        state.mode = if max_active <= 1 { 1 } else { 2 };
    }
    match role {
        KeyGateRole::UltraParent => state.parent_active = true,
        KeyGateRole::UltraChild => state.child_active = true,
        KeyGateRole::Normal => {}
    }
    state.active += 1;
    drop(state);
    Some(KeyConcurrencyGuard { lock, role })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn normal_request_is_exclusive_per_key() {
        let first = acquire_key_concurrency("key-a", 1, false, Duration::from_millis(50))
            .expect("first slot");
        assert!(
            acquire_key_concurrency("key-a", 1, false, Duration::from_millis(10)).is_none()
        );
        drop(first);
        assert!(
            acquire_key_concurrency("key-a", 1, false, Duration::from_millis(10)).is_some()
        );
    }

    #[test]
    fn ultra_parent_and_one_child_can_overlap() {
        let parent = acquire_key_concurrency("key-b", 2, false, Duration::from_millis(50))
            .expect("parent slot");
        let child = acquire_key_concurrency("key-b", 2, true, Duration::from_millis(50))
            .expect("child slot");
        assert!(
            acquire_key_concurrency("key-b", 2, false, Duration::from_millis(10)).is_none(),
            "second parent must wait"
        );
        assert!(
            acquire_key_concurrency("key-b", 2, true, Duration::from_millis(10)).is_none(),
            "second child must wait"
        );
        drop(parent);
        drop(child);
    }

    #[test]
    fn normal_and_ultra_do_not_share_a_key() {
        let normal = acquire_key_concurrency("key-d", 1, false, Duration::from_millis(50))
            .expect("normal slot");
        assert!(
            acquire_key_concurrency("key-d", 2, false, Duration::from_millis(10)).is_none()
        );
        drop(normal);
        assert!(
            acquire_key_concurrency("key-d", 2, false, Duration::from_millis(10)).is_some()
        );
    }

    #[test]
    fn waiting_request_wakes_when_slot_is_released() {
        let first = acquire_key_concurrency("key-c", 1, false, Duration::from_millis(50))
            .expect("first slot");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let guard = acquire_key_concurrency("key-c", 1, false, Duration::from_secs(1));
            tx.send(guard.is_some()).unwrap();
            drop(guard);
        });
        thread::sleep(Duration::from_millis(20));
        drop(first);
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
    }
}
