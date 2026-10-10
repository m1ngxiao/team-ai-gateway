#!/usr/bin/env python3
"""Validate a private static subscription; --apply enables transactional reload."""

import argparse
import os
from pathlib import Path

from maintenance import error_label, load_settings, report, update_subscription


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True, help="private maintenance JSON")
    parser.add_argument("--downloaded", type=Path, help="reuse private YAML without downloading")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        code, result = update_subscription(load_settings(args.config), downloaded=args.downloaded, apply=args.apply)
        report(**result)
        return code
    except Exception as error:
        report("failed", reason=error_label(error))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
