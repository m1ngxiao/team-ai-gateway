-- Model and route rows are inserted transactionally from the frozen built-in seed.
INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt61_sol_gpt6_luna_catalog_revision', '2026-10-09-verified')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt61_sol_price_source', 'https://developers.openai.com/api/docs/models/gpt-6.1-sol')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt6_luna_price_source', 'https://developers.openai.com/api/docs/models/gpt-6-luna')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;
