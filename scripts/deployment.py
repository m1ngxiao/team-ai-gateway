"""Portable Linux deployment checks. Requires Python 3.10+ and Docker Compose v2."""

import argparse
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
SERVICES = ("gateway", "collector", "dashboard")
CONTAINER_UID = 10001


class DeploymentError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise DeploymentError(message)


def capture(arguments, *, env=None):
    """Do not echo rendered Compose configuration or errors containing secrets."""
    try:
        result = subprocess.run(arguments, env=env, capture_output=True, text=True, check=False)
    except OSError:
        raise DeploymentError(f"Cannot execute {arguments[0]}; check required tools.") from None
    require(result.returncode == 0, f"{arguments[0]} configuration check failed; no service was started.")
    return result.stdout


def checked_path(base, relative):
    """Refuse symlinks in every deployment-owned path, including parent directories."""
    current = base
    require(not current.is_symlink(), "Deployment directory must not be a symbolic link.")
    for part in Path(relative).parts:
        require(part not in {"..", "/"}, "Invalid deployment path.")
        current = current / part
        require(not current.is_symlink(), f"Deployment path must not be linked: {relative}.")
    return current


def validate_env_file(path):
    require(not path.is_symlink() and path.is_file() and not path.name.endswith(".example"),
            "Use an explicit local --env-file; copy deploy/.env.example without overwriting an existing file.")
    require(path.stat().st_mode & 0o077 == 0,
            "The environment file must be private: chmod 600 the selected file.")
    return path.resolve()


def initialize_directories(root):
    """Create missing directories only. Never chmod/chown existing data or credentials."""
    require(os.name == "posix", "The deployment helper requires Linux.")
    base = root / "deploy"
    entries = [("data", 0o750), ("data/gateway", 0o700),
               ("data/dashboard", 0o700), ("private", 0o700)]
    missing = []
    for relative, mode in entries:
        path = checked_path(base, relative)
        if path.exists():
            require(path.is_dir(), f"Existing {relative} is not a directory; it was left unchanged.")
            require(path.stat().st_uid == CONTAINER_UID,
                    f"Existing {relative} must be owned by UID 10001; it was left unchanged.")
            if relative == "private":
                require(path.stat().st_mode & 0o077 == 0,
                        "Existing private directory must have mode 700; it was left unchanged.")
        else:
            missing.append((path, mode))
    require(not missing or os.geteuid() in {0, CONTAINER_UID},
            "Creating container-owned directories requires sudo (or UID 10001).")
    for path, mode in missing:
        # mkdir without exist_ok rejects a concurrent replacement.
        path.mkdir(mode=mode)
        if os.geteuid() == 0:
            os.chown(path, CONTAINER_UID, CONTAINER_UID, follow_symlinks=False)
        path.chmod(mode)
    return len(missing)


def exact_origin(value):
    require(isinstance(value, str) and value and not any(c.isspace() for c in value),
            "PUBLIC_* values must be exact origins.")
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError:
        raise DeploymentError("Invalid port or hostname in PUBLIC_* origin.") from None
    require(parsed.hostname, "PUBLIC_* values must include a hostname.")
    try:
        local = parsed.hostname == "localhost" or ipaddress.ip_address(parsed.hostname).is_loopback
    except ValueError:
        local = False
    require(parsed.hostname and not parsed.username and not parsed.password
            and not parsed.path and not parsed.query and not parsed.fragment
            and (parsed.scheme == "https" or parsed.scheme == "http" and local)
            and value == f"{parsed.scheme}://{parsed.netloc}"
            and (port is None or 1 <= port <= 65535),
            "PUBLIC_* values must be HTTPS origins, or HTTP loopback origins, without paths or credentials.")


def compose_environment(compose_path, environ=None):
    environment = dict(os.environ if environ is None else environ)
    settings = set(re.findall(r"\$\{([A-Z_][A-Z_0-9]*)", compose_path.read_text(encoding="utf-8")))
    for key in list(environment):
        if key in settings or key.startswith("COMPOSE_"):
            environment.pop(key)
    environment["COMPOSE_DISABLE_ENV_FILE"] = "1"
    return environment


def validate_config(config, root):
    services = config.get("services", {})
    require(set(services) == set(SERVICES), "Compose must contain gateway, collector, and dashboard.")
    expected_ports = {"gateway": {48760, 48761}, "collector": set(), "dashboard": {48763}}
    mounts = {
        "gateway": {"/data": ("data/gateway", False)},
        "collector": {"/source": ("data/gateway", True),
                      "/run/dashboard/collector.json": ("private/collector.json", True),
                      "/data": ("data/dashboard", False)},
        "dashboard": {"/data": ("data/dashboard", True),
                      "/run/dashboard/auth.json": ("private/auth.json", True)},
    }
    host_ports = set()
    for name, service in services.items():
        require(service.get("user") == "10001:10001", f"{name} must run as UID/GID 10001.")
        require(not service.get("privileged") and service.get("network_mode") != "host",
                f"Host networking or privileged mode is not allowed for {name}.")
        require("ALL" in service.get("cap_drop", [])
                and "no-new-privileges:true" in service.get("security_opt", []),
                f"{name} must drop capabilities and disable privilege escalation.")
        actual = set()
        for port in service.get("ports", []):
            target = port.get("target")
            published = str(port.get("published", ""))
            require(port.get("host_ip") == "127.0.0.1"
                    and target in expected_ports[name] and target not in actual
                    and published.isdigit() and 1 <= int(published) <= 65535
                    and published not in host_ports and port.get("protocol", "tcp") == "tcp",
                    f"{name} must publish distinct TCP ports on 127.0.0.1 only.")
            actual.add(target)
            host_ports.add(published)
        require(actual == expected_ports[name], f"Missing loopback port for {name}.")
        volumes = service.get("volumes", [])
        require(len(volumes) == len(mounts[name]), f"Unexpected volume count for {name}.")
        seen = set()
        for volume in volumes:
            target = volume.get("target")
            require(target in mounts[name] and target not in seen, f"Unexpected mount for {name}.")
            seen.add(target)
            relative, read_only = mounts[name][target]
            expected = checked_path(root / "deploy", relative)
            require(volume.get("type") == "bind"
                    and Path(volume.get("source", "")).absolute() == expected.absolute()
                    and bool(volume.get("read_only")) == read_only
                    and volume.get("bind", {}).get("create_host_path", False) is False,
                    f"Unexpected source, permissions, or directory creation for {name} mount.")
    require(services["collector"].get("network_mode") == "none", "Collector must have no network.")
    require(services["dashboard"].get("read_only") and services["collector"].get("read_only"),
            "Dashboard and collector root filesystems must be read-only.")
    require(services["gateway"].get("profiles", []) == []
            and services["collector"].get("profiles") == ["dashboard"]
            and services["dashboard"].get("profiles") == ["dashboard"],
            "Only collector and dashboard may use the optional dashboard profile.")
    build = services["gateway"].get("build", {})
    require(Path(build.get("context", "")).resolve() == root.resolve()
            and build.get("dockerfile") == "deploy/Dockerfile"
            and not build.get("additional_contexts"), "Gateway must build directly from this repository.")
    require(Path(services["dashboard"].get("build", {}).get("context", "")).resolve()
            == (root / "services/dashboard").resolve(), "Unexpected dashboard build context.")
    args = build.get("args", {})
    for key in ("PUBLIC_API_ORIGIN", "PUBLIC_ADMIN_ORIGIN", "PUBLIC_STATS_ORIGIN"):
        exact_origin(args.get(key))


def verify_runtime(root, selected):
    required = set()
    if set(selected) & {"gateway", "collector"}:
        required.add("data/gateway")
    if set(selected) & {"collector", "dashboard"}:
        required.add("data/dashboard")
    if "collector" in selected:
        required.update({"private/collector.json", "data/gateway/codexmanager.db"})
    if "dashboard" in selected:
        required.add("private/auth.json")
    for relative in sorted(required):
        path = checked_path(root / "deploy", relative)
        is_file = relative.endswith((".json", ".db"))
        require(path.is_file() if is_file else path.is_dir(),
                f"Missing {relative}; initialize directories or dashboard configuration first.")
        metadata = path.stat()
        require(metadata.st_uid == CONTAINER_UID, f"{relative} must be owned by UID 10001.")
        if relative.startswith("private/"):
            require(stat.S_IMODE(metadata.st_mode) & 0o077 == 0,
                    f"{relative} must not be accessible to group or others.")


def selected_services(dashboard, requested):
    require(all(name in SERVICES for name in requested), "Service must be gateway, collector, or dashboard.")
    return list(dict.fromkeys(requested or (SERVICES if dashboard else ("gateway",))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", required=True, type=Path)
    parser.add_argument("--dashboard", action="store_true", help="Include the optional collector and dashboard")
    parser.add_argument("command", choices=("init", "build", "up", "status", "stop", "check"))
    parser.add_argument("services", nargs="*", metavar="SERVICE")
    args = parser.parse_args()
    env_file = validate_env_file(args.env_file)
    selected = selected_services(args.dashboard, args.services)
    if args.command == "init":
        count = initialize_directories(ROOT)
        print(f"Created {count} deployment directories; existing data and credentials were left unchanged.")
        return
    compose_path = ROOT / "deploy/compose.yml"
    environment = compose_environment(compose_path)
    base = ["docker", "compose", "--project-directory", str(ROOT / "deploy"),
            "--env-file", str(env_file), "-f", str(compose_path)]
    config = json.loads(capture([*base, "--profile", "dashboard", "config", "--format", "json"], env=environment))
    require(isinstance(config, dict), "Expected an object from Compose configuration check.")
    validate_config(config, ROOT)
    compose = base + (["--profile", "dashboard"] if set(selected) & {"collector", "dashboard"} else [])
    if args.command == "build":
        build_services = list(dict.fromkeys("dashboard" if name == "collector" else name for name in selected))
        subprocess.run([*compose, "build", *build_services], env=environment, check=True)
    elif args.command in {"up", "check"}:
        verify_runtime(ROOT, selected)
        if args.command == "up":
            subprocess.run([*compose, "up", "-d", "--no-build", "--pull", "never", *selected],
                           env=environment, check=True)
            subprocess.run([*compose, "ps", *selected], env=environment, check=True)
        else:
            print("Checks passed: Compose security boundaries and selected runtime prerequisites.")
    else:
        subprocess.run([*compose, "ps" if args.command == "status" else "stop", *selected],
                       env=environment, check=True)


if __name__ == "__main__":
    try:
        main()
    except (DeploymentError, OSError, ValueError, TypeError, KeyError, subprocess.CalledProcessError) as error:
        message = str(error) if isinstance(error, DeploymentError) else "Deployment check or operation failed; inspect local prerequisites."
        print(f"Error: {message}", file=sys.stderr)
        sys.exit(1)
