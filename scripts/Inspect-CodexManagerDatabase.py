"""Read-only deployment checks. Never print accounts, keys, tokens or settings."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import sqlite3
import time


COUNT_TABLES = ("accounts", "tokens", "api_keys", "aggregate_apis")
RELEASE_MIGRATIONS = (
    "131_model_catalog_gpt6_astra",
    "132_model_catalog_gpt56_metadata_fix",
    "133_aggregate_api_user_agent",
)


@contextmanager
def readonly_database(path: Path):
    connection = sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True, timeout=2)
    try:
        connection.execute("PRAGMA query_only=ON")
        deadline = time.monotonic() + 15
        connection.set_progress_handler(lambda: int(time.monotonic() > deadline), 10000)
        connection.execute("BEGIN")
        yield connection
    finally:
        connection.close()


def inspect(path: Path) -> dict:
    with readonly_database(path) as connection:
        # Do not print failed integrity-check details, which can contain values.
        if connection.execute("PRAGMA quick_check").fetchall() != [("ok",)]:
            raise ValueError("database integrity check failed")
        counts = {table: connection.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0]
                  for table in COUNT_TABLES}
        migrations = {name: bool(connection.execute(
            "SELECT 1 FROM schema_migrations WHERE version=?", (name,)).fetchone())
            for name in RELEASE_MIGRATIONS}
        astra = connection.execute(
            "SELECT COUNT(*) FROM models WHERE slug=? AND enabled=1 AND supported_in_api=1 AND visibility='list'",
            ("gpt-6-astra",)).fetchone()[0]
    return {"integrity": "ok", "counts": counts, "release_migrations": migrations,
            "astra_visible_and_enabled": astra > 0}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path)
    parser.add_argument("--require-empty", action="store_true")
    parser.add_argument("--require-v060", action="store_true")
    parser.add_argument("--require-astra", action="store_true")
    args = parser.parse_args()
    try:
        report = inspect(args.database)
    except (OSError, sqlite3.Error, ValueError):
        print("Database inspection failed; no source changes were made.")
        return 1
    print(json.dumps(report))
    if args.require_empty and any(report["counts"].values()):
        print("Not an empty candidate database; refusing to proceed.")
        return 1
    if args.require_v060 and not all(report["release_migrations"].values()):
        print("Required v0.6.0 migrations are missing.")
        return 1
    if args.require_astra and not report["astra_visible_and_enabled"]:
        print("Required Astra model is not visible and enabled for API use.")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
