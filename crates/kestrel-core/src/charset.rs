//! The zero-centered tryte character table (SPEC §4).
//!
//! 0 is a space, so zeroed memory reads as blank. Upper case letters are
//! positive and lower case negative, so negating a tryte swaps the case.

const PUNCT: [char; 27] = [
    '\n', '\t', '.', ',', ':', ';', '!', '?', '-', '+', '/', '_', '=', '(', ')', '[', ']',
    '*', '<', '>', '@', '#', '%', '{', '}', '"', '\'',
];

/// Accented letters: upper case at +54.., lower case at -54.., so negation
/// still swaps the case.
const ACCENT_UPPER: [char; 12] = ['Á', 'À', 'Â', 'Ã', 'É', 'Ê', 'Í', 'Ó', 'Ô', 'Õ', 'Ú', 'Ç'];
const ACCENT_LOWER: [char; 12] = ['á', 'à', 'â', 'ã', 'é', 'ê', 'í', 'ó', 'ô', 'õ', 'ú', 'ç'];

/// Character for a tryte value, or `None` if the value is unassigned.
pub fn tryte_to_char(v: i64) -> Option<char> {
    match v {
        0 => Some(' '),
        1..=26 => Some((b'A' + (v - 1) as u8) as char),
        -26..=-1 => Some((b'a' + (-v - 1) as u8) as char),
        27..=36 => Some((b'0' + (v - 27) as u8) as char),
        -53..=-27 => Some(PUNCT[(-v - 27) as usize]),
        54..=65 => Some(ACCENT_UPPER[(v - 54) as usize]),
        -65..=-54 => Some(ACCENT_LOWER[(-v - 54) as usize]),
        _ => None,
    }
}

/// Character classes for `is_digit` and friends (by tryte value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Digit,
    Letter,
    Upper,
    Lower,
    Space,
}

impl Class {
    pub const ALL: [Class; 5] = [Class::Digit, Class::Letter, Class::Upper, Class::Lower, Class::Space];

    pub fn contains(self, v: i64) -> bool {
        let upper = (1..=26).contains(&v) || (54..=65).contains(&v);
        let lower = (-26..=-1).contains(&v) || (-65..=-54).contains(&v);
        match self {
            Class::Digit => (27..=36).contains(&v),
            Class::Letter => upper || lower,
            Class::Upper => upper,
            Class::Lower => lower,
            Class::Space => v == 0 || v == -27 || v == -28, // ' ', '\n', '\t'
        }
    }
}

/// Tryte value for a character, or `None` if it is not in the table.
pub fn char_to_tryte(c: char) -> Option<i64> {
    match c {
        ' ' => Some(0),
        'A'..='Z' => Some(i64::from(c as u8 - b'A') + 1),
        'a'..='z' => Some(-(i64::from(c as u8 - b'a') + 1)),
        '0'..='9' => Some(i64::from(c as u8 - b'0') + 27),
        _ => {
            let find = |t: &[char]| t.iter().position(|&p| p == c).map(|i| i as i64);
            find(&PUNCT)
                .map(|i| -i - 27)
                .or_else(|| find(&ACCENT_UPPER).map(|i| i + 54))
                .or_else(|| find(&ACCENT_LOWER).map(|i| -i - 54))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_values() {
        assert_eq!(char_to_tryte(' '), Some(0));
        assert_eq!(char_to_tryte('A'), Some(1));
        assert_eq!(char_to_tryte('Z'), Some(26));
        assert_eq!(char_to_tryte('a'), Some(-1));
        assert_eq!(char_to_tryte('0'), Some(27));
        assert_eq!(char_to_tryte('9'), Some(36));
        assert_eq!(char_to_tryte('\n'), Some(-27));
        assert_eq!(char_to_tryte(']'), Some(-43));
        assert_eq!(char_to_tryte('*'), Some(-44));
        assert_eq!(char_to_tryte('\''), Some(-53));
        assert_eq!(char_to_tryte('Á'), Some(54));
        assert_eq!(char_to_tryte('ç'), Some(-65));
        assert_eq!(char_to_tryte('&'), None);
        assert_eq!(char_to_tryte('ñ'), None);
        assert_eq!(tryte_to_char(37), None);
        assert_eq!(tryte_to_char(-54), Some('á'));
        assert_eq!(tryte_to_char(-66), None);
    }

    #[test]
    fn negation_swaps_case() {
        for c in ('A'..='Z').chain(ACCENT_UPPER) {
            let v = char_to_tryte(c).unwrap();
            assert_eq!(tryte_to_char(-v), c.to_lowercase().next());
        }
    }

    #[test]
    fn roundtrip_whole_table() {
        let mut count = 0;
        for v in -364..=364 {
            if let Some(c) = tryte_to_char(v) {
                assert_eq!(char_to_tryte(c), Some(v), "{c:?}");
                count += 1;
            }
        }
        assert_eq!(count, 1 + 26 + 26 + 10 + 27 + 12 + 12);
    }
}
