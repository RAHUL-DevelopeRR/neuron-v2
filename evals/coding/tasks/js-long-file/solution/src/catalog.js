// Product catalog helpers. Prices are in cents.

/**
 * Price in cents for `units` of tier 01: 137 cents each, with a
 * 3% discount from 12 units.
 */
export function tierPrice01(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 137;
  const discount = units >= 12 ? Math.round(base * 0.03) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 02: 174 cents each, with a
 * 4% discount from 19 units.
 */
export function tierPrice02(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 174;
  const discount = units >= 19 ? Math.round(base * 0.04) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 03: 211 cents each, with a
 * 5% discount from 26 units.
 */
export function tierPrice03(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 211;
  const discount = units >= 26 ? Math.round(base * 0.05) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 04: 248 cents each, with a
 * 6% discount from 33 units.
 */
export function tierPrice04(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 248;
  const discount = units >= 33 ? Math.round(base * 0.06) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 05: 285 cents each, with a
 * 7% discount from 10 units.
 */
export function tierPrice05(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 285;
  const discount = units >= 10 ? Math.round(base * 0.07) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 06: 322 cents each, with a
 * 8% discount from 17 units.
 */
export function tierPrice06(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 322;
  const discount = units >= 17 ? Math.round(base * 0.08) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 07: 359 cents each, with a
 * 9% discount from 24 units.
 */
export function tierPrice07(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 359;
  const discount = units >= 24 ? Math.round(base * 0.09) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 08: 396 cents each, with a
 * 10% discount from 31 units.
 */
export function tierPrice08(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 396;
  const discount = units >= 31 ? Math.round(base * 0.1) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 09: 433 cents each, with a
 * 2% discount from 8 units.
 */
export function tierPrice09(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 433;
  const discount = units >= 8 ? Math.round(base * 0.02) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 10: 470 cents each, with a
 * 3% discount from 15 units.
 */
export function tierPrice10(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 470;
  const discount = units >= 15 ? Math.round(base * 0.03) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 11: 107 cents each, with a
 * 4% discount from 22 units.
 */
export function tierPrice11(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 107;
  const discount = units >= 22 ? Math.round(base * 0.04) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 12: 144 cents each, with a
 * 5% discount from 29 units.
 */
export function tierPrice12(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 144;
  const discount = units >= 29 ? Math.round(base * 0.05) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 13: 181 cents each, with a
 * 6% discount from 6 units.
 */
export function tierPrice13(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 181;
  const discount = units >= 6 ? Math.round(base * 0.06) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 14: 218 cents each, with a
 * 7% discount from 13 units.
 */
export function tierPrice14(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 218;
  const discount = units >= 13 ? Math.round(base * 0.07) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 15: 255 cents each, with a
 * 8% discount from 20 units.
 */
export function tierPrice15(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 255;
  const discount = units >= 20 ? Math.round(base * 0.08) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 16: 292 cents each, with a
 * 9% discount from 27 units.
 */
export function tierPrice16(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 292;
  const discount = units >= 27 ? Math.round(base * 0.09) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 17: 329 cents each, with a
 * 10% discount from 34 units.
 */
export function tierPrice17(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 329;
  const discount = units >= 34 ? Math.round(base * 0.1) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 18: 366 cents each, with a
 * 2% discount from 11 units.
 */
export function tierPrice18(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 366;
  const discount = units >= 11 ? Math.round(base * 0.02) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 19: 403 cents each, with a
 * 3% discount from 18 units.
 */
export function tierPrice19(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 403;
  const discount = units >= 18 ? Math.round(base * 0.03) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 20: 440 cents each, with a
 * 4% discount from 25 units.
 */
export function tierPrice20(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 440;
  const discount = units >= 25 ? Math.round(base * 0.04) : 0;
  return base - discount;
}

/**
 * URL slug for a product name.
 */
export function slugify(text) {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

/**
 * Price in cents for `units` of tier 21: 477 cents each, with a
 * 5% discount from 32 units.
 */
export function tierPrice21(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 477;
  const discount = units >= 32 ? Math.round(base * 0.05) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 22: 114 cents each, with a
 * 6% discount from 9 units.
 */
export function tierPrice22(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 114;
  const discount = units >= 9 ? Math.round(base * 0.06) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 23: 151 cents each, with a
 * 7% discount from 16 units.
 */
export function tierPrice23(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 151;
  const discount = units >= 16 ? Math.round(base * 0.07) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 24: 188 cents each, with a
 * 8% discount from 23 units.
 */
export function tierPrice24(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 188;
  const discount = units >= 23 ? Math.round(base * 0.08) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 25: 225 cents each, with a
 * 9% discount from 30 units.
 */
export function tierPrice25(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 225;
  const discount = units >= 30 ? Math.round(base * 0.09) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 26: 262 cents each, with a
 * 10% discount from 7 units.
 */
export function tierPrice26(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 262;
  const discount = units >= 7 ? Math.round(base * 0.1) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 27: 299 cents each, with a
 * 2% discount from 14 units.
 */
export function tierPrice27(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 299;
  const discount = units >= 14 ? Math.round(base * 0.02) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 28: 336 cents each, with a
 * 3% discount from 21 units.
 */
export function tierPrice28(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 336;
  const discount = units >= 21 ? Math.round(base * 0.03) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 29: 373 cents each, with a
 * 4% discount from 28 units.
 */
export function tierPrice29(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 373;
  const discount = units >= 28 ? Math.round(base * 0.04) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 30: 410 cents each, with a
 * 5% discount from 5 units.
 */
export function tierPrice30(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 410;
  const discount = units >= 5 ? Math.round(base * 0.05) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 31: 447 cents each, with a
 * 6% discount from 12 units.
 */
export function tierPrice31(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 447;
  const discount = units >= 12 ? Math.round(base * 0.06) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 32: 484 cents each, with a
 * 7% discount from 19 units.
 */
export function tierPrice32(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 484;
  const discount = units >= 19 ? Math.round(base * 0.07) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 33: 121 cents each, with a
 * 8% discount from 26 units.
 */
export function tierPrice33(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 121;
  const discount = units >= 26 ? Math.round(base * 0.08) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 34: 158 cents each, with a
 * 9% discount from 33 units.
 */
export function tierPrice34(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 158;
  const discount = units >= 33 ? Math.round(base * 0.09) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 35: 195 cents each, with a
 * 10% discount from 10 units.
 */
export function tierPrice35(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 195;
  const discount = units >= 10 ? Math.round(base * 0.1) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 36: 232 cents each, with a
 * 2% discount from 17 units.
 */
export function tierPrice36(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 232;
  const discount = units >= 17 ? Math.round(base * 0.02) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 37: 269 cents each, with a
 * 3% discount from 24 units.
 */
export function tierPrice37(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 269;
  const discount = units >= 24 ? Math.round(base * 0.03) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 38: 306 cents each, with a
 * 4% discount from 31 units.
 */
export function tierPrice38(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 306;
  const discount = units >= 31 ? Math.round(base * 0.04) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 39: 343 cents each, with a
 * 5% discount from 8 units.
 */
export function tierPrice39(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 343;
  const discount = units >= 8 ? Math.round(base * 0.05) : 0;
  return base - discount;
}

/**
 * Price in cents for `units` of tier 40: 380 cents each, with a
 * 6% discount from 15 units.
 */
export function tierPrice40(units) {
  if (!Number.isInteger(units) || units < 0) {
    throw new RangeError('units must be a non-negative integer');
  }
  const base = units * 380;
  const discount = units >= 15 ? Math.round(base * 0.06) : 0;
  return base - discount;
}

export const TIER_COUNT = 40;
