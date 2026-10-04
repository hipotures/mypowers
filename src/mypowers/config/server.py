"""Strict YAML configuration with explicit source-relative paths."""

import ipaddress
import os
import re
from pathlib import Path
from typing import Any, Literal

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator

from mypowers.config import base_url, environment, token
from mypowers.contracts import Level
from mypowers.protocol import PROFILE, STATION_ADDRESS, STATION_NAME


class Settings(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, allow_inf_nan=False)


class Device(Settings):
    address: str = STATION_ADDRESS
    expected_name: str = STATION_NAME
    profile: Literal["s300-v2-qualified-2026-10-04"] = PROFILE  # type: ignore[assignment]

    @field_validator("address")
    @classmethod
    def mac(cls, value: str) -> str:
        if not re.fullmatch(r"(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}", value):
            raise ValueError("Use a full MAC address.")
        return value.upper()


class Bluetooth(Settings):
    adapter_address: str = "F4:4E:FC:A1:CB:FF"
    scan_timeout_seconds: float = Field(default=20, gt=0, le=20)
    setup_timeout_seconds: float = Field(default=25, gt=0, le=25)
    first_sample_timeout_seconds: float = Field(default=5, gt=0, le=5)
    stale_after_seconds: float = Field(default=3, gt=0, le=3)
    reconnect_after_seconds: float = Field(default=10, ge=3, le=120)

    @field_validator("adapter_address")
    @classmethod
    def adapter_mac(cls, value: str) -> str:
        return Device.mac(value)


class History(Settings):
    enabled: bool = True
    interval_seconds: float = Field(default=10, gt=0, le=86400)


class Logging(Settings):
    level: str = "INFO"
    max_file_bytes: int = Field(default=10485760, ge=1024, le=1073741824)
    backup_count: int = Field(default=5, ge=1, le=20)

    @field_validator("level")
    @classmethod
    def level_valid(cls, value: str) -> str:
        return Level(value).value


class API(Settings):
    bind_host: str = "127.0.0.1"
    port: int = Field(default=8765, ge=1, le=65535)
    auth_required: bool = True


class ServerConfig(Settings):
    schema_version: Literal[1] = 1
    device: Device = Field(default_factory=Device)
    bluetooth: Bluetooth = Field(default_factory=Bluetooth)
    history: History = Field(default_factory=History)
    logging: Logging = Field(default_factory=Logging)
    api: API = Field(default_factory=API)
    environment: Literal["development", "production"] = "development"
    backend: Literal["ble", "simulated"] = "ble"
    data_dir: Path
    log_dir: Path
    runtime_dir: Path
    api_token: str | None = Field(default=None, repr=False, exclude=True)
    public_url: str | None = None


class UniqueLoader(yaml.SafeLoader):
    pass


def unique_mapping(loader: UniqueLoader, node: yaml.MappingNode) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node)
        if not isinstance(key, str) or key in result:
            raise ValueError("Duplicate or non-string YAML key.")
        result[key] = loader.construct_object(value_node)
    return result


UniqueLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, unique_mapping)


def load(
    env_file: str | None = None, config: str | None = None, **overrides: object
) -> ServerConfig:
    values = environment(env_file)
    home = Path.home()
    state = Path(os.environ.get("XDG_STATE_HOME", str(home / ".local/state"))) / "mypowers"
    default_config = (
        Path(os.environ.get("XDG_CONFIG_HOME", str(home / ".config"))) / "mypowers/config.yaml"
    )
    selected = config or values.get("MYPOWERS_CONFIG")
    path = Path(selected).resolve() if selected else default_config
    data: dict[str, Any] = {}
    if path.exists():
        with path.open() as file:
            data = yaml.load(file, Loader=UniqueLoader)
        if not isinstance(data, dict):
            raise ValueError("YAML root must be a mapping.")
        for key in ("data_dir", "log_dir", "runtime_dir"):
            if key in data:
                data[key] = (path.parent / data[key]).resolve()
    elif selected:
        raise ValueError("Selected YAML file does not exist.")
    data.setdefault(
        "data_dir", Path(os.environ.get("XDG_DATA_HOME", str(home / ".local/share"))) / "mypowers"
    )
    data.setdefault("log_dir", state / "logs")
    runtime = (
        Path(os.environ["XDG_RUNTIME_DIR"]) / "mypowers"
        if "XDG_RUNTIME_DIR" in os.environ
        else state / "run"
    )
    data.setdefault("runtime_dir", runtime)
    mappings = {
        "ENV": ("environment",),
        "BACKEND": ("backend",),
        "PUBLIC_URL": ("public_url",),
        "BIND_HOST": ("api", "bind_host"),
        "PORT": ("api", "port"),
        "AUTH_REQUIRED": ("api", "auth_required"),
        "LOG_LEVEL": ("logging", "level"),
        "DATA_DIR": ("data_dir",),
        "LOG_DIR": ("log_dir",),
        "RUNTIME_DIR": ("runtime_dir",),
    }
    for name, keys in mappings.items():
        value: Any = values.get("MYPOWERS_" + name)
        if value is None:
            continue
        if name == "PORT":
            value = int(value)
        elif name == "AUTH_REQUIRED":
            if value.lower() not in {"true", "false", "1", "0"}:
                raise ValueError("AUTH_REQUIRED must be a boolean.")
            value = value.lower() in {"true", "1"}
        elif name.endswith("_DIR"):
            value = Path(value).resolve()
        target = data
        for key in keys[:-1]:
            target = target.setdefault(key, {})
        target[keys[-1]] = value
    for key, value in overrides.items():
        if value is not None:
            if key in {"bind_host", "port"}:
                data.setdefault("api", {})[key] = value
            else:
                data[key] = value
    data["api_token"] = token(values)
    try:
        cfg = ServerConfig.model_validate(data)
    except ValidationError as error:
        locations = [".".join(map(str, item["loc"])) for item in error.errors(include_input=False)]
        raise ValueError("Invalid configuration fields: " + ", ".join(locations)) from None
    cfg.data_dir, cfg.log_dir, cfg.runtime_dir = (
        p.resolve() for p in (cfg.data_dir, cfg.log_dir, cfg.runtime_dir)
    )
    try:
        loopback = (
            cfg.api.bind_host == "localhost" or ipaddress.ip_address(cfg.api.bind_host).is_loopback
        )
    except ValueError:
        loopback = False
    if cfg.api.auth_required and not cfg.api_token:
        raise ValueError("Authentication requires an API token.")
    if cfg.environment == "production" and (
        not cfg.api.auth_required or cfg.backend != "ble" or not loopback
    ):
        raise ValueError("Production requires authentication, BLE and a loopback bind.")
    if not cfg.api.auth_required and not loopback:
        raise ValueError("Disabled authentication requires loopback.")
    if cfg.public_url:
        cfg.public_url = base_url(cfg.public_url)
        if cfg.environment == "production" and not cfg.public_url.startswith("https://"):
            raise ValueError("Production public origin must use HTTPS.")
    return cfg
