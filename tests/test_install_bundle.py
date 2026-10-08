"""Boot integration must preserve SSH and refuse unknown container startup scripts."""

import importlib.util
from pathlib import Path

import pytest

path = Path(__file__).resolve().parents[1] / "deploy/bundle/install.py"
spec = importlib.util.spec_from_file_location("bundle_install", path)
assert spec and spec.loader
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


def test_boot_integration_preserves_ssh_and_is_idempotent():
    original = (
        "import os\nimport subprocess\nimport time\n"
        'subprocess.run(["/usr/sbin/sshd"], check=True)\n'
        "while True:\n    time.sleep(3600)\n"
    )
    updated = installer.boot_script(original)
    assert 'subprocess.run(["/usr/sbin/sshd"], check=True)' in updated
    assert updated.index("subprocess.run") < updated.index("os.execv")
    assert "/run/mypowers" in updated
    compile(updated, "/startup.py", "exec")
    assert installer.boot_script(updated) == updated
    assert original.endswith("time.sleep(3600)\n")


def test_unknown_boot_script_is_rejected():
    with pytest.raises(ValueError, match="refusing"):
        installer.boot_script('print("custom entrypoint")\n')


update_path = path.with_name("update.py")
update_spec = importlib.util.spec_from_file_location("bundle_update", update_path)
assert update_spec and update_spec.loader
updater = importlib.util.module_from_spec(update_spec)
update_spec.loader.exec_module(updater)


def test_update_switches_atomically_and_restarts_only_daemon(tmp_path, monkeypatch):
    old, new = tmp_path / "old", tmp_path / "new"
    old.mkdir()
    new.mkdir()
    current = tmp_path / "current"
    current.symlink_to(old)
    monkeypatch.setattr(updater, "CURRENT", current)
    calls = []
    monkeypatch.setattr(updater, "supervisor", lambda action: calls.append(action))
    monkeypatch.setattr(updater, "health", lambda python: "new-instance")
    updater.activate(new, old, "old-instance")
    assert current.resolve() == new
    assert calls == ["restart"]


def test_failed_restart_restores_previous_code(tmp_path, monkeypatch):
    old, new = tmp_path / "old", tmp_path / "new"
    old.mkdir()
    new.mkdir()
    current = tmp_path / "current"
    current.symlink_to(old)
    monkeypatch.setattr(updater, "CURRENT", current)
    calls = []

    def restart(action):
        calls.append(current.resolve())
        if len(calls) == 1:
            raise RuntimeError("new daemon failed")

    monkeypatch.setattr(updater, "supervisor", restart)
    monkeypatch.setattr(updater, "health", lambda python: "rollback-instance")
    with pytest.raises(RuntimeError, match="new daemon failed"):
        updater.activate(new, old, "old-instance")
    assert current.resolve() == old
    assert calls == [new, old]


def test_unhealthy_update_rolls_back_without_accepting_old_process(tmp_path, monkeypatch):
    old, new = tmp_path / "old", tmp_path / "new"
    old.mkdir()
    new.mkdir()
    current = tmp_path / "current"
    current.symlink_to(old)
    monkeypatch.setattr(updater, "CURRENT", current)
    monkeypatch.setattr(updater, "supervisor", lambda action: None)
    monkeypatch.setattr(updater, "health", lambda python: "old-instance")
    ticks = iter([0, 31])
    monkeypatch.setattr(updater.time, "monotonic", lambda: next(ticks))
    with pytest.raises(RuntimeError, match="health check"):
        updater.activate(new, old, "old-instance")
    assert current.resolve() == old
