from colibri.permissions_store import ProjectGrants, ProjectPermissionStore


def test_project_permission_store_loads_missing_file_as_empty(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)

    grants = store.load()

    assert grants.shell_commands == set()
    assert grants.tool_names == set()


def test_project_permission_store_saves_and_loads_deduplicated_toml(tmp_path):
    store = ProjectPermissionStore.for_cwd(tmp_path)

    store.save(
        ProjectGrants(
            shell_commands={"pwd", "git status", "pwd"},
            tool_names={"files.list", "files.read", "files.list"},
        )
    )
    grants = store.load()

    assert grants.shell_commands == {"pwd", "git status"}
    assert grants.tool_names == {"files.list", "files.read"}
    text = (tmp_path / ".colibri" / "permissions.toml").read_text(encoding="utf-8")
    assert "[shell]" in text
    assert 'commands = ["git status", "pwd"]' in text
    assert "[tools]" in text
    assert 'names = ["files.list", "files.read"]' in text
