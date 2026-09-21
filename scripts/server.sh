#!/usr/bin/env bash
set -euo pipefail
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
command -v python3 >/dev/null 2>&1 || {
  printf '%s\n' 'Python 3.10+ is required.' >&2
  exit 1
}
exec python3 "$root_dir/scripts/deployment.py" "$@"
