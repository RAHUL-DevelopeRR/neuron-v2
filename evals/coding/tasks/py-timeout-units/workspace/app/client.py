"""HTTP client used by the rest of the app."""
from app.transport import Transport


class HttpClient:
    def __init__(self, config, transport=None):
        self.config = config
        self.transport = transport or Transport()
        self.timeout_ms = config.timeout

    def get(self, url):
        return self.transport.send("GET", url, timeout_ms=self.timeout_ms)

    def post(self, url):
        return self.transport.send("POST", url, timeout_ms=self.timeout_ms)
