//! `Word27`: the Binary-Encoded Ternary (BET) form of a word.
//!
//! Each trit takes 2 bits (SPEC §1.1): `-` = 10, `0` = 00, `+` = 01, `?` = 11.
//! Trit 0 (least significant) sits in bits 0..2, trit 26 in bits 52..54.
//! This form is used only at the edges: `.ksb` files, `dump`, and the
//! `0t` / `0v` notations. Arithmetic happens on [`Word`].

use crate::word::{Word, WORD_MAX, WORD_TRITS};

const MASK_54: u64 = (1u64 << 54) - 1;

/// A 27-trit word in BET encoding, stored in the low 54 bits of a `u64`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Word27(u64);

impl Word27 {
    /// The canonical NaT: every trit is `?` (11).
    pub const NAT: Word27 = Word27(MASK_54);

    /// Raw 54-bit pattern, as written to `.ksb` files.
    pub fn bits(self) -> u64 {
        self.0
    }

    /// Rebuilds from raw bits. Bits above 54 are rejected.
    pub fn from_bits(bits: u64) -> Option<Word27> {
        (bits & !MASK_54 == 0).then_some(Word27(bits))
    }

    /// Trit `i` (0 = least significant) as -1, 0, +1, or `None` for `?`.
    pub fn trit(self, i: u32) -> Option<i8> {
        match (self.0 >> (i * 2)) & 0b11 {
            0b00 => Some(0),
            0b01 => Some(1),
            0b10 => Some(-1),
            _ => None,
        }
    }

    /// Formats as `0t` followed by `width` trits, most significant first.
    pub fn to_trits(self, width: u32) -> String {
        let mut s = String::from("0t");
        for i in (0..width.min(WORD_TRITS)).rev() {
            s.push(match self.trit(i) {
                Some(0) => '0',
                Some(1) => '+',
                Some(_) => '-',
                None => '?',
            });
        }
        s
    }

    /// Formats as `0v` followed by 9 heptavintimal digits (3 trits each).
    pub fn to_hepta(self) -> String {
        let mut s = String::from("0v");
        for g in (0..9).rev() {
            let mut v = 0i32;
            let mut nat = false;
            for t in 0..3 {
                match self.trit(g * 3 + t) {
                    Some(d) => v += i32::from(d) * [1, 3, 9][t as usize],
                    None => nat = true,
                }
            }
            s.push(if nat { '?' } else { hepta_char(v) });
        }
        s
    }
}

impl From<Word> for Word27 {
    fn from(w: Word) -> Word27 {
        let Some(mut v) = w.get() else {
            return Word27::NAT;
        };
        let mut raw = 0u64;
        for i in 0..WORD_TRITS {
            // Balanced digit in -1..=1, carrying the rest upward.
            let r = v.rem_euclid(3);
            let (bits, digit) = match r {
                0 => (0b00, 0),
                1 => (0b01, 1),
                _ => (0b10, -1),
            };
            raw |= bits << (i * 2);
            v = (v - digit) / 3;
        }
        Word27(raw)
    }
}

impl From<Word27> for Word {
    fn from(b: Word27) -> Word {
        let mut acc = 0i64;
        for i in (0..WORD_TRITS).rev() {
            match b.trit(i) {
                Some(d) => acc = acc * 3 + i64::from(d),
                None => return Word::NAT,
            }
        }
        Word::new(acc)
    }
}

/// Heptavintimal digit for a value in -13..=13.
fn hepta_char(v: i32) -> char {
    match v {
        0 => '0',
        1..=13 => (b'A' + (v - 1) as u8) as char,
        -13..=-1 => (b'a' + (-v - 1) as u8) as char,
        _ => unreachable!("hepta digit out of range: {v}"),
    }
}

/// Errors from parsing `0t` / `0v` literal bodies.
#[derive(Debug, PartialEq, Eq)]
pub enum LiteralError {
    Empty,
    TooLong { max: usize, got: usize },
    BadDigit(char),
}

/// Parses the digits after `0t` (e.g. `"+0-"`). Any `?` makes the word NaT.
pub fn parse_trits(body: &str) -> Result<Word, LiteralError> {
    let n = body.chars().count();
    if n == 0 {
        return Err(LiteralError::Empty);
    }
    if n > WORD_TRITS as usize {
        return Err(LiteralError::TooLong { max: WORD_TRITS as usize, got: n });
    }
    let mut acc = 0i64;
    let mut nat = false;
    for c in body.chars() {
        let d = match c {
            '-' => -1,
            '0' => 0,
            '+' => 1,
            '?' => {
                nat = true;
                0
            }
            _ => return Err(LiteralError::BadDigit(c)),
        };
        acc = acc * 3 + d;
    }
    Ok(if nat { Word::NAT } else { Word::new(acc) })
}

/// Parses the digits after `0v` (e.g. `"Bm"`). Any `?` makes the word NaT.
pub fn parse_hepta(body: &str) -> Result<Word, LiteralError> {
    let n = body.chars().count();
    if n == 0 {
        return Err(LiteralError::Empty);
    }
    if n > 9 {
        return Err(LiteralError::TooLong { max: 9, got: n });
    }
    let mut acc = 0i64;
    let mut nat = false;
    for c in body.chars() {
        let d = match c {
            '0' => 0,
            'A'..='M' => i64::from(c as u8 - b'A') + 1,
            'a'..='m' => -(i64::from(c as u8 - b'a') + 1),
            '?' => {
                nat = true;
                0
            }
            _ => return Err(LiteralError::BadDigit(c)),
        };
        acc = acc * 27 + d;
    }
    debug_assert!(nat || acc.abs() <= WORD_MAX);
    Ok(if nat { Word::NAT } else { Word::new(acc) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: i64) -> Word {
        Word::new(v)
    }

    #[test]
    fn spec_examples() {
        // SPEC §2: 0t+0- == 8, and negation flips every trit.
        assert_eq!(parse_trits("+0-"), Ok(w(8)));
        assert_eq!(Word27::from(w(8)).to_trits(3), "0t+0-");
        assert_eq!(Word27::from(w(-8)).to_trits(3), "0t-0+");
    }

    #[test]
    fn negation_swaps_bit_pairs() {
        for v in [0, 1, -1, 8, 1234567, -WORD_MAX, WORD_MAX] {
            let a = Word27::from(w(v)).bits();
            let b = Word27::from(w(-v)).bits();
            let even = 0x0015_5555_5555_5555u64;
            let swapped = ((a & even) << 1) | ((a >> 1) & even);
            assert_eq!(swapped, b, "value {v}");
        }
    }

    #[test]
    fn roundtrip() {
        for v in [0, 1, -1, 2, -2, 13, 14, -364, 364, 999_999_999, WORD_MAX, -WORD_MAX] {
            let b = Word27::from(w(v));
            assert_eq!(Word::from(b), w(v), "value {v}");
            assert_eq!(Word27::from_bits(b.bits()), Some(b));
        }
        assert_eq!(Word27::from(Word::NAT), Word27::NAT);
        assert!(Word::from(Word27::NAT).is_nat());
        assert_eq!(Word27::from_bits(1 << 60), None);
    }

    #[test]
    fn extreme_patterns() {
        assert_eq!(Word27::from(w(WORD_MAX)).to_trits(27), format!("0t{}", "+".repeat(27)));
        assert_eq!(Word27::from(w(-WORD_MAX)).to_trits(27), format!("0t{}", "-".repeat(27)));
        assert_eq!(Word27::from(w(WORD_MAX)).to_hepta(), "0vMMMMMMMMM");
        assert_eq!(Word27::NAT.to_hepta(), "0v?????????");
    }

    #[test]
    fn hepta() {
        assert_eq!(Word27::from(w(8)).to_hepta(), "0v00000000H");
        assert_eq!(Word27::from(w(-13)).to_hepta(), "0v00000000m");
        assert_eq!(Word27::from(w(14)).to_hepta(), "0v0000000Am");
        assert_eq!(parse_hepta("Am"), Ok(w(14)));
        assert_eq!(parse_hepta("MMMMMMMMM"), Ok(w(WORD_MAX)));
        for v in [-500, -27, 0, 27, 100_000, -WORD_MAX] {
            let s = Word27::from(w(v)).to_hepta();
            assert_eq!(parse_hepta(&s[2..]), Ok(w(v)), "value {v}");
        }
    }

    #[test]
    fn literal_errors_and_nat() {
        assert_eq!(parse_trits(""), Err(LiteralError::Empty));
        assert_eq!(parse_trits(&"+".repeat(28)), Err(LiteralError::TooLong { max: 27, got: 28 }));
        assert_eq!(parse_trits("+x"), Err(LiteralError::BadDigit('x')));
        assert_eq!(parse_hepta("N"), Err(LiteralError::BadDigit('N')));
        assert_eq!(parse_hepta("0000000000"), Err(LiteralError::TooLong { max: 9, got: 10 }));
        assert!(parse_trits("+?0").unwrap().is_nat());
        assert!(parse_hepta("A?").unwrap().is_nat());
    }
}
