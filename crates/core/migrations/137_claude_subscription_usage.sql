CREATE TABLE IF NOT EXISTS claude_subscription_usage (
  account_id TEXT PRIMARY KEY REFERENCES claude_subscription_accounts(id) ON DELETE CASCADE,
  five_hour_used_percent REAL,
  five_hour_resets_at INTEGER,
  seven_day_used_percent REAL,
  seven_day_resets_at INTEGER,
  captured_at INTEGER,
  last_attempt_at INTEGER,
  next_attempt_at INTEGER,
  last_error TEXT,
  CHECK (five_hour_used_percent IS NULL OR five_hour_used_percent BETWEEN 0 AND 100),
  CHECK (seven_day_used_percent IS NULL OR seven_day_used_percent BETWEEN 0 AND 100)
);
