import unittest

from app import settings
from app.client import HttpClient
from app.config import load
from app.retry import backoff_delays
from app.transport import Transport


class GradeTimeout(unittest.TestCase):
    def test_default_timeout_reaches_the_transport_in_ms(self):
        transport = Transport()
        HttpClient(load({}), transport).get("https://example.com")
        self.assertEqual(transport.sent[0]["timeout_ms"], 30_000)

    def test_environment_timeout_in_seconds(self):
        transport = Transport()
        HttpClient(load({"APP_TIMEOUT": "2.5"}), transport).post("https://example.com")
        self.assertEqual(transport.sent[0]["timeout_ms"], 2_500)

    def test_configuration_stays_in_seconds(self):
        self.assertEqual(settings.REQUEST_TIMEOUT, 30)
        self.assertEqual(load({}).timeout, 30)
        self.assertEqual(load({"APP_TIMEOUT": "5"}).timeout, 5)

    def test_retry_delays_unchanged(self):
        self.assertEqual(backoff_delays(load({})), [0.5, 1.0, 2.0])


if __name__ == "__main__":
    unittest.main()
