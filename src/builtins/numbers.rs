//! Number functions (SPEC 9.5). `min` and `max` live in `collections`.

use super::{Args, Builtin, fixed, func, type_err};
use crate::error::Result;
use crate::eval::Interp;
use crate::eval::short;
use crate::int::Int;
use crate::value::{V, Value};

pub const BUILTINS: &[Builtin] = &[
    func("abs", abs),
    func("sign", sign),
    fixed("gcd", 2, gcd),
    fixed("lcm", 2, lcm),
    fixed("divmod", 2, divmod),
    func("int", int),
    func("isqrt", isqrt),
    func("bits", bits),
    func("ones", ones),
    func("digits", digits),
    fixed("modinv", 2, modinv),
    fixed("modpow", 3, modpow),
    func("crt", crt),
    func("shoelace", shoelace),
    fixed("solve", 2, solve),
];

pub fn int_arg(args: &Args, i: usize, fname: &str) -> Result<Int> {
    match args.pos[i].view() {
        V::Int(n) => Ok(n),
        other => Err(type_err(args, fname, "an int", other)),
    }
}

fn abs(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("abs", 1, 1, &[])?;
    Ok(Value::from_int(int_arg(&args, 0, "abs")?.abs()))
}

fn isqrt(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("isqrt", 1, 1, &[])?;
    let n = int_arg(&args, 0, "isqrt")?;
    match n.isqrt() {
        Some(r) => Ok(Value::from_int(r)),
        None => Err(args.err(format!(
            "`isqrt` needs a finite int of 0 or more, found {n}"
        ))),
    }
}

/// `n.bits`: how many binary digits `n` has, for an int of 0 or more
/// (SPEC 9.5). `0.bits == 0`.
fn bits(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("bits", 1, 1, &[])?;
    let n = int_arg(&args, 0, "bits")?;
    if n.is_inf() || n < Int::Small(0) {
        return Err(args.err(format!("`bits` needs a finite int of 0 or more, found {n}")));
    }
    if n == Int::Small(0) {
        return Ok(Value::int(0));
    }
    let digits = n
        .to_str_radix(2)
        .ok_or_else(|| args.err(format!("`bits` cannot measure {n}")))?;
    Ok(Value::int(digits.len() as i64))
}

/// `n.ones`: how many 1s an int of 0 or more has in binary (SPEC 9.5).
fn ones(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("ones", 1, 1, &[])?;
    let n = int_arg(&args, 0, "ones")?;
    match n {
        Int::Small(i) if i >= 0 => Ok(Value::int(i64::from(i.count_ones()))),
        Int::Big(b) if b.sign() != num_bigint::Sign::Minus => {
            Ok(Value::int(b.magnitude().count_ones() as i64))
        }
        _ => Err(args.err(format!("`ones` needs a finite int of 0 or more, found {n}"))),
    }
}

/// `n.digits` or `digits(n, base)`: the digits of an int of 0 or more,
/// most significant first (SPEC 9.5). `0.digits == [0]`.
fn digits(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("digits", 1, 2, &[])?;
    let n = int_arg(&args, 0, "digits")?;
    let base = match args.pos.len() {
        2 => base_arg(&args, 1, "digits")?,
        _ => 10,
    };
    if n.is_inf() || n < Int::Small(0) {
        return Err(args.err(format!(
            "`digits` needs a finite int of 0 or more, found {n}"
        )));
    }
    let text = n
        .to_str_radix(base)
        .ok_or_else(|| args.err(format!("`digits` cannot take {n} apart")))?;
    Ok(Value::list(
        text.chars()
            .map(|c| Value::int(i64::from(c.to_digit(base).unwrap_or(0))))
            .collect::<Vec<_>>(),
    ))
}

fn sign(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("sign", 1, 1, &[])?;
    Ok(Value::int(int_arg(&args, 0, "sign")?.signum()))
}

fn gcd(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("gcd", 2, 2, &[])?;
    let r = int_arg(&args, 0, "gcd")?.gcd(&int_arg(&args, 1, "gcd")?);
    r.map(Value::from_int).map_err(|m| args.err(m))
}

fn lcm(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("lcm", 2, 2, &[])?;
    let r = int_arg(&args, 0, "lcm")?.lcm(&int_arg(&args, 1, "lcm")?);
    r.map(Value::from_int).map_err(|m| args.err(m))
}

fn divmod(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("divmod", 2, 2, &[])?;
    let a = int_arg(&args, 0, "divmod")?;
    let b = int_arg(&args, 1, "divmod")?;
    let q = a.div(&b).map_err(|m| args.err(m))?;
    let r = a.rem(&b).map_err(|m| args.err(m))?;
    Ok(Value::pair(Value::from_int(q), Value::from_int(r)))
}

/// `base` argument of `int` and `str`: 2 to 36.
pub fn base_arg(args: &Args, i: usize, fname: &str) -> Result<u32> {
    match args.pos[i].view() {
        V::Int(Int::Small(b)) if (2..=36).contains(&b) => Ok(b as u32),
        V::Int(b) => Err(args.err(format!("`{fname}` needs a base from 2 to 36, found {b}"))),
        other => Err(type_err(args, fname, "an int base", other)),
    }
}

/// `int(s)`, `int(s, base)`, `int(c)`, `int(b)` (SPEC 9.1).
fn int(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("int", 1, 2, &[])?;
    if args.pos.len() == 2 {
        let base = base_arg(&args, 1, "int")?;
        let s: std::rc::Rc<str> = match args.pos[0].view() {
            V::Str(s) => s.clone(),
            // A single digit, such as 'f' in base 16.
            V::Char(c) => c.to_string().into(),
            _ => {
                return Err(type_err(
                    &args,
                    "int",
                    "a str or char to parse in a base",
                    &args.pos[0],
                ));
            }
        };
        return Int::parse_radix(s.trim(), base)
            .map(Value::from_int)
            .ok_or_else(|| args.err(format!("`int` cannot parse {:?} in base {base}", &*s)));
    }
    match args.pos[0].view() {
        V::Int(i) => Ok(Value::from_int(i.clone())),
        V::Bool(b) => Ok(Value::int(i64::from(b))),
        V::Char(c) => match c.to_digit(10) {
            Some(d) => Ok(Value::int(i64::from(d))),
            None => Err(args.err(format!("`int` expects a digit, found {c:?}"))),
        },
        V::Str(s) => {
            let t = s.trim();
            let t = t.strip_prefix('+').unwrap_or(t);
            let digits = t.strip_prefix('-').unwrap_or(t);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(args.err(format!("`int` cannot parse {:?}", &**s)));
            }
            Int::parse(t)
                .map(Value::from_int)
                .ok_or_else(|| args.err(format!("`int` cannot parse {:?}", &**s)))
        }
        other => Err(type_err(&args, "int", "a str, char, bool or int", other)),
    }
}

// --- Number theory and geometry (SPEC 9.5) ---------------------------------

type IntResult = std::result::Result<Int, String>;

fn one() -> Int {
    Int::Small(1)
}

fn zero() -> Int {
    Int::Small(0)
}

/// A positive, finite modulus argument.
fn modulus_arg(args: &Args, i: usize, fname: &str) -> Result<Int> {
    match args.pos[i].view() {
        V::Int(m) if m.signum() > 0 && !m.is_inf() => Ok(m),
        V::Int(m) => Err(args.err(format!("`{fname}` needs a positive modulus, found {m}"))),
        other => Err(type_err(args, fname, "an int modulus", other)),
    }
}

/// The inverse of `a` modulo `m > 0`, if `gcd(a, m) == 1` and `m > 1`.
fn inverse(a: &Int, m: &Int) -> std::result::Result<Option<Int>, String> {
    if *m == one() {
        return Ok(None);
    }
    // Extended Euclid on non-negative values.
    let (mut r0, mut r1) = (a.rem(m)?, m.clone());
    let (mut s0, mut s1) = (one(), zero());
    while !r1.is_zero() {
        let q = r0.div(&r1)?;
        (r0, r1) = (r1.clone(), r0.sub(&q.mul(&r1)?)?);
        (s0, s1) = (s1.clone(), s0.sub(&q.mul(&s1)?)?);
    }
    if r0 != one() {
        return Ok(None);
    }
    Ok(Some(s0.rem(m)?))
}

fn modinv(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("modinv", 2, 2, &[])?;
    let a = int_arg(&args, 0, "modinv")?;
    let m = modulus_arg(&args, 1, "modinv")?;
    let inv = inverse(&a, &m).map_err(|e| args.err(e))?;
    Ok(inv.map_or(Value::NONE, Value::from_int))
}

fn modpow(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("modpow", 3, 3, &[])?;
    let b = int_arg(&args, 0, "modpow")?;
    let e = int_arg(&args, 1, "modpow")?;
    let m = modulus_arg(&args, 2, "modpow")?;
    if e.signum() < 0 {
        return Err(args.err(format!(
            "`modpow` needs an exponent of 0 or more, found {e}"
        )));
    }
    b.modpow(&e, &m)
        .map(Value::from_int)
        .ok_or_else(|| args.err("`modpow` is undefined for inf"))
}

/// Combines congruences `x = r (mod n)` one at a time; moduli may share
/// factors.
fn crt(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("crt", 1, 1, &[])?;
    let pairs = it.to_vec(&args.pos[0])?;
    let step =
        |x: &Int, m: &Int, r: &Int, n: &Int| -> std::result::Result<Option<(Int, Int)>, String> {
            let r = r.rem(n)?;
            let g = m.gcd(n)?;
            let diff = r.sub(x)?;
            if !diff.rem(&g)?.is_zero() {
                return Ok(None);
            }
            let n_g = n.div(&g)?;
            // Solve m * t = diff (mod n) for t.
            let t = match inverse(&m.div(&g)?, &n_g)? {
                Some(inv) => diff.div(&g)?.mul(&inv)?.rem(&n_g)?,
                None => zero(), // n / g == 1: any t works
            };
            let lcm = m.mul(&n_g)?;
            Ok(Some((x.add(&m.mul(&t)?)?.rem(&lcm)?, lcm)))
        };
    let (mut x, mut m) = (zero(), one());
    for pair in &pairs {
        let (r, n) = match pair.view() {
            V::Tuple(t) if t.len() == 2 => match (t[0].view(), t[1].view()) {
                (V::Int(r), V::Int(n)) if !r.is_inf() && !n.is_inf() => (r, n),
                _ => {
                    return Err(args.err(format!(
                        "`crt` expects (remainder, modulus) ints, found {}",
                        short(pair)
                    )));
                }
            },
            _ => {
                return Err(args.err(format!(
                    "`crt` expects (remainder, modulus) pairs, found {}",
                    short(pair)
                )));
            }
        };
        if n.signum() <= 0 {
            return Err(args.err(format!("`crt` needs positive moduli, found {n}")));
        }
        match step(&x, &m, &r, &n).map_err(|e| args.err(e))? {
            Some((x2, m2)) => (x, m) = (x2, m2),
            None => return Ok(Value::NONE),
        }
    }
    Ok(Value::pair(Value::from_int(x), Value::from_int(m)))
}

fn shoelace(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("shoelace", 1, 1, &[])?;
    let mut points = Vec::new();
    for p in it.to_vec(&args.pos[0])? {
        match p.view() {
            V::Tuple(t) if t.len() == 2 => match (t[0].view(), t[1].view()) {
                (V::Int(x), V::Int(y)) => points.push((x.clone(), y.clone())),
                _ => {
                    return Err(args.err(format!(
                        "`shoelace` expects (x, y) ints, found {}",
                        short(&p)
                    )));
                }
            },
            _ => {
                return Err(args.err(format!(
                    "`shoelace` expects (x, y) points, found {}",
                    short(&p)
                )));
            }
        }
    }
    let twice = (|| -> IntResult {
        let mut sum = zero();
        for (i, (x1, y1)) in points.iter().enumerate() {
            let (x2, y2) = &points[(i + 1) % points.len()];
            sum = sum.add(&x1.mul(y2)?.sub(&x2.mul(y1)?)?)?;
        }
        Ok(sum.abs())
    })()
    .map_err(|e| args.err(e))?;
    if !twice
        .rem(&Int::Small(2))
        .map_err(|e| args.err(e))?
        .is_zero()
    {
        return Err(args.err(format!(
            "`shoelace` area is {}.5, which isn't a whole number",
            twice.div(&Int::Small(2)).map_err(|e| args.err(e))?
        )));
    }
    twice
        .div(&Int::Small(2))
        .map(Value::from_int)
        .map_err(|e| args.err(e))
}

/// An exact fraction with a positive denominator, in lowest terms.
#[derive(Clone)]
struct Frac {
    n: Int,
    d: Int,
}

impl Frac {
    fn int(n: Int) -> Frac {
        Frac { n, d: one() }
    }

    fn new(n: Int, d: Int) -> std::result::Result<Frac, String> {
        let g = n.gcd(&d)?;
        let g = if d.signum() < 0 { g.neg() } else { g };
        Ok(Frac {
            n: n.div(&g)?,
            d: d.div(&g)?,
        })
    }

    fn is_zero(&self) -> bool {
        self.n.is_zero()
    }

    fn sub(&self, o: &Frac) -> std::result::Result<Frac, String> {
        Frac::new(
            self.n.mul(&o.d)?.sub(&o.n.mul(&self.d)?)?,
            self.d.mul(&o.d)?,
        )
    }

    fn mul(&self, o: &Frac) -> std::result::Result<Frac, String> {
        Frac::new(self.n.mul(&o.n)?, self.d.mul(&o.d)?)
    }

    fn div(&self, o: &Frac) -> std::result::Result<Frac, String> {
        Frac::new(self.n.mul(&o.d)?, self.d.mul(&o.n)?)
    }
}

/// Gauss-Jordan elimination over exact fractions.
fn solve(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("solve", 2, 2, &[])?;
    let ints = |it: &mut Interp<'_>, v: &Value, what: &str| -> Result<Vec<Frac>> {
        it.to_vec(v)?
            .into_iter()
            .map(|x| match x.view() {
                V::Int(i) if !i.is_inf() => Ok(Frac::int(i)),
                other => Err(args.err(format!(
                    "`solve` expects {what} of ints, found {}",
                    other.type_name()
                ))),
            })
            .collect()
    };
    let rows = it.to_vec(&args.pos[0])?;
    let rhs = ints(it, &args.pos[1], "a right-hand side")?;
    if rows.is_empty() {
        return Err(args.err("`solve` needs at least one row"));
    }
    if rows.len() != rhs.len() {
        return Err(args.err(format!(
            "`solve` needs one right-hand value per row: found {} rows and {} values",
            rows.len(),
            rhs.len()
        )));
    }
    let mut m: Vec<Vec<Frac>> = Vec::new();
    for (row, b) in rows.iter().zip(rhs) {
        let mut r = ints(it, row, "rows")?;
        r.push(b);
        m.push(r);
    }
    let n = m[0].len() - 1;
    if m.iter().any(|r| r.len() != n + 1) {
        return Err(args.err("`solve` needs rows of equal length"));
    }
    let result = (|| -> std::result::Result<Option<Vec<Value>>, String> {
        let mut rank = 0;
        for col in 0..n {
            let Some(p) = (rank..m.len()).find(|&i| !m[i][col].is_zero()) else {
                continue;
            };
            m.swap(rank, p);
            let pivot = m[rank][col].clone();
            for v in m[rank].iter_mut() {
                *v = v.div(&pivot)?;
            }
            let pivot_row = m[rank].clone();
            for (i, row) in m.iter_mut().enumerate() {
                if i != rank && !row[col].is_zero() {
                    let f = row[col].clone();
                    for (v, p) in row.iter_mut().zip(&pivot_row) {
                        *v = v.sub(&f.mul(p)?)?;
                    }
                }
            }
            rank += 1;
        }
        // An equation 0 = c with c != 0 means no solution; fewer pivots
        // than unknowns means infinitely many.
        if m.iter()
            .any(|r| r[..n].iter().all(Frac::is_zero) && !r[n].is_zero())
            || rank < n
        {
            return Ok(None);
        }
        let mut xs = Vec::new();
        for r in &m[..n] {
            if r[n].d != one() {
                return Ok(None);
            }
            xs.push(Value::from_int(r[n].n.clone()));
        }
        Ok(Some(xs))
    })()
    .map_err(|e| args.err(e))?;
    Ok(result.map_or(Value::NONE, |xs| {
        Value::list(xs.into_iter().collect::<crate::value::List>())
    }))
}
