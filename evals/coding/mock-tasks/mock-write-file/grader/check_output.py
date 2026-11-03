"""Passes when the scripted write_file call landed: generated/output.txt with the mock's content."""
import sys
from pathlib import Path

path = Path("generated/output.txt")
if not path.is_file():
    sys.exit(f"{path} was not written")
content = path.read_text(encoding="utf-8")
if content != "created by mock service\n":
    sys.exit(f"{path} has unexpected content: {content!r}")
print(f"{path} written as scripted")
