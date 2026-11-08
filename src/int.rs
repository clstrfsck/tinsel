//! Tinsel integers: an `i64` fast path that promotes to `BigInt` on
//! overflow, plus `inf` and `-inf` (SPEC 4).
//!
//! Invariant: a `Big` value never fits in an `i64`, so structural equality
//! and hashing work without normalising at comparison time.

use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;
use std::fmt;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Int {
    Small(i64),
    Big(Rc<BigInt>),
    /// `true` is `inf`, `false` is `-inf`.
    Inf(bool),
}

pub type IntResult = Result<Int, String>;

impl Int {
    pub fn from_big(b: BigInt) -> Int {
        match i64::try_from(&b) {
            Ok(i) => Int::Small(i),
            Err(_) => Int::Big(Rc::new(b)),
        }
    }

    pub fn parse(s: &str) -> Option<Int> {
        let digits: String = s.chars().filter(|&c| c != '_').collect();
        if let Ok(i) = digits.parse::<i64>() {
            return Some(Int::Small(i));
        }
        digits.parse::<BigInt>().ok().map(Int::from_big)
    }

    /// Parses digits in `base` (2 to 36), with an optional sign. Letters
    /// stand for 10 and up, in either case. No other characters are allowed.
    pub fn parse_radix(s: &str, base: u32) -> Option<Int> {
        let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(base)) {
            return None;
        }
        let n = BigInt::parse_bytes(digits.as_bytes(), base)?;
        Some(Int::from_big(if s.starts_with('-') { -n } else { n }))
    }

    /// Formats in `base` (2 to 36) with lowercase letters; `None` for inf.
    pub fn to_str_radix(&self, base: u32) -> Option<String> {
        self.to_big().map(|b| b.to_str_radix(base))
    }

    /// `self ** e % m` for finite `e >= 0` and `m > 0`, in `0..<m`.
    pub fn modpow(&self, e: &Int, m: &Int) -> Option<Int> {
        let (b, e, m) = (self.to_big()?, e.to_big()?, m.to_big()?);
        let r = Int::from_big(b.modpow(&e, &m));
        // Normalise the sign, whatever num-bigint does for a negative base.
        r.rem(&Int::from_big(m)).ok()
    }

    fn to_big(&self) -> Option<BigInt> {
        match self {
            Int::Small(i) => Some(BigInt::from(*i)),
            Int::Big(b) => Some((**b).clone()),
            Int::Inf(_) => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Int::Small(i) => Some(*i),
            _ => None,
        }
    }

    pub fn is_inf(&self) -> bool {
        matches!(self, Int::Inf(_))
    }

    /// -1, 0 or 1.
    pub fn signum(&self) -> i64 {
        match self {
            Int::Small(i) => i.signum(),
            Int::Big(b) => match b.sign() {
                Sign::Minus => -1,
                Sign::NoSign => 0,
                Sign::Plus => 1,
            },
            Int::Inf(pos) => {
                if *pos {
                    1
                } else {
                    -1
                }
            }
        }
    }

    /// The largest `r` with `r * r <= self`, or `None` for a negative or
    /// infinite int.
    pub fn isqrt(&self) -> Option<Int> {
        if self.signum() < 0 {
            return None;
        }
        match self {
            Int::Small(n) => {
                let n = i128::from(*n);
                let mut r = (n as f64).sqrt() as i128;
                while r * r > n {
                    r -= 1;
                }
                while (r + 1) * (r + 1) <= n {
                    r += 1;
                }
                i64::try_from(r).ok().map(Int::Small)
            }
            Int::Big(b) => Some(Int::from_big(b.sqrt())),
            Int::Inf(_) => None,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.signum() == 0
    }

    pub fn neg(&self) -> Int {
        match self {
            Int::Small(i) => match i.checked_neg() {
                Some(r) => Int::Small(r),
                None => Int::from_big(-BigInt::from(*i)),
            },
            Int::Big(b) => Int::from_big(-(**b).clone()),
            Int::Inf(pos) => Int::Inf(!*pos),
        }
    }

    pub fn abs(&self) -> Int {
        if self.signum() < 0 {
            self.neg()
        } else {
            self.clone()
        }
    }

    pub fn add(&self, o: &Int) -> IntResult {
        match (self, o) {
            (Int::Small(a), Int::Small(b)) => Ok(match a.checked_add(*b) {
                Some(r) => Int::Small(r),
                None => Int::from_big(BigInt::from(*a) + BigInt::from(*b)),
            }),
            (Int::Inf(a), Int::Inf(b)) => {
                if a == b {
                    Ok(Int::Inf(*a))
                } else {
                    Err("inf - inf is undefined".into())
                }
            }
            (Int::Inf(a), _) | (_, Int::Inf(a)) => Ok(Int::Inf(*a)),
            _ => Ok(Int::from_big(
                self.to_big().unwrap_or_default() + o.to_big().unwrap_or_default(),
            )),
        }
    }

    pub fn sub(&self, o: &Int) -> IntResult {
        self.add(&o.neg())
    }

    pub fn mul(&self, o: &Int) -> IntResult {
        match (self, o) {
            (Int::Small(a), Int::Small(b)) => Ok(match a.checked_mul(*b) {
                Some(r) => Int::Small(r),
                None => Int::from_big(BigInt::from(*a) * BigInt::from(*b)),
            }),
            _ if self.is_inf() || o.is_inf() => {
                let s = self.signum() * o.signum();
                if s == 0 {
                    Err("inf * 0 is undefined".into())
                } else {
                    Ok(Int::Inf(s > 0))
                }
            }
            _ => Ok(Int::from_big(
                self.to_big().unwrap_or_default() * o.to_big().unwrap_or_default(),
            )),
        }
    }

    /// Floor division (rounds toward negative infinity).
    pub fn div(&self, o: &Int) -> IntResult {
        if o.is_zero() {
            return Err("division by zero".into());
        }
        match (self, o) {
            (Int::Small(a), Int::Small(b)) => {
                if let (Some(q), Some(r)) = (a.checked_div(*b), a.checked_rem(*b)) {
                    let q = if r != 0 && ((r < 0) != (*b < 0)) {
                        q - 1
                    } else {
                        q
                    };
                    return Ok(Int::Small(q));
                }
                Ok(Int::from_big(
                    big_floor_div(&BigInt::from(*a), &BigInt::from(*b)).0,
                ))
            }
            (Int::Inf(_), Int::Inf(_)) => Err("inf / inf is undefined".into()),
            (Int::Inf(pos), _) => Ok(Int::Inf(*pos == (o.signum() > 0))),
            (_, Int::Inf(_)) => Err("division by inf is undefined".into()),
            _ => Ok(Int::from_big(
                big_floor_div(
                    &self.to_big().unwrap_or_default(),
                    &o.to_big().unwrap_or_default(),
                )
                .0,
            )),
        }
    }

    /// Modulo with the sign of the divisor.
    pub fn rem(&self, o: &Int) -> IntResult {
        if o.is_zero() {
            return Err("modulo by zero".into());
        }
        if self.is_inf() || o.is_inf() {
            return Err("modulo with inf is undefined".into());
        }
        match (self, o) {
            (Int::Small(a), Int::Small(b)) => match a.checked_rem(*b) {
                Some(r) => Ok(Int::Small(if r != 0 && ((r < 0) != (*b < 0)) {
                    r + b
                } else {
                    r
                })),
                None => Ok(Int::Small(0)), // i64::MIN % -1
            },
            _ => Ok(Int::from_big(
                big_floor_div(
                    &self.to_big().unwrap_or_default(),
                    &o.to_big().unwrap_or_default(),
                )
                .1,
            )),
        }
    }

    pub fn pow(&self, o: &Int) -> IntResult {
        let e = match o {
            Int::Small(e) if *e >= 0 => *e,
            Int::Small(_) | Int::Big(_) if o.signum() < 0 => {
                return Err("negative exponent".into());
            }
            Int::Inf(_) => return Err("inf exponent is undefined".into()),
            _ => return Err("exponent too large".into()),
        };
        match self {
            Int::Inf(pos) => {
                if e == 0 {
                    Err("inf ** 0 is undefined".into())
                } else {
                    Ok(Int::Inf(*pos || e % 2 == 0))
                }
            }
            Int::Small(b) => {
                if let Ok(e32) = u32::try_from(e)
                    && let Some(r) = b.checked_pow(e32)
                {
                    return Ok(Int::Small(r));
                }
                if *b == 0 || *b == 1 {
                    return Ok(Int::Small(*b));
                }
                if *b == -1 {
                    return Ok(Int::Small(if e % 2 == 0 { 1 } else { -1 }));
                }
                let e32 = u32::try_from(e).map_err(|_| "exponent too large".to_string())?;
                let b = BigInt::from(*b);
                Int::check_pow_size(&b, e32)?;
                Ok(Int::from_big(b.pow(e32)))
            }
            Int::Big(b) => {
                let e32 = u32::try_from(e).map_err(|_| "exponent too large".to_string())?;
                Int::check_pow_size(b, e32)?;
                Ok(Int::from_big(b.pow(e32)))
            }
        }
    }

    /// `b ** e` has at least `(b.bits() - 1) * e` bits; refuse it when that
    /// is past [`crate::MAX_VALUE_BYTES`]. `b` is not 0, 1 or -1.
    fn check_pow_size(b: &BigInt, e: u32) -> Result<(), String> {
        let bits = b.bits().saturating_sub(1).saturating_mul(u64::from(e));
        if bits > crate::MAX_VALUE_BYTES * 8 {
            return Err(format!("result of ** is too large: exponent {e}"));
        }
        Ok(())
    }

    fn finite_pair(&self, o: &Int, op: &str) -> Result<(BigInt, BigInt), String> {
        match (self.to_big(), o.to_big()) {
            (Some(a), Some(b)) => Ok((a, b)),
            _ => Err(format!("{op} is undefined for inf")),
        }
    }

    pub fn bitand(&self, o: &Int) -> IntResult {
        if let (Int::Small(a), Int::Small(b)) = (self, o) {
            return Ok(Int::Small(a & b));
        }
        let (a, b) = self.finite_pair(o, "&")?;
        Ok(Int::from_big(a & b))
    }

    pub fn bitor(&self, o: &Int) -> IntResult {
        if let (Int::Small(a), Int::Small(b)) = (self, o) {
            return Ok(Int::Small(a | b));
        }
        let (a, b) = self.finite_pair(o, "|")?;
        Ok(Int::from_big(a | b))
    }

    pub fn bitxor(&self, o: &Int) -> IntResult {
        if let (Int::Small(a), Int::Small(b)) = (self, o) {
            return Ok(Int::Small(a ^ b));
        }
        let (a, b) = self.finite_pair(o, "xor")?;
        Ok(Int::from_big(a ^ b))
    }

    fn shift_amount(o: &Int) -> Result<u64, String> {
        match o {
            Int::Small(s) if *s >= 0 => Ok(*s as u64),
            Int::Small(_) => Err("negative shift amount".into()),
            _ => Err("shift amount too large".into()),
        }
    }

    pub fn shl(&self, o: &Int) -> IntResult {
        let s = Int::shift_amount(o)?;
        if let Int::Small(a) = self
            && s < 64
        {
            let r = a.wrapping_shl(s as u32);
            if r.wrapping_shr(s as u32) == *a {
                return Ok(Int::Small(r));
            }
        }
        let a = self.to_big().ok_or("<< is undefined for inf")?;
        if a.sign() == Sign::NoSign {
            return Ok(Int::Small(0));
        }
        if s.saturating_add(a.bits()) > crate::MAX_VALUE_BYTES * 8 {
            return Err(format!("result of << is too large: shift {s}"));
        }
        Ok(Int::from_big(a << s))
    }

    pub fn shr(&self, o: &Int) -> IntResult {
        let s = Int::shift_amount(o)?;
        if let Int::Small(a) = self {
            return Ok(Int::Small(if s >= 64 {
                if *a < 0 { -1 } else { 0 }
            } else {
                a >> s
            }));
        }
        let a = self.to_big().ok_or(">> is undefined for inf")?;
        Ok(Int::from_big(a >> s))
    }

    pub fn gcd(&self, o: &Int) -> IntResult {
        let mut a = self.abs();
        let mut b = o.abs();
        if a.is_inf() || b.is_inf() {
            return Err("gcd is undefined for inf".into());
        }
        while !b.is_zero() {
            let r = a.rem(&b)?;
            a = b;
            b = r;
        }
        Ok(a)
    }

    pub fn lcm(&self, o: &Int) -> IntResult {
        if self.is_zero() || o.is_zero() {
            return Ok(Int::Small(0));
        }
        let g = self.gcd(o)?;
        self.div(&g)?.mul(o).map(|r| r.abs())
    }
}

/// Floor division and modulo for big integers.
fn big_floor_div(a: &BigInt, b: &BigInt) -> (BigInt, BigInt) {
    let q = a / b;
    let r = a % b;
    if r.sign() != Sign::NoSign && ((r.sign() == Sign::Minus) != (b.sign() == Sign::Minus)) {
        (q - 1, r + b)
    } else {
        (q, r)
    }
}

impl Ord for Int {
    fn cmp(&self, o: &Int) -> Ordering {
        match (self, o) {
            (Int::Small(a), Int::Small(b)) => a.cmp(b),
            (Int::Inf(a), Int::Inf(b)) => a.cmp(b),
            (Int::Inf(a), _) => {
                if *a {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (_, Int::Inf(b)) => {
                if *b {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            _ => self.to_big().cmp(&o.to_big()),
        }
    }
}

impl PartialOrd for Int {
    fn partial_cmp(&self, o: &Int) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Int::Small(i) => write!(f, "{i}"),
            Int::Big(b) => write!(f, "{b}"),
            Int::Inf(true) => write!(f, "inf"),
            Int::Inf(false) => write!(f, "-inf"),
        }
    }
}

impl From<i64> for Int {
    fn from(i: i64) -> Int {
        Int::Small(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(x: i64) -> Int {
        Int::Small(x)
    }

    #[test]
    fn floor_div_and_mod() {
        assert_eq!(i(-7).div(&i(2)).unwrap(), i(-4));
        assert_eq!(i(-7).rem(&i(3)).unwrap(), i(2));
        assert_eq!(i(7).rem(&i(-3)).unwrap(), i(-2));
        assert_eq!(
            i(i64::MIN).div(&i(-1)).unwrap().to_string(),
            "9223372036854775808"
        );
    }

    #[test]
    fn promotion_and_demotion() {
        let big = i(i64::MAX).add(&i(1)).unwrap();
        assert!(matches!(big, Int::Big(_)));
        assert_eq!(big.sub(&i(1)).unwrap(), i(i64::MAX));
        assert_eq!(
            i(2).pow(&i(100)).unwrap().to_string(),
            "1267650600228229401496703205376"
        );
        assert_eq!(i(1).shl(&i(70)).unwrap().shr(&i(70)).unwrap(), i(1));
    }

    #[test]
    fn inf_rules() {
        let inf = Int::Inf(true);
        assert_eq!(inf.add(&i(1)).unwrap(), inf);
        assert!(inf.sub(&inf).is_err());
        assert!(inf.mul(&i(0)).is_err());
        assert!(i(5) < inf);
        assert!(Int::Inf(false) < i(-5));
    }
}
