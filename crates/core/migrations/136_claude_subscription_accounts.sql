CREATE TABLE IF NOT EXISTS claude_subscription_accounts (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  email TEXT,
  account_uuid TEXT,
  organization_uuid TEXT,
  subscription_type TEXT,
  status TEXT NOT NULL DEFAULT 'disabled'
    CHECK (status IN ('active', 'disabled', 'needs_login')),
  sort INTEGER NOT NULL DEFAULT 0,
  access_token TEXT NOT NULL,
  refresh_token TEXT NOT NULL,
  scopes TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_claude_subscription_accounts_status_sort
  ON claude_subscription_accounts(status, sort ASC, updated_at DESC);

CREATE TABLE IF NOT EXISTS claude_subscription_login_sessions (
  id TEXT PRIMARY KEY,
  state TEXT NOT NULL UNIQUE,
  code_verifier TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'completing', 'completed', 'failed', 'expired')),
  error TEXT,
  account_id TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_claude_subscription_login_sessions_status_created
  ON claude_subscription_login_sessions(status, created_at);
