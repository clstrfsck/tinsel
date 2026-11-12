//! String functions (SPEC 9.1). `join` lives in `collections`.

use super::{Args, Builtin, fixed, func, type_err};
use crate::error::Result;
use crate::eval::Interp;
use crate::int::Int;
use crate::value::{V, Value};

pub const BUILTINS: &[Builtin] = &[
    func("lines", lines),
    func("blocks", blocks),
    func("words", words),
    func("chars", chars),
    func("ints", ints),
    fixed("split", 2, split),
    func("trim", trim),
    fixed("starts_with", 2, starts_with),
    fixed("ends_with", 2, ends_with),
    func("str", str_),
    fixed("replace", 3, replace),
    func("ord", ord),
    func("chr", chr),
    fixed("is_digit", 1, is_digit),
    fixed("is_lower", 1, is_lower),
    fixed("is_upper", 1, is_upper),
];

pub fn str_arg<'a>(args: &'a Args, i: usize, fname: &str) -> Result<&'a str> {
    match args.pos[i].view() {
        V::Str(s) => Ok(s),
        other => Err(type_err(args, fname, "a str", other)),
    }
}

fn str_list<'a>(items: impl Iterator<Item = &'a str>) -> Value {
    Value::list(items.map(Value::str).collect::<Vec<_>>())
}

/// Split on `\n`; one trailing newline doesn't make an empty last line.
pub fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.split('\n').collect()
}

fn lines(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("lines", 1, 1, &[])?;
    Ok(str_list(
        split_lines(str_arg(&args, 0, "lines")?).into_iter(),
    ))
}

fn blocks(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("blocks", 1, 1, &[])?;
    let s = str_arg(&args, 0, "blocks")?;
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in split_lines(s) {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(Value::str(&current.join("\n")));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        out.push(Value::str(&current.join("\n")));
    }
    Ok(Value::list(out))
}

fn words(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("words", 1, 1, &[])?;
    Ok(str_list(str_arg(&args, 0, "words")?.split_whitespace()))
}

fn chars(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("chars", 1, 1, &[])?;
    let s = str_arg(&args, 0, "chars")?;
    // An ASCII string has as many chars as bytes, so the list can be made
    // at its final size.
    Ok(Value::list(if s.is_ascii() {
        crate::value::List::from_exact(s.bytes().map(|b| Value::char(char::from(b))))
    } else {
        s.chars().map(Value::char).collect::<crate::value::List>()
    }))
}

/// Every integer in the string. A `-` is a sign only when it doesn't follow
/// a letter or digit.
fn ints(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("ints", 1, 1, &[])?;
    let s = str_arg(&args, 0, "ints")?.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i].is_ascii_digit() {
            let start = i;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
            }
            let negative = start > 0
                && s[start - 1] == b'-'
                && (start < 2 || !s[start - 2].is_ascii_alphanumeric());
            let text = std::str::from_utf8(&s[start..i]).unwrap_or("0");
            let n = Int::parse(text).unwrap_or(Int::Small(0));
            out.push(Value::from_int(if negative { n.neg() } else { n }));
        } else {
            i += 1;
        }
    }
    Ok(Value::list(out))
}

fn split(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("split", 2, 2, &[])?;
    let s = str_arg(&args, 0, "split")?;
    match args.pos[1].view() {
        V::Str(sep) if sep.is_empty() => Err(args.err("`split` needs a non-empty separator")),
        V::Str(sep) => Ok(str_list(s.split(&**sep))),
        V::Char(c) => Ok(str_list(s.split(c))),
        other => Err(type_err(&args, "split", "a str or char separator", other)),
    }
}

fn trim(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("trim", 1, 1, &[])?;
    Ok(Value::str(str_arg(&args, 0, "trim")?.trim()))
}

fn affix_arg(args: &Args, fname: &str) -> Result<String> {
    match args.pos[1].view() {
        V::Str(s) => Ok(s.to_string()),
        V::Char(c) => Ok(c.to_string()),
        other => Err(type_err(args, fname, "a str or char", other)),
    }
}

fn starts_with(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("starts_with", 2, 2, &[])?;
    let s = str_arg(&args, 0, "starts_with")?;
    Ok(Value::bool(
        s.starts_with(&affix_arg(&args, "starts_with")?),
    ))
}

fn ends_with(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("ends_with", 2, 2, &[])?;
    let s = str_arg(&args, 0, "ends_with")?;
    Ok(Value::bool(s.ends_with(&affix_arg(&args, "ends_with")?)))
}

/// `str(x)`, or `str(n, base)` and `str(n, base, width)` for an int (SPEC 9.1).
fn str_(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("str", 1, 3, &[])?;
    if args.pos.len() >= 2 {
        let base = super::numbers::base_arg(&args, 1, "str")?;
        let V::Int(n) = args.pos[0].view() else {
            return Err(type_err(
                &args,
                "str",
                "an int to format in a base",
                &args.pos[0],
            ));
        };
        let digits = n
            .to_str_radix(base)
            .ok_or_else(|| args.err(format!("`str` cannot format {n} in a base")))?;
        // Pad with zeros, after any sign, to at least `width` digits.
        let width =
            match args.pos.get(2).map(Value::view) {
                None => 0,
                Some(V::Int(w)) => w.as_i64().filter(|w| *w >= 0).ok_or_else(|| {
                    args.err(format!("`str` needs a width of 0 or more, found {w}"))
                })? as usize,
                Some(other) => return Err(type_err(&args, "str", "an int width", other)),
            };
        let (sign, body) = match digits.strip_prefix('-') {
            Some(b) => ("-", b),
            None => ("", digits.as_str()),
        };
        // A width too large to allocate is an error, not a crash, as for
        // `repeat`.
        let too_long = || args.err(format!("`str` result is too long: width {width}"));
        if width as u64 > crate::MAX_VALUE_BYTES {
            return Err(too_long());
        }
        let pad = width.saturating_sub(body.len());
        let mut out = String::new();
        out.try_reserve_exact(sign.len() + pad + body.len())
            .map_err(|_| too_long())?;
        out.push_str(sign);
        out.extend(std::iter::repeat_n('0', pad));
        out.push_str(body);
        return Ok(Value::str(&out));
    }
    Ok(Value::str(&args.pos[0].display()))
}

fn replace(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("replace", 3, 3, &[])?;
    let s = str_arg(&args, 0, "replace")?;
    let text = |i: usize| match args.pos[i].view() {
        V::Str(t) => Ok(t.to_string()),
        V::Char(c) => Ok(c.to_string()),
        other => Err(type_err(&args, "replace", "a str or char", other)),
    };
    let (old, new) = (text(1)?, text(2)?);
    if old.is_empty() {
        return Err(args.err("`replace` needs a non-empty string to replace"));
    }
    Ok(Value::str(&s.replace(&old, &new)))
}

fn ord(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("ord", 1, 1, &[])?;
    match args.pos[0].view() {
        V::Char(c) => Ok(Value::int(i64::from(u32::from(c)))),
        other => Err(type_err(&args, "ord", "a char", other)),
    }
}

/// Whether a char passes `test`, or a str is non-empty and every char
/// does.
fn char_class(args: &Args, fname: &str, test: fn(&char) -> bool) -> Result<Value> {
    args.check(fname, 1, 1, &[])?;
    match args.pos[0].view() {
        V::Char(c) => Ok(Value::bool(test(&c))),
        V::Str(s) => Ok(Value::bool(!s.is_empty() && s.chars().all(|c| test(&c)))),
        other => Err(type_err(args, fname, "a char or str", other)),
    }
}

fn is_digit(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    char_class(&args, "is_digit", char::is_ascii_digit)
}

fn is_lower(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    char_class(&args, "is_lower", char::is_ascii_lowercase)
}

fn is_upper(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    char_class(&args, "is_upper", char::is_ascii_uppercase)
}

fn chr(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("chr", 1, 1, &[])?;
    let n = match args.pos[0].view() {
        V::Int(n) => n,
        other => return Err(type_err(&args, "chr", "an int", other)),
    };
    n.as_i64()
        .and_then(|i| u32::try_from(i).ok())
        .and_then(char::from_u32)
        .map(Value::char)
        .ok_or_else(|| args.err(format!("`chr` needs a valid Unicode code point, found {n}")))
}
