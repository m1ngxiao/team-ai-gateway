-- Model rows are inserted transactionally from the versioned built-in seed.
INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt6_sol_catalog_revision', '2026-09-23-official')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt6_sol_price_source', 'https://developers.openai.com/api/docs/models/gpt-6-sol')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;
