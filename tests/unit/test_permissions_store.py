from pathlib import Path

import colibri.permissions_store as permissions_store
from colibri.permissions_store import UserGrants, UserPermissionStore


def test_user_permission_store_loads_missing_file_as_empty(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()

    grants = store.load()

    assert grants.shell_commands == set()
    assert grants.shell_executables == set()
    assert grants.tool_names == set()
    assert grants.file_roots == set()


def test_user_permission_store_saves_and_loads_deduplicated_toml(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()

    store.save(
        UserGrants(
            shell_commands={"pwd", "git status", "pwd"},
            shell_executables={"cargo", "git", "cargo"},
            tool_names={"files.list", "files.read", "files.list"},
            file_roots={"/tmp/workspace", "/tmp/workspace"},
        )
    )
    grants = store.load()

    assert grants.shell_commands == {"pwd", "git status"}
    assert grants.shell_executables == {"cargo", "git"}
    assert grants.tool_names == {"files.list", "files.read"}
    assert grants.file_roots == {"/tmp/workspace"}
    text = (tmp_path / ".colibri" / "permissions.toml").read_text(encoding="utf-8")
    assert "[shell]" in text
    assert 'commands = ["git status", "pwd"]' in text
    assert 'executables = ["cargo", "git"]' in text
    assert "[tools]" in text
    assert 'names = ["files.list", "files.read"]' in text
    assert "[files]" in text
    assert 'roots = ["/tmp/workspace"]' in text
    assert "paths =" not in text


def test_user_permission_store_loads_file_roots(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()
    store.path.parent.mkdir()
    store.path.write_text('[files]\nroots = ["/tmp/workspace"]\n', encoding="utf-8")

    grants = store.load()

    assert grants.file_roots == {"/tmp/workspace"}


def test_user_permission_store_loads_shell_executables(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()
    store.path.parent.mkdir()
    store.path.write_text('[shell]\nexecutables = ["cargo", "git"]\n', encoding="utf-8")

    grants = store.load()

    assert grants.shell_executables == {"cargo", "git"}


def test_user_permission_store_ignores_obsolete_shell_prefixes(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()
    store.path.parent.mkdir()
    store.path.write_text('[shell]\nprefixes = ["cargo", "git"]\n', encoding="utf-8")

    grants = store.load()

    assert grants.shell_executables == set()


def test_user_permission_store_reuses_cached_parse_when_file_is_unchanged(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()
    store.path.parent.mkdir()
    store.path.write_text('[shell]\nexecutables = ["git"]\n', encoding="utf-8")
    original_loads = permissions_store.tomllib.loads
    parse_count = 0

    def counting_loads(text: str):
        nonlocal parse_count
        parse_count += 1
        return original_loads(text)

    monkeypatch.setattr(permissions_store.tomllib, "loads", counting_loads)

    assert store.load().shell_executables == {"git"}
    assert store.load().shell_executables == {"git"}
    assert parse_count == 1


def test_user_permission_store_refreshes_cache_after_atomic_replacement(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    store = UserPermissionStore.for_user()
    store.path.parent.mkdir()
    store.path.write_text('[shell]\nexecutables = ["git"]\n', encoding="utf-8")

    assert store.load().shell_executables == {"git"}

    replacement = Path(str(store.path) + ".replacement")
    replacement.write_text('[shell]\nexecutables = ["cargo"]\n', encoding="utf-8")
    replacement.replace(store.path)

    assert store.load().shell_executables == {"cargo"}


def test_user_permission_store_merge_preserves_concurrent_stale_grants(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    first = UserPermissionStore.for_user()
    second = UserPermissionStore.for_user()

    assert first.load() == UserGrants()
    assert second.load() == UserGrants()

    first.merge(UserGrants(shell_commands={"git status"}))
    second.merge(UserGrants(shell_executables={"cargo"}))

    grants = UserPermissionStore.for_user().load()
    assert grants.shell_commands == {"git status"}
    assert grants.shell_executables == {"cargo"}
