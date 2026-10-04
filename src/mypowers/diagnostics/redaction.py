"""Sanitize supervisor/Uvicorn messages before their stderr formatter runs."""

import logging
import re
import traceback

from mypowers.contracts import safe_text


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
        message = re.sub(r"(?:https?|wss?)://\S+", "[url redacted]", message)
        record.msg = safe_text(message)
        record.args = ()
        record.exc_info = None
        record.exc_text = None
        return True
