"""Human-readable durations."""


def format_duration(seconds):
    """Formats a number of seconds such as 3723 as "1h 02m 03s"."""
    if seconds < 0:
        raise ValueError("seconds must not be negative")
    hours, rest = divmod(seconds, 3600)
    minutes, secs = divmod(rest, 60)
    if hours:
        return f"{hours}h {minutes:02d}m {secs:02d}s"
    if minutes:
        return f"{minutes}m {secs:02d}s"
    return f"{secs}s"
