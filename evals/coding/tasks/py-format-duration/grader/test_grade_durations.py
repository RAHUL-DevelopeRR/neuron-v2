import unittest

from durations import format_duration


class GradeFormatDuration(unittest.TestCase):
    def test_examples_from_the_spec(self):
        for seconds, expected in [
            (3723, "1h 02m 03s"),
            (65, "1m 05s"),
            (7, "7s"),
            (0, "0s"),
            (3600, "1h 00m 00s"),
            (90000, "25h 00m 00s"),
        ]:
            self.assertEqual(format_duration(seconds), expected, seconds)

    def test_boundaries(self):
        for seconds, expected in [
            (59, "59s"),
            (60, "1m 00s"),
            (61, "1m 01s"),
            (3599, "59m 59s"),
            (3601, "1h 00m 01s"),
            (36000, "10h 00m 00s"),
            (359999, "99h 59m 59s"),
        ]:
            self.assertEqual(format_duration(seconds), expected, seconds)

    def test_negative_is_rejected(self):
        with self.assertRaises(ValueError):
            format_duration(-1)


if __name__ == "__main__":
    unittest.main()
