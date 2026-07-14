from colibri.permissions_store import ProjectGrants, ProjectPermissionStore


def test_project_permission_store_loads_missing_file_as_empty(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)

    grants = store.load()

    assert grants.shell_commands == set()
    assert grants.shell_prefixes == set()
    assert grants.tool_names == set()
    assert grants.file_roots == set()


def test_project_permission_store_saves_and_loads_deduplicated_toml(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)

    store.save(
        ProjectGrants(
            shell_commands={"pwd", "git status", "pwd"},
            shell_prefixes={"cargo test", "git status", "cargo test"},
            tool_names={"files.list", "files.read", "files.list"},
            file_roots={"/tmp/workspace", "/tmp/workspace"},
        )
    )
    grants = store.load()

    assert grants.shell_commands == {"pwd", "git status"}
    assert grants.shell_prefixes == {"cargo test", "git status"}
    assert grants.tool_names == {"files.list", "files.read"}
    assert grants.file_roots == {"/tmp/workspace"}
    text = (tmp_path / ".colibri" / "permissions.toml").read_text(encoding="utf-8")
    assert "[shell]" in text
    assert 'commands = ["git status", "pwd"]' in text
    assert 'prefixes = ["cargo test", "git status"]' in text
    assert "[tools]" in text
    assert 'names = ["files.list", "files.read"]' in text
    assert "[files]" in text
    assert 'roots = ["/tmp/workspace"]' in text
    assert "paths =" not in text


def test_project_permission_store_loads_file_roots(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)
    store.path.parent.mkdir()
    store.path.write_text('[files]\nroots = ["/tmp/workspace"]\n', encoding="utf-8")

    grants = store.load()

    assert grants.file_roots == {"/tmp/workspace"}


def test_project_permission_store_loads_shell_prefixes(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)
    store.path.parent.mkdir()
    store.path.write_text('[shell]\nprefixes = ["cargo test", "git status"]\n', encoding="utf-8")

    grants = store.load()

    assert grants.shell_prefixes == {"cargo test", "git status"}
