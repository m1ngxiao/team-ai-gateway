"""Create a consistent SQLite backup without copying live WAL files or secrets to stdout."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import sqlite3


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    destination = args.destination.resolve()
    if source == destination:
        parser.error('Source and destination must differ.')
    if not destination.parent.is_dir():
        parser.error('Create a private backup directory first.')
    fd = os.open(destination, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(fd)
    try:
        with sqlite3.connect(source.as_uri() + '?mode=ro', uri=True) as live:
            live.execute('PRAGMA query_only=ON')
            with sqlite3.connect(destination) as backup:
                live.backup(backup, pages=256, sleep=0.1)
                result = backup.execute('PRAGMA integrity_check').fetchall()
                if result != [('ok',)]:
                    raise RuntimeError('Backup failed the SQLite integrity check.')
    except Exception:
        # Keep the partial file for inspection. Never overwrite a previous backup.
        raise
    print(f'Consistent SQLite backup verified: {destination}')


if __name__ == '__main__':
    main()
