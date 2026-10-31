import unittest

from pagination import paginate


class PaginateTest(unittest.TestCase):
    def test_first_page(self):
        self.assertEqual(paginate(list(range(25)), 1, 10), (list(range(10)), 3))

    def test_last_partial_page(self):
        self.assertEqual(paginate(list(range(25)), 3, 10), ([20, 21, 22, 23, 24], 3))


if __name__ == "__main__":
    unittest.main()
