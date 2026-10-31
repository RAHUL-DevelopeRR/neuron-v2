"""The low-level transport. Its API takes durations in milliseconds."""


class Transport:
    def __init__(self):
        self.sent = []

    def send(self, method, url, timeout_ms):
        if timeout_ms <= 0:
            raise ValueError("timeout_ms must be positive")
        self.sent.append({"method": method, "url": url, "timeout_ms": timeout_ms})
        return {"status": 200, "timeout_ms": timeout_ms}
