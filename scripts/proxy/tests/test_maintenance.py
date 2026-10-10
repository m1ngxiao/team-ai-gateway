"""Synthetic node and controller fixtures; no credentials or network required."""

import copy
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unittest
from contextlib import ExitStack
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import maintenance as m


def node(label, name=None, **overrides):
    host = label + ".example.invalid"
    value = {"name": name or "Example @" + host + ":443", "server": host, "port": 443, "type": "vless",
             "uuid": "00000000-0000-4000-8000-000000000000"}
    value.update(overrides)
    return value


def config():
    first, second = node("us-a"), node("jp-b")
    return {"external-controller": "127.0.0.1:12345", "secret": "synthetic-test-controller-value",
            "proxies": [first, second],
            "proxy-groups": [{"name": "Proxy", "type": "select", "proxies": ["Auto", first["name"], second["name"]]},
                             {"name": "Auto", "type": "url-test", "proxies": [first["name"], second["name"]], "url": "https://example.invalid/204"}],
            "rules": ["DOMAIN,Auto,DIRECT", "IP-CIDR,192.0.2.0/24,Auto,no-resolve", "MATCH,Auto"],
            "profile": {"store-selected": True}, "mixed-port": 12346}


def manifest():
    first = node("us-a")
    return {"version": 1, "verified_at_utc": "2026-01-01T00:00:00Z",
            "verified_nodes": {first["server"]: {**{key: first[key] for key in ["server", "port", "type"]},
                                                 "hostname": first["server"], "country": "US", "exit_ip": "192.0.2.10"}}}


class MaintenanceTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        for name in ["personal", "business"]:
            directory = self.root / name
            directory.mkdir()
            (directory / "config.yaml").write_bytes(m.yaml_bytes(config()))
            (directory / "selection.json").write_text(json.dumps({"Proxy": "Auto"}))
        (self.root / "source.yaml").write_bytes(m.yaml_bytes(config()))
        (self.root / "manifest.json").write_text(json.dumps(manifest()))
        self.settings_file = self.root / "maintenance.json"
        self.raw = {"version": 1, "subscription_url_file": "subscription.url", "source_file": "source.yaml",
                    "state_dir": "state", "download_proxy": "http://127.0.0.1:12346",
                    "targets": [{"name": "personal", "directory": "personal", "selections_file": "selection.json"},
                                {"name": "business", "directory": "business", "selections_file": "selection.json",
                                 "us_auto": {"group": "US Auto", "selector": "Proxy", "replaced_groups": ["Auto"],
                                             "manifest_file": "manifest.json", "egress_proxy": "http://127.0.0.1:12346"}}]}
        self.settings = self.load()
        self.policy = self.settings.targets[1].us_auto
        self.states = {target.name: {"Proxy": "Auto"} for target in self.settings.targets}
        self.events = []

    def load(self):
        self.settings_file.write_text(json.dumps(self.raw))
        return m.load_settings(self.settings_file)

    def fake_selections(self, value):
        # Instance ports distinguish controllers, as they do in real deployments.
        return dict(self.states["personal" if value["mixed-port"] == 12346 else "business"])

    def mock_runtime(self, validator=None):
        # Give business a distinct controller identity without contacting either.
        value = config()
        value["mixed-port"] = 12347
        self.policy["egress_proxy"] = "http://127.0.0.1:12347"
        self.settings.targets[1].config_file.write_bytes(m.yaml_bytes(value))
        stack = ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(patch.object(m, "runtime_selections", side_effect=self.fake_selections))
        stack.enter_context(patch.object(m, "validate_candidate", side_effect=validator or (lambda *args: True)))

        def reload(value, path):
            self.events.append(("reload", path.parent.name))

        def restore(value, selected):
            name = "personal" if value["mixed-port"] == 12346 else "business"
            self.states[name] = dict(selected)
            self.events.append(("restore", name))

        stack.enter_context(patch.object(m, "reload_config", side_effect=reload))
        stack.enter_context(patch.object(m, "restore_selections", side_effect=restore))
        stack.enter_context(patch.object(m, "verify_us_runtime"))
        return stack

    def captured_files(self):
        paths = [self.settings.source_file, self.policy["manifest_file"]]
        paths += [path for target in self.settings.targets for path in [target.config_file, target.selections_file]]
        return {path: m.read_optional(path) for path in paths}

    def assert_files_equal(self, before):
        self.assertEqual(before, {path: m.read_optional(path) for path in before})

    def downloaded(self, value):
        path = self.root / "downloaded.yaml"
        path.write_bytes(m.yaml_bytes(value))
        return path

    def test_paths_resolve_relative_to_configuration_and_target(self):
        self.assertEqual(self.settings.source_file, self.root / "source.yaml")
        self.assertEqual(self.settings.targets[1].binary, self.root / "business/mihomo")
        self.assertEqual(self.policy["manifest_file"], self.root / "manifest.json")
        self.assertEqual(self.policy["interval"], 180)
        self.assertEqual(self.policy["tolerance"], 30)

    def test_invalid_settings_rejected(self):
        for field, value in [("download_proxy", "http://example.invalid:1234"), ("node_identity", "anything"), ("version", 2)]:
            with self.subTest(field=field):
                old = self.raw[field] if field in self.raw else None
                self.raw[field] = value
                with self.assertRaises(m.MaintenanceError):
                    self.load()
                if old is None:
                    del self.raw[field]
                else:
                    self.raw[field] = old

    def test_duplicate_targets_and_managed_paths_rejected(self):
        self.raw["targets"][1]["name"] = "personal"
        with self.assertRaisesRegex(m.MaintenanceError, "duplicate_target_name"):
            self.load()
        self.raw["targets"][1]["name"] = "business"
        self.raw["targets"][1]["directory"] = "personal"
        with self.assertRaisesRegex(m.MaintenanceError, "duplicate_managed_path"):
            self.load()

    def test_server_identity_allows_non_subscription_node_names(self):
        value = {"proxies": [node("us-a", name="US example")]}
        self.assertIn("us-a.example.invalid", m.node_index(value, "server"))
        with self.assertRaisesRegex(m.MaintenanceError, "unmapped_node_hostname"):
            m.node_index(value)

    def test_duplicate_node_identity_rejected(self):
        value = {"proxies": [node("us-a"), node("us-a", name="Other @us-a.example.invalid:443")]}
        with self.assertRaisesRegex(m.MaintenanceError, "duplicate_node_identity"):
            m.node_index(value)

    def test_renaming_nodes_preserves_groups_rules_and_metadata(self):
        old = config()
        old["proxy-groups"][0]["default-selected"] = old["proxies"][0]["name"]
        new = copy.deepcopy(old)
        new["proxies"][0]["name"] = "Renamed @us-a.example.invalid:443"
        candidate, mapping, issues = m.merge_nodes(old, new)
        self.assertFalse(issues)
        self.assertEqual(candidate["rules"], old["rules"])
        self.assertEqual(candidate["mixed-port"], old["mixed-port"])
        self.assertEqual(candidate["proxy-groups"][0]["default-selected"], new["proxies"][0]["name"])
        self.assertEqual(candidate["proxy-groups"][1]["proxies"][0], new["proxies"][0]["name"])
        self.assertEqual(mapping[old["proxies"][0]["name"]], new["proxies"][0]["name"])

    def test_roster_changes_and_direct_rule_renames_require_review(self):
        old, new = config(), config()
        new["proxies"].append(node("us-c"))
        self.assertIn("hostname_roster_changed", m.merge_nodes(old, new)[2])
        old["rules"] = ["MATCH," + old["proxies"][0]["name"]]
        new = config()
        new["proxies"][0]["name"] = "Renamed @us-a.example.invalid:443"
        self.assertIn("renamed_node_referenced_by_rule", m.merge_nodes(old, new)[2])

    def test_us_group_contains_only_verified_us_nodes_and_defaults(self):
        old = config()
        candidate, _ = m.build_us_auto(old, self.policy, manifest())
        groups = m.group_index(candidate)
        self.assertNotIn("Auto", groups)
        group = groups["US Auto"]
        self.assertEqual(group["proxies"], [node("us-a")["name"]])
        self.assertEqual((group["interval"], group["tolerance"], group["lazy"]), (180, 30, False))
        self.assertEqual(groups["Proxy"]["proxies"], ["US Auto", node("us-a")["name"]])
        self.assertEqual(candidate["rules"], ["DOMAIN,Auto,DIRECT", "IP-CIDR,192.0.2.0/24,US Auto,no-resolve", "MATCH,US Auto"])
        self.assertEqual(old, config())

    def test_manifest_country_and_endpoint_changes_are_rejected(self):
        candidate, _ = m.build_us_auto(config(), self.policy, manifest())
        for field, value, reason in [("server", "moved.example.invalid", "us_auto_node_changed"),
                                     ("port", 8443, "us_auto_node_changed"), ("type", "ss", "us_auto_node_changed")]:
            changed = copy.deepcopy(candidate)
            changed["proxies"][0][field] = value
            self.assertIn(reason, m.validate_us_auto(changed, self.policy, manifest()))
        changed = manifest()
        changed["verified_nodes"]["us-a.example.invalid"]["country"] = "JP"
        self.assertIn("us_auto_non_us_member", m.validate_us_auto(candidate, self.policy, changed))

    def test_unverified_unknown_and_invalid_egress_members_rejected(self):
        candidate, _ = m.build_us_auto(config(), self.policy, manifest())
        self.assertIn("us_auto_unverified_member", m.validate_us_auto(candidate, self.policy, {"verified_nodes": {}}))
        bad = manifest()
        bad["verified_nodes"]["us-a.example.invalid"]["exit_ip"] = "not-an-address"
        self.assertIn("us_auto_invalid_exit_ip", m.validate_us_auto(candidate, self.policy, bad))
        candidate["proxy-groups"][-1]["proxies"].append("DIRECT")
        self.assertIn("us_auto_unknown_member", m.validate_us_auto(candidate, self.policy, manifest()))

    def test_invalid_manifest_timestamp_rejected(self):
        value = manifest()
        value["verified_at_utc"] = "2026-01-01"
        with self.assertRaisesRegex(m.MaintenanceError, "invalid_us_verification_timestamp"):
            m.read_manifest(self.policy, m.json_bytes(value))

    def test_us_dry_run_leaves_files_and_selectors_unchanged(self):
        self.mock_runtime()
        before = self.captured_files()
        code, result = m.configure_us_auto(self.settings, "business")
        self.assertEqual(code, 0)
        self.assertEqual(result["status"], "dry_run_passed")
        self.assert_files_equal(before)
        self.assertFalse(self.events)

    def test_us_apply_preserves_personal_and_selects_business_auto(self):
        self.mock_runtime()
        before = self.captured_files()
        code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (0, "applied"))
        self.assertEqual(self.states["business"], {"Proxy": "US Auto"})
        self.assertEqual(self.states["personal"], {"Proxy": "Auto"})
        for path in [self.settings.source_file, self.policy["manifest_file"], self.settings.targets[0].config_file, self.settings.targets[0].selections_file]:
            self.assertEqual(path.read_bytes(), before[path])
        self.assertTrue((Path(result["backup_path"]) / "index.json").exists())

    def test_reload_failure_restores_files_and_prior_selector(self):
        self.mock_runtime()
        before = self.captured_files()
        with patch.object(m, "reload_config", side_effect=[RuntimeError("private detail omitted"), None]):
            code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (3, "rolled_back"))
        self.assertEqual(result["reason"], "RuntimeError")
        self.assert_files_equal(before)
        self.assertEqual(self.states["business"], {"Proxy": "Auto"})

    def test_apply_preserves_existing_config_and_selection_ownership_and_mode(self):
        self.mock_runtime()
        target = self.settings.targets[1]
        paths = [target.config_file, target.selections_file]
        for path in paths:
            path.chmod(0o640)
        before = {path: (path.stat().st_uid, path.stat().st_gid, stat.S_IMODE(path.stat().st_mode)) for path in paths}
        code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (0, "applied"))
        self.assertEqual(before, {path: (path.stat().st_uid, path.stat().st_gid, stat.S_IMODE(path.stat().st_mode)) for path in paths})
        for backup in Path(result["backup_path"]).iterdir():
            self.assertEqual(stat.S_IMODE(backup.stat().st_mode), 0o600)

    def test_rollback_preserves_existing_0640_file_attributes_and_content(self):
        self.mock_runtime()
        target = self.settings.targets[1]
        paths = [target.config_file, target.selections_file]
        for path in paths:
            path.chmod(0o640)
        before = {path: (path.read_bytes(), path.stat().st_uid, path.stat().st_gid, stat.S_IMODE(path.stat().st_mode)) for path in paths}
        def fail_egress(*args):
            for path in paths:
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o640)
            raise m.MaintenanceError("egress_not_us")
        with patch.object(m, "verify_us_runtime", side_effect=fail_egress):
            code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (3, "rolled_back"))
        self.assertEqual(before, {path: (path.read_bytes(), path.stat().st_uid, path.stat().st_gid, stat.S_IMODE(path.stat().st_mode)) for path in paths})

    def test_live_us_verification_failure_rolls_back(self):
        self.mock_runtime()
        before = self.captured_files()
        with patch.object(m, "verify_us_runtime", side_effect=m.MaintenanceError("egress_not_us")):
            code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (3, "rolled_back"))
        self.assert_files_equal(before)
        self.assertEqual(self.states["business"], {"Proxy": "Auto"})

    def test_partial_write_failure_restores_files(self):
        self.mock_runtime()
        before = self.captured_files()
        real_write = m.atomic_write
        failed = False

        def write(path, content):
            nonlocal failed
            if path == self.settings.targets[1].selections_file and not failed:
                failed = True
                raise OSError("synthetic write failure")
            real_write(path, content)

        with patch.object(m, "atomic_write", side_effect=write):
            code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (3, "rolled_back"))
        self.assert_files_equal(before)

    def test_missing_selection_file_is_removed_on_rollback(self):
        self.mock_runtime()
        self.settings.targets[1].selections_file.unlink()
        with patch.object(m, "verify_us_runtime", side_effect=m.MaintenanceError("egress_not_us")):
            code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (3, "rolled_back"))
        self.assertFalse(self.settings.targets[1].selections_file.exists())

    def test_configuration_change_during_validation_refuses_apply(self):
        def validate(*args):
            self.settings.targets[1].config_file.write_text("concurrent edit")
            return True

        self.mock_runtime(validate)
        with self.assertRaisesRegex(m.MaintenanceError, "configuration_changed_during_validation"):
            m.configure_us_auto(self.settings, "business", apply=True)
        self.assertFalse(self.events)
        self.assertEqual(self.settings.targets[1].config_file.read_text(), "concurrent edit")

    def test_selector_change_during_validation_refuses_apply(self):
        def validate(*args):
            self.states["business"] = {"Proxy": node("us-a")["name"]}
            return True

        self.mock_runtime(validate)
        before = self.captured_files()
        with self.assertRaisesRegex(m.MaintenanceError, "selector_changed_during_validation"):
            m.configure_us_auto(self.settings, "business", apply=True)
        self.assert_files_equal(before)
        self.assertFalse(self.events)

    def test_concurrent_edit_during_apply_is_not_overwritten_by_rollback(self):
        self.mock_runtime()
        target = self.settings.targets[1]

        def fail(*args):
            target.config_file.write_text("operator edited during reload")
            raise RuntimeError()

        with patch.object(m, "reload_config", side_effect=fail):
            code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (3, "rollback_failed"))
        self.assertIn("concurrent_edit_blocks_rollback", result["rollback_errors"])
        self.assertEqual(target.config_file.read_text(), "operator edited during reload")

    def test_subscription_dry_run_does_not_mutate_running_files(self):
        self.mock_runtime()
        before = self.captured_files()
        value = config()
        value["proxies"][0]["name"] = "New @us-a.example.invalid:443"
        code, result = m.update_subscription(self.settings, downloaded=self.downloaded(value))
        self.assertEqual((code, result["status"]), (0, "dry_run_passed"))
        self.assert_files_equal(before)
        self.assertFalse(self.events)

    def test_subscription_refresh_preserves_us_group_and_selection(self):
        self.mock_runtime()
        m.configure_us_auto(self.settings, "business", apply=True)
        value = config()
        value["proxies"][0]["name"] = "New @us-a.example.invalid:443"
        code, result = m.update_subscription(self.settings, downloaded=self.downloaded(value), apply=True)
        self.assertEqual((code, result["status"]), (0, "applied"))
        actual = m.config_from_bytes(self.settings.targets[1].config_file.read_bytes())
        self.assertEqual(m.group_index(actual)["US Auto"]["proxies"], [value["proxies"][0]["name"]])
        self.assertEqual(self.states["business"], {"Proxy": "US Auto"})
        self.assertEqual(self.states["personal"], {"Proxy": "Auto"})

    def test_subscription_endpoint_change_refused_with_us_group_active(self):
        self.mock_runtime()
        m.configure_us_auto(self.settings, "business", apply=True)
        before = self.captured_files()
        value = config()
        value["proxies"][0]["port"] = 8443
        code, result = m.update_subscription(self.settings, downloaded=self.downloaded(value), apply=True)
        self.assertEqual((code, result["status"]), (2, "refused"))
        self.assertIn("business:us_auto_node_changed", result["reasons"])
        self.assert_files_equal(before)

    def test_second_instance_reload_failure_rolls_back_all_subscription_files(self):
        self.mock_runtime()
        before = self.captured_files()
        value = config()
        value["proxies"][0]["name"] = "Renamed @us-a.example.invalid:443"
        with patch.object(m, "reload_config", side_effect=[None, RuntimeError(), None, None]):
            code, result = m.update_subscription(self.settings, downloaded=self.downloaded(value), apply=True)
        self.assertEqual((code, result["status"]), (3, "rolled_back"))
        self.assert_files_equal(before)
        self.assertEqual(self.states, {"personal": {"Proxy": "Auto"}, "business": {"Proxy": "Auto"}})

    def test_provider_subscription_rejected(self):
        value = config()
        value["proxy-providers"] = {"example": {"type": "http"}}
        with self.assertRaisesRegex(m.MaintenanceError, "provider_subscription_requires_manual_migration"):
            m.update_subscription(self.settings, downloaded=self.downloaded(value), apply=True)

    def test_mihomo_validation_failure_refuses_apply(self):
        self.mock_runtime(lambda *args: False)
        before = self.captured_files()
        code, result = m.configure_us_auto(self.settings, "business", apply=True)
        self.assertEqual((code, result["status"]), (2, "refused"))
        self.assert_files_equal(before)
        self.assertFalse(self.events)

    def test_atomic_files_and_candidate_directories_private(self):
        path = self.root / "private-file"
        m.atomic_write(path, b"synthetic")
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.mock_runtime()
        _, result = m.configure_us_auto(self.settings, "business")
        self.assertEqual(stat.S_IMODE(Path(result["candidates"]).stat().st_mode), 0o700)

    def test_lock_refuses_overlapping_operations(self):
        with m.maintenance_lock(self.settings):
            with self.assertRaisesRegex(m.MaintenanceError, "another_maintenance_is_running"):
                with m.maintenance_lock(self.settings):
                    pass

    def test_controller_rejects_remote_address_before_network(self):
        with self.assertRaisesRegex(m.MaintenanceError, "controller_must_be_loopback"):
            m.controller({"external-controller": "example.invalid:1234"}, "GET", "/proxies")

    def test_runtime_non_us_and_fixed_group_are_rejected(self):
        self.policy["egress_proxy"] = "http://127.0.0.1:12346"
        candidate, _ = m.build_us_auto(config(), self.policy, manifest())
        state = {"proxies": {"Proxy": {"now": "US Auto"},
                             "US Auto": {"type": "URLTest", "all": [node("us-a")["name"]], "now": node("us-a")["name"]}}}
        responses = [{node("us-a")["name"]: 100}, state]
        with patch.object(m, "controller", side_effect=responses), patch.object(m, "proxy_fetch", return_value=b"loc=JP\n"):
            with self.assertRaisesRegex(m.MaintenanceError, "egress_not_us"):
                m.verify_us_runtime(candidate, self.policy)
        state["proxies"]["US Auto"]["fixed"] = True
        with patch.object(m, "controller", side_effect=[responses[0], state]):
            with self.assertRaisesRegex(m.MaintenanceError, "us_auto_runtime_mismatch"):
                m.verify_us_runtime(candidate, self.policy)

    def test_egress_proxy_must_belong_to_target_instance(self):
        candidate, _ = m.build_us_auto(config(), self.policy, manifest())
        self.policy["egress_proxy"] = "http://127.0.0.1:9999"
        with patch.object(m, "controller") as controller:
            with self.assertRaisesRegex(m.MaintenanceError, "egress_proxy_does_not_match_instance"):
                m.verify_us_runtime(candidate, self.policy)
            controller.assert_not_called()

    def test_validation_uses_isolated_runtime_and_private_log(self):
        target = self.settings.targets[0]
        target.binary.write_text("#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done\n")
        target.binary.chmod(0o700)
        resource = target.directory / "Country.mmdb"
        resource.write_bytes(b"synthetic geodata")
        work = self.root / "validation"
        work.mkdir()
        candidate = work / "candidate.yaml"
        candidate.write_bytes(m.yaml_bytes(config()))
        self.assertTrue(m.validate_candidate(target, candidate, work))
        self.assertEqual((work / "validation-runtime/Country.mmdb").resolve(), resource)
        self.assertIn(str(work / "validation-runtime"), (work / "validation.log").read_text())
        self.assertEqual(stat.S_IMODE((work / "validation.log").stat().st_mode), 0o600)

    def test_private_exception_details_are_not_reported(self):
        self.assertEqual(m.error_label(RuntimeError("synthetic private data")), "RuntimeError")


if __name__ == "__main__":
    unittest.main()
