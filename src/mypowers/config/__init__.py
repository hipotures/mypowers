"""Shared environment and client settings without server-only imports."""

import os
import re
import stat
from pathlib import Path
from urllib.parse import urlsplit
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from dotenv import dotenv_values
from pydantic import BaseModel, ConfigDict

ENV_NAMES = frozenset(
    "ENV CONFIG BIND_HOST PORT DATA_DIR LOG_DIR RUNTIME_DIR LOG_LEVEL "
    "AUTH_REQUIRED API_TOKEN_FILE API_TOKEN PUBLIC_URL SERVER_URL TIMEZONE CA_FILE BACKEND "
    "TEST_ALLOW_OUTPUT_CHANGES TELEGRAM_BOT_TOKEN TELEGRAM_CHAT_ID".split()
)
PATH_NAMES = frozenset(
    {"CONFIG", "DATA_DIR", "LOG_DIR", "RUNTIME_DIR", "API_TOKEN_FILE", "CA_FILE"}
)


def environment(env_file: str | None = None) -> dict[str, str]:
    values: dict[str, str] = {}
    if env_file is None and Path(".env").exists():
        env_file = ".env"
    if env_file:
        path = Path(env_file).resolve()
        if not path.is_file():
            raise ValueError("Selected env file does not exist.")
        for key, value in dotenv_values(path, interpolate=False).items():
            if value is not None and key.startswith("MYPOWERS_"):
                name = key.removeprefix("MYPOWERS_")
                if name in PATH_NAMES:
                    value = str((path.parent / value).resolve())
                values[key] = value
    values.update({key: value for key, value in os.environ.items() if key.startswith("MYPOWERS_")})
    unknown = sorted(key for key in values if key.removeprefix("MYPOWERS_") not in ENV_NAMES)
    if unknown:
        raise ValueError("Unknown environment names: " + ", ".join(unknown))
    return values


def token(values: dict[str, str]) -> str | None:
    direct, file = values.get("MYPOWERS_API_TOKEN"), values.get("MYPOWERS_API_TOKEN_FILE")
    if direct is not None and file is not None:
        raise ValueError("Select either API_TOKEN or API_TOKEN_FILE.")
    if file:
        path = Path(file)
        if stat.S_IMODE(path.stat().st_mode) & 0o077:
            raise ValueError("Token file must have private permissions (0600).")
        direct = path.read_text().strip()
    if direct is not None and (
        len(direct) < 32
        or len(direct) > 1024
        or not direct.isascii()
        or any(c.isspace() for c in direct)
        or re.search(r"placeholder|change.?me|example", direct, re.I)
    ):
        raise ValueError("API token must contain at least 32 characters and be non-placeholder.")
    return direct


def base_url(value: str) -> str:
    parsed = urlsplit(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username is not None:
        raise ValueError("Server URL must be HTTP(S), without credentials.")
    if parsed.query or parsed.fragment or parsed.path not in {"", "/"}:
        raise ValueError("Server URL must be an origin without path, query or fragment.")
    try:
        _ = parsed.port
    except ValueError:
        raise ValueError("Invalid server port.") from None
    return value.rstrip("/")


class ClientConfig(BaseModel):
    model_config = ConfigDict(extra="forbid")
    server: str = "http://127.0.0.1:8765"
    api_token: str | None = None
    ca_file: str | None = None
    timezone: str | None = None
    timeout: float = 10
    no_color: bool = False


def client_config(env_file: str | None = None, **overrides: object) -> ClientConfig:
    values = environment(env_file)
    data: dict[str, object] = {
        "server": values.get("MYPOWERS_SERVER_URL", "http://127.0.0.1:8765"),
        "ca_file": values.get("MYPOWERS_CA_FILE"),
        "timezone": values.get("MYPOWERS_TIMEZONE"),
    }
    token_file = overrides.pop("token_file", None)
    if token_file:
        values.pop("MYPOWERS_API_TOKEN", None)
        values["MYPOWERS_API_TOKEN_FILE"] = str(token_file)
    data["api_token"] = token(values)
    data.update({key: value for key, value in overrides.items() if value is not None})
    cfg = ClientConfig.model_validate(data)
    cfg.server = base_url(cfg.server)
    if not 0 < cfg.timeout <= 120:
        raise ValueError("Timeout must be between zero and 120 seconds.")
    if cfg.timezone:
        try:
            ZoneInfo(cfg.timezone)
        except ZoneInfoNotFoundError:
            raise ValueError("Unknown timezone.") from None
    return cfg
