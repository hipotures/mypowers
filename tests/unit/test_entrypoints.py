import sys
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from mypowers import entrypoints
from mypowers.daemon import main as daemon


def test_lazy_daemon_entrypoint_and_missing_server_extra(monkeypatch, capsys):
    function = Mock()
    importer = Mock(return_value=SimpleNamespace(main=function))
    monkeypatch.setattr(entrypoints.importlib, "import_module", importer)
    entrypoints.daemon()
    importer.assert_called_once_with("mypowers.daemon.main")
    function.assert_called_once()
    importer.side_effect = ModuleNotFoundError("missing", name="dependency")
    with pytest.raises(SystemExit) as caught:
        entrypoints.daemon()
    assert caught.value.code == 2
    message = capsys.readouterr().err
    assert "mypowers[server]" in message
    assert "uv run --locked --extra server mypowersd" in message


def test_daemon_config_foreground_workers_and_private_token(config, tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(daemon, "load", lambda *args, **kwargs: config)
    monkeypatch.setattr(sys, "argv", ["mypowersd", "check-config"])
    daemon.main()
    assert "data_dir" in capsys.readouterr().out
    runner = Mock()
    monkeypatch.setattr(daemon.uvicorn, "run", runner)
    monkeypatch.setattr(sys, "argv", ["mypowersd"])
    daemon.main()
    assert runner.call_args.kwargs["workers"] == 1
    monkeypatch.setattr(sys, "argv", ["mypowersd", "--workers", "2"])
    with pytest.raises(SystemExit) as caught:
        daemon.main()
    assert caught.value.code == 2
    path = tmp_path / "token"
    monkeypatch.setattr(sys, "argv", ["mypowersd", "token", "generate", "--output", str(path)])
    daemon.main()
    assert path.stat().st_mode & 0o777 == 0o600
    assert len(path.read_text().strip()) >= 32
    assert path.read_text().strip() not in capsys.readouterr().out
    with pytest.raises(SystemExit):
        daemon.main()
