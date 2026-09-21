import importlib.util
import json
from pathlib import Path
import subprocess

import pytest


spec = importlib.util.spec_from_file_location("publication", Path(__file__).parents[1] / "check-publication.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def synthetic_token():
    return ("gh" + "p_") + "a" * 36


def exception(path, category, value):
    return {"path": path, "category": category, "sha256": module.digest(value),
            "reason": "Reviewed synthetic fixture with no external account."}


def allowlist(*entries):
    return json.dumps({"version": 1, "entries": list(entries)}).encode()


@pytest.mark.parametrize("path", ["deploy/server.env", "private/auth.json", "data/live.db",
                                     ".cloudflared/credentials.json", "backup.db-wal", "login.local.txt"])
def test_private_files_rejected(path):
    assert (path, 0, "private-path") in list(module.findings(path, b"example"))


@pytest.mark.parametrize("path", ["README.md", "deploy/server.env.example", "crates/core/src/data/mod.rs"])
def test_source_and_examples_allowed(path):
    assert not list(module.findings(path, b"https://api.example.com"))


def test_secret_detection_never_echoes_value_or_exempts_tests():
    secret = synthetic_token()
    result = list(module.findings("tests/fixture.rs", secret.encode()))
    assert result == [("tests/fixture.rs", 1, "github-token")]
    assert secret not in repr(result)
    url = "https://" + "test:fixture@example.com"
    assert list(module.findings("tests/url.rs", url.encode())) == [("tests/url.rs", 1, "credential-url")]


@pytest.mark.parametrize("value,category", [
    ("/home/" + "private-user/project", "user-home-path"),
    ("/opt/" + "private-service/config", "private-install-path"),
    ('password="' + 'synthetic-secret-12345' + '"', "embedded-secret"),
    ('SERVICE_' + 'TOKEN=' + 'z' * 32, "environment-secret"),
])
def test_sensitive_content(value, category):
    assert ("fixture.txt", 1, category) in list(module.findings("fixture.txt", value.encode()))


@pytest.mark.parametrize("value", ["gateway.example.internal", "api.gateway.example.internal",
                                   "https://API.GATEWAY.EXAMPLE.INTERNAL:443/path", "gateway.example.internal."])
def test_custom_denied_domain_matches_exact_domain_and_subdomains(value):
    rules = (module.domain_pattern("gateway.example.internal"),)
    assert list(module.findings("fixture.txt", value.encode(), domain_patterns=rules)) == [("fixture.txt", 1, "denied-domain")]


@pytest.mark.parametrize("value", ["notgateway.example.internal", "gateway.example.internal.evil",
                                   "a-gateway.example.internal", "gateway-example.internal"])
def test_custom_denied_domain_observes_hostname_boundaries(value):
    rules = (module.domain_pattern("gateway.example.internal"),)
    assert not list(module.findings("fixture.txt", value.encode(), domain_patterns=rules))


def test_custom_denied_network_matches_only_contained_ipv4_addresses():
    networks = (module.ipaddress.IPv4Network("192.168.20.0/24"),)
    assert list(module.findings("fixture.txt", b"http://192.168.20.42:8080", networks=networks)) == [("fixture.txt", 1, "denied-address")]
    assert not list(module.findings("fixture.txt", b"192.168.21.42 192.168.20.999 1192.168.20.42", networks=networks))


def test_example_windows_home_allowed_but_private_home_rejected():
    assert not list(module.findings("fixture.rs", ("C:" + "\\Users\\example\\.codex").encode()))
    assert list(module.findings("fixture.rs", ("C:" + "\\Users\\private-person\\.codex").encode()))


def test_rpc_method_is_not_an_absolute_home_path():
    assert not list(module.findings("dispatch.rs", b'"accountManager/users/list"'))


def test_exception_matches_only_reviewed_path_and_digest():
    content = synthetic_token().encode()
    allowed = module.load_allowlist(allowlist(exception("tests/fixture.rs", "github-token", content)))
    assert module.review({"tests/fixture.rs": content}, allowed) == []
    assert ("src/fixture.rs", 1, "github-token") in module.review({"src/fixture.rs": content}, allowed)
    assert ("tests/fixture.rs", 1, "github-token") in module.review({"tests/fixture.rs": content + b"b"}, allowed)
    assert module.review({"tests/fixture.rs": b"clean"}, allowed) == [("tests/fixture.rs", 0, "unused-allowlist-entry")]


@pytest.mark.parametrize("patch", [{"path": "tests/*"}, {"path": "../private"}, {"sha256": "bad"},
                                     {"reason": ""}, {"category": "private-path"}, {"path": 7}])
def test_invalid_exceptions_rejected(patch):
    entry = exception("tests/fixture.rs", "github-token", b"example")
    entry.update(patch)
    with pytest.raises(ValueError):
        module.load_allowlist(allowlist(entry))


def test_duplicate_exceptions_rejected():
    entry = exception("tests/fixture.rs", "github-token", b"example")
    with pytest.raises(ValueError):
        module.load_allowlist(allowlist(entry, entry))


def test_binary_requires_exact_review_and_correct_asset_type():
    content = b"\x89PNG\r\n\x1a\n\0synthetic-image"
    path = "apps/public/fixture.png"
    assert list(module.findings(path, content)) == [(path, 0, "static-asset-review-required")]
    allowed = module.load_allowlist(allowlist(exception(path, "static-asset-review-required", content)))
    assert module.review({path: content}, allowed) == []
    assert (path, 0, "static-asset-review-required") in module.review({path: content + b"changed"}, allowed)
    assert list(module.findings("dump.bin", content)) == [("dump.bin", 0, "binary-file-review-required")]
    assert list(module.findings(path, b"not-image\0")) == [(path, 0, "binary-file-review-required")]


def test_asset_exception_cannot_hide_embedded_token():
    path = "apps/public/fixture.png"
    content = b"\x89PNG\r\n\x1a\n\0" + synthetic_token().encode()
    allowed = module.load_allowlist(allowlist(exception(path, "static-asset-review-required", content)))
    assert any(category == "github-token" for _, _, category in module.review({path: content}, allowed))


@pytest.fixture
def repository(tmp_path, monkeypatch):
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    monkeypatch.chdir(tmp_path)
    return tmp_path


def stage(path, content):
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(content)
    subprocess.run(["git", "add", "--", path], check=True)


def test_checks_staged_blob_even_if_worktree_was_cleaned(repository, capsys):
    secret = synthetic_token().encode()
    stage("README.md", secret)
    Path("README.md").write_text("clean")
    assert module.main([]) == 1
    output = capsys.readouterr().out
    assert "README.md:1: github-token" in output
    assert secret.decode() not in output


def test_untracked_source_blocks_publication(repository, capsys):
    stage("README.md", b"clean")
    Path("new-source.py").write_text("print('hello')")
    assert module.main([]) == 1
    assert "new-source.py:0: untracked-file-not-reviewed" in capsys.readouterr().out


def test_repeatable_private_deny_arguments_are_external_and_redacted(repository, capsys):
    stage("README.md", b"https://api.gateway.example.internal\nhttps://admin.example.internal\n192.168.20.42\n")
    assert module.main([]) == 0
    capsys.readouterr()
    assert module.main(["--deny-domain", "gateway.example.internal", "--deny-domain", "admin.example.internal",
                        "--deny-network", "192.168.20.0/24", "--deny-network", "172.16.0.0/16"]) == 1
    output = capsys.readouterr().out
    assert "README.md:1: denied-domain" in output
    assert "README.md:2: denied-domain" in output
    assert "README.md:3: denied-address" in output
    assert "example.internal" not in output
    assert "192.168.20.42" not in output


@pytest.mark.parametrize("arguments", [["--deny-domain", "https://gateway.example.internal"],
                                      ["--deny-domain", "*.example.internal"],
                                      ["--deny-network", "invalid-network"]])
def test_invalid_private_deny_arguments_fail_closed_without_echo(arguments, repository, capsys):
    stage("README.md", b"clean")
    with pytest.raises(SystemExit) as error:
        module.main(arguments)
    assert error.value.code == 2
    assert arguments[-1] not in capsys.readouterr().err


def test_ignored_source_entrypoint_still_blocks_publication(repository, capsys):
    stage("README.md", b"clean")
    stage(".gitignore", b"crates/\n")
    manifest = Path("crates/service/Cargo.toml")
    manifest.parent.mkdir(parents=True)
    manifest.write_text("[package]\nname = 'fixture'\n")
    assert module.main([]) == 1
    assert "crates/service/Cargo.toml:0: source-entrypoint-not-staged" in capsys.readouterr().out


def test_allowlist_must_be_staged_and_report_is_redacted(repository, tmp_path, capsys):
    content = synthetic_token().encode()
    stage("tests/fixture.rs", content)
    stage(module.ALLOWLIST, allowlist(exception("tests/fixture.rs", "github-token", content)))
    Path(module.ALLOWLIST).write_text("invalid working tree JSON")
    report = tmp_path.parent / "publication-report.json"
    assert module.main(["--report-json", str(report)]) == 0
    assert json.loads(report.read_text())["findings"] == []
    assert content.decode() not in report.read_text() + capsys.readouterr().out


def test_malformed_staged_allowlist_fails_closed(repository, capsys):
    stage("README.md", b"clean")
    stage(module.ALLOWLIST, b"{}")
    assert module.main([]) == 1
    assert "invalid-allowlist" in capsys.readouterr().out


def test_symlink_is_not_a_source_blob(repository, capsys):
    Path("external").symlink_to("/tmp/fixture")
    subprocess.run(["git", "add", "external"], check=True)
    assert module.main([]) == 1
    assert "unmerged-or-nonregular-index-entry" in capsys.readouterr().out
