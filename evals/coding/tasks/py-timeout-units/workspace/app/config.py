"""Configuration from the environment, falling back to app.settings (seconds)."""
from dataclasses import dataclass

from app import settings


@dataclass(frozen=True)
class Config:
    timeout: float  # seconds
    retry_attempts: int
    retry_base_delay: float  # seconds


def load(env):
    """Builds a Config from a mapping such as os.environ."""
    return Config(
        timeout=float(env.get("APP_TIMEOUT", settings.REQUEST_TIMEOUT)),
        retry_attempts=int(env.get("APP_RETRY_ATTEMPTS", settings.RETRY_ATTEMPTS)),
        retry_base_delay=float(env.get("APP_RETRY_BASE_DELAY", settings.RETRY_BASE_DELAY)),
    )
