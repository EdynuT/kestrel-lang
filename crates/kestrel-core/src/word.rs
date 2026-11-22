//! `Word`: the VM's working representation of a 27-trit integer.
//!
//! Values live in an `i64` for fast native arithmetic. The poison value
//! NaT is the sentinel `i64::MIN`, which lies far outside the 27-trit range.
//! Every operation propagates NaT, and any result outside the range of the
//! target width becomes NaT (no wrap-around, SPEC §3).

use std::fmt;

/// Largest magnitude of a 27-trit word: (3^27 - 1) / 2.
pub const WORD_MAX: i64 = 3_812_798_742_493;
/// Largest magnitude of a 6-trit tryte: (3^6 - 1) / 2.
pub const TRYTE_MAX: i64 = 364;
/// Largest magnitude of a single trit.
pub const TRIT_MAX: i64 = 1;

/// Number of trits in a word.
pub const WORD_TRITS: u32 = 27;

/// 3^k for k in 0..=27. 3^27 still fits in an i64.
pub const POW3: [i64; 28] = {
    let mut t = [1i64; 28];
    let mut i = 1;
    while i < 28 {
        t[i] = t[i - 1] * 3;
        i += 1;
    }
    t
};

/// A 27-trit balanced-ternary integer, or NaT.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Word(i64);

impl Word {
    pub const NAT: Word = Word(i64::MIN);
    pub const ZERO: Word = Word(0);
    pub const ONE: Word = Word(1);
    pub const NEG_ONE: Word = Word(-1);

    /// Builds a word, turning out-of-range values into NaT.
    #[inline]
    pub fn new(v: i64) -> Word {
        Self::in_range(v, WORD_MAX)
    }

    #[inline]
    fn in_range(v: i64, max: i64) -> Word {
        if (-max..=max).contains(&v) { Word(v) } else { Self::NAT }
    }

    #[inline]
    fn from_opt(v: Option<i64>) -> Word {
        v.map_or(Self::NAT, Self::new)
    }

    #[inline]
    pub fn is_nat(self) -> bool {
        self.0 == i64::MIN
    }

    /// The numeric value, or `None` for NaT.
    #[inline]
    pub fn get(self) -> Option<i64> {
        if self.is_nat() { None } else { Some(self.0) }
    }

    /// Applies `f` to both operands, short-circuiting to NaT.
    #[inline]
    fn bin(self, rhs: Word, f: impl FnOnce(i64, i64) -> Option<i64>) -> Word {
        match (self.get(), rhs.get()) {
            (Some(a), Some(b)) => Self::from_opt(f(a, b)),
            _ => Self::NAT,
        }
    }

    /// `self ** exp`. A negative exponent is NaT; `0 ** 0 == 1`.
    pub fn pow(self, exp: Word) -> Word {
        self.bin(exp, |base, e| {
            if e < 0 {
                return None;
            }
            match base {
                0 => return Some(if e == 0 { 1 } else { 0 }),
                1 => return Some(1),
                -1 => return Some(if e % 2 == 0 { 1 } else { -1 }),
                _ => {}
            }
            // |base| >= 2 leaves the word range after at most 42 steps.
            let mut acc: i64 = 1;
            for _ in 0..e {
                acc = acc.checked_mul(base)?;
                if acc.abs() > WORD_MAX {
                    return None;
                }
            }
            Some(acc)
        })
    }

    /// `self << k`: multiplies by 3^k. Any non-zero trit pushed out is NaT.
    pub fn shl3(self, k: Word) -> Word {
        self.bin(k, |a, k| {
            if k < 0 {
                return None;
            }
            if a == 0 {
                return Some(0);
            }
            let p = *POW3.get(usize::try_from(k).ok()?)?;
            a.checked_mul(p)
        })
    }

    /// `self >> k`: drops the k low trits, which rounds to the nearest integer.
    pub fn shr3(self, k: Word) -> Word {
        self.bin(k, |a, k| {
            if k < 0 {
                return None;
            }
            let Some(&d) = POW3.get(usize::try_from(k).ok()?) else {
                return Some(0);
            };
            // d is odd, so a / d is never exactly halfway between two integers.
            Some((a + (d - 1) / 2).div_euclid(d))
        })
    }

    /// Three-way comparison: -1, 0 or +1.
    pub fn cmp3(self, rhs: Word) -> Word {
        self.bin(rhs, |a, b| Some(a.cmp(&b) as i64))
    }

    /// Kleene conjunction (`&&`): the minimum. Strict: NaT on either side is NaT.
    pub fn kand(self, rhs: Word) -> Word {
        self.bin(rhs, |a, b| Some(a.min(b)))
    }

    /// Kleene disjunction (`||`): the maximum. Strict: NaT on either side is NaT.
    pub fn kor(self, rhs: Word) -> Word {
        self.bin(rhs, |a, b| Some(a.max(b)))
    }

    /// `as tryte`: NaT if the value does not fit in 6 trits.
    pub fn narrow_tryte(self) -> Word {
        self.get().map_or(Self::NAT, |a| Self::in_range(a, TRYTE_MAX))
    }

    /// `as trit`: NaT if the value is not -1, 0 or +1.
    pub fn narrow_trit(self) -> Word {
        self.get().map_or(Self::NAT, |a| Self::in_range(a, TRIT_MAX))
    }
}

macro_rules! binop {
    ($trait:ident, $method:ident, $op:expr) => {
        impl std::ops::$trait for Word {
            type Output = Word;
            #[inline]
            fn $method(self, rhs: Word) -> Word {
                self.bin(rhs, $op)
            }
        }
    };
}

binop!(Add, add, i64::checked_add);
binop!(Sub, sub, i64::checked_sub);
binop!(Mul, mul, i64::checked_mul);
// Truncates toward zero. Division by zero is NaT.
binop!(Div, div, i64::checked_div);
// Sign follows the dividend. Remainder by zero is NaT.
binop!(Rem, rem, i64::checked_rem);

/// Negation. In balanced ternary this is also the trit-wise `~`.
impl std::ops::Neg for Word {
    type Output = Word;
    #[inline]
    fn neg(self) -> Word {
        self.get().map_or(Word::NAT, |a| Word(-a))
    }
}

impl fmt::Debug for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.get() {
            Some(v) => f.pad(&v.to_string()),
            None => f.pad("NaT"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::{Add, Div, Mul, Neg, Rem, Sub};

    fn w(v: i64) -> Word {
        Word::new(v)
    }

    #[test]
    fn range_edges() {
        assert_eq!(w(WORD_MAX).get(), Some(WORD_MAX));
        assert_eq!(w(-WORD_MAX).get(), Some(-WORD_MAX));
        assert!(w(WORD_MAX + 1).is_nat());
        assert!(w(-WORD_MAX - 1).is_nat());
        assert_eq!(POW3[27], 2 * WORD_MAX + 1);
    }

    #[test]
    fn overflow_is_nat_not_wrap() {
        assert!(w(WORD_MAX).add(w(1)).is_nat());
        assert!(w(-WORD_MAX).sub(w(1)).is_nat());
        assert!(w(2_000_000).mul(w(2_000_000)).is_nat());
        assert_eq!(w(WORD_MAX).add(w(-1)).get(), Some(WORD_MAX - 1));
    }

    #[test]
    fn nat_propagates() {
        let n = Word::NAT;
        for r in [n.add(w(1)), w(1).mul(n), n.neg(), n.cmp3(w(0)), w(-1).kand(n), n.pow(w(0))] {
            assert!(r.is_nat());
        }
    }

    #[test]
    fn division_and_remainder() {
        assert_eq!(w(7).div(w(2)).get(), Some(3));
        assert_eq!(w(-7).div(w(2)).get(), Some(-3));
        assert_eq!(w(-7).rem(w(2)).get(), Some(-1));
        assert_eq!(w(7).rem(w(-2)).get(), Some(1));
        assert!(w(5).div(w(0)).is_nat());
        assert!(w(5).rem(w(0)).is_nat());
    }

    #[test]
    fn shifts() {
        assert_eq!(w(2).shl3(w(2)).get(), Some(18));
        assert!(POW3[26] * 3 > WORD_MAX);
        assert_eq!(w(1).shl3(w(26)).get(), Some(POW3[26]));
        assert!(w(1).shl3(w(27)).is_nat());
        assert!(w(1).shl3(w(100)).is_nat());
        assert_eq!(w(0).shl3(w(100)).get(), Some(0));
        assert!(w(5).shl3(w(-1)).is_nat());
        assert!(w(5).shl3(Word::NAT).is_nat());

        // Dropping trits rounds to nearest: 2 >> 1 == 1 while 2 / 3 == 0.
        assert_eq!(w(2).shr3(w(1)).get(), Some(1));
        assert_eq!(w(-2).shr3(w(1)).get(), Some(-1));
        assert_eq!(w(1).shr3(w(1)).get(), Some(0));
        assert_eq!(w(4).shr3(w(1)).get(), Some(1));
        assert_eq!(w(5).shr3(w(1)).get(), Some(2));
        assert_eq!(w(WORD_MAX).shr3(w(27)).get(), Some(0));
        assert_eq!(w(WORD_MAX).shr3(w(500)).get(), Some(0));
        assert!(w(5).shr3(w(-1)).is_nat());
    }

    #[test]
    fn power() {
        assert_eq!(w(2).pow(w(10)).get(), Some(1024));
        assert_eq!(w(-3).pow(w(3)).get(), Some(-27));
        assert_eq!(w(0).pow(w(0)).get(), Some(1));
        assert_eq!(w(-1).pow(w(1_000_001)).get(), Some(-1));
        assert_eq!(w(1).pow(w(WORD_MAX)).get(), Some(1));
        assert!(w(2).pow(w(-1)).is_nat());
        assert!(w(3).pow(w(27)).is_nat());
        assert_eq!(w(3).pow(w(26)).get(), Some(POW3[26]));
    }

    #[test]
    fn kleene_logic() {
        let t = [-1, 0, 1];
        for a in t {
            for b in t {
                assert_eq!(w(a).kand(w(b)).get(), Some(a.min(b)));
                assert_eq!(w(a).kor(w(b)).get(), Some(a.max(b)));
            }
        }
        // Strict propagation: no short-circuit.
        assert!(w(-1).kand(Word::NAT).is_nat());
        assert!(w(1).kor(Word::NAT).is_nat());
    }

    #[test]
    fn compare_and_narrow() {
        assert_eq!(w(3).cmp3(w(10)).get(), Some(-1));
        assert_eq!(w(10).cmp3(w(10)).get(), Some(0));
        assert_eq!(w(11).cmp3(w(10)).get(), Some(1));
        assert_eq!(w(364).narrow_tryte().get(), Some(364));
        assert!(w(365).narrow_tryte().is_nat());
        assert!(w(2).narrow_trit().is_nat());
        assert_eq!(w(-1).narrow_trit().get(), Some(-1));
    }
}
