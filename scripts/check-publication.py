"""Review every Git index blob without printing candidate secret values.

Exceptions in scripts/publication-allowlist.json bind an exact repository path,
category, SHA-256 of the matched text (or entire static asset), and review reason.
They never exempt a whole source file or a test directory. Restage changes before
running this check: the index, including the allowlist, is the publication input.
"""

import argparse
import hashlib
import ipaddress
import json
import re
import subprocess
from pathlib import Path, PurePosixPath


ALLOWLIST = "scripts/publication-allowlist.json"
PRIVATE_PARTS = {"private", "source-data", "source-backups", "backups", ".cloudflared",
                 ".venv", "node_modules", "__pycache__", ".next", "target"}
PATTERNS = {
    "private-key": re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----"),
    "github-token": re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})\b"),
    "provider-key": re.compile(r"\bsk-(?:proj-|ant-api\d+-)?[A-Za-z0-9_-]{24,}\b"),
    "jwt": re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"),
    "credential-url": re.compile(r"https?://[^\s/\"'<>]+:[^\s/\"'<>]+@"),
    "private-install-path": re.compile(r"/opt/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*"),
    "user-home-path": re.compile(r"(?<![A-Za-z0-9_])(?:/(?:home|Users)/(?!example(?:/|\b)|test-user(?:/|\b))[A-Za-z0-9_.-]+|C:[\\/]+Users[\\/]+(?!(?:example|test-user)[\\/])[A-Za-z\u4e00-\u9fff][^\s\"']+)", re.I),
    "embedded-secret": re.compile(r"\b(?:password|api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|secret[_-]?key)\b[\"']?\s*[:=]\s*[\"'][A-Za-z0-9_./+=:-]{16,}[\"']", re.I),
    "environment-secret": re.compile(r"\b[A-Z][A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|API_KEY)\s*=\s*[\"']?[A-Za-z0-9_./+=:-]{20,}(?=[\"'\s]|$)"),
}
EXCEPTION_CATEGORIES = set(PATTERNS) | {"static-asset-review-required"}
ASSET_PREFIXES = ("apps/public/", "apps/src-tauri/icons/")
IPV4_ADDRESS = re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])")
SOURCE_ENTRYPOINTS = ("Cargo.toml", "Cargo.lock", "apps/package.json", "apps/pnpm-lock.yaml",
                      "apps/src/app/page.tsx", "apps/src-tauri/Cargo.toml",
                      "crates/core/Cargo.toml", "crates/service/Cargo.toml",
                      "crates/web/Cargo.toml", "crates/start/Cargo.toml",
                      "services/dashboard/dashboard/server.py", "services/dashboard/requirements.txt")


def digest(content):
    return hashlib.sha256(content).hexdigest()


def domain_pattern(value):
    domain = value.lower().rstrip(".")
    label = r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
    if len(domain) > 253 or not re.fullmatch(rf"(?:{label}\.)+{label}", domain):
        raise ValueError("invalid denied domain")
    return re.compile(r"(?<![a-z0-9_.-])(?:[a-z0-9-]+\.)*" + re.escape(domain)
                      + r"\.?(?![a-z0-9_.-])", re.I)


def static_asset(path, content):
    if not path.startswith(ASSET_PREFIXES) or len(content) > 10 * 1024 * 1024:
        return False
    extension = PurePosixPath(path).suffix.lower()
    signatures = {".png": b"\x89PNG\r\n\x1a\n", ".ico": b"\0\0\1\0",
                  ".icns": b"icns", ".jpg": b"\xff\xd8\xff", ".jpeg": b"\xff\xd8\xff",
                  ".gif": b"GIF8", ".woff2": b"wOF2"}
    if extension == ".webp":
        return content.startswith(b"RIFF") and content[8:12] == b"WEBP"
    return extension in signatures and content.startswith(signatures[extension])


def records(path, content, domain_patterns=(), networks=()):
    """Yield path, line, category, digest. Candidate content is never returned."""
    parts = PurePosixPath(path).parts
    name = parts[-1].lower()
    if (set(parts) & PRIVATE_PARTS or name.endswith((".db", ".sqlite", ".sqlite3", ".pem", ".key", ".p12", ".log", ".tar", ".zip"))
            or ".db-" in name or name in {"auth.json", "collector.json", "login.local.txt"}
            or (name.endswith(".env") or name == ".env" or name.startswith(".env.")) and not name.endswith(".example")):
        yield path, 0, "private-path", digest(content)
    binary = b"\0" in content
    try:
        text = content.decode("utf-8-sig")
    except UnicodeDecodeError:
        binary = True
        text = content.decode("latin1")
    if binary:
        category = "static-asset-review-required" if static_asset(path, content) else "binary-file-review-required"
        yield path, 0, category, digest(content)
    # Also scan ASCII content in assets: an image exception must not mask an
    # appended token, private path, or credential-bearing metadata.
    for number, line in enumerate(text.splitlines(), 1):
        for category, pattern in PATTERNS.items():
            for match in pattern.finditer(line):
                yield path, number, category, digest(match.group().encode("utf-8"))
        for pattern in domain_patterns:
            for match in pattern.finditer(line):
                yield path, number, "denied-domain", digest(match.group().encode("utf-8"))
        if networks:
            for match in IPV4_ADDRESS.finditer(line):
                try:
                    address = ipaddress.IPv4Address(match.group())
                except ipaddress.AddressValueError:
                    continue
                if any(address in network for network in networks):
                    yield path, number, "denied-address", digest(match.group().encode("utf-8"))


def findings(path, content, domain_patterns=(), networks=()):
    for filename, line, category, _ in records(path, content, domain_patterns, networks):
        yield filename, line, category


def load_allowlist(content):
    value = json.loads(content)
    if not isinstance(value, dict) or set(value) != {"version", "entries"} or value["version"] != 1 or not isinstance(value["entries"], list):
        raise ValueError("invalid allowlist schema")
    allowed = set()
    for entry in value["entries"]:
        if not isinstance(entry, dict) or set(entry) != {"path", "category", "sha256", "reason"}:
            raise ValueError("invalid allowlist entry")
        path = entry["path"]
        if (not isinstance(path, str) or not path or PurePosixPath(path).is_absolute()
                or ".." in PurePosixPath(path).parts or any(c in path for c in "*?[]\\\r\n\0")
                or not isinstance(entry["category"], str) or entry["category"] not in EXCEPTION_CATEGORIES
                or not isinstance(entry["sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", entry["sha256"])
                or not isinstance(entry["reason"], str) or len(entry["reason"].strip()) < 12):
            raise ValueError("invalid allowlist scope")
        key = path, entry["category"], entry["sha256"]
        if key in allowed:
            raise ValueError("duplicate allowlist entry")
        allowed.add(key)
    return allowed


def review(blobs, allowed, domain_patterns=(), networks=()):
    errors, used = [], set()
    for path, content in blobs.items():
        for filename, line, category, fingerprint in records(path, content, domain_patterns, networks):
            key = filename, category, fingerprint
            if key in allowed:
                used.add(key)
            else:
                errors.append((filename, line, category))
    for path, _, _ in sorted(allowed - used):
        errors.append((path, 0, "unused-allowlist-entry"))
    return errors


def git(*args):
    return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL)


def staged_blobs():
    """Use the index object IDs, not working-tree files or only the latest diff."""
    entries, errors = [], []
    for entry in git("ls-files", "--stage", "-z").split(b"\0"):
        if not entry:
            continue
        metadata, raw_path = entry.split(b"\t", 1)
        mode, oid, stage = metadata.split()
        path = raw_path.decode("utf-8")
        if stage != b"0" or mode not in {b"100644", b"100755"}:
            errors.append((path, 0, "unmerged-or-nonregular-index-entry"))
        else:
            entries.append((path, oid))
    process = subprocess.run(["git", "cat-file", "--batch"],
                             input=b"".join(oid + b"\n" for _, oid in entries),
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True)
    output, cursor, blobs = process.stdout, 0, {}
    for path, _ in entries:
        end = output.index(b"\n", cursor)
        _, kind, size = output[cursor:end].split()
        if kind != b"blob":
            raise ValueError("non-blob index entry")
        cursor = end + 1
        blobs[path] = output[cursor:cursor + int(size)]
        cursor += int(size) + 1
    return blobs, errors


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report-json", type=Path, help="Write the same value-free report outside the repository")
    parser.add_argument("--deny-domain", action="append", default=[], help="Additional private domain and its subdomains to reject; repeat as needed")
    parser.add_argument("--deny-network", action="append", default=[], help="Additional private IPv4 network in CIDR notation to reject; repeat as needed")
    args = parser.parse_args(argv)
    try:
        domain_patterns = tuple(domain_pattern(value) for value in args.deny_domain)
        networks = tuple(ipaddress.IPv4Network(value, strict=False) for value in args.deny_network)
    except ValueError:
        parser.error("Use bare DNS domain names and valid IPv4 networks in deny arguments.")
    try:
        blobs, errors = staged_blobs()
        if not blobs:
            errors.append((".", 0, "empty-index-stage-reviewed-source-first"))
        untracked = git("ls-files", "--others", "--exclude-standard", "-z")
        errors.extend((path.decode("utf-8"), 0, "untracked-file-not-reviewed") for path in untracked.split(b"\0") if path)
        root = Path(git("rev-parse", "--show-toplevel").decode("utf-8").strip())
        for path in SOURCE_ENTRYPOINTS:
            if (root / path).is_file() and path not in blobs:
                errors.append((path, 0, "source-entrypoint-not-staged"))
        try:
            allowed = load_allowlist(blobs.get(ALLOWLIST, b'{"version":1,"entries":[]}'))
        except (ValueError, TypeError, UnicodeError):
            allowed = set()
            errors.append((ALLOWLIST, 0, "invalid-allowlist"))
        errors.extend(review(blobs, allowed, domain_patterns, networks))
    except (OSError, ValueError, subprocess.SubprocessError, UnicodeError):
        print(".:0: index-read-failed")
        return 2
    errors = sorted(set(errors))
    for path, line, category in errors:
        safe_path = path.replace("\r", "\\r").replace("\n", "\\n").replace("\t", "\\t")
        print(f"{safe_path}:{line}: {category}")
    print(f"Publication check: {len(blobs)} index files, {len(errors)} finding(s).")
    if args.report_json:
        report = {"index_files": len(blobs), "findings": [dict(path=path, line=line, category=category) for path, line, category in errors]}
        try:
            args.report_json.parent.mkdir(parents=True, exist_ok=True)
            args.report_json.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        except OSError:
            print(".:0: report-write-failed")
            return 2
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
