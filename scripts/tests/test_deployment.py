"""Deployment safety contracts; these tests never invoke Docker or read real data."""

import copy
import importlib.util
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "deployment.py"
SPEC = importlib.util.spec_from_file_location("deployment", SCRIPT)
deployment = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(deployment)


def configuration(root):
    def service(ports, mounts):
        return {"user": "10001:10001", "cap_drop": ["ALL"],
                "security_opt": ["no-new-privileges:true"],
                "ports": [{"host_ip": "127.0.0.1", "target": port,
                           "published": str(port), "protocol": "tcp"} for port in ports],
                "volumes": [{"type": "bind", "source": str(root / "deploy" / source),
                             "target": target, "read_only": read_only,
                             "bind": {"create_host_path": False}}
                            for target, source, read_only in mounts]}
    gateway = service([48760, 48761], [("/data", "data/gateway", False)])
    gateway["build"] = {"context": str(root), "dockerfile": "deploy/Dockerfile", "args": {
        "PUBLIC_API_ORIGIN": "http://127.0.0.1:48760",
        "PUBLIC_ADMIN_ORIGIN": "http://127.0.0.1:48761",
        "PUBLIC_STATS_ORIGIN": "http://127.0.0.1:48763"}}
    collector = service([], [("/source", "data/gateway", True),
                            ("/run/dashboard/collector.json", "private/collector.json", True),
                            ("/data", "data/dashboard", False)])
    collector.update(profiles=["dashboard"], network_mode="none", read_only=True)
    dashboard = service([48763], [("/data", "data/dashboard", True),
                                ("/run/dashboard/auth.json", "private/auth.json", True)])
    dashboard.update(profiles=["dashboard"], read_only=True,
                     build={"context": str(root / "services/dashboard")})
    return {"services": {"gateway": gateway, "collector": collector, "dashboard": dashboard}}


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "deploy").mkdir()
        self.config = configuration(self.root)

    def test_portable_defaults_and_changed_host_ports_are_allowed(self):
        deployment.validate_config(self.config, self.root)
        self.config["services"]["gateway"]["ports"][0]["published"] = "58880"
        deployment.validate_config(self.config, self.root)

    def test_exposed_or_colliding_ports_are_rejected(self):
        for host in ("0.0.0.0", "::", "", None):
            config = copy.deepcopy(self.config)
            config["services"]["gateway"]["ports"][0]["host_ip"] = host
            with self.subTest(host=host), self.assertRaises(deployment.DeploymentError):
                deployment.validate_config(config, self.root)
        self.config["services"]["dashboard"]["ports"][0]["published"] = "48760"
        with self.assertRaises(deployment.DeploymentError):
            deployment.validate_config(self.config, self.root)

    def test_collector_cannot_access_network_or_write_source(self):
        for field, value in (("network_mode", "bridge"), ("read_only", False)):
            config = copy.deepcopy(self.config)
            config["services"]["collector"][field] = value
            with self.subTest(field=field), self.assertRaises(deployment.DeploymentError):
                deployment.validate_config(config, self.root)
        self.config["services"]["collector"]["volumes"][0]["read_only"] = False
        with self.assertRaises(deployment.DeploymentError):
            deployment.validate_config(self.config, self.root)

    def test_unexpected_mount_and_auto_creation_are_rejected(self):
        for field, value in (("source", "/tmp/other-database"),
                             ("bind", {"create_host_path": True})):
            config = copy.deepcopy(self.config)
            config["services"]["gateway"]["volumes"][0][field] = value
            with self.subTest(field=field), self.assertRaises(deployment.DeploymentError):
                deployment.validate_config(config, self.root)

    def test_gateway_is_default_dashboard_is_opt_in(self):
        self.assertEqual(deployment.selected_services(False, []), ["gateway"])
        self.assertEqual(deployment.selected_services(True, []), list(deployment.SERVICES))
        self.assertEqual(deployment.selected_services(False, ["collector"]), ["collector"])
        with self.assertRaises(deployment.DeploymentError):
            deployment.selected_services(False, ["other"])
        self.config["services"]["dashboard"].pop("profiles")
        with self.assertRaises(deployment.DeploymentError):
            deployment.validate_config(self.config, self.root)

    def test_origins_allow_loopback_and_https_only(self):
        for value in ("http://127.0.0.1:48760", "http://localhost:48761", "http://[::1]:48763",
                      "https://api.example.com"):
            with self.subTest(value=value):
                deployment.exact_origin(value)
        for value in ("http://example.com", "https://user:password@example.com", "https://example.com/v1",
                      "https://example.com?token=x", "https://example.com#x", "https://", "https://x:99999",
                      None, "https://exa mple.com"):
            with self.subTest(value=value), self.assertRaises(deployment.DeploymentError):
                deployment.exact_origin(value)

    def test_compose_variables_cannot_be_silently_overridden_by_shell(self):
        compose = self.root / "deploy/compose.yml"
        compose.write_text("a: ${PUBLIC_API_ORIGIN:-http://127.0.0.1}\nb: ${UPSTREAM_PROXY-}\n")
        environment = deployment.compose_environment(compose, {
            "PUBLIC_API_ORIGIN": "http://bad.example", "UPSTREAM_PROXY": "private",
            "COMPOSE_FILE": "other.yml", "PATH": "/bin", "DOCKER_HOST": "local-context"})
        self.assertEqual(environment, {"PATH": "/bin", "DOCKER_HOST": "local-context",
                                       "COMPOSE_DISABLE_ENV_FILE": "1"})

    def test_explicit_env_is_private_and_not_an_example_or_symlink(self):
        path = self.root / "local.env"
        path.write_text("UPSTREAM_PROXY=\n")
        path.chmod(0o600)
        self.assertEqual(deployment.validate_env_file(path), path)
        path.chmod(0o644)
        with self.assertRaises(deployment.DeploymentError):
            deployment.validate_env_file(path)
        example = self.root / ".env.example"
        example.write_text("")
        example.chmod(0o600)
        with self.assertRaises(deployment.DeploymentError):
            deployment.validate_env_file(example)
        linked = self.root / "linked.env"
        linked.symlink_to(path)
        with self.assertRaises(deployment.DeploymentError):
            deployment.validate_env_file(linked)

    def test_init_preserves_existing_files_and_modes(self):
        # Use this test user's UID so ordinary non-root CI can exercise idempotency.
        with patch.object(deployment, "CONTAINER_UID", os.geteuid()):
            self.assertEqual(deployment.initialize_directories(self.root), 4)
            private = self.root / "deploy/private"
            credential = private / "auth.json"
            credential.write_bytes(b"existing-secret-is-not-rewritten")
            credential.chmod(0o600)
            before = credential.stat()
            self.assertEqual(deployment.initialize_directories(self.root), 0)
            self.assertEqual(credential.read_bytes(), b"existing-secret-is-not-rewritten")
            self.assertEqual(credential.stat().st_mtime_ns, before.st_mtime_ns)
            self.assertEqual(stat.S_IMODE(private.stat().st_mode), 0o700)

    def test_init_rejects_all_invalid_existing_paths_before_creating_any(self):
        (self.root / "deploy/private").write_text("existing data")
        with self.assertRaises(deployment.DeploymentError):
            deployment.initialize_directories(self.root)
        self.assertFalse((self.root / "deploy/data").exists())
        self.assertEqual((self.root / "deploy/private").read_text(), "existing data")

    def test_init_refuses_symlinked_parent_without_touching_target(self):
        target = self.root / "outside"
        target.mkdir()
        (self.root / "deploy/data").symlink_to(target, target_is_directory=True)
        with self.assertRaises(deployment.DeploymentError):
            deployment.initialize_directories(self.root)
        self.assertEqual(list(target.iterdir()), [])

    def test_init_requires_privilege_and_chowns_only_new_directories(self):
        with patch.object(deployment.os, "geteuid", return_value=12345):
            with self.assertRaises(deployment.DeploymentError):
                deployment.initialize_directories(self.root)
        self.assertFalse((self.root / "deploy/data").exists())
        with patch.object(deployment.os, "geteuid", return_value=0), patch.object(deployment.os, "chown") as chown:
            self.assertEqual(deployment.initialize_directories(self.root), 4)
            self.assertEqual(chown.call_count, 4)
            for call in chown.call_args_list:
                self.assertEqual(call.args[1:], (10001, 10001))
                self.assertEqual(call.kwargs, {"follow_symlinks": False})

    def test_check_gateway_needs_no_database_but_collector_does(self):
        with patch.object(deployment, "CONTAINER_UID", os.geteuid()):
            deployment.initialize_directories(self.root)
            deployment.verify_runtime(self.root, ["gateway"])
            with self.assertRaises(deployment.DeploymentError):
                deployment.verify_runtime(self.root, ["collector"])


if __name__ == "__main__":
    unittest.main()
