"""Retry delays for failed requests."""


def backoff_delays(config):
    """Delays in seconds before each retry: base, 2 * base, 4 * base, ..."""
    return [config.retry_base_delay * (2 ** attempt) for attempt in range(config.retry_attempts)]
