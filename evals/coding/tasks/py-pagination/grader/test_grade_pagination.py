import unittest

from pagination import paginate


class GradePaginate(unittest.TestCase):
    def test_first_page(self):
        self.assertEqual(paginate(list(range(25)), 1, 10), (list(range(10)), 3))

    def test_last_partial_page(self):
        self.assertEqual(paginate(list(range(25)), 3, 10), ([20, 21, 22, 23, 24], 3))

    def test_exact_multiple(self):
        self.assertEqual(paginate(list(range(20)), 2, 10), (list(range(10, 20)), 2))

    def test_empty_list_has_no_pages(self):
        self.assertEqual(paginate([], 1), ([], 0))

    def test_page_past_the_end_is_empty(self):
        self.assertEqual(paginate(list(range(25)), 4, 10), ([], 3))

    def test_default_page_size(self):
        self.assertEqual(paginate(list(range(11)), 2), ([10], 2))

    def test_invalid_arguments(self):
        with self.assertRaises(ValueError):
            paginate([1], 0)
        with self.assertRaises(ValueError):
            paginate([1], 1, 0)


if __name__ == "__main__":
    unittest.main()
