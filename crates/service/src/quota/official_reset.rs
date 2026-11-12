use codexmanager_core::storage::{now_ts, QuotaResetEvent, Storage};
use serde::Serialize;

const WEEKLY_WINDOW_MINUTES: i64 = 10_080;
const EARLY_RESET_MARGIN_SECS: i64 = 3_600;
const MAX_CAPTURE_GAP_SECS: i64 = 900;
const GLOBAL_ACCOUNT_THRESHOLD: usize = 3;
const GLOBAL_EVENT_DEDUPE_SECS: i64 = 3_600;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EarlyResetCandidate {
    pub account_id: String,
    pub previous_resets_at: i64,
    pub current_resets_at: i64,
    pub previous_used_percent: f64,
    pub current_used_percent: f64,
    pub captured_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResetEventView {
    pub id: i64,
    pub scope: String,
    pub source: String,
    pub occurred_at: i64,
    pub confirmed_by: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OfficialResetDetection {
    pub candidates: Vec<EarlyResetCandidate>,
    pub global_detected: bool,
    pub event: Option<ResetEventView>,
}

impl From<QuotaResetEvent> for ResetEventView {
    fn from(event: QuotaResetEvent) -> Self {
        Self {
            id: event.id,
            scope: event.scope,
            source: event.source,
            occurred_at: event.occurred_at,
            confirmed_by: event.confirmed_by,
            note: event.note,
            created_at: event.created_at,
        }
    }
}

/// Detect a *pending* official global reset by comparing each account's two
/// newest weekly snapshots. A reset is considered early when `resets_at` moves
/// earlier by more than an hour and the used percentage collapses to near zero.
/// Three such accounts within the same 15-minute window produce a pending
/// event; we deliberately do not move Key epochs automatically.
pub(crate) fn detect_official_global_reset(
    storage: &Storage,
    now: i64,
) -> Result<OfficialResetDetection, String> {
    let accounts = storage.list_accounts().map_err(|err| err.to_string())?;
    let mut candidates = Vec::new();
    for account in accounts {
        if account.status != "active" {
            continue;
        }
        let Some(current) = storage
            .latest_usage_snapshot_for_account(&account.id)
            .map_err(|err| err.to_string())?
        else {
            continue;
        };
        if current.window_minutes.unwrap_or(0) < WEEKLY_WINDOW_MINUTES {
            continue;
        }
        let Some(current_resets_at) = current.resets_at else {
            continue;
        };
        let Some(previous) = storage
            .previous_usage_snapshot_for_account(&account.id, current.captured_at)
            .map_err(|err| err.to_string())?
        else {
            continue;
        };
        let Some(previous_resets_at) = previous.resets_at else {
            continue;
        };
        if current_resets_at.saturating_add(EARLY_RESET_MARGIN_SECS) >= previous_resets_at {
            continue;
        }
        let current_used = current.used_percent.unwrap_or(0.0);
        let previous_used = previous.used_percent.unwrap_or(0.0);
        if current_used > 1.0 || previous_used < 10.0 {
            continue;
        }
        if current.captured_at.saturating_sub(previous.captured_at) > MAX_CAPTURE_GAP_SECS {
            continue;
        }
        candidates.push(EarlyResetCandidate {
            account_id: account.id,
            previous_resets_at,
            current_resets_at,
            previous_used_percent: previous_used,
            current_used_percent: current_used,
            captured_at: current.captured_at,
        });
    }
    candidates.sort_by_key(|candidate| candidate.captured_at);
    let global_detected = candidates.len() >= GLOBAL_ACCOUNT_THRESHOLD
        && candidates
            .last()
            .zip(candidates.first())
            .is_some_and(|(last, first)| {
                last.captured_at.saturating_sub(first.captured_at) <= MAX_CAPTURE_GAP_SECS
            });

    let mut event = None;
    if global_detected {
        let already_recorded = storage
            .list_quota_reset_events(50)
            .map_err(|err| err.to_string())?
            .into_iter()
            .any(|existing| {
                existing.scope == "global"
                    && existing.source == "official_pending"
                    && now.saturating_sub(existing.occurred_at) <= GLOBAL_EVENT_DEDUPE_SECS
            });
        if !already_recorded {
            let note = format!(
                "auto-detected early weekly reset on {} accounts",
                candidates.len()
            );
            let id = storage
                .record_quota_reset_event(
                    "global",
                    "official_pending",
                    now,
                    None,
                    Some(note.as_str()),
                )
                .map_err(|err| err.to_string())?;
            event = storage
                .list_quota_reset_events(1)
                .map_err(|err| err.to_string())?
                .into_iter()
                .find(|recorded| recorded.id == id)
                .map(ResetEventView::from);
        }
    }

    Ok(OfficialResetDetection {
        candidates,
        global_detected,
        event,
    })
}

pub(crate) fn read_official_reset_candidates() -> Result<OfficialResetDetection, String> {
    let storage = crate::storage_helpers::open_storage().ok_or("storage unavailable")?;
    detect_official_global_reset(&storage, now_ts())
}

pub(crate) fn observe_usage_snapshot(storage: &Storage) -> Result<(), String> {
    let detection = detect_official_global_reset(storage, now_ts())?;
    if let Some(event) = detection.event {
        log::warn!(
            "event=gateway_official_global_reset_pending event_id={} accounts={} note={:?}",
            event.id,
            detection.candidates.len(),
            event.note
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codexmanager_core::storage::{Account, UsageSnapshotRecord};

    fn account(id: &str) -> Account {
        Account {
            id: id.to_string(),
            label: id.to_string(),
            issuer: "openai".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn snapshot(
        account_id: &str,
        used_percent: f64,
        resets_at: i64,
        captured_at: i64,
    ) -> UsageSnapshotRecord {
        UsageSnapshotRecord {
            account_id: account_id.to_string(),
            used_percent: Some(used_percent),
            window_minutes: Some(10_080),
            resets_at: Some(resets_at),
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            credits_json: None,
            captured_at,
        }
    }

    #[test]
    fn three_early_resets_create_pending_global_event() {
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = 1_800_000_000;
        for (index, id) in ["acc-a", "acc-b", "acc-c"].iter().enumerate() {
            storage.insert_account(&account(id)).expect("account");
            storage
                .insert_usage_snapshot(&snapshot(id, 50.0, now + 604_800, now - 60))
                .expect("previous snapshot");
            storage
                .insert_usage_snapshot(&snapshot(
                    id,
                    0.0,
                    now + 172_800 + index as i64,
                    now,
                ))
                .expect("current snapshot");
        }

        let detected = detect_official_global_reset(&storage, now).expect("detect");
        assert_eq!(detected.candidates.len(), 3);
        assert!(detected.global_detected);
        assert!(detected.event.is_some());

        let second = detect_official_global_reset(&storage, now + 10).expect("detect again");
        assert!(second.event.is_none(), "duplicate pending event must be suppressed");
    }

    #[test]
    fn non_early_reset_is_ignored() {
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = 1_800_000_000;
        storage.insert_account(&account("acc-normal")).expect("account");
        storage
            .insert_usage_snapshot(&snapshot("acc-normal", 50.0, now + 604_800, now - 60))
            .expect("previous");
        storage
            .insert_usage_snapshot(&snapshot("acc-normal", 0.0, now + 604_800, now))
            .expect("current");

        let detected = detect_official_global_reset(&storage, now).expect("detect");
        assert!(detected.candidates.is_empty());
        assert!(!detected.global_detected);
    }
}
