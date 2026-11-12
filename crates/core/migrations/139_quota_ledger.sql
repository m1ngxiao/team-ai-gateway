-- 139_quota_ledger.sql
-- Durable admission/quota ledger for per-key rolling weekly USD budgets and
-- per-account weekly USD budgets. Money is stored as integer cents.

CREATE TABLE IF NOT EXISTS key_quota_state (
  key_id TEXT PRIMARY KEY,
  epoch_at INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS key_usage_hourly (
  key_id TEXT NOT NULL,
  hour_start INTEGER NOT NULL,
  cost_cents INTEGER NOT NULL DEFAULT 0,
  request_count INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (key_id, hour_start)
);

CREATE INDEX IF NOT EXISTS idx_key_usage_hourly_hour_start
  ON key_usage_hourly(hour_start);

CREATE TABLE IF NOT EXISTS account_weekly_usage (
  account_id TEXT PRIMARY KEY,
  window_start INTEGER NOT NULL,
  window_end INTEGER NOT NULL,
  used_cents INTEGER NOT NULL DEFAULT 0,
  reserved_cents INTEGER NOT NULL DEFAULT 0,
  reset_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_account_weekly_usage_reset_at
  ON account_weekly_usage(reset_at);

CREATE TABLE IF NOT EXISTS quota_reset_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  scope TEXT NOT NULL,
  source TEXT NOT NULL,
  occurred_at INTEGER NOT NULL,
  confirmed_by TEXT,
  note TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_quota_reset_events_occurred_at
  ON quota_reset_events(occurred_at);