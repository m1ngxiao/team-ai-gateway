ALTER TABLE api_keys
  ADD COLUMN upstream_provider TEXT NOT NULL DEFAULT 'openai'
  CHECK (upstream_provider IN ('openai', 'claude'));

ALTER TABLE api_keys
  ADD COLUMN requires_route_review INTEGER NOT NULL DEFAULT 0
  CHECK (requires_route_review IN (0, 1));

-- A pinned aggregate-only Claude key is a useful initial classification for
-- the administrator. It still needs review below whenever legacy fallback
-- could have crossed pools.
UPDATE api_keys
SET upstream_provider = 'claude'
WHERE rotation_strategy = 'aggregate_api_rotation'
  AND aggregate_api_id IS NOT NULL
  AND EXISTS (
    SELECT 1
    FROM aggregate_apis AS a
    WHERE a.id = api_keys.aggregate_api_id
      AND REPLACE(LOWER(TRIM(a.provider_type)), '-', '_') IN ('claude', 'anthropic', 'anthropic_native', 'claude_code')
  );

-- Legacy requests selected the provider by request path. Even an explicitly
-- pinned aggregate was only the first candidate: failure could fall through
-- to a different provider's candidate pool. Therefore every aggregate/hybrid
-- key needs review if a native Claude upstream exists. Without native Claude,
-- `compatible` belonged to the old Messages pool while Codex belonged to the
-- old Responses pool. Their combination also needs review unless a key pins
-- an active Codex upstream, which was the first candidate for both paths.
UPDATE api_keys
SET status = 'disabled', requires_route_review = 1
WHERE rotation_strategy IN (
    'aggregate_api_rotation',
    'hybrid_rotation',
    'hybrid_aggregate_first_rotation'
  )
  AND (
    EXISTS (
      SELECT 1 FROM aggregate_apis AS a
      WHERE REPLACE(LOWER(TRIM(a.provider_type)), '-', '_')
        IN ('claude', 'anthropic', 'anthropic_native', 'claude_code')
    )
    OR (
      EXISTS (
        SELECT 1 FROM aggregate_apis AS a
        WHERE REPLACE(LOWER(TRIM(a.provider_type)), '-', '_') = 'compatible'
      )
      AND EXISTS (
        SELECT 1 FROM aggregate_apis AS a
        WHERE REPLACE(LOWER(TRIM(a.provider_type)), '-', '_')
          NOT IN ('claude', 'anthropic', 'anthropic_native', 'claude_code',
                  'gemini', 'gemini_native', 'google', 'google_ai', 'google_gemini',
                  'compatible')
      )
      AND NOT EXISTS (
        SELECT 1 FROM aggregate_apis AS a
        WHERE a.id = TRIM(api_keys.aggregate_api_id)
          AND LOWER(TRIM(a.status)) = 'active'
          AND REPLACE(LOWER(TRIM(a.provider_type)), '-', '_')
            NOT IN ('claude', 'anthropic', 'anthropic_native', 'claude_code',
                    'gemini', 'gemini_native', 'google', 'google_ai', 'google_gemini',
                    'compatible')
      )
    )
  );
