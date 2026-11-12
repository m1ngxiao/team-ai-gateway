-- 140_quota_ledger_usage_included.sql
-- Rebuild the rolling key ledger from durable cost stats, but only count
-- requests whose usage was actually included by the upstream. Failed or
-- non-billable attempts must not consume a user's weekly budget.

DELETE FROM key_usage_hourly;

INSERT INTO key_usage_hourly (
  key_id, hour_start, cost_cents, request_count, updated_at
)
SELECT
  key_id,
  (created_at / 3600) * 3600,
  CAST(ROUND(SUM(estimated_cost_usd) * 100.0) AS INTEGER),
  COUNT(*),
  CAST(strftime('%s', 'now') AS INTEGER)
FROM request_token_stats
WHERE usage_included = 1
  AND created_at >= CAST(strftime('%s', 'now') AS INTEGER) - 604800
  AND estimated_cost_usd IS NOT NULL
GROUP BY key_id, created_at / 3600;
