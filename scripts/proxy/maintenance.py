"""Private, transactional maintenance for static Mihomo subscriptions."""

from __future__ import annotations

import copy
import datetime as dt
import fcntl
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from contextlib import contextmanager
from dataclasses import dataclass

import yaml


BUILTINS = {"DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE"}
HOST_PATTERN = re.compile(r"@([^\s:@]+):(\d+)$")
MAX_DOWNLOAD_BYTES = 8 * 1024 * 1024


class MaintenanceError(Exception):
    """Only constant, credential-free status labels belong in this exception."""


def error_label(error):
    if isinstance(error, MaintenanceError):
        return str(error)
    if isinstance(error, urllib.error.HTTPError):
        return "HTTP_" + str(error.code)
    return type(error).__name__


def report(status, **fields):
    print(json.dumps({"status": status, **fields}, ensure_ascii=False), flush=True)


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.chmod(0o700)


def atomic_write(path, content):
    path = Path(path)
    previous = path.stat() if path.exists() else None
    fd, temporary = tempfile.mkstemp(prefix="." + path.name + ".", dir=path.parent)
    try:
        if previous is not None:
            os.fchown(fd, previous.st_uid, previous.st_gid)
        with os.fdopen(fd, "wb") as output:
            output.write(content)
            output.flush()
            os.fchmod(output.fileno(), stat.S_IMODE(previous.st_mode) if previous is not None else 0o600)
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def yaml_bytes(value):
    return yaml.safe_dump(value, allow_unicode=True, sort_keys=False).encode()


def config_from_bytes(content):
    value = yaml.safe_load(content.decode("utf-8-sig"))
    if not isinstance(value, dict):
        raise MaintenanceError("invalid_yaml_config")
    return value


def resolve(base, value):
    if not isinstance(value, str) or not value.strip():
        raise MaintenanceError("invalid_path_setting")
    path = Path(value).expanduser()
    return (path if path.is_absolute() else base / path).resolve()


def positive_integer(value, minimum=1):
    if type(value) is not int or value < minimum:
        raise MaintenanceError("invalid_numeric_setting")
    return value


def https_url(value):
    parsed = urllib.parse.urlparse(value) if isinstance(value, str) else None
    if not parsed or parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise MaintenanceError("invalid_https_setting")
    return value


def proxy_url(value):
    parsed = urllib.parse.urlparse(value) if isinstance(value, str) else None
    if not parsed or parsed.scheme != "http" or not parsed.hostname or parsed.username or parsed.password:
        raise MaintenanceError("invalid_proxy_setting")
    # Requests carrying private subscription URLs only go to a local proxy.
    try:
        if not ipaddress.ip_address(parsed.hostname).is_loopback or not parsed.port:
            raise ValueError
    except ValueError:
        raise MaintenanceError("proxy_must_be_loopback") from None
    return value


@dataclass
class Target:
    name: str
    directory: Path
    config_file: Path
    binary: Path
    selections_file: Path | None
    us_auto: dict | None


@dataclass
class Settings:
    subscription_url_file: Path
    source_file: Path
    state_dir: Path
    lock_file: Path
    download_proxy: str
    node_identity: str
    targets: list[Target]


def load_settings(path):
    path = Path(path).resolve()
    raw = json.loads(path.read_text())
    if not isinstance(raw, dict) or raw.get("version") != 1:
        raise MaintenanceError("invalid_maintenance_settings")
    base = path.parent
    state = resolve(base, raw["state_dir"])
    identity = raw.get("node_identity", "name-hostname")
    if identity not in {"name-hostname", "server"}:
        raise MaintenanceError("invalid_node_identity_setting")
    targets, names, paths = [], set(), {resolve(base, raw["source_file"])}
    for item in raw.get("targets", []):
        if not isinstance(item, dict) or not re.fullmatch(r"[A-Za-z0-9_-]{1,32}", item.get("name", "")):
            raise MaintenanceError("invalid_target_setting")
        name = item["name"]
        if name in names:
            raise MaintenanceError("duplicate_target_name")
        names.add(name)
        directory = resolve(base, item["directory"])
        config = resolve(directory, item.get("config_file", "config.yaml"))
        selections = resolve(directory, item["selections_file"]) if item.get("selections_file") else None
        for candidate in [config, selections]:
            if candidate is not None:
                if candidate in paths:
                    raise MaintenanceError("duplicate_managed_path")
                paths.add(candidate)
        policy = copy.deepcopy(item.get("us_auto"))
        if policy is not None:
            if not isinstance(policy, dict):
                raise MaintenanceError("invalid_us_auto_setting")
            for key in ["group", "selector"]:
                if not isinstance(policy.get(key), str) or not policy[key] or policy[key] in BUILTINS:
                    raise MaintenanceError("invalid_us_group_setting")
            if policy["group"] == policy["selector"]:
                raise MaintenanceError("invalid_us_group_setting")
            replaced = policy.get("replaced_groups", [])
            if not isinstance(replaced, list) or any(not isinstance(x, str) or not x for x in replaced):
                raise MaintenanceError("invalid_replaced_groups_setting")
            if policy["selector"] in replaced or any(x in BUILTINS for x in replaced):
                raise MaintenanceError("invalid_replaced_groups_setting")
            policy["manifest_file"] = resolve(base, policy["manifest_file"])
            if policy["manifest_file"] in paths:
                raise MaintenanceError("duplicate_managed_path")
            policy["health_check_url"] = https_url(policy.get("health_check_url", "https://www.gstatic.com/generate_204"))
            policy["interval"] = positive_integer(policy.get("interval", 180))
            policy["tolerance"] = positive_integer(policy.get("tolerance", 30), 0)
            policy["timeout"] = positive_integer(policy.get("timeout", 5000))
            policy["expected_status"] = positive_integer(policy.get("expected_status", 204))
            if policy["expected_status"] > 599 or policy["expected_status"] < 100:
                raise MaintenanceError("invalid_expected_status_setting")
            policy["egress_proxy"] = proxy_url(policy["egress_proxy"])
            policy["egress_check_url"] = https_url(policy.get("egress_check_url", "https://www.cloudflare.com/cdn-cgi/trace"))
        targets.append(Target(name, directory, config, resolve(directory, item.get("binary", "mihomo")), selections, policy))
    if not targets:
        raise MaintenanceError("missing_maintenance_targets")
    return Settings(resolve(base, raw["subscription_url_file"]), resolve(base, raw["source_file"]), state,
                    resolve(base, raw["lock_file"]) if raw.get("lock_file") else state / "maintenance.lock",
                    proxy_url(raw["download_proxy"]), identity, targets)


def node_id(node, identity="name-hostname"):
    if not isinstance(node, dict) or not isinstance(node.get("name"), str) or not node["name"]:
        raise MaintenanceError("invalid_node_object")
    if identity == "server":
        value = node.get("server")
        if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9.:-]+", value):
            raise MaintenanceError("invalid_node_server")
        return value.lower()
    match = HOST_PATTERN.search(node["name"])
    if match is None:
        raise MaintenanceError("unmapped_node_hostname")
    return match.group(1).lower()


def node_index(config, identity="name-hostname"):
    nodes = config.get("proxies")
    if not isinstance(nodes, list) or not nodes:
        raise MaintenanceError("missing_static_nodes")
    indexed, names = {}, set()
    for node in nodes:
        key = node_id(node, identity)
        if key in indexed or node["name"] in names or node["name"] in BUILTINS:
            raise MaintenanceError("duplicate_node_identity")
        indexed[key] = node
        names.add(node["name"])
    return indexed


def group_index(config):
    groups = config.get("proxy-groups", [])
    if not isinstance(groups, list):
        raise MaintenanceError("invalid_proxy_groups")
    indexed = {}
    for group in groups:
        if not isinstance(group, dict) or not isinstance(group.get("name"), str) or not group["name"]:
            raise MaintenanceError("invalid_proxy_group")
        if group["name"] in indexed or group["name"] in BUILTINS:
            raise MaintenanceError("duplicate_proxy_group")
        if "proxies" in group and (not isinstance(group["proxies"], list) or any(not isinstance(x, str) for x in group["proxies"])):
            raise MaintenanceError("invalid_proxy_group_members")
        indexed[group["name"]] = group
    return indexed


def merge_nodes(old, subscription, identity="name-hostname"):
    before, after = node_index(old, identity), node_index(subscription, identity)
    issues = ["hostname_roster_changed"] if set(before) != set(after) else []
    mapping = {node["name"]: after[key]["name"] for key, node in before.items() if key in after}
    candidate = copy.deepcopy(old)
    candidate["proxies"] = copy.deepcopy(subscription["proxies"])
    groups = group_index(candidate)
    if set(groups) & {node["name"] for node in candidate["proxies"]}:
        issues.append("node_group_name_collision")
    for group in groups.values():
        if "proxies" in group:
            group["proxies"] = [mapping.get(name, name) for name in group["proxies"]]
        if "default-selected" in group:
            group["default-selected"] = mapping.get(group["default-selected"], group["default-selected"])
    valid = {node["name"] for node in candidate["proxies"]} | set(groups) | BUILTINS
    if any(name not in valid for group in groups.values() for name in group.get("proxies", [])):
        issues.append("unresolved_group_member")
    for rule in old.get("rules", []):
        if isinstance(rule, str) and any(name in rule and name != new for name, new in mapping.items()):
            issues.append("renamed_node_referenced_by_rule")
            break
    return candidate, mapping, issues


def read_manifest(policy, content=None):
    try:
        value = json.loads(policy["manifest_file"].read_bytes() if content is None else content)
    except (OSError, ValueError):
        raise MaintenanceError("us_verification_unavailable") from None
    if not isinstance(value, dict) or value.get("version") != 1 or not isinstance(value.get("verified_nodes"), dict):
        raise MaintenanceError("invalid_us_verification_manifest")
    try:
        date = dt.datetime.fromisoformat(value["verified_at_utc"].replace("Z", "+00:00"))
        if date.utcoffset() != dt.timedelta(0):
            raise ValueError
    except (KeyError, ValueError, TypeError, AttributeError):
        raise MaintenanceError("invalid_us_verification_timestamp") from None
    return value


def validate_us_auto(candidate, policy, manifest, identity="name-hostname"):
    groups = group_index(candidate)
    group = groups.get(policy["group"])
    if group is None:
        return []
    members = group.get("proxies")
    if group.get("type") != "url-test" or not isinstance(members, list) or not members or group.get("use") or len(set(members)) != len(members):
        return ["us_auto_requires_static_url_test_members"]
    nodes = {node["name"]: node for node in node_index(candidate, identity).values()}
    records = manifest.get("verified_nodes", {})
    issues = []
    for name in members:
        if name not in nodes:
            issues.append("us_auto_unknown_member")
            continue
        node = nodes[name]
        key = node_id(node, identity)
        record = records.get(key)
        if not isinstance(record, dict):
            issues.append("us_auto_unverified_member")
        elif record.get("country") != "US":
            issues.append("us_auto_non_us_member")
        elif record.get("hostname") != key or any(record.get(field) != node.get(field) for field in ("server", "type", "port")):
            issues.append("us_auto_node_changed")
        else:
            try:
                ipaddress.ip_address(record["exit_ip"])
            except (KeyError, ValueError, TypeError):
                issues.append("us_auto_invalid_exit_ip")
    return sorted(set(issues))


def build_us_auto(old, policy, manifest, identity="name-hostname"):
    candidate = copy.deepcopy(old)
    indexed = node_index(candidate, identity)
    records = manifest["verified_nodes"]
    identities = [key for key, record in records.items() if isinstance(record, dict) and record.get("country") == "US"]
    if not identities or any(key not in indexed for key in identities):
        raise MaintenanceError("us_manifest_members_missing")
    members = [indexed[key]["name"] for key in identities]
    groups = group_index(candidate)
    if policy["group"] in {node["name"] for node in indexed.values()}:
        raise MaintenanceError("node_group_name_collision")
    selector = groups.get(policy["selector"])
    if selector is None or selector.get("type") != "select" or selector.get("use"):
        raise MaintenanceError("us_selector_requires_static_select_group")
    removed = set(policy.get("replaced_groups", [])) | {policy["group"]}
    replacements = {name: policy["group"] for name in removed}
    for group in groups.values():
        if "proxies" in group:
            group["proxies"] = list(dict.fromkeys(replacements.get(name, name) for name in group["proxies"]))
        if "default-selected" in group:
            group["default-selected"] = replacements.get(group["default-selected"], group["default-selected"])
    selector["proxies"] = [policy["group"], *members]
    selector["default-selected"] = policy["group"]
    candidate["proxy-groups"] = [group for name, group in groups.items() if name not in removed]
    candidate["proxy-groups"].append({"name": policy["group"], "type": "url-test", "proxies": members,
                                     "url": policy["health_check_url"], "interval": policy["interval"],
                                     "tolerance": policy["tolerance"], "lazy": False, "timeout": policy["timeout"],
                                     "expected-status": policy["expected_status"]})
    rules = candidate.get("rules", [])
    remapped = []
    for rule in rules:
        if isinstance(rule, str):
            fields = rule.split(",")
            action = -2 if fields[-1].lower() == "no-resolve" and len(fields) > 1 else -1
            fields[action] = replacements.get(fields[action], fields[action])
            rule = ",".join(fields)
        remapped.append(rule)
    candidate["rules"] = remapped
    candidate.setdefault("profile", {})["store-selected"] = True
    issues = validate_us_auto(candidate, policy, manifest, identity)
    if issues:
        raise MaintenanceError(issues[0])
    return candidate, replacements


def download_subscription(settings):
    url = settings.subscription_url_file.read_text().strip()
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise MaintenanceError("invalid_subscription_url")
    return proxy_fetch(settings.download_proxy, url, MAX_DOWNLOAD_BYTES)


def proxy_fetch(proxy, url, limit, timeout=40):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({"http": proxy, "https": proxy}))
    request = urllib.request.Request(url, headers={"User-Agent": "mihomo", "Accept": "*/*"})
    previous = {key: os.environ.get(key) for key in ("NO_PROXY", "no_proxy")}
    try:
        for key in previous:
            os.environ[key] = ""
        with opener.open(request, timeout=timeout) as response:
            content = response.read(limit + 1)
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
    if len(content) > limit:
        raise MaintenanceError("response_too_large")
    return content


def controller(config, method, route, payload=None):
    address = config.get("external-controller")
    if not isinstance(address, str) or not re.fullmatch(r"127\.0\.0\.1:\d+", address):
        raise MaintenanceError("controller_must_be_loopback")
    headers = {"Authorization": "Bearer " + str(config.get("secret", ""))}
    data = json_bytes(payload) if payload is not None else None
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request("http://" + address + route, data=data, headers=headers, method=method)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=15) as response:
        content = response.read(4 * 1024 * 1024 + 1)
    if len(content) > 4 * 1024 * 1024:
        raise MaintenanceError("controller_response_too_large")
    return json.loads(content) if content else None


def runtime_selections(config):
    state = controller(config, "GET", "/proxies")
    if not isinstance(state, dict) or not isinstance(state.get("proxies"), dict):
        raise MaintenanceError("invalid_controller_state")
    result = {}
    for name, group in group_index(config).items():
        if group.get("type") == "select":
            current = state["proxies"].get(name, {}).get("now")
            if not isinstance(current, str):
                raise MaintenanceError("selector_state_unavailable")
            result[name] = current
    return result


def restore_selections(config, selections):
    for name, current in selections.items():
        controller(config, "PUT", "/proxies/" + urllib.parse.quote(name, safe=""), {"name": current})
    if runtime_selections(config) != selections:
        raise MaintenanceError("selector_verification_failed")


def reload_config(config, path):
    controller(config, "PUT", "/configs?force=true", {"path": str(path)})


def validate_candidate(target, candidate, work):
    runtime = work / "validation-runtime"
    private_dir(runtime)
    for name in ["Country.mmdb", "geoip.dat", "geosite.dat"]:
        resource = target.directory / name
        if resource.exists():
            (runtime / name).symlink_to(resource)
    try:
        result = subprocess.run([str(target.binary), "-d", str(runtime), "-t", "-f", str(candidate)],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=40, check=False)
        atomic_write(work / "validation.log", result.stdout)
        return result.returncode == 0
    except subprocess.TimeoutExpired as error:
        atomic_write(work / "validation.log", error.stdout or b"")
        return False


def verify_us_runtime(config, policy):
    group = group_index(config).get(policy["group"])
    if group is None:
        return
    proxy_port = urllib.parse.urlparse(policy["egress_proxy"]).port
    if proxy_port not in {config.get("mixed-port"), config.get("port")}:
        raise MaintenanceError("egress_proxy_does_not_match_instance")
    query = urllib.parse.urlencode({"url": policy["health_check_url"], "timeout": policy["timeout"], "expected": policy["expected_status"]})
    delays = controller(config, "GET", "/group/" + urllib.parse.quote(policy["group"], safe="") + "/delay?" + query)
    if not isinstance(delays, dict) or not any(type(value) is int and value > 0 for name, value in delays.items() if name in group["proxies"]):
        raise MaintenanceError("no_healthy_us_node")
    state = controller(config, "GET", "/proxies")
    state = state["proxies"] if isinstance(state, dict) and isinstance(state.get("proxies"), dict) else {}
    auto = state.get(policy["group"], {})
    if auto.get("type") != "URLTest" or set(auto.get("all", [])) != set(group["proxies"]) or auto.get("now") not in group["proxies"] or auto.get("fixed", False):
        raise MaintenanceError("us_auto_runtime_mismatch")
    # The selected group must carry the probe; otherwise a US result proves a different path.
    if state.get(policy["selector"], {}).get("now") == policy["group"]:
        body = proxy_fetch(policy["egress_proxy"], policy["egress_check_url"], 64 * 1024, 20).decode()
        trace = dict(line.split("=", 1) for line in body.splitlines() if "=" in line)
        if trace.get("loc") != "US":
            raise MaintenanceError("egress_not_us")


@contextmanager
def maintenance_lock(settings):
    private_dir(settings.state_dir)
    fd = os.open(settings.lock_file, os.O_RDWR | os.O_CREAT, 0o600)
    os.fchmod(fd, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise MaintenanceError("another_maintenance_is_running") from None
        yield
    finally:
        os.close(fd)


def new_work(settings):
    name = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + secrets.token_hex(3)
    path = settings.state_dir / name
    private_dir(path)
    return path


def read_optional(path):
    return path.read_bytes() if path.exists() else None


def snapshot_targets(settings):
    snapshots, runtimes = {}, []
    for target in settings.targets:
        content = target.config_file.read_bytes()
        old = config_from_bytes(content)
        snapshots[target.config_file] = content
        if target.selections_file:
            snapshots[target.selections_file] = read_optional(target.selections_file)
        if target.us_auto:
            snapshots[target.us_auto["manifest_file"]] = target.us_auto["manifest_file"].read_bytes()
        runtimes.append({"target": target, "old": old, "selected": runtime_selections(old)})
    return snapshots, runtimes


def record_selections(updates, original, target, mapping, selector_override=None):
    if not target.selections_file:
        return
    content = original[target.selections_file]
    recorded = json.loads(content) if content is not None else {}
    if not isinstance(recorded, dict):
        raise MaintenanceError("invalid_selection_record")
    recorded = {group: mapping.get(name, name) if isinstance(name, str) else name for group, name in recorded.items()}
    if selector_override:
        recorded.update(selector_override)
    replacement = json_bytes(recorded)
    if content is not None or selector_override:
        updates[target.selections_file] = replacement


def check_selections(candidate, selections):
    groups = group_index(candidate)
    if any(name not in groups.get(group, {}).get("proxies", []) for group, name in selections.items()):
        raise MaintenanceError("current_selector_missing")


def commit_changes(originals, updates, runtimes, changes, work):
    backup = work / "backups"
    private_dir(backup)
    index = []
    for number, (path, content) in enumerate(originals.items()):
        name = str(number) + ".backup"
        if content is not None:
            atomic_write(backup / name, content)
        index.append({"path": str(path), "backup": name if content is not None else None})
    atomic_write(backup / "index.json", json_bytes(index))
    if any(read_optional(path) != content for path, content in originals.items()):
        raise MaintenanceError("configuration_changed_during_validation")
    if any(runtime_selections(item["old"]) != item["selected"] for item in runtimes):
        raise MaintenanceError("selector_changed_during_validation")
    written = []
    try:
        for path, content in updates.items():
            written.append(path)
            atomic_write(path, content)
        for item in changes:
            target = item["target"]
            reload_config(item["old"], target.config_file)
            restore_selections(item["candidate"], item["mapped_selected"])
            if target.us_auto:
                verify_us_runtime(item["candidate"], target.us_auto)
        changed_names = {item["target"].name for item in changes}
        for item in runtimes:
            if item["target"].name not in changed_names and runtime_selections(item["old"]) != item["selected"]:
                raise MaintenanceError("unmodified_selector_changed")
        if any(read_optional(path) != content for path, content in originals.items() if path not in updates):
            raise MaintenanceError("unmodified_file_changed")
    except BaseException as error:
        rollback_errors = []
        for path in reversed(written):
            try:
                content = originals[path]
                current = read_optional(path)
                if current not in (content, updates[path]):
                    raise MaintenanceError("concurrent_edit_blocks_rollback")
                if content is None:
                    path.unlink(missing_ok=True)
                else:
                    atomic_write(path, content)
            except BaseException as rollback_error:
                rollback_errors.append(error_label(rollback_error))
        for item in changes:
            try:
                # Never reload a concurrent file edit that was not restored.
                if read_optional(item["target"].config_file) != originals[item["target"].config_file]:
                    raise MaintenanceError("concurrent_edit_blocks_reload")
                reload_config(item["old"], item["target"].config_file)
                restore_selections(item["old"], item["selected"])
            except BaseException as rollback_error:
                rollback_errors.append(error_label(rollback_error))
        result = {"status": "rollback_failed" if rollback_errors else "rolled_back", "reason": error_label(error),
                  "rollback_errors": rollback_errors, "backup_path": str(backup)}
        atomic_write(work / "result.json", json_bytes(result))
        return 3, result
    result = {"status": "applied", "targets": [item["target"].name for item in changes], "backup_path": str(backup)}
    atomic_write(work / "result.json", json_bytes(result))
    return 0, result


def update_subscription(settings, *, downloaded=None, apply=False):
    with maintenance_lock(settings):
        work = new_work(settings)
        raw = Path(downloaded).read_bytes() if downloaded else download_subscription(settings)
        if len(raw) > MAX_DOWNLOAD_BYTES:
            raise MaintenanceError("response_too_large")
        atomic_write(work / "downloaded.yaml", raw)
        subscription = config_from_bytes(raw)
        if subscription.get("proxy-providers") or subscription.get("rule-providers"):
            raise MaintenanceError("provider_subscription_requires_manual_migration")
        node_index(subscription, settings.node_identity)
        originals, runtimes = snapshot_targets(settings)
        originals[settings.source_file] = settings.source_file.read_bytes()
        updates, changes, issues = {settings.source_file: raw}, [], []
        for item in runtimes:
            target = item["target"]
            candidate, mapping, target_issues = merge_nodes(item["old"], subscription, settings.node_identity)
            if target.us_auto and target.us_auto["group"] in group_index(candidate):
                target_issues += validate_us_auto(candidate, target.us_auto,
                                                  read_manifest(target.us_auto, originals[target.us_auto["manifest_file"]]),
                                                  settings.node_identity)
            selected = {group: mapping.get(name, name) for group, name in item["selected"].items()}
            check_selections(candidate, selected)
            directory = work / target.name
            private_dir(directory)
            candidate_path = directory / "config.candidate.yaml"
            updates[target.config_file] = yaml_bytes(candidate)
            atomic_write(candidate_path, updates[target.config_file])
            record_selections(updates, originals, target, mapping)
            if not validate_candidate(target, candidate_path, directory):
                target_issues.append("mihomo_validation_failed")
            issues.extend(target.name + ":" + issue for issue in target_issues)
            changes.append({**item, "candidate": candidate, "mapped_selected": selected})
        if issues:
            return 2, {"status": "refused", "reasons": sorted(set(issues)), "candidates": str(work)}
        if not apply:
            return 0, {"status": "dry_run_passed", "nodes": len(subscription["proxies"]), "candidates": str(work)}
        return commit_changes(originals, updates, runtimes, changes, work)


def configure_us_auto(settings, target_name, *, apply=False, verify=False):
    with maintenance_lock(settings):
        target = next((target for target in settings.targets if target.name == target_name), None)
        if target is None or not target.us_auto:
            raise MaintenanceError("unknown_us_auto_target")
        policy = target.us_auto
        if verify:
            manifest = read_manifest(policy)
            config = config_from_bytes(target.config_file.read_bytes())
            if policy["group"] not in group_index(config):
                raise MaintenanceError("us_auto_group_missing")
            issues = validate_us_auto(config, policy, manifest, settings.node_identity)
            if issues:
                raise MaintenanceError(issues[0])
            if runtime_selections(config).get(policy["selector"]) != policy["group"]:
                raise MaintenanceError("us_auto_not_selected")
            verify_us_runtime(config, policy)
            return 0, {"status": "verified", "target": target.name}
        originals, runtimes = snapshot_targets(settings)
        manifest = read_manifest(policy, originals[policy["manifest_file"]])
        item = next(item for item in runtimes if item["target"].name == target_name)
        candidate, mapping = build_us_auto(item["old"], policy, manifest, settings.node_identity)
        selected = {group: mapping.get(name, name) for group, name in item["selected"].items()}
        selected[policy["selector"]] = policy["group"]
        check_selections(candidate, selected)
        work = new_work(settings)
        path = work / "config.candidate.yaml"
        updates = {target.config_file: yaml_bytes(candidate)}
        atomic_write(path, updates[target.config_file])
        record_selections(updates, originals, target, mapping, {policy["selector"]: policy["group"]})
        if not validate_candidate(target, path, work):
            return 2, {"status": "refused", "reason": "mihomo_validation_failed", "candidates": str(work)}
        result = {"target": target.name, "members": len(group_index(candidate)[policy["group"]]["proxies"]),
                  "interval_seconds": policy["interval"], "tolerance_ms": policy["tolerance"], "candidates": str(work)}
        if not apply:
            return 0, {"status": "dry_run_passed", **result}
        return commit_changes(originals, updates, runtimes, [{**item, "candidate": candidate, "mapped_selected": selected}], work)
