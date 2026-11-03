"""HTTP client used by the rest of the app."""
from app.transport import Transport


class HttpClient:
    def __init__(self, config, transport=None):
        self.config = config
        self.transport = transport or Transport()
        # The config is in seconds; the transport takes milliseconds.
        self.timeout_ms = config.timeout * 1000

    def get(self, url):
        return self.transport.send("GET", url, timeout_ms=self.timeout_ms)

    def post(self, url):
        return self.transport.send("POST", url, timeout_ms=self.timeout_ms)
