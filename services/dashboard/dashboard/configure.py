"""Initialize or manually rotate only the dashboard's shared viewer password."""

import argparse
import getpass
import json
import os
import re
import secrets
import subprocess
from pathlib import Path
from urllib.parse import urlsplit

from .collector import source_connection
from .security import password_hash


def private_directory(path):
    path.mkdir(parents=True, exist_ok=True)
    if os.name == "nt":
        output = subprocess.check_output(["whoami", "/user", "/fo", "csv", "/nh"])
        sid = re.search(rb"S-1-(?:\d+-)+\d+", output)
        if not sid:
            raise RuntimeError("Unable to resolve current user's SID")
        identity = "*" + sid.group().decode("ascii")
        subprocess.run(["icacls", str(path), "/inheritance:r", "/grant:r",
                        f"{identity}:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F"],
                       check=True, stdout=subprocess.DEVNULL)
    else:
        path.chmod(0o700)


def write_private(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8") as handle:
        handle.write(value)
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--private-dir", type=Path, default=Path("private"))
    parser.add_argument("--source-db", type=Path)
    parser.add_argument("--username", default="team")
    parser.add_argument("--public-origin", help="Exact HTTPS dashboard origin behind Cloudflare Tunnel, without a trailing slash")
    parser.add_argument("--rotate", action="store_true")
    parser.add_argument("--prompt-password", action="store_true")
    args = parser.parse_args()
    if args.public_origin:
        origin = urlsplit(args.public_origin)
        if (origin.scheme != "https" or not origin.hostname or origin.username or origin.password
                or origin.path or origin.query or origin.fragment
                or args.public_origin != f"https://{origin.netloc}"
                or any(ch.isspace() for ch in args.public_origin)):
            parser.error("--public-origin must be an exact HTTPS origin, without credentials, path, query or fragment")
    directory = args.private_dir.resolve()
    auth_file = directory / "auth.json"
    protected_files = (auth_file, directory / "collector.json", directory / "login.local.txt")
    if not args.rotate and any(path.exists() or path.is_symlink() for path in protected_files):
        parser.error("Private files already exist. Preserve or restore the existing configuration; use --rotate only with a complete auth.json.")
    if args.rotate:
        if auth_file.is_symlink() or not auth_file.is_file():
            parser.error("Rotation requires an existing regular auth.json. Initialize or restore it first.")
        try:
            auth = json.loads(auth_file.read_text(encoding="utf-8"))
            if (not isinstance(auth, dict)
                    or not isinstance(auth.get("username"), str) or not auth["username"]
                    or not isinstance(auth.get("origins"), list) or not auth["origins"]
                    or not all(isinstance(origin, str) and origin for origin in auth["origins"])
                    or not isinstance(auth.get("salt"), str) or not re.fullmatch(r"[a-f0-9]{32}", auth["salt"])
                    or not isinstance(auth.get("password_hash"), str) or not re.fullmatch(r"[a-f0-9]{64}", auth["password_hash"])):
                raise ValueError()
        except (OSError, UnicodeError, ValueError):
            parser.error("Rotation requires a complete auth.json. Restore the existing configuration first.")
    if not args.rotate and not args.source_db:
        parser.error("--source-db is required for initialization")
    password = getpass.getpass("New shared viewer password (at least 16 characters): ") if args.prompt_password else secrets.token_urlsafe(24)
    if len(password) < 16 or len(password) > 256 or len(set(password)) < 8:
        parser.error("Use a stronger password (16-256 characters and at least 8 distinct characters).")
    private_directory(directory)
    if not args.rotate:
        with source_connection(args.source_db) as connection:
            accounts = connection.execute("SELECT id FROM accounts ORDER BY created_at,id").fetchall()
            keys = connection.execute("SELECT id FROM api_keys ORDER BY created_at,id").fetchall()
        collector = {"source_db": str(args.source_db.resolve(strict=True)), "id_secret": secrets.token_hex(32),
                     "account_aliases": {row["id"]: f"账号 {i:02}" for i, row in enumerate(accounts, 1)},
                     "key_aliases": {row["id"]: f"Key {i:02}" for i, row in enumerate(keys, 1)}}
        write_private(directory / "collector.json", json.dumps(collector, ensure_ascii=False, indent=2))
        auth = {"username": args.username, "origins": ["http://127.0.0.1:48763", "http://localhost:48763"]}
    if args.public_origin:
        auth["origins"] = list(dict.fromkeys(auth["origins"] + [args.public_origin]))
        auth["cloudflare_tunnel_origins"] = list(dict.fromkeys(auth.get("cloudflare_tunnel_origins", []) + [args.public_origin]))
    salt = secrets.token_hex(16)
    auth.update(salt=salt, password_hash=password_hash(password, salt))
    write_private(auth_file, json.dumps(auth, indent=2))
    write_private(directory / "login.local.txt",
                  f"Shared read-only dashboard\nUsername: {auth['username']}\nPassword: {password}\n\n"
                  "Keep this file private. This is NOT a CodexManager administrator password or platform API key.\n"
                  "After password rotation, restart the dashboard web service to revoke existing sessions.\n")
    print("Dashboard credentials saved privately. Password was not written to terminal output.")
    print("Restart the dashboard web service after changing credentials. Do not restart CodexManager.")


if __name__ == "__main__":
    main()
