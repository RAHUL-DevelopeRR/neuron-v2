"""Split a list into pages."""


def paginate(items, page, per_page=10):
    """Return the items on `page` (1-based) and the total number of pages.

    An empty list has 0 pages. A page past the end is empty.
    Raises ValueError when `page` or `per_page` is below 1.
    """
    if page < 1 or per_page < 1:
        raise ValueError("page and per_page must be at least 1")
    total_pages = len(items) // per_page
    start = page * per_page
    end = start + per_page
    return items[start:end], total_pages
