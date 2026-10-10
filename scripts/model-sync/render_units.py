#!/usr/bin/env python3
"""Render reviewable systemd units for the operator's paths; never install them."""
import argparse
import os
from pathlib import Path
import re
import sys

from model_sync import SyncError, atomic_write, load_config


TEMPLATES = Path(__file__).resolve().parents[2] / 'deploy/model-sync'


def single_line(value):
    if not value or any(ord(character) < 32 or ord(character) == 127 for character in value):
        raise SyncError('invalid_systemd_template_value')
    return value


def quote(value, *, executable_argument=False):
    value = single_line(str(value)).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%')
    if executable_argument:
        value = value.replace('$', '$$')
    return '"' + value + '"'


def render(config_path, script_path, python_path, user, calendar):
    config_path = Path(config_path).resolve()
    config = load_config(config_path)
    if not re.fullmatch(r'[a-zA-Z_][a-zA-Z0-9_-]*\$?', user):
        raise SyncError('invalid_service_user')
    replacements = {
        '@USER@': user,
        '@PYTHON@': quote(Path(python_path).resolve(), executable_argument=True),
        '@SCRIPT@': quote(Path(script_path).resolve(), executable_argument=True),
        '@CONFIG@': quote(config_path, executable_argument=True),
        '@WRITABLE_PATHS@': ' '.join(quote(config[key]) for key in ('state_dir', 'official_cache_dir')),
        '@CALENDAR@': single_line(calendar),
    }
    result = {}
    for name in ('team-ai-gateway-model-sync.service', 'team-ai-gateway-model-sync.timer'):
        content = (TEMPLATES / (name + '.in')).read_text()
        for key, value in replacements.items():
            content = content.replace(key, value)
        if re.search(r'@[A-Z_]+@', content):
            raise SyncError('unfilled_systemd_template')
        result[name] = content
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--script', type=Path, default=Path(__file__).with_name('model_sync.py'))
    parser.add_argument('--python', type=Path, default=Path(sys.executable))
    parser.add_argument('--user', required=True, help='account with read access to credentials and write access to sync/cache directories')
    parser.add_argument('--calendar', default='*-*-* 03:00:00 UTC', help='systemd OnCalendar expression')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        files = render(args.config, args.script, args.python, args.user, args.calendar)
        args.output.mkdir(parents=True, exist_ok=True)
        for name, content in files.items():
            atomic_write(args.output / name, content.encode())
    except (SyncError, OSError) as error:
        parser.exit(1, 'Unit rendering failed: ' + (str(error) if isinstance(error, SyncError) else type(error).__name__) + '\n')
    print('Rendered ' + ', '.join(files) + '. Review them before installation.')


if __name__ == '__main__':
    main()
