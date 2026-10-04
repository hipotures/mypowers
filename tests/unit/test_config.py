import pytest

from mypowers.config import base_url, client_config, environment, token
from mypowers.config.server import load


def write_env(tmp_path, data):
    path = tmp_path / "selected.env"
    path.write_text(data)
    return str(path)


def test_precedence_and_relative_sources(tmp_path, monkeypatch):
    nested = tmp_path / "nested"
    nested.mkdir()
    yaml = nested / "app.yaml"
    yaml.write_text("api:\n  auth_required: false\n  port: 1000\ndata_dir: ./yaml-data\n")
    env = write_env(
        tmp_path,
        "MYPOWERS_CONFIG=./nested/app.yaml\nMYPOWERS_PORT=2000\nMYPOWERS_LOG_DIR=./file-logs\n",
    )
    monkeypatch.setenv("MYPOWERS_PORT", "3000")
    cfg = load(env, port=4000)
    assert cfg.api.port == 4000
    assert cfg.data_dir == nested / "yaml-data"
    assert cfg.log_dir == tmp_path / "file-logs"
    assert load(env).api.port == 3000
    monkeypatch.delenv("MYPOWERS_PORT")
    assert load(env).api.port == 2000


@pytest.mark.parametrize(
    "yaml",
    [
        "api:\n  port: 2\n  port: 3\n",
        "unknown: true\n",
        "device:\n  address: nonsense\n",
        "bluetooth:\n  stale_after_seconds: 4\n",
        "history:\n  interval_seconds: .nan\n",
        "history:\n  enabled: 'false'\n",
        "!!python/object/apply:os.system ['false']",
        "[]",
        "schema_version: 2",
        "logging:\n  level: TRACE\n",
    ],
)
def test_invalid_yaml_without_echo(tmp_path, yaml):
    path = tmp_path / "bad.yaml"
    path.write_text(yaml)
    with pytest.raises((ValueError, __import__("yaml").YAMLError)):
        load(config=str(path))


def test_explicit_missing_defaults_unknown_env_and_no_implicit_dotenv(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))
    (tmp_path / ".env").write_text("MYPOWERS_AUTH_REQUIRED=false\n")
    assert environment() == {}
    with pytest.raises(ValueError, match="token"):
        load()
    with pytest.raises(ValueError, match="does not exist"):
        load(config=str(tmp_path / "absent"))
    with pytest.raises(ValueError):
        environment(str(tmp_path / "absent.env"))
    monkeypatch.setenv("MYPOWERS_AUTH_REQUIERD", "not-a-secret")
    with pytest.raises(ValueError, match="Unknown environment names"):
        load()


@pytest.mark.parametrize(
    "values",
    [
        {"MYPOWERS_ENV": "production", "MYPOWERS_AUTH_REQUIRED": "false"},
        {
            "MYPOWERS_ENV": "production",
            "MYPOWERS_BACKEND": "simulated",
            "MYPOWERS_API_TOKEN": "x" * 40,
        },
        {
            "MYPOWERS_ENV": "production",
            "MYPOWERS_BIND_HOST": "0.0.0.0",
            "MYPOWERS_API_TOKEN": "x" * 40,
        },
        {"MYPOWERS_AUTH_REQUIRED": "false", "MYPOWERS_BIND_HOST": "0.0.0.0"},
        {"MYPOWERS_AUTH_REQUIRED": "false", "MYPOWERS_BIND_HOST": "untrusted.example"},
        {"MYPOWERS_AUTH_REQUIRED": "maybe"},
        {
            "MYPOWERS_ENV": "production",
            "MYPOWERS_PUBLIC_URL": "http://localhost",
            "MYPOWERS_API_TOKEN": "x" * 40,
        },
    ],
)
def test_safety_policies(monkeypatch, tmp_path, values):
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))
    for key, value in values.items():
        monkeypatch.setenv(key, value)
    with pytest.raises(ValueError):
        load()


def test_token_file_secret_never_serialized(tmp_path, monkeypatch):
    secret = "s" * 48
    path = tmp_path / "token"
    path.write_text(secret)
    path.chmod(0o600)
    monkeypatch.setenv("MYPOWERS_API_TOKEN_FILE", str(path))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))
    cfg = load()
    assert secret not in str(cfg.model_dump()) and secret not in repr(cfg)
    path.chmod(0o644)
    with pytest.raises(ValueError, match="permissions"):
        load()
    path.chmod(0o600)
    monkeypatch.setenv("MYPOWERS_API_TOKEN", secret)
    with pytest.raises(ValueError, match="either"):
        load()
    with pytest.raises(ValueError):
        token({"MYPOWERS_API_TOKEN": "change-me" * 10})


@pytest.mark.parametrize(
    "url",
    [
        "ftp://host",
        "http://user:secret@host",
        "http://host?token=x",
        "http://host#x",
        "http://host/path",
        "http://host:99999",
        "http:///",
        "http://:80",
    ],
)
def test_client_bad_url(url):
    with pytest.raises(ValueError):
        base_url(url)


def test_client_overrides_timezone_and_timeouts(tmp_path, monkeypatch):
    env = write_env(
        tmp_path, "MYPOWERS_SERVER_URL=http://localhost:1\nMYPOWERS_TIMEZONE=Europe/Warsaw\n"
    )
    assert (
        client_config(env, server="https://localhost:2", timezone="UTC").server
        == "https://localhost:2"
    )
    with pytest.raises(ValueError):
        client_config(env, timezone="Invalid/Timezone")
    with pytest.raises(ValueError):
        client_config(env, timeout=-1)
    monkeypatch.setenv("MYPOWERS_TEST_ALLOW_OUTPUT_CHANGES", "0")
    assert client_config(env).timezone == "Europe/Warsaw"
