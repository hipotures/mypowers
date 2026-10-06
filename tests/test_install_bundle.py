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
