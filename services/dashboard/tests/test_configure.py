import json
import sys

import pytest

from dashboard import configure


def prepare(monkeypatch, tmp_path):
    directory = tmp_path / "private"
    monkeypatch.setattr(configure, "private_directory", lambda path: path.mkdir(parents=True, exist_ok=True))
    monkeypatch.setattr(configure, "password_hash", lambda password, salt: "ab" * 32)
    return directory


def test_initialize_public_origin_without_printing_password(sample, tmp_path, monkeypatch, capsys):
    directory = prepare(monkeypatch, tmp_path)
    monkeypatch.setattr(sys, "argv", ["configure", "--source-db", str(sample["db"]), "--private-dir", str(directory),
                                    "--username", "team", "--public-origin", "https://stats.example.com"])
    configure.main()
    auth = json.loads((directory / "auth.json").read_text())
    assert auth["username"] == "team"
    assert auth["cloudflare_tunnel_origins"] == ["https://stats.example.com"]
    assert "https://stats.example.com" in auth["origins"]
    assert "http://127.0.0.1:48763" in auth["origins"]
    assert (directory / "collector.json").is_file()
    assert "Password:" not in capsys.readouterr().out


@pytest.mark.parametrize("origin", ["http://stats.example.com", "https://stats.example.com/", "https://stats.example.com/path",
                                   "https://stats.example.com?x=1", "https://stats.example.com#x", "https://u:p@stats.example.com", "https://stats.example.com invalid"])
def test_invalid_origin_does_not_create_private_files(origin, tmp_path, monkeypatch):
    directory = prepare(monkeypatch, tmp_path)
    monkeypatch.setattr(sys, "argv", ["configure", "--source-db", str(tmp_path / "unused.db"),
                                    "--private-dir", str(directory), "--public-origin", origin])
    with pytest.raises(SystemExit):
        configure.main()
    assert not directory.exists()


def reject_side_effects(monkeypatch):
    def unexpected(*args, **kwargs):
        pytest.fail("Refused configuration must not read the database, prompt, generate credentials, or modify directories")

    for name in ("source_connection", "private_directory", "write_private"):
        monkeypatch.setattr(configure, name, unexpected)
    monkeypatch.setattr(configure.getpass, "getpass", unexpected)
    monkeypatch.setattr(configure.secrets, "token_urlsafe", unexpected)


@pytest.mark.parametrize("filename", ["auth.json", "collector.json", "login.local.txt"])
@pytest.mark.parametrize("kind", ["file", "directory", "symlink", "dangling-symlink"])
def test_initialization_preserves_any_existing_private_entry(filename, kind, tmp_path, monkeypatch):
    directory = tmp_path / "private"
    directory.mkdir(mode=0o750)
    existing = directory / filename
    target = tmp_path / "target"
    if kind == "file":
        existing.write_bytes(b"preserve existing configuration")
    elif kind == "directory":
        existing.mkdir()
    else:
        if kind == "symlink":
            target.write_bytes(b"preserve symlink target")
        existing.symlink_to(target)
    original = existing.lstat()
    directory_before = directory.stat()
    reject_side_effects(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["configure", "--private-dir", str(directory),
                                    "--source-db", str(tmp_path / "must-not-open.db"), "--prompt-password"])
    with pytest.raises(SystemExit) as error:
        configure.main()
    assert error.value.code == 2
    assert list(directory.iterdir()) == [existing]
    assert (existing.lstat().st_mode, existing.lstat().st_mtime_ns) == (original.st_mode, original.st_mtime_ns)
    assert (directory.stat().st_mode, directory.stat().st_mtime_ns) == (directory_before.st_mode, directory_before.st_mtime_ns)
    if kind == "file":
        assert existing.read_bytes() == b"preserve existing configuration"
    elif kind == "symlink":
        assert existing.is_symlink()
        assert target.read_bytes() == b"preserve symlink target"
    elif kind == "dangling-symlink":
        assert existing.is_symlink()
        assert not target.exists()


def test_rotation_preserves_collector_secret_aliases_and_login_identity(sample, tmp_path, monkeypatch):
    directory = prepare(monkeypatch, tmp_path)
    monkeypatch.setattr(sys, "argv", ["configure", "--source-db", str(sample["db"]),
                                    "--private-dir", str(directory), "--username", "viewer"])
    configure.main()
    collector_file = directory / "collector.json"
    collector = collector_file.read_bytes()
    collector_stat = collector_file.stat()
    original_auth = json.loads((directory / "auth.json").read_text())
    original_login = (directory / "login.local.txt").read_bytes()

    def unexpected_database_read(*args, **kwargs):
        pytest.fail("Rotation must not read the source database")

    monkeypatch.setattr(configure, "source_connection", unexpected_database_read)
    monkeypatch.setattr(sys, "argv", ["configure", "--rotate", "--private-dir", str(directory), "--username", "ignored"])
    configure.main()
    rotated_auth = json.loads((directory / "auth.json").read_text())
    assert collector_file.read_bytes() == collector
    assert collector_file.stat().st_mtime_ns == collector_stat.st_mtime_ns
    assert rotated_auth["username"] == original_auth["username"] == "viewer"
    assert rotated_auth["origins"] == original_auth["origins"]
    assert rotated_auth["salt"] != original_auth["salt"]
    assert (directory / "login.local.txt").read_bytes() != original_login


@pytest.mark.parametrize("kind", ["missing", "incomplete", "invalid", "symlink", "dangling-symlink"])
def test_rotation_requires_complete_regular_auth_before_side_effects(kind, tmp_path, monkeypatch):
    directory = tmp_path / "private"
    directory.mkdir()
    collector = directory / "collector.json"
    collector.write_bytes(b"preserve collector configuration")
    auth_file = directory / "auth.json"
    if kind in {"symlink", "dangling-symlink"}:
        target = tmp_path / "auth-target"
        if kind == "symlink":
            target.write_text("{}")
        auth_file.symlink_to(target)
    elif kind == "incomplete":
        auth_file.write_text(json.dumps({"username": "viewer", "origins": ["http://127.0.0.1:48763"]}))
    elif kind == "invalid":
        auth_file.write_text("not valid JSON")
    reject_side_effects(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["configure", "--rotate", "--private-dir", str(directory), "--prompt-password"])
    with pytest.raises(SystemExit) as error:
        configure.main()
    assert error.value.code == 2
    assert collector.read_bytes() == b"preserve collector configuration"
    assert not (directory / "login.local.txt").exists()
