"""Sanitize supervisor/Uvicorn messages before their stderr formatter runs."""

import logging
import re
import traceback

from mypowers.contracts import safe_text


def redact_url(match: re.Match[str]) -> str:
    value = match.group()
    # A bare listener origin is useful diagnostics; paths and credentials stay private.
    if re.fullmatch(r"(?:https?|wss?)://(?:[A-Za-z0-9.-]+|\[[0-9A-Fa-f:]+\]):[0-9]+", value):
        return value
    return "[url redacted]"


class RedactionFilter(logging.Filter):
    def __init__(self, secrets: tuple[str, ...] = ()):
        super().__init__()
        self.secrets = secrets

    def filter(self, record: logging.LogRecord) -> bool:
        message = record.getMessage()
        if record.exc_info:
            message += " " + "".join(traceback.format_exception(*record.exc_info))
        for secret in self.secrets:
            if secret:
                message = message.replace(secret, "[redacted]")
        message = re.sub(r"(?i)bearer\s+\S+", "Bearer [redacted]", message)
        message = re.sub(
            r"(?i)(token|cookie|password|credential|secret)=([^\s&]+)", r"\1=[redacted]", message
        )
        message = re.sub(r"(?:https?|wss?)://\S+", redact_url, message)
        record.msg = safe_text(message)
        record.args = ()
        # Uvicorn's color formatter would otherwise restore its unsanitized format string.
        record.__dict__.pop("color_message", None)
        record.exc_info = None
        record.exc_text = None
        return True
