//! What the single-byte codes of a simple PDF font mean when the font has no
//! `ToUnicode` map for them: its `/Encoding`, a named base encoding
//! (`WinAnsiEncoding`, `MacRomanEncoding`, `StandardEncoding`) changed by a
//! `/Differences` list of glyph names. pdfTeX without `\pdfgentounicode`
//! (most older papers set in Computer Modern) and older macOS and
//! Ghostscript output rely on these, so ligatures, dashes and quotes come out
//! right only through them.
//!
//! Glyph names are read as the Adobe Glyph List reads them, for the names
//! these fonts use: Latin letters and punctuation, ligatures, quotes, dashes,
//! accents, Greek, a few math signs, `uniXXXX` and `uXXXX` names, a suffix
//! after a period dropped (`a.sc`), and components joined by `_` (`f_f_i`).

/// The text of each code, 0 to 255.
pub(super) type CodeTable = Vec<String>;

/// A single-byte code read as Windows-1252, the usual simple-font encoding.
pub(super) fn win_ansi(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}',
        '\u{017d}', '\u{fffd}', '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}',
        '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// Codes 0x80 to 0xFF of `MacRomanEncoding`.
const MAC_ROMAN_HIGH: [char; 128] = [
    '\u{c4}', '\u{c5}', '\u{c7}', '\u{c9}', '\u{d1}', '\u{d6}', '\u{dc}', '\u{e1}', // 80
    '\u{e0}', '\u{e2}', '\u{e4}', '\u{e3}', '\u{e5}', '\u{e7}', '\u{e9}', '\u{e8}', // 88
    '\u{ea}', '\u{eb}', '\u{ed}', '\u{ec}', '\u{ee}', '\u{ef}', '\u{f1}', '\u{f3}', // 90
    '\u{f2}', '\u{f4}', '\u{f6}', '\u{f5}', '\u{fa}', '\u{f9}', '\u{fb}', '\u{fc}', // 98
    '\u{2020}', '\u{b0}', '\u{a2}', '\u{a3}', '\u{a7}', '\u{2022}', '\u{b6}', '\u{df}', // A0
    '\u{ae}', '\u{a9}', '\u{2122}', '\u{b4}', '\u{a8}', '\u{2260}', '\u{c6}', '\u{d8}', // A8
    '\u{221e}', '\u{b1}', '\u{2264}', '\u{2265}', '\u{a5}', '\u{b5}', '\u{2202}',
    '\u{2211}', // B0
    '\u{220f}', '\u{3c0}', '\u{222b}', '\u{aa}', '\u{ba}', '\u{3a9}', '\u{e6}',
    '\u{f8}', // B8
    '\u{bf}', '\u{a1}', '\u{ac}', '\u{221a}', '\u{192}', '\u{2248}', '\u{2206}',
    '\u{ab}', // C0
    '\u{bb}', '\u{2026}', '\u{a0}', '\u{c0}', '\u{c3}', '\u{d5}', '\u{152}', '\u{153}', // C8
    '\u{2013}', '\u{2014}', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}', '\u{f7}',
    '\u{25ca}', // D0
    '\u{ff}', '\u{178}', '\u{2044}', '\u{a4}', '\u{2039}', '\u{203a}', '\u{fb01}',
    '\u{fb02}', // D8
    '\u{2021}', '\u{b7}', '\u{201a}', '\u{201e}', '\u{2030}', '\u{c2}', '\u{ca}',
    '\u{c1}', // E0
    '\u{cb}', '\u{c8}', '\u{cd}', '\u{ce}', '\u{cf}', '\u{cc}', '\u{d3}', '\u{d4}', // E8
    '\u{fffd}', '\u{d2}', '\u{da}', '\u{db}', '\u{d9}', '\u{131}', '\u{2c6}', '\u{2dc}', // F0
    '\u{af}', '\u{2d8}', '\u{2d9}', '\u{2da}', '\u{b8}', '\u{2dd}', '\u{2db}',
    '\u{2c7}', // F8
];

fn mac_roman(byte: u8) -> char {
    if byte < 0x80 {
        char::from(byte)
    } else {
        MAC_ROMAN_HIGH[usize::from(byte - 0x80)]
    }
}

/// `StandardEncoding`, Type 1 fonts' own; codes it leaves empty read as
/// Windows-1252.
fn standard(byte: u8) -> char {
    match byte {
        0x27 => '\u{2019}',
        0x60 => '\u{2018}',
        0xA1 => '\u{a1}',
        0xA2 => '\u{a2}',
        0xA3 => '\u{a3}',
        0xA4 => '\u{2044}',
        0xA5 => '\u{a5}',
        0xA6 => '\u{192}',
        0xA7 => '\u{a7}',
        0xA8 => '\u{a4}',
        0xA9 => '\'',
        0xAA => '\u{201c}',
        0xAB => '\u{ab}',
        0xAC => '\u{2039}',
        0xAD => '\u{203a}',
        0xAE => '\u{fb01}',
        0xAF => '\u{fb02}',
        0xB1 => '\u{2013}',
        0xB2 => '\u{2020}',
        0xB3 => '\u{2021}',
        0xB4 => '\u{b7}',
        0xB6 => '\u{b6}',
        0xB7 => '\u{2022}',
        0xB8 => '\u{201a}',
        0xB9 => '\u{201e}',
        0xBA => '\u{201d}',
        0xBB => '\u{bb}',
        0xBC => '\u{2026}',
        0xBD => '\u{2030}',
        0xBF => '\u{bf}',
        0xC1 => '`',
        0xC2 => '\u{b4}',
        0xC3 => '\u{2c6}',
        0xC4 => '\u{2dc}',
        0xC5 => '\u{af}',
        0xC6 => '\u{2d8}',
        0xC7 => '\u{2d9}',
        0xC8 => '\u{a8}',
        0xCA => '\u{2da}',
        0xCB => '\u{b8}',
        0xCD => '\u{2dd}',
        0xCE => '\u{2db}',
        0xCF => '\u{2c7}',
        0xD0 => '\u{2014}',
        0xE1 => '\u{c6}',
        0xE3 => '\u{aa}',
        0xE8 => '\u{141}',
        0xE9 => '\u{d8}',
        0xEA => '\u{152}',
        0xEB => '\u{ba}',
        0xF1 => '\u{e6}',
        0xF5 => '\u{131}',
        0xF8 => '\u{142}',
        0xF9 => '\u{f8}',
        0xFA => '\u{153}',
        0xFB => '\u{df}',
        _ => win_ansi(byte),
    }
}

fn table(code: fn(u8) -> char) -> CodeTable {
    (0..=255u8)
        .map(|byte| {
            let mut text = String::new();
            push_char(&mut text, code(byte));
            text
        })
        .collect()
}

/// Adds `ch` to `text`, a ligature as its letters (`fi`), as pdfTeX's own
/// glyph-to-Unicode map gives them, so the words read and search whole.
fn push_char(text: &mut String, ch: char) {
    match ch {
        '\u{fb00}' => text.push_str("ff"),
        '\u{fb01}' => text.push_str("fi"),
        '\u{fb02}' => text.push_str("fl"),
        '\u{fb03}' => text.push_str("ffi"),
        '\u{fb04}' => text.push_str("ffl"),
        _ => text.push(ch),
    }
}

/// The table of a named base encoding; `None` for a name this does not
/// know, which reads as Windows-1252 like a font without one.
pub(super) fn named(name: &str) -> Option<CodeTable> {
    match name {
        "WinAnsiEncoding" => Some(table(win_ansi)),
        "MacRomanEncoding" => Some(table(mac_roman)),
        "StandardEncoding" => Some(table(standard)),
        _ => None,
    }
}

/// The table a `/Differences` list starts from when no base is named.
pub(super) fn default_table() -> CodeTable {
    table(win_ansi)
}

/// Names of codes 0xC0 to 0xFF of Latin-1, in order.
const LATIN1_LETTERS: [&str; 64] = [
    "Agrave",
    "Aacute",
    "Acircumflex",
    "Atilde",
    "Adieresis",
    "Aring",
    "AE",
    "Ccedilla",
    "Egrave",
    "Eacute",
    "Ecircumflex",
    "Edieresis",
    "Igrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Eth",
    "Ntilde",
    "Ograve",
    "Oacute",
    "Ocircumflex",
    "Otilde",
    "Odieresis",
    "multiply",
    "Oslash",
    "Ugrave",
    "Uacute",
    "Ucircumflex",
    "Udieresis",
    "Yacute",
    "Thorn",
    "germandbls",
    "agrave",
    "aacute",
    "acircumflex",
    "atilde",
    "adieresis",
    "aring",
    "ae",
    "ccedilla",
    "egrave",
    "eacute",
    "ecircumflex",
    "edieresis",
    "igrave",
    "iacute",
    "icircumflex",
    "idieresis",
    "eth",
    "ntilde",
    "ograve",
    "oacute",
    "ocircumflex",
    "otilde",
    "odieresis",
    "divide",
    "oslash",
    "ugrave",
    "uacute",
    "ucircumflex",
    "udieresis",
    "yacute",
    "thorn",
    "ydieresis",
];

/// Greek capitals from U+0391 (U+03A2 is unassigned) and small letters from
/// U+03B1, in order.
const GREEK_CAPITALS: [&str; 25] = [
    "Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta", "Iota", "Kappa",
    "Lambda", "Mu", "Nu", "Xi", "Omicron", "Pi", "Rho", "", "Sigma", "Tau", "Upsilon", "Phi",
    "Chi", "Psi", "Omega",
];
const GREEK_SMALL: [&str; 25] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
    "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma1", "sigma", "tau", "upsilon", "phi",
    "chi", "psi", "omega",
];

/// Other glyph names and their characters.
const NAMES: &[(&str, char)] = &[
    ("space", ' '),
    ("exclam", '!'),
    ("quotedbl", '"'),
    ("numbersign", '#'),
    ("dollar", '$'),
    ("percent", '%'),
    ("ampersand", '&'),
    ("quotesingle", '\''),
    ("parenleft", '('),
    ("parenright", ')'),
    ("asterisk", '*'),
    ("plus", '+'),
    ("comma", ','),
    ("hyphen", '-'),
    ("period", '.'),
    ("slash", '/'),
    ("zero", '0'),
    ("one", '1'),
    ("two", '2'),
    ("three", '3'),
    ("four", '4'),
    ("five", '5'),
    ("six", '6'),
    ("seven", '7'),
    ("eight", '8'),
    ("nine", '9'),
    ("colon", ':'),
    ("semicolon", ';'),
    ("less", '<'),
    ("equal", '='),
    ("greater", '>'),
    ("question", '?'),
    ("at", '@'),
    ("bracketleft", '['),
    ("backslash", '\\'),
    ("bracketright", ']'),
    ("asciicircum", '^'),
    ("underscore", '_'),
    ("grave", '`'),
    ("braceleft", '{'),
    ("bar", '|'),
    ("braceright", '}'),
    ("asciitilde", '~'),
    ("quoteleft", '\u{2018}'),
    ("quoteright", '\u{2019}'),
    ("quotedblleft", '\u{201c}'),
    ("quotedblright", '\u{201d}'),
    ("quotesinglbase", '\u{201a}'),
    ("quotedblbase", '\u{201e}'),
    ("guillemotleft", '\u{ab}'),
    ("guillemotright", '\u{bb}'),
    ("guilsinglleft", '\u{2039}'),
    ("guilsinglright", '\u{203a}'),
    ("endash", '\u{2013}'),
    ("emdash", '\u{2014}'),
    ("minus", '\u{2212}'),
    ("bullet", '\u{2022}'),
    ("ellipsis", '\u{2026}'),
    ("dagger", '\u{2020}'),
    ("daggerdbl", '\u{2021}'),
    ("perthousand", '\u{2030}'),
    ("trademark", '\u{2122}'),
    ("Euro", '\u{20ac}'),
    ("florin", '\u{192}'),
    ("fraction", '\u{2044}'),
    ("circumflex", '\u{2c6}'),
    ("tilde", '\u{2dc}'),
    ("macron", '\u{af}'),
    ("breve", '\u{2d8}'),
    ("dotaccent", '\u{2d9}'),
    ("ring", '\u{2da}'),
    ("cedilla", '\u{b8}'),
    ("hungarumlaut", '\u{2dd}'),
    ("ogonek", '\u{2db}'),
    ("caron", '\u{2c7}'),
    ("acute", '\u{b4}'),
    ("dieresis", '\u{a8}'),
    ("ff", '\u{fb00}'),
    ("fi", '\u{fb01}'),
    ("fl", '\u{fb02}'),
    ("ffi", '\u{fb03}'),
    ("ffl", '\u{fb04}'),
    ("dotlessi", '\u{131}'),
    ("dotlessj", '\u{237}'),
    ("Lslash", '\u{141}'),
    ("lslash", '\u{142}'),
    ("Scaron", '\u{160}'),
    ("scaron", '\u{161}'),
    ("Zcaron", '\u{17d}'),
    ("zcaron", '\u{17e}'),
    ("OE", '\u{152}'),
    ("oe", '\u{153}'),
    ("Ydieresis", '\u{178}'),
    ("exclamdown", '\u{a1}'),
    ("cent", '\u{a2}'),
    ("sterling", '\u{a3}'),
    ("currency", '\u{a4}'),
    ("yen", '\u{a5}'),
    ("brokenbar", '\u{a6}'),
    ("section", '\u{a7}'),
    ("copyright", '\u{a9}'),
    ("ordfeminine", '\u{aa}'),
    ("logicalnot", '\u{ac}'),
    ("registered", '\u{ae}'),
    ("degree", '\u{b0}'),
    ("plusminus", '\u{b1}'),
    ("twosuperior", '\u{b2}'),
    ("threesuperior", '\u{b3}'),
    ("mu", '\u{b5}'),
    ("paragraph", '\u{b6}'),
    ("periodcentered", '\u{b7}'),
    ("onesuperior", '\u{b9}'),
    ("ordmasculine", '\u{ba}'),
    ("onequarter", '\u{bc}'),
    ("onehalf", '\u{bd}'),
    ("threequarters", '\u{be}'),
    ("questiondown", '\u{bf}'),
    ("nbspace", '\u{a0}'),
    ("sfthyphen", '\u{ad}'),
    ("visiblespace", '\u{2423}'),
    ("notequal", '\u{2260}'),
    ("infinity", '\u{221e}'),
    ("lessequal", '\u{2264}'),
    ("greaterequal", '\u{2265}'),
    ("partialdiff", '\u{2202}'),
    ("summation", '\u{2211}'),
    ("product", '\u{220f}'),
    ("integral", '\u{222b}'),
    ("radical", '\u{221a}'),
    ("approxequal", '\u{2248}'),
    ("lozenge", '\u{25ca}'),
    ("arrowleft", '\u{2190}'),
    ("arrowup", '\u{2191}'),
    ("arrowright", '\u{2192}'),
    ("arrowdown", '\u{2193}'),
];

/// The character one glyph name (with no suffix or components) stands for.
fn glyph_char(name: &str) -> Option<char> {
    if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
        return name.chars().next();
    }
    if let Some(&(_, ch)) = NAMES.iter().find(|(known, _)| *known == name) {
        return Some(ch);
    }
    let offset = |names: &[&str], first: u32| {
        names
            .iter()
            .position(|known| *known == name)
            .and_then(|index| char::from_u32(first + u32::try_from(index).ok()?))
    };
    if let Some(ch) = offset(&LATIN1_LETTERS, 0xC0)
        .or_else(|| offset(&GREEK_CAPITALS, 0x391))
        .or_else(|| offset(&GREEK_SMALL, 0x3B1))
    {
        return Some(ch);
    }
    // `uXXXX` to `uXXXXXX`.
    let hex = name.strip_prefix('u')?;
    if (4..=6).contains(&hex.len()) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return char::from_u32(u32::from_str_radix(hex, 16).ok()?);
    }
    None
}

/// The text a glyph name stands for, as the Adobe Glyph List reads names:
/// anything after a period is dropped, components joined by `_` are read
/// one by one, and `uniXXXX` may hold several characters. `None` when any
/// part is unknown.
pub(super) fn glyph_text(name: &str) -> Option<String> {
    let name = name.split('.').next().filter(|base| !base.is_empty())?;
    let mut text = String::new();
    for part in name.split('_') {
        if let Some(hex) = part.strip_prefix("uni").filter(|hex| {
            !hex.is_empty()
                && hex.len() % 4 == 0
                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            for at in (0..hex.len()).step_by(4) {
                let unit = u32::from_str_radix(&hex[at..at + 4], 16).ok()?;
                push_char(&mut text, char::from_u32(unit)?);
            }
        } else {
            push_char(&mut text, glyph_char(part)?);
        }
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::{glyph_text, named};

    #[test]
    fn glyph_names_read_as_the_adobe_glyph_list_reads_them() {
        for (name, text) in [
            ("fi", "fi"),
            ("ffi", "ffi"),
            ("uniFB02", "fl"),
            ("endash", "\u{2013}"),
            ("quotedblleft", "\u{201c}"),
            ("eacute", "\u{e9}"),
            ("Ydieresis", "\u{178}"),
            ("Gamma", "\u{393}"),
            ("omega", "\u{3c9}"),
            ("a", "a"),
            ("seven", "7"),
            ("a.sc", "a"),
            ("f_f_i", "ffi"),
            ("uni00E9", "\u{e9}"),
            ("uni00660069", "fi"),
            ("u1F600", "\u{1f600}"),
        ] {
            assert_eq!(glyph_text(name).as_deref(), Some(text), "{name}");
        }
        for name in [".notdef", "g123", "uniD800", "cid1234", ""] {
            assert_eq!(glyph_text(name), None, "{name}");
        }
    }

    #[test]
    fn named_encodings_have_every_code() {
        for name in ["WinAnsiEncoding", "MacRomanEncoding", "StandardEncoding"] {
            assert_eq!(named(name).expect(name).len(), 256);
        }
        let mac = named("MacRomanEncoding").expect("mac");
        assert_eq!(mac[0x8E], "\u{e9}");
        assert_eq!(mac[0xD0], "\u{2013}");
        let standard = named("StandardEncoding").expect("standard");
        assert_eq!(standard[0x27], "\u{2019}");
        assert_eq!(standard[0xAE], "fi");
        assert!(named("Custom").is_none());
    }
}
