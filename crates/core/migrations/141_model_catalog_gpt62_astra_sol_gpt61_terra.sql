-- Model and route rows are inserted transactionally from the frozen built-in seed.
INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt62_astra_sol_gpt61_terra_catalog_revision', '2026-11-18-verified')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt62_sol_price_source', 'https://developers.openai.com/api/docs/models/gpt-6.2-sol')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt62_astra_price_source', 'https://developers.openai.com/api/docs/models/gpt-6.2-astra')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;

INSERT INTO model_catalog_v2_meta(key, value)
VALUES('gpt61_terra_price_source', 'https://developers.openai.com/api/docs/models/gpt-6.1-terra')
ON CONFLICT(key) DO UPDATE SET value = excluded.value;
