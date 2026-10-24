"""Generates this eval task (run it to rebuild task.json, workspace/, grader/ and
solution/): a long module with a bug in the middle, and a grader that checks every
function in it, so a cut-off or lossy rewrite of the file fails."""
import json
import math
import os

TASK = os.path.dirname(os.path.abspath(__file__))
for sub in ("workspace/src", "grader", "solution/src"):
    os.makedirs(os.path.join(TASK, sub), exist_ok=True)

TIERS = []
for n in range(1, 41):
    price = 100 + n * 37 % 400  # cents per unit
    threshold = 5 + (n * 7) % 30  # units for the discount
    rate = round(0.02 + (n % 9) * 0.01, 2)
    TIERS.append((n, price, threshold, rate))


def tier_fn(n, price, threshold, rate):
    return f"""/**
 * Price in cents for `units` of tier {n:02d}: {price} cents each, with a
 * {int(rate * 100)}% discount from {threshold} units.
 */
export function tierPrice{n:02d}(units) {{
  if (!Number.isInteger(units) || units < 0) {{
    throw new RangeError('units must be a non-negative integer');
  }}
  const base = units * {price};
  const discount = units >= {threshold} ? Math.round(base * {rate}) : 0;
  return base - discount;
}}
"""


BUGGY_SLUGIFY = """/**
 * URL slug for a product name.
 */
export function slugify(text) {
  return text.toLowerCase().replace(/ /g, '-');
}
"""

FIXED_SLUGIFY = """/**
 * URL slug for a product name.
 */
export function slugify(text) {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}
"""


def module(slugify):
    parts = ["// Product catalog helpers. Prices are in cents.\n\n"]
    for index, tier in enumerate(TIERS):
        parts.append(tier_fn(*tier))
        parts.append("\n")
        if index == 19:
            parts.append(slugify)
            parts.append("\n")
    parts.append("export const TIER_COUNT = 40;\n")
    return "".join(parts)


def write(relative, text):
    with open(os.path.join(TASK, relative), "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


write("workspace/src/catalog.js", module(BUGGY_SLUGIFY))
write("solution/src/catalog.js", module(FIXED_SLUGIFY))
write("workspace/package.json", json.dumps(
    {"name": "catalog", "version": "1.0.0", "private": True, "type": "module", "scripts": {"test": "node --test"}},
    indent=2,
) + "\n")

cases = []
for n, price, threshold, rate in TIERS:
    for units in (0, threshold - 1, threshold, threshold + 3):
        base = units * price
        # Math.round rounds halves up.
        discount = math.floor(base * rate + 0.5) if units >= threshold else 0
        cases.append((n, units, base - discount))

write("grader/catalog.test.js", "".join([
    "import { test } from 'node:test';\n",
    "import assert from 'node:assert/strict';\n\n",
    "import * as catalog from '../src/catalog.js';\n\n",
    "test('slugify collapses separators and trims dashes', () => {\n",
    "  assert.equal(catalog.slugify('  Hello -- World!! '), 'hello-world');\n",
    "  assert.equal(catalog.slugify('Blue Mug (12 oz)'), 'blue-mug-12-oz');\n",
    "  assert.equal(catalog.slugify('already-a-slug'), 'already-a-slug');\n",
    "  assert.equal(catalog.slugify('--Edge__Case--'), 'edge-case');\n",
    "  assert.equal(catalog.slugify('!!!'), '');\n",
    "});\n\n",
    "test('every tier function is intact', () => {\n",
    "  assert.equal(catalog.TIER_COUNT, 40);\n",
    f"  const cases = {json.dumps(cases)};\n",
    "  for (const [n, units, expected] of cases) {\n",
    "    const fn = catalog[`tierPrice${String(n).padStart(2, '0')}`];\n",
    "    assert.equal(typeof fn, 'function', `tierPrice${n} is missing`);\n",
    "    assert.equal(fn(units), expected, `tierPrice${n}(${units})`);\n",
    "  }\n",
    "  assert.throws(() => catalog.tierPrice07(-1), RangeError);\n",
    "});\n",
]))

write("task.json", json.dumps({
    "id": "js-long-file",
    "summary": "Fix one function in the middle of a ~530-line module without damaging the rest.",
    "tags": ["javascript", "bugfix", "long-file"],
    "prompt": (
        "`slugify` in src/catalog.js is wrong. It must lowercase the text, replace every run of "
        "characters other than a-z and 0-9 with a single dash, and remove dashes from both ends. "
        "For example '  Hello -- World!! ' becomes 'hello-world' and 'Blue Mug (12 oz)' becomes "
        "'blue-mug-12-oz'. Leave the rest of the file as it is."
    ),
    "allow": ["bash(node --test:*)", "bash(node -e:*)", "bash(npm test:*)"],
    "grader": {"from": "grader", "to": "_grader"},
    "check": "node --test",
}, indent=2) + "\n")
