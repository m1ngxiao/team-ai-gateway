#!/usr/bin/env python3
"""Prepare or apply a verified US URL-test group; no subscription timer is added."""

import argparse
import os
from pathlib import Path

from maintenance import configure_us_auto, error_label, load_settings, report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True, help="private maintenance JSON")
    parser.add_argument("--target", required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true")
    mode.add_argument("--verify", action="store_true", help="check current group and live US egress")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        code, result = configure_us_auto(load_settings(args.config), args.target, apply=args.apply, verify=args.verify)
        report(**result)
        return code
    except Exception as error:
        report("failed", reason=error_label(error))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
