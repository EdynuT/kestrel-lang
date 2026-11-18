//! The zero-centered tryte character table (SPEC §4).
//!
//! 0 is a space, so zeroed memory reads as blank. Upper case letters are
//! positive and lower case negative, so negating a tryte swaps the case.

const PUNCT: [char; 17] = [
    '\n', '\t', '.', ',', ':', ';', '!', '?', '-', '+', '/', '_', '=', '(', ')', '[', ']',
];

/// Character for a tryte value, or `None` if the value is unassigned.
pub fn tryte_to_char(v: i64) -> Option<char> {
    match v {
        0 => Some(' '),
        1..=26 => Some((b'A' + (v - 1) as u8) as char),
        -26..=-1 => Some((b'a' + (-v - 1) as u8) as char),
        27..=36 => Some((b'0' + (v - 27) as u8) as char),
        -43..=-27 => Some(PUNCT[(-v - 27) as usize]),
        _ => None,
    }
}

/// Tryte value for a character, or `None` if it is not in the table.
pub fn char_to_tryte(c: char) -> Option<i64> {
    match c {
        ' ' => Some(0),
        'A'..='Z' => Some(i64::from(c as u8 - b'A') + 1),
        'a'..='z' => Some(-(i64::from(c as u8 - b'a') + 1)),
        '0'..='9' => Some(i64::from(c as u8 - b'0') + 27),
        _ => PUNCT.iter().position(|&p| p == c).map(|i| -(i as i64) - 27),
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
        assert_eq!(char_to_tryte('@'), None);
        assert_eq!(char_to_tryte('ç'), None);
        assert_eq!(tryte_to_char(37), None);
        assert_eq!(tryte_to_char(-44), None);
    }

    #[test]
    fn negation_swaps_case() {
        for c in 'A'..='Z' {
            let v = char_to_tryte(c).unwrap();
            assert_eq!(tryte_to_char(-v), Some(c.to_ascii_lowercase()));
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
        assert_eq!(count, 1 + 26 + 26 + 10 + 17);
    }
}
