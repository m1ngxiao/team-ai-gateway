use rusqlite::{params, OptionalExtension, Result};

use super::{now_ts, Storage};

pub const SECONDS_PER_HOUR: i64 = 60 * 60;
pub const SECONDS_PER_WEEK: i64 = 7 * 24 * SECONDS_PER_HOUR;

/// Default per-key rolling weekly budget: $240.00.
pub const DEFAULT_KEY_WEEKLY_LIMIT_CENTS: i64 = 24_000;
/// Default per-account weekly budget: $1,200.00.
pub const DEFAULT_ACCOUNT_WEEKLY_LIMIT_CENTS: i64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyQuotaState {
    pub key_id: String,
    pub epoch_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountWeeklyUsage {
    pub account_id: String,
    pub window_start: i64,
    pub window_end: i64,
    pub used_cents: i64,
    pub reserved_cents: i64,
    pub reset_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaResetEvent {
    pub id: i64,
    pub scope: String,
    pub source: String,
    pub occurred_at: i64,
    pub confirmed_by: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
}

fn key_quota_state_from_row(row: &rusqlite::Row<'_>) -> Result<KeyQuotaState> {
    Ok(KeyQuotaState {
        key_id: row.get(0)?,
        epoch_at: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

fn account_weekly_usage_from_row(row: &rusqlite::Row<'_>) -> Result<AccountWeeklyUsage> {
    Ok(AccountWeeklyUsage {
        account_id: row.get(0)?,
        window_start: row.get(1)?,
        window_end: row.get(2)?,
        used_cents: row.get(3)?,
        reserved_cents: row.get(4)?,
        reset_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn quota_reset_event_from_row(row: &rusqlite::Row<'_>) -> Result<QuotaResetEvent> {
    Ok(QuotaResetEvent {
        id: row.get(0)?,
        scope: row.get(1)?,
        source: row.get(2)?,
        occurred_at: row.get(3)?,
        confirmed_by: row.get(4)?,
        note: row.get(5)?,
        created_at: row.get(6)?,
    })
}

impl Storage {
    /// Ensure a durable per-key quota state row exists and return it.
    pub fn ensure_key_quota_state(&self, key_id: &str) -> Result<KeyQuotaState> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO key_quota_state (key_id, epoch_at, created_at, updated_at)
             VALUES (?1, 0, ?2, ?2)
             ON CONFLICT(key_id) DO NOTHING",
            params![key_id, now],
        )?;
        self.get_key_quota_state(key_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_key_quota_state(&self, key_id: &str) -> Result<Option<KeyQuotaState>> {
        self.conn
            .query_row(
                "SELECT key_id, epoch_at, created_at, updated_at
                 FROM key_quota_state
                 WHERE key_id = ?1",
                params![key_id],
                key_quota_state_from_row,
            )
            .optional()
    }

    /// Advance a key's quota epoch. Usage before `epoch_at` no longer counts.
    pub fn set_key_quota_epoch(&self, key_id: &str, epoch_at: i64) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO key_quota_state (key_id, epoch_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(key_id) DO UPDATE SET
                epoch_at = excluded.epoch_at,
                updated_at = excluded.updated_at",
            params![key_id, epoch_at, now],
        )?;
        Ok(())
    }

    /// Advance every active API key's rolling quota epoch. Returns how many
    /// keys were advanced. This is used for an official upstream global reset.
    pub fn reset_all_key_quota_epochs(&self, epoch_at: i64) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM api_keys ORDER BY id")?;
        let key_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>>>()?;
        drop(stmt);
        for key_id in &key_ids {
            self.set_key_quota_epoch(key_id, epoch_at)?;
        }
        Ok(key_ids.len())
    }

    /// Add settled cost to the hourly bucket. Money is stored as integer cents.
    pub fn add_key_usage_hourly(
        &self,
        key_id: &str,
        hour_start: i64,
        cost_cents: i64,
        request_count: i64,
    ) -> Result<()> {
        if cost_cents <= 0 && request_count <= 0 {
            return Ok(());
        }
        let aligned_hour = hour_start - hour_start.rem_euclid(SECONDS_PER_HOUR);
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO key_usage_hourly (
                key_id, hour_start, cost_cents, request_count, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key_id, hour_start) DO UPDATE SET
                cost_cents = key_usage_hourly.cost_cents + excluded.cost_cents,
                request_count = key_usage_hourly.request_count + excluded.request_count,
                updated_at = excluded.updated_at",
            params![
                key_id,
                aligned_hour,
                cost_cents.max(0),
                request_count.max(0),
                now
            ],
        )?;
        Ok(())
    }

    pub fn key_usage_cents_since(&self, key_id: &str, since: i64) -> Result<i64> {
        self.conn.query_row(
            "SELECT COALESCE(SUM(cost_cents), 0)
             FROM key_usage_hourly
             WHERE key_id = ?1 AND hour_start >= ?2",
            params![key_id, since],
            |row| row.get(0),
        )
    }

    /// Rolling seven-day usage, bounded below by the key's current epoch.
    pub fn key_usage_cents_rolling_7d(&self, key_id: &str, now: i64) -> Result<i64> {
        let state = self.ensure_key_quota_state(key_id)?;
        let since = (now - SECONDS_PER_WEEK).max(state.epoch_at);
        // Billing is finalized after the token-stat row is inserted, so the
        // incremental hourly ledger can be empty for successful requests. Derive
        // the rolling total from the durable cost stats first; fall back to the
        // hourly ledger for older rows/tests that predate cost snapshots.
        let derived: i64 = self.conn.query_row(
            "SELECT CAST(ROUND(COALESCE(SUM(estimated_cost_usd), 0) * 100.0) AS INTEGER)
             FROM request_token_stats
             WHERE key_id = ?1 AND created_at >= ?2 AND usage_included = 1",
            params![key_id, since],
            |row| row.get(0),
        )?;
        if derived > 0 {
            return Ok(derived);
        }
        self.key_usage_cents_since(key_id, since)
    }

    /// Rolling seven-day account usage derived from durable request cost stats.
    /// The latest upstream weekly window end for an account. `usage_snapshots`
    /// stores the reset instant of the 7-day window, so the window start is
    /// `reset_at - 7d`; do not count usage from the previous window.
    pub fn latest_account_weekly_reset_at(&self, account_id: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT resets_at
                 FROM usage_snapshots
                 WHERE account_id = ?1
                   AND window_minutes >= 10080
                   AND resets_at IS NOT NULL
                 ORDER BY captured_at DESC, id DESC
                 LIMIT 1",
                params![account_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn account_usage_cents_rolling_7d(&self, account_id: &str, now: i64) -> Result<i64> {
        let reset_at = self.latest_account_weekly_reset_at(account_id)?;
        let since = reset_at
            .map(|reset| {
                reset
                    .saturating_sub(SECONDS_PER_WEEK)
                    .max(now.saturating_sub(SECONDS_PER_WEEK))
            })
            .unwrap_or_else(|| now.saturating_sub(SECONDS_PER_WEEK));
        self.conn.query_row(
            "SELECT COALESCE(SUM(CAST(ROUND(estimated_cost_usd * 100.0) AS INTEGER)), 0)
             FROM request_token_stats
             WHERE account_id = ?1 AND created_at >= ?2 AND usage_included = 1",
            params![account_id, since],
            |row| row.get(0),
        )
    }

    pub fn ensure_account_weekly_usage(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
    ) -> Result<AccountWeeklyUsage> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)
             ON CONFLICT(account_id) DO NOTHING",
            params![account_id, window_start, window_end, reset_at, now],
        )?;
        self.get_account_weekly_usage(account_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_account_weekly_usage(
        &self,
        account_id: &str,
    ) -> Result<Option<AccountWeeklyUsage>> {
        self.conn
            .query_row(
                "SELECT account_id, window_start, window_end, used_cents,
                        reserved_cents, reset_at, updated_at
                 FROM account_weekly_usage
                 WHERE account_id = ?1",
                params![account_id],
                account_weekly_usage_from_row,
            )
            .optional()
    }

    pub fn reset_account_weekly_usage(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
    ) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)
             ON CONFLICT(account_id) DO UPDATE SET
                window_start = excluded.window_start,
                window_end = excluded.window_end,
                used_cents = 0,
                reserved_cents = 0,
                reset_at = excluded.reset_at,
                updated_at = excluded.updated_at",
            params![account_id, window_start, window_end, reset_at, now],
        )?;
        Ok(())
    }

    pub fn add_account_usage_cents(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
        cost_cents: i64,
    ) -> Result<()> {
        if cost_cents <= 0 {
            return Ok(());
        }
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)
             ON CONFLICT(account_id) DO UPDATE SET
                used_cents = account_weekly_usage.used_cents + excluded.used_cents,
                updated_at = excluded.updated_at",
            params![account_id, window_start, window_end, cost_cents, reset_at, now],
        )?;
        Ok(())
    }

    pub fn add_account_reserved_cents(
        &self,
        account_id: &str,
        window_start: i64,
        window_end: i64,
        reset_at: i64,
        reserved_cents: i64,
    ) -> Result<()> {
        if reserved_cents <= 0 {
            return Ok(());
        }
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO account_weekly_usage (
                account_id, window_start, window_end, used_cents, reserved_cents, reset_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)
             ON CONFLICT(account_id) DO UPDATE SET
                reserved_cents = account_weekly_usage.reserved_cents + excluded.reserved_cents,
                updated_at = excluded.updated_at",
            params![account_id, window_start, window_end, reserved_cents, reset_at, now],
        )?;
        Ok(())
    }

    pub fn record_quota_reset_event(
        &self,
        scope: &str,
        source: &str,
        occurred_at: i64,
        confirmed_by: Option<&str>,
        note: Option<&str>,
    ) -> Result<i64> {
        let now = now_ts();
        self.conn.execute(
            "INSERT INTO quota_reset_events (
                scope, source, occurred_at, confirmed_by, note, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![scope, source, occurred_at, confirmed_by, note, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_quota_reset_events(&self, limit: i64) -> Result<Vec<QuotaResetEvent>> {
        let limit = limit.clamp(1, 500);
        let mut stmt = self.conn.prepare(
            "SELECT id, scope, source, occurred_at, confirmed_by, note, created_at
             FROM quota_reset_events
             ORDER BY occurred_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], quota_reset_event_from_row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ApiKey, RequestTokenStat, UpstreamProvider, UsageSnapshotRecord};

    fn open_storage() -> Storage {
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        storage
    }

    #[test]
    fn key_quota_epoch_excludes_old_usage() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .add_key_usage_hourly("key-1", now - 12 * 3600, 100, 1)
            .expect("recent usage");
        storage
            .add_key_usage_hourly("key-1", now - 8 * 24 * 3600, 200, 1)
            .expect("old usage");
        storage
            .set_key_quota_epoch("key-1", now - 24 * 3600)
            .expect("epoch");

        let usage = storage
            .key_usage_cents_rolling_7d("key-1", now)
            .expect("rolling usage");
        assert_eq!(usage, 100);
    }

    #[test]
    fn account_weekly_usage_accumulates_and_resets() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        let week = SECONDS_PER_WEEK;

        storage
            .add_account_usage_cents("acct-1", now, now + week, now + week, 500)
            .expect("first usage");
        storage
            .add_account_usage_cents("acct-1", now, now + week, now + week, 250)
            .expect("second usage");

        let usage = storage
            .get_account_weekly_usage("acct-1")
            .expect("read usage")
            .expect("usage row");
        assert_eq!(usage.used_cents, 750);

        storage
            .reset_account_weekly_usage("acct-1", now + week, now + 2 * week, now + 2 * week)
            .expect("reset");
        let usage = storage
            .get_account_weekly_usage("acct-1")
            .expect("read usage")
            .expect("usage row");
        assert_eq!(usage.used_cents, 0);
        assert_eq!(usage.window_start, now + week);
    }

    #[test]
    fn latest_account_weekly_reset_uses_seven_day_window() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acct-reset".to_string(),
                used_percent: Some(10.0),
                window_minutes: Some(10080),
                resets_at: Some(now + 3 * 24 * 3600),
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                credits_json: None,
                captured_at: now,
            })
            .expect("snapshot");
        assert_eq!(
            storage
                .latest_account_weekly_reset_at("acct-reset")
                .expect("reset"),
            Some(now + 3 * 24 * 3600)
        );
    }

    #[test]
    fn quota_reset_events_round_trip() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        let id = storage
            .record_quota_reset_event(
                "global",
                "official",
                now,
                Some("admin"),
                Some("upstream reset"),
            )
            .expect("record event");

        let events = storage.list_quota_reset_events(10).expect("list events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id);
        assert_eq!(events[0].scope, "global");
        assert_eq!(events[0].source, "official");
        assert_eq!(events[0].confirmed_by.as_deref(), Some("admin"));
    }
    #[test]
    fn key_usage_rolling_7d_prefers_cost_stats() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id: 0,
                key_id: Some("key-stats".to_string()),
                account_id: None,
                model: Some("gpt-6-astra".to_string()),
                actual_source_kind: None,
                actual_source_id: None,
                input_tokens: Some(1_000),
                cached_input_tokens: Some(0),
                output_tokens: Some(100),
                total_tokens: Some(1_100),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(1.25),
                created_at: now - 12 * 3600,
            })
            .expect("token stat");
        assert_eq!(
            storage.key_usage_cents_rolling_7d("key-stats", now).expect("usage"),
            125
        );
    }

    #[test]
    fn key_usage_accumulates_sub_cent_costs_before_rounding() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        for request_log_id in 0..2 {
            storage
                .insert_request_token_stat(&RequestTokenStat {
                    request_log_id,
                    key_id: Some("key-subcent".to_string()),
                    account_id: None,
                    model: Some("gpt-6-luna".to_string()),
                    actual_source_kind: None,
                    actual_source_id: None,
                    input_tokens: Some(20),
                    cached_input_tokens: Some(0),
                    output_tokens: Some(5),
                    total_tokens: Some(25),
                    reasoning_output_tokens: Some(0),
                    estimated_cost_usd: Some(0.004),
                    created_at: now - 3600,
                })
                .expect("token stat");
        }
        assert_eq!(
            storage
                .key_usage_cents_rolling_7d("key-subcent", now)
                .expect("usage"),
            1
        );
    }

    #[test]
    fn reset_all_key_quota_epochs_zeroes_all_keys() {
        let storage = open_storage();
        let now = 1_700_000_000_i64;
        for (id, status) in [("key-a", "active"), ("key-b", "active"), ("key-c", "disabled")] {
            storage
                .insert_api_key(&ApiKey {
                    id: id.to_string(),
                    name: Some(id.to_string()),
                    model_slug: None,
                    reasoning_effort: None,
                    service_tier: None,
                    rotation_strategy: "account_rotation".to_string(),
                    upstream_provider: UpstreamProvider::Openai,
                    aggregate_api_id: None,
                    account_plan_filter: None,
                    aggregate_api_url: None,
                    client_type: "codex".to_string(),
                    protocol_type: "openai_compat".to_string(),
                    auth_scheme: "authorization_bearer".to_string(),
                    upstream_base_url: None,
                    static_headers_json: None,
                    key_hash: format!("hash-{id}"),
                    status: status.to_string(),
                    created_at: now,
                    last_used_at: None,
                })
                .expect("api key");
            storage
                .add_key_usage_hourly(id, now - 3600, 1_000, 1)
                .expect("usage");
            assert_eq!(
                storage.key_usage_cents_rolling_7d(id, now).expect("usage"),
                1_000
            );
        }

        let reset = storage
            .reset_all_key_quota_epochs(now + 10)
            .expect("reset all");
        assert_eq!(reset, 3);
        for id in ["key-a", "key-b", "key-c"] {
            assert_eq!(
                storage
                    .key_usage_cents_rolling_7d(id, now + 10)
                    .expect("usage after reset"),
                0
            );
        }
    }

}