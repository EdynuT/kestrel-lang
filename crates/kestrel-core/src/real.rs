//! `Real`: the ternary floating-point type (SPEC §1.5).
//!
//! A real fits in one 27-trit word: 6 trits of exponent on top of 21 trits
//! of mantissa, and its value is `(m / 3^20) * 3^e`. The mantissa is kept
//! normalized (its top trit is never 0, except for zero itself), so
//! `|m|` lies in `MANT_MIN..=MANT_MAX`. There is no sign trit: the sign
//! lives in the balanced mantissa.
//!
//! Arithmetic is exact on 128-bit integers followed by one rounding to the
//! nearest representable value. Overflow is NaT (there is no infinity) and
//! underflow flushes to zero.

use std::cmp::Ordering;
use std::fmt;

use crate::bet::Word27;
use crate::word::{POW3, Word};

/// Largest exponent magnitude (the exponent is a tryte).
pub const EXP_MAX: i32 = 364;

/// Smallest normalized mantissa magnitude: (3^20 + 1) / 2.
const MANT_MIN: i128 = (POW3[20] as i128 + 1) / 2;
/// Largest mantissa magnitude: (3^21 - 1) / 2.
const MANT_MAX: i128 = (POW3[21] as i128 - 1) / 2;
/// 3^20: the mantissa's implicit scale.
const SCALE: i128 = POW3[20] as i128;
/// 3^21: the weight of the exponent field inside the packed word.
const EXP_WEIGHT: i64 = POW3[21];

/// A ternary floating-point number, or NaT.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Real {
    m: i64,
    e: i16,
}

impl Real {
    pub const NAT: Real = Real { m: i64::MIN, e: 0 };
    pub const ZERO: Real = Real { m: 0, e: 0 };
    /// 1.0 = (3^20 / 3^20) * 3^0.
    pub const ONE: Real = Real { m: POW3[20], e: 0 };

    #[inline]
    pub fn is_nat(self) -> bool {
        self.m == i64::MIN
    }

    /// Mantissa and exponent, as they are in memory (native code writes them).
    pub fn parts(self) -> (i64, i16) {
        (self.m, self.e)
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.m == 0
    }

    /// Rounds `num / den * 3^(e - 20)` to the nearest real.
    /// Returns `None` only if the 128-bit intermediate would overflow.
    fn from_frac(num: i128, den: i128, e: i32) -> Option<Real> {
        debug_assert!(den != 0);
        let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
        if num == 0 {
            return Some(Self::ZERO);
        }
        // Pick k so that round(num / (den * 3^k)) has exactly 21 trits.
        let mut k = num.unsigned_abs().ilog(3) as i32 - (den as u128).ilog(3) as i32 - 20;
        for _ in 0..8 {
            let q = scaled_round(num, den, k)?;
            if q.abs() > MANT_MAX {
                k += 1;
            } else if q.abs() < MANT_MIN {
                k -= 1;
            } else {
                return Some(Self::pack(q, e + k));
            }
        }
        unreachable!("mantissa normalization did not converge")
    }

    fn pack(m: i128, e: i32) -> Real {
        if e > EXP_MAX {
            Self::NAT
        } else if e < -EXP_MAX {
            Self::ZERO
        } else {
            Real { m: m as i64, e: e as i16 }
        }
    }

    /// Builds a real from a decimal literal split at the point, e.g. `("3", "14")`.
    /// Returns `None` if the literal has too many digits to convert exactly.
    pub fn from_decimal(int_digits: &str, frac_digits: &str) -> Option<Real> {
        let all = format!("{int_digits}{frac_digits}");
        if all.is_empty() || !all.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let num: i128 = all.parse().ok()?;
        let den = 10i128.checked_pow(u32::try_from(frac_digits.len()).ok()?)?;
        Self::from_frac(num, den, 20)
    }

    /// `w as real`: rounds to the nearest real (words above ~5 * 10^9 lose trits).
    pub fn from_word(w: Word) -> Real {
        match w.get() {
            Some(v) => Self::from_frac(i128::from(v), 1, 20).unwrap_or(Self::NAT),
            None => Self::NAT,
        }
    }

    /// `r as int`: truncates toward zero. Out of range is NaT.
    pub fn to_word(self) -> Word {
        if self.is_nat() {
            return Word::NAT;
        }
        let s = i32::from(self.e) - 20;
        let m = i128::from(self.m);
        if s >= 0 {
            // |m| >= MANT_MIN, so beyond 27 extra trits the word range is long gone.
            if s > 27 {
                return Word::NAT;
            }
            i64::try_from(m * 3i128.pow(s as u32)).map_or(Word::NAT, Word::new)
        } else {
            let t = (-s) as u32;
            if t > 40 { Word::ZERO } else { Word::new((m / 3i128.pow(t)) as i64) }
        }
    }

    /// `self ** n`, by repeated squaring. A negative `n` gives `1 / self ** -n`.
    pub fn pow(self, n: Word) -> Real {
        let Some(n) = n.get() else {
            return Self::NAT;
        };
        if self.is_nat() {
            return Self::NAT;
        }
        let mut base = self;
        let mut e = n.unsigned_abs();
        let mut acc = Self::ONE;
        while e > 0 {
            if e & 1 == 1 {
                acc = acc * base;
            }
            e >>= 1;
            if e > 0 {
                base = base * base;
            }
            if acc.is_nat() || base.is_nat() {
                return Self::NAT;
            }
        }
        if n < 0 { Self::ONE / acc } else { acc }
    }

    /// Three-way comparison: -1, 0 or +1 (NaT if either side is NaT).
    pub fn cmp3(self, rhs: Real) -> Word {
        if self.is_nat() || rhs.is_nat() {
            return Word::NAT;
        }
        let (sa, sb) = (self.m.signum(), rhs.m.signum());
        let ord = if sa != sb {
            sa.cmp(&sb)
        } else if sa == 0 {
            Ordering::Equal
        } else {
            // Normalized mantissas: the exponent decides first, then the mantissa.
            let mag = (self.e, self.m.abs()).cmp(&(rhs.e, rhs.m.abs()));
            if sa > 0 { mag } else { mag.reverse() }
        };
        Word::new(ord as i64)
    }

    /// Approximate value as an `f64`, for display only.
    pub fn to_f64(self) -> f64 {
        if self.is_nat() {
            return f64::NAN;
        }
        self.m as f64 * 3f64.powi(i32::from(self.e) - 20)
    }
}

/// round(num / (den * 3^k)) for den > 0, half away from zero.
fn scaled_round(num: i128, den: i128, k: i32) -> Option<i128> {
    if k >= 0 {
        let d = den.checked_mul(3i128.checked_pow(k as u32)?)?;
        Some(round_div(num, d))
    } else {
        let n = num.checked_mul(3i128.checked_pow((-k) as u32)?)?;
        Some(round_div(n, den))
    }
}

fn round_div(n: i128, d: i128) -> i128 {
    let (q, r) = (n.abs() / d, n.abs() % d);
    let q = if r >= d - r { q + 1 } else { q };
    if n < 0 { -q } else { q }
}

impl std::ops::Neg for Real {
    type Output = Real;
    fn neg(self) -> Real {
        if self.is_nat() { self } else { Real { m: -self.m, e: self.e } }
    }
}

impl std::ops::Add for Real {
    type Output = Real;
    fn add(self, rhs: Real) -> Real {
        if self.is_nat() || rhs.is_nat() {
            return Real::NAT;
        }
        if self.is_zero() {
            return rhs;
        }
        if rhs.is_zero() {
            return self;
        }
        let (big, small) = if self.e >= rhs.e { (self, rhs) } else { (rhs, self) };
        let d = i32::from(big.e) - i32::from(small.e);
        // `small` is below half an ulp of `big`, and `big` is exact.
        if d > 22 {
            return big;
        }
        let num = i128::from(big.m) * 3i128.pow(d as u32) + i128::from(small.m);
        Real::from_frac(num, 1, i32::from(small.e)).unwrap_or(Real::NAT)
    }
}

impl std::ops::Sub for Real {
    type Output = Real;
    fn sub(self, rhs: Real) -> Real {
        self + -rhs
    }
}

impl std::ops::Mul for Real {
    type Output = Real;
    fn mul(self, rhs: Real) -> Real {
        if self.is_nat() || rhs.is_nat() {
            return Real::NAT;
        }
        let num = i128::from(self.m) * i128::from(rhs.m);
        Real::from_frac(num, SCALE, i32::from(self.e) + i32::from(rhs.e)).unwrap_or(Real::NAT)
    }
}

impl std::ops::Div for Real {
    type Output = Real;
    /// Division by zero is NaT.
    fn div(self, rhs: Real) -> Real {
        if self.is_nat() || rhs.is_nat() || rhs.is_zero() {
            return Real::NAT;
        }
        let num = i128::from(self.m) * SCALE;
        Real::from_frac(num, i128::from(rhs.m), i32::from(self.e) - i32::from(rhs.e))
            .unwrap_or(Real::NAT)
    }
}

/// Packed form: the word `e * 3^21 + m`, whose 27 balanced trits are exactly
/// the 6 exponent trits followed by the 21 mantissa trits.
impl From<Real> for Word27 {
    fn from(r: Real) -> Word27 {
        if r.is_nat() {
            return Word27::NAT;
        }
        Word27::from(Word::new(i64::from(r.e) * EXP_WEIGHT + r.m))
    }
}

impl From<Word27> for Real {
    fn from(b: Word27) -> Real {
        let Some(v) = Word::from(b).get() else {
            return Real::NAT;
        };
        let e = (v + (EXP_WEIGHT - 1) / 2).div_euclid(EXP_WEIGHT);
        let m = v - e * EXP_WEIGHT;
        Real::from_frac(i128::from(m), 1, e as i32).unwrap_or(Real::NAT)
    }
}

impl fmt::Display for Real {
    /// 9 significant digits: the part that 21 trits always get right
    /// (0.5, for example, is not exact in ternary and is stored as 0.50000000014).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nat() {
            return f.pad("NaT");
        }
        let x = self.to_f64();
        if x == 0.0 {
            return f.pad("0.0");
        }
        let mag = x.abs().log10().floor() as i32;
        if (-4..9).contains(&mag) {
            let s = format!("{:.*}", (8 - mag).max(0) as usize, x);
            let s = s.trim_end_matches('0');
            if s.ends_with('.') { f.pad(&format!("{s}0")) } else { f.pad(s) }
        } else {
            let s = format!("{x:.8e}");
            let (mant, exp) = s.split_once('e').expect("scientific format");
            let mant = mant.trim_end_matches('0').trim_end_matches('.');
            f.pad(&format!("{mant}e{exp}"))
        }
    }
}

impl fmt::Debug for Real {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Real {
        let (neg, s) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (i, f) = s.split_once('.').unwrap();
        let x = Real::from_decimal(i, f).unwrap();
        if neg { -x } else { x }
    }

    fn wi(v: i64) -> Real {
        Real::from_word(Word::new(v))
    }

    #[test]
    fn thirds_are_exact() {
        let third = r("1.0") / r("3.0");
        assert_eq!(third * r("3.0"), r("1.0"));
        assert_eq!(third, Real::pack(POW3[20] as i128, -1));
        assert_eq!(r("1.0") / r("9.0") * r("9.0"), Real::ONE);
    }

    #[test]
    fn literals_and_display() {
        assert_eq!(r("3.14159").to_string(), "3.14159");
        assert_eq!(r("2.0").to_string(), "2.0");
        assert_eq!(r("0.5").to_string(), "0.5");
        assert_eq!((r("1.0") / r("3.0")).to_string(), "0.333333333");
        assert_eq!(r("0.0").to_string(), "0.0");
        assert_eq!((-r("2.5")).to_string(), "-2.5");
        assert_eq!(r("123456789012.0").to_string(), "1.23456789e11");
        assert_eq!(Real::NAT.to_string(), "NaT");
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(wi(7) + wi(5), wi(12));
        assert_eq!(wi(7) - wi(12), wi(-5));
        assert_eq!(wi(6) * wi(-7), wi(-42));
        assert_eq!(wi(42) / wi(6), wi(7));
        assert_eq!(r("2.5") + r("2.5"), wi(5));
        assert!(((r("0.1") + r("0.2")).to_f64() - 0.3).abs() < 1e-9);
        assert_eq!(r("1.5") - r("1.5"), Real::ZERO);
    }

    #[test]
    fn nat_overflow_underflow() {
        let big = Real::pack(MANT_MAX, EXP_MAX);
        assert!((big * r("3.0")).is_nat());
        assert!((big + big).is_nat());
        assert!((r("1.0") / Real::ZERO).is_nat());
        assert!((Real::NAT + r("1.0")).is_nat());
        let tiny = Real::pack(MANT_MIN, -EXP_MAX);
        assert_eq!(tiny / r("3.0") / r("3.0"), Real::ZERO);
    }

    #[test]
    fn conversions() {
        assert_eq!(r("6.9").to_word().get(), Some(6));
        assert_eq!(r("-6.9").to_word().get(), Some(-6));
        assert_eq!(r("0.4").to_word().get(), Some(0));
        for v in [0, 1, -1, 13, -364, 1_000_000, 5_230_176_601] {
            assert_eq!(wi(v).to_word().get(), Some(v), "value {v}");
        }
        assert!(Real::pack(MANT_MAX, 60).to_word().is_nat());
        assert!(Real::from_word(Word::NAT).is_nat());
    }

    #[test]
    fn power() {
        assert_eq!(r("2.0").pow(Word::new(10)), wi(1024));
        assert_eq!(r("3.0").pow(Word::new(-2)), r("1.0") / r("9.0"));
        assert_eq!(Real::ZERO.pow(Word::ZERO), Real::ONE);
        assert!(Real::ZERO.pow(Word::new(-1)).is_nat());
        assert_eq!(r("2.0").pow(Word::new(-1)).to_string(), "0.5");
        assert!(r("10.0").pow(Word::new(200)).is_nat());
        assert_eq!(r("1.0").pow(Word::new(3_000_000_000_000)), Real::ONE);
    }

    #[test]
    fn compare() {
        assert_eq!(r("1.5").cmp3(r("2.0")).get(), Some(-1));
        assert_eq!(r("2.0").cmp3(r("2.0")).get(), Some(0));
        assert_eq!(r("-1.0").cmp3(r("-2.0")).get(), Some(1));
        assert_eq!(r("-1.0").cmp3(Real::ZERO).get(), Some(-1));
        assert_eq!(wi(1000).cmp3(r("999.9")).get(), Some(1));
        assert!(Real::NAT.cmp3(Real::ZERO).is_nat());
    }

    #[test]
    fn packed_roundtrip() {
        for x in [r("3.14159"), -r("0.001"), wi(1), Real::ZERO, Real::pack(MANT_MAX, EXP_MAX), Real::pack(-MANT_MIN, -EXP_MAX)] {
            assert_eq!(Real::from(Word27::from(x)), x, "value {x}");
        }
        assert_eq!(Word27::from(Real::NAT), Word27::NAT);
        // Exponent occupies the top 6 trits: 1.0 has e = 0, so the top trits are 0.
        assert_eq!(&Word27::from(Real::ONE).to_trits(27)[2..8], "000000");
    }
}
