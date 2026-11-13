//! Collection functions (SPEC 9.2), plus indexing and slicing.

use super::{Args, Builtin, fixed, func, type_err};
use crate::ast::BinOp;
use crate::error::{Error, Result};
use crate::eval::{Generator, Interp, binary, short, with_article};
use crate::fxhash::{FxHashMap, FxHashSet};
use crate::int::Int;
use crate::value::{self, Heap, V, Value, compare};
use crate::value::{ValueMap, ValueSet};
use imbl::OrdMap;
use std::cmp::Ordering;

pub const BUILTINS: &[Builtin] = &[
    func("len", len),
    fixed("map", 2, map),
    fixed("filter", 2, filter),
    func("count", count),
    func("any", any),
    func("all", all),
    fixed("index", 2, index_of),
    func("indices", indices),
    func("sum", sum),
    func("product", product),
    func("min", min),
    func("max", max),
    fixed("min_by", 2, min_by),
    fixed("max_by", 2, max_by),
    func("sort", sort),
    fixed("sort_by", 2, sort_by),
    func("reverse", reverse),
    fixed("rotate", 2, rotate),
    func("first", first),
    func("last", last),
    fixed("bisect", 2, bisect),
    fixed("take", 2, take),
    fixed("take_while", 2, take_while),
    fixed("drop", 2, drop),
    fixed("drop_while", 2, drop_while),
    fixed("fold", 3, fold),
    fixed("scan", 3, scan),
    func("enumerate", enumerate),
    func("zip", zip),
    func("pairs", pairs),
    fixed("windows", 2, windows),
    fixed("chunks", 2, chunks),
    func("uniq", uniq),
    fixed("repeat", 2, repeat),
    func("flatten", flatten),
    func("group_by", group_by),
    func("counts", counts),
    func("tally", tally),
    func("subsets", subsets),
    fixed("combos", 2, combos),
    func("perms", perms),
    func("transpose", transpose),
    func("list", list),
    func("tuple", tuple),
    func("set", set),
    func("join", join),
    func("keys", keys),
    func("values", values),
    func("items", items),
    fixed("add", 2, add),
    fixed("remove", 2, remove),
    fixed("subset", 2, subset),
    func("union", union),
    func("intersection", intersection),
    func("bounds", bounds),
    func("get", get),
];

// --- Helpers -----------------------------------------------------------------

fn list_of(items: Vec<Value>) -> Value {
    Value::list(items)
}

/// A count of 0 or more. One too large for a `usize` (a bigint, or `inf`)
/// is more than any collection holds, so it saturates.
fn usize_arg(args: &Args, i: usize, fname: &str) -> Result<usize> {
    match args.pos[i].view() {
        V::Int(n) if n.signum() >= 0 => Ok(n
            .as_i64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(usize::MAX)),
        other => Err(type_err(args, fname, "a non-negative int", other)),
    }
}

/// Calls a predicate, which must return a bool (SPEC 4: no truthiness).
fn pred(it: &mut Interp<'_>, f: &Value, x: Value, fname: &str, args: &Args) -> Result<bool> {
    match (it.call(f, vec![x], args.span)?).view() {
        V::Bool(b) => Ok(b),
        other => Err(args.err(format!(
            "the predicate passed to `{fname}` must return a bool, but returned {}",
            other.type_name()
        ))),
    }
}

fn as_bool(v: &Value, fname: &str, args: &Args) -> Result<bool> {
    match v.view() {
        V::Bool(b) => Ok(b),
        other => Err(args.err(format!(
            "`{fname}` with no argument needs bool elements, found {}",
            other.type_name()
        ))),
    }
}

fn is_func(v: &Value) -> bool {
    matches!(v.view(), V::Func(_))
}

/// Sorts with the user-visible ordering, reporting the first comparison error.
fn sort_values(items: &mut [Value], args: &Args) -> Result<()> {
    let mut err = None;
    items.sort_by(|a, b| match compare(a, b) {
        Ok(o) => o,
        Err(e) => {
            err.get_or_insert(e);
            Ordering::Equal
        }
    });
    err.map_or(Ok(()), |e| Err(args.err(e)))
}

fn cmp(a: &Value, b: &Value, args: &Args) -> Result<Ordering> {
    compare(a, b).map_err(|e| args.err(e))
}

fn char_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

fn char_at(s: &str, i: usize) -> Option<char> {
    if s.is_ascii() {
        s.as_bytes().get(i).map(|&b| b as char)
    } else {
        s.chars().nth(i)
    }
}

// --- Indexing -------------------------------------------------------------------

/// Resolves a possibly negative index against a length.
fn resolve_index(i: i64, len: usize) -> Option<usize> {
    let idx = if i < 0 { len as i64 + i } else { i };
    (0..len as i64).contains(&idx).then_some(idx as usize)
}

/// `x[i]` for an integer index; `x.0` also uses this.
pub fn index_int(v: &Value, i: i64) -> std::result::Result<Value, String> {
    let out_of_range = |len: usize| {
        format!(
            "index {i} is out of range for a {} of length {len}",
            v.type_name()
        )
    };
    match v.view() {
        V::List(items) => resolve_index(i, items.len())
            .map(|k| items[k].clone())
            .ok_or_else(|| out_of_range(items.len())),
        V::Tuple(items) => resolve_index(i, items.len())
            .map(|k| items[k].clone())
            .ok_or_else(|| out_of_range(items.len())),
        V::Str(s) => {
            let len = char_len(s);
            resolve_index(i, len)
                .map(|k| Value::char(char_at(s, k).unwrap_or('\0')))
                .ok_or_else(|| out_of_range(len))
        }
        other => Err(format!(
            "cannot index {} with an int",
            with_article(other.type_name())
        )),
    }
}

/// `x[i]` (SPEC 9.2): ints for sequences, keys for maps, positions for grids.
pub fn index(v: &Value, i: &Value) -> std::result::Result<Value, String> {
    // The commonest case first: a list indexed by a small int.
    if let Some(n) = i.as_small()
        && let V::List(items) = v.view()
        && let Some(k) = resolve_index(n, items.len())
    {
        return Ok(items[k].clone());
    }
    match (v.view(), i.view()) {
        (V::Map(m), _) => m
            .get(i)
            .cloned()
            .ok_or_else(|| format!("key {} is not in the map", short(i))),
        (V::Grid(g), _) => match super::grids::position(g, i) {
            Some(idx) => Ok(g.cells[idx].clone()),
            None => Err(format!(
                "position {} is outside the {}x{} grid",
                short(i),
                g.width,
                g.height
            )),
        },
        (_, V::Range(r)) => slice(
            v,
            Some(Value::from_int(r.start.clone())),
            r.end.clone().map(Value::from_int),
            true,
        ),
        (_, V::Int(Int::Small(n))) => index_int(v, n),
        (_, V::Int(_)) => Err("index out of range".into()),
        (V::List(_) | V::Tuple(_) | V::Str(_), other) => Err(format!(
            "a {} index must be an int, found {}",
            v.type_name(),
            other.type_name()
        )),
        (other, _) => Err(format!("cannot index {}", with_article(other.type_name()))),
    }
}

/// `xs[a..b]`, `xs[a..<b]`, `xs[a..]`, `xs[..<b]`. Bounds are clamped.
pub fn slice(
    v: &Value,
    lo: Option<Value>,
    hi: Option<Value>,
    inclusive: bool,
) -> std::result::Result<Value, String> {
    let len = match v.view() {
        V::List(items) => items.len(),
        V::Tuple(items) => items.len(),
        V::Str(s) => char_len(s),
        other => return Err(format!("cannot slice {}", with_article(other.type_name()))),
    } as i64;
    let bound = |b: Option<Value>, default: i64| -> std::result::Result<i64, String> {
        match b.as_ref().map(Value::view) {
            None => Ok(default),
            // A bound past either end saturates: it is clamped to the
            // length below, but only after an inclusive end has had 1 added,
            // so a huge negative bound must stay negative.
            Some(V::Int(n)) => {
                let i = n
                    .as_i64()
                    .unwrap_or(if n.signum() < 0 { i64::MIN } else { i64::MAX });
                Ok(if i < 0 { len.saturating_add(i) } else { i })
            }
            Some(other) => Err(format!(
                "slice bounds must be ints, found {}",
                other.type_name()
            )),
        }
    };
    let start = bound(lo, 0)?.clamp(0, len) as usize;
    let mut end = bound(hi.clone(), len)?;
    if inclusive && hi.is_some() {
        // Saturating: the bound may be as large as `i64::MAX`, and is
        // clamped to the length next anyway.
        end = end.saturating_add(1);
    }
    let end = (end.clamp(0, len) as usize).max(start);
    Ok(match v.view() {
        V::List(items) => Value::list(items.slice(start, end)),
        V::Tuple(items) => Value::tuple(items[start..end].to_vec()),
        V::Str(s) => {
            if s.is_ascii() {
                Value::str(&s[start..end])
            } else {
                Value::from_str_rc(
                    s.chars()
                        .skip(start)
                        .take(end - start)
                        .collect::<String>()
                        .into(),
                )
            }
        }
        _ => Value::NONE,
    })
}

// --- Functions --------------------------------------------------------------------

fn len(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("len", 1, 1, &[])?;
    let n = match args.pos[0].view() {
        V::Str(s) => char_len(s),
        V::List(x) => x.len(),
        V::Tuple(x) => x.len(),
        V::Set(x) => x.len(),
        V::Map(x) => x.len(),
        V::Range(r) => {
            return r
                .len()
                .map(Value::from_int)
                .ok_or_else(|| args.err("an infinite range has no length"));
        }
        V::Gen(_) => it.to_vec(&args.pos[0])?.len(),
        V::RangeSet(r) => return Ok(Value::from_int(r.len())),
        other => return Err(type_err(&args, "len", "a collection", other)),
    };
    Ok(Value::int(n as i64))
}

/// Generators and infinite ranges are read lazily: `map`, `filter`,
/// `enumerate` and `zip` over them give generators (SPEC 9.2).
fn lazy(v: &Value) -> bool {
    match v.view() {
        V::Gen(_) => true,
        V::Range(r) => r.end.as_ref().is_none_or(Int::is_inf),
        _ => false,
    }
}

fn generator(g: Generator) -> Value {
    Value::generator(g)
}

fn map(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("map", 2, 2, &[])?;
    if lazy(&args.pos[0]) {
        let src = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
        return Ok(generator(Generator::map(
            src,
            args.pos[1].clone(),
            args.span,
        )));
    }
    let items = it.to_vec(&args.pos[0])?;
    let mut out = Vec::new();
    for x in items {
        out.push(it.call(&args.pos[1], vec![x], args.span)?);
    }
    Ok(Value::list(out))
}

fn filter(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("filter", 2, 2, &[])?;
    if lazy(&args.pos[0]) {
        let src = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
        return Ok(generator(Generator::filter(
            src,
            args.pos[1].clone(),
            args.span,
        )));
    }
    let items = it.to_vec(&args.pos[0])?;
    let mut out = Vec::new();
    for x in items {
        if pred(it, &args.pos[1], x.clone(), "filter", &args)? {
            out.push(x);
        }
    }
    Ok(Value::list(out))
}

/// `count`, `count(pred)` or `count(value)`.
fn count(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("count", 1, 2, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut n = 0i64;
    for x in items {
        let hit = match args.pos.get(1) {
            None => as_bool(&x, "count", &args)?,
            Some(f) if is_func(f) => pred(it, f, x, "count", &args)?,
            Some(target) => x == *target,
        };
        n += i64::from(hit);
    }
    Ok(Value::int(n))
}

fn any(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("any", 1, 2, &[])?;
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    while let Some(x) = iter.next(it)? {
        let hit = match args.pos.get(1) {
            None => as_bool(&x, "any", &args)?,
            Some(f) => pred(it, f, x, "any", &args)?,
        };
        if hit {
            return Ok(Value::bool(true));
        }
    }
    Ok(Value::bool(false))
}

fn all(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("all", 1, 2, &[])?;
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    while let Some(x) = iter.next(it)? {
        let hit = match args.pos.get(1) {
            None => as_bool(&x, "all", &args)?,
            Some(f) => pred(it, f, x, "all", &args)?,
        };
        if !hit {
            return Ok(Value::bool(false));
        }
    }
    Ok(Value::bool(true))
}

fn index_of(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("index", 2, 2, &[])?;
    // A predicate: the first element for which it is true (SPEC 9.2).
    if is_func(&args.pos[1]) {
        let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
        let mut i = 0i64;
        while let Some(x) = iter.next(it)? {
            if pred(it, &args.pos[1], x, "index", &args)? {
                return Ok(Value::int(i));
            }
            i += 1;
        }
        return Ok(Value::NONE);
    }
    if let V::Str(s) = args.pos[0].view() {
        let byte = match args.pos[1].view() {
            V::Char(c) => s.find(c),
            V::Str(sub) => s.find(&**sub),
            other => {
                return Err(type_err(
                    &args,
                    "index",
                    "a char or str to find in a str",
                    other,
                ));
            }
        };
        return Ok(byte.map_or(Value::NONE, |b| Value::int(s[..b].chars().count() as i64)));
    }
    // Fast path: scan lists and tuples by reference, without cloning.
    let found = match args.pos[0].view() {
        V::List(items) => Some(items.iter().position(|x| *x == args.pos[1])),
        V::Tuple(items) => Some(items.iter().position(|x| *x == args.pos[1])),
        _ => None,
    };
    if let Some(found) = found {
        return Ok(found.map_or(Value::NONE, |i| Value::int(i as i64)));
    }
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    let mut i = 0i64;
    while let Some(x) = iter.next(it)? {
        if x == args.pos[1] {
            return Ok(Value::int(i));
        }
        i += 1;
    }
    Ok(Value::NONE)
}

fn indices(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("indices", 1, 1, &[])?;
    let V::Int(n) = (len(
        it,
        Args {
            pos: args.pos.clone(),
            named: Vec::new(),
            span: args.span,
        },
    )?)
    .view() else {
        return Err(args.err("internal: len"));
    };
    let end = n.sub(&Int::Small(1)).map_err(|m| args.err(m))?;
    Ok(Value::range_rc(std::rc::Rc::new(value::Range {
        start: Int::Small(0),
        end: Some(end),
        exclusive: true,
    })))
}

fn fold_op(it: &mut Interp<'_>, args: &Args, init: Value, op: BinOp) -> Result<Value> {
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    if iter.is_infinite() {
        return Err(args.err("cannot sum an infinite range"));
    }
    let mut acc = init;
    while let Some(x) = iter.next(it)? {
        acc = binary(op, &acc, &x).map_err(|m| args.err(m))?;
    }
    Ok(acc)
}

fn sum(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("sum", 1, 1, &[])?;
    fold_op(it, &args, Value::int(0), BinOp::Add)
}

fn product(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("product", 1, 1, &[])?;
    fold_op(it, &args, Value::int(1), BinOp::Mul)
}

fn extreme(it: &mut Interp<'_>, args: &Args, fname: &str, want: Ordering) -> Result<Value> {
    args.check(fname, 1, usize::MAX, &[])?;
    // Sets and maps are sorted, so their extremes are at the ends.
    if let [coll] = args.pos.as_slice()
        && let V::Set(_) | V::Map(_) | V::RangeSet(_) = coll.view()
    {
        return Ok(sorted_end(coll, want == Ordering::Greater));
    }
    // A range's extremes are its ends, so huge ranges cost nothing.
    if let [x] = args.pos.as_slice()
        && let V::Range(r) = x.view()
    {
        // An infinite end (`a..`, `a..inf`, `-inf..b`) has no element there.
        return match want {
            _ if r.is_empty() => Ok(Value::NONE),
            Ordering::Less if r.start.is_inf() => Err(args.err("an infinite range has no minimum")),
            Ordering::Less => Ok(Value::from_int(r.start.clone())),
            _ => match r.finite_end() {
                Some(e) => Ok(Value::from_int(e.clone())),
                None => Err(args.err("an infinite range has no maximum")),
            },
        };
    }
    let items = if args.pos.len() >= 2 {
        args.pos.clone()
    } else {
        it.to_vec(&args.pos[0])?
    };
    let mut best: Option<Value> = None;
    for x in items {
        best = Some(match best {
            Some(b) if cmp(&x, &b, args)? != want => b,
            _ => x,
        });
    }
    Ok(best.unwrap_or(Value::NONE))
}

fn min(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    extreme(it, &args, "min", Ordering::Less)
}

fn max(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    extreme(it, &args, "max", Ordering::Greater)
}

fn extreme_by(it: &mut Interp<'_>, args: &Args, fname: &str, want: Ordering) -> Result<Value> {
    args.check(fname, 2, 2, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut best: Option<(Value, Value)> = None;
    for x in items {
        let k = it.call(&args.pos[1], vec![x.clone()], args.span)?;
        best = Some(match best {
            Some((bk, bx)) if cmp(&k, &bk, args)? != want => (bk, bx),
            _ => (k, x),
        });
    }
    Ok(best.map_or(Value::NONE, |(_, x)| x))
}

fn min_by(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    extreme_by(it, &args, "min_by", Ordering::Less)
}

fn max_by(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    extreme_by(it, &args, "max_by", Ordering::Greater)
}

/// The `desc:` named argument of `sort` and `sort_by`.
fn descending(args: &Args, fname: &str) -> Result<bool> {
    match (args.named("desc")).map(Value::view) {
        None => Ok(false),
        Some(V::Bool(b)) => Ok(b),
        Some(other) => Err(type_err(args, fname, "`desc:` to be a bool", other)),
    }
}

fn sort(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("sort", 1, 1, &["desc"])?;
    let desc = descending(&args, "sort")?;
    let mut items = it.to_vec(&args.pos[0])?;
    sort_values(&mut items, &args)?;
    if desc {
        items.reverse();
    }
    Ok(list_of(items))
}

fn sort_by(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("sort_by", 2, 2, &["desc"])?;
    let desc = descending(&args, "sort_by")?;
    let items = it.to_vec(&args.pos[0])?;
    let mut keyed = Vec::with_capacity(items.len());
    for x in items {
        keyed.push((it.call(&args.pos[1], vec![x.clone()], args.span)?, x));
    }
    let mut err = None;
    keyed.sort_by(|a, b| match compare(&a.0, &b.0) {
        Ok(o) => o,
        Err(e) => {
            err.get_or_insert(e);
            Ordering::Equal
        }
    });
    if let Some(e) = err {
        return Err(args.err(e));
    }
    // Equal keys keep their original order, descending or not.
    if desc {
        keyed.reverse();
        let mut start = 0;
        while start < keyed.len() {
            let mut end = start + 1;
            while end < keyed.len() && keyed[end].0 == keyed[start].0 {
                end += 1;
            }
            keyed[start..end].reverse();
            start = end;
        }
    }
    Ok(list_of(keyed.into_iter().map(|(_, x)| x).collect()))
}

fn reverse(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("reverse", 1, 1, &[])?;
    match args.pos[0].view() {
        V::Str(s) => Ok(Value::from_str_rc(
            s.chars().rev().collect::<String>().into(),
        )),
        V::Tuple(t) => Ok(Value::tuple(t.iter().rev().cloned().collect())),
        _ => {
            let mut items = it.to_vec(&args.pos[0])?;
            items.reverse();
            Ok(list_of(items))
        }
    }
}

/// `xs.rotate(k)`: the elements from position `k` on, then those before
/// it, for a list, tuple or str. `k` counts from the end when negative, and
/// is taken modulo the length (SPEC 9.2).
fn rotate(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("rotate", 2, 2, &[])?;
    let k = super::numbers::int_arg(&args, 1, "rotate")?;
    if k.is_inf() {
        return Err(args.err(format!("`rotate` needs a finite int, found {k}")));
    }
    // Where the result starts, for a sequence of length n: k modulo n.
    let start = |n: usize| -> usize {
        let Ok(n) = i64::try_from(n) else { return 0 };
        if n == 0 {
            return 0;
        }
        match &k {
            Int::Small(k) => k.rem_euclid(n) as usize,
            Int::Big(b) => {
                let r = &**b % num_bigint::BigInt::from(n);
                let r = i64::try_from(r).unwrap_or(0);
                r.rem_euclid(n) as usize
            }
            Int::Inf(_) => 0,
        }
    };
    match args.pos[0].view() {
        V::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            let at = start(chars.len());
            let out: String = chars[at..].iter().chain(&chars[..at]).collect();
            Ok(Value::from_str_rc(out.into()))
        }
        V::Tuple(t) => {
            let at = start(t.len());
            Ok(Value::tuple(
                t[at..].iter().chain(&t[..at]).cloned().collect(),
            ))
        }
        V::List(l) => {
            let items: Vec<Value> = l.iter().cloned().collect();
            let at = start(items.len());
            Ok(list_of(
                items[at..].iter().chain(&items[..at]).cloned().collect(),
            ))
        }
        other => Err(type_err(&args, "rotate", "a list, tuple or str", other)),
    }
}

fn first(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("first", 1, 2, &[])?;
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    while let Some(x) = iter.next(it)? {
        match args.pos.get(1) {
            None => return Ok(x),
            Some(f) => {
                if pred(it, f, x.clone(), "first", &args)? {
                    return Ok(x);
                }
            }
        }
    }
    Ok(Value::NONE)
}

/// `r.bisect(f)`: the first int of the range `r` for which `f` is true,
/// assuming `f` is false up to some point and true from then on.
fn bisect(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("bisect", 2, 2, &[])?;
    let V::Range(r) = args.pos[0].view() else {
        return Err(type_err(&args, "bisect", "a range", &args.pos[0]));
    };
    if r.start.is_inf() {
        return Err(args.err("`bisect` needs a range with a finite start"));
    }
    let f = &args.pos[1];
    let int_err = |m: String| args.err(m);
    let test =
        |it: &mut Interp<'_>, n: &Int| pred(it, f, Value::from_int(n.clone()), "bisect", &args);
    // Find `lo` where `f` is false and `hi` where it is true.
    let start = r.start.clone();
    if r.is_empty() {
        return Ok(Value::NONE);
    }
    if test(it, &start)? {
        return Ok(Value::from_int(start));
    }
    let (mut lo, mut hi) = match r.finite_end() {
        Some(end) => {
            if !test(it, end)? {
                return Ok(Value::NONE);
            }
            (start, end.clone())
        }
        None => {
            // An infinite range: double the step until `f` turns true.
            let mut lo = start.clone();
            let mut step = Int::Small(1);
            loop {
                let hi = start.add(&step).map_err(int_err)?;
                if test(it, &hi)? {
                    break (lo, hi);
                }
                lo = hi;
                step = step.mul(&Int::Small(2)).map_err(int_err)?;
            }
        }
    };
    let one = Int::Small(1);
    let two = Int::Small(2);
    while hi.sub(&lo).map_err(int_err)? > one {
        let mid = lo
            .add(&hi.sub(&lo).map_err(int_err)?.div(&two).map_err(int_err)?)
            .map_err(int_err)?;
        if test(it, &mid)? {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok(Value::from_int(hi))
}

/// The first `n` elements, read lazily.
fn take(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("take", 2, 2, &[])?;
    let n = usize_arg(&args, 1, "take")?;
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    let mut out = Vec::new();
    while out.len() < n {
        match iter.next(it)? {
            Some(x) => out.push(x),
            None => break,
        }
    }
    Ok(list_of(out))
}

/// The leading elements for which `f` is true, read lazily.
fn take_while(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("take_while", 2, 2, &[])?;
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    let mut out = Vec::new();
    while let Some(x) = iter.next(it)? {
        if !pred(it, &args.pos[1], x.clone(), "take_while", &args)? {
            break;
        }
        out.push(x);
    }
    Ok(list_of(out))
}

/// The elements after the first `n`: a list, or a lazy generator for a
/// generator or an infinite range.
fn drop(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("drop", 2, 2, &[])?;
    let n = usize_arg(&args, 1, "drop")?;
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    if lazy(&args.pos[0]) {
        return Ok(generator(Generator::drop(iter, n, None, args.span)));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(x) = iter.next(it)? {
        if i >= n {
            out.push(x);
        } else {
            i += 1;
        }
    }
    Ok(list_of(out))
}

/// The elements after the leading ones for which `f` is true: a list, or a
/// lazy generator for a generator or an infinite range.
fn drop_while(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("drop_while", 2, 2, &[])?;
    let f = args.pos[1].clone();
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    if lazy(&args.pos[0]) {
        return Ok(generator(Generator::drop(iter, 0, Some(f), args.span)));
    }
    let mut out = Vec::new();
    let mut dropping = true;
    while let Some(x) = iter.next(it)? {
        if dropping && it.drop_while_test(&f, &x, args.span)? {
            continue;
        }
        dropping = false;
        out.push(x);
    }
    Ok(list_of(out))
}

fn last(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("last", 1, 1, &[])?;
    if let V::Range(r) = args.pos[0].view() {
        return match r.finite_end() {
            _ if r.is_empty() => Ok(Value::NONE),
            Some(e) => Ok(Value::from_int(e.clone())),
            None => Err(args.err("an infinite range has no last element")),
        };
    }
    if let V::Set(_) | V::Map(_) | V::RangeSet(_) = args.pos[0].view() {
        return Ok(sorted_end(&args.pos[0], true));
    }
    Ok(it.to_vec(&args.pos[0])?.pop().unwrap_or(Value::NONE))
}

/// The smallest or largest element of a set (or key of a map), or `none`.
fn sorted_end(coll: &Value, largest: bool) -> Value {
    let found = match (coll.view(), largest) {
        (V::Set(s), false) => s.get_min().cloned(),
        (V::Set(s), true) => s.get_max().cloned(),
        (V::Map(m), false) => m.get_min().map(|(k, _)| k.clone()),
        (V::Map(m), true) => m.get_max().map(|(k, _)| k.clone()),
        (V::RangeSet(r), false) => r.spans.first().map(|s| Value::from_int(s.0.clone())),
        (V::RangeSet(r), true) => r.spans.last().map(|s| Value::from_int(s.1.clone())),
        _ => None,
    };
    found.unwrap_or(Value::NONE)
}

fn fold(it: &mut Interp<'_>, mut args: Args) -> Result<Value> {
    args.check("fold", 3, 3, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    // Taken rather than cloned, so the accumulator is unshared and the
    // function can update it in place (SPEC 4.1).
    let mut acc = std::mem::replace(&mut args.pos[1], Value::NONE);
    for x in items {
        acc = it.call(&args.pos[2], vec![acc, x], args.span)?;
    }
    Ok(acc)
}

fn scan(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("scan", 3, 3, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut acc = args.pos[1].clone();
    let mut out = Vec::new();
    out.push(acc.clone());
    for x in items {
        acc = it.call(&args.pos[2], vec![acc, x], args.span)?;
        out.push(acc.clone());
    }
    Ok(Value::list(out))
}

fn enumerate(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("enumerate", 1, 1, &[])?;
    if lazy(&args.pos[0]) {
        let src = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
        return Ok(generator(Generator::enumerate(src)));
    }
    let items = it.to_vec(&args.pos[0])?;
    Ok(list_of(
        items
            .into_iter()
            .enumerate()
            .map(|(i, x)| Value::pair(Value::int(i as i64), x))
            .collect(),
    ))
}

fn zip(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("zip", 2, usize::MAX, &[])?;
    let mut iters = Vec::new();
    for a in &args.pos {
        iters.push(it.iter(a).map_err(|e| e.at(args.span))?);
    }
    // Lazy when any argument is a generator, or none of them ends.
    if args.pos.iter().any(|a| matches!(a.view(), V::Gen(_))) || args.pos.iter().all(lazy) {
        return Ok(generator(Generator::zip(iters)));
    }
    let mut out = Vec::new();
    'outer: loop {
        let mut row = Vec::with_capacity(iters.len());
        for i in &mut iters {
            match i.next(it)? {
                Some(x) => row.push(x),
                None => break 'outer,
            }
        }
        out.push(Value::tuple(row));
    }
    Ok(Value::list(out))
}

fn pairs(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("pairs", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    Ok(list_of(
        items.windows(2).map(|w| Value::tuple(w.to_vec())).collect(),
    ))
}

fn windows(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("windows", 2, 2, &[])?;
    let n = usize_arg(&args, 1, "windows")?;
    if n == 0 {
        return Err(args.err("`windows` size must be positive"));
    }
    let items = it.to_vec(&args.pos[0])?;
    Ok(list_of(
        items.windows(n).map(|w| list_of(w.to_vec())).collect(),
    ))
}

fn chunks(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("chunks", 2, 2, &[])?;
    let n = usize_arg(&args, 1, "chunks")?;
    if n == 0 {
        return Err(args.err("`chunks` size must be positive"));
    }
    let items = it.to_vec(&args.pos[0])?;
    Ok(list_of(
        items.chunks(n).map(|w| list_of(w.to_vec())).collect(),
    ))
}

/// `a.subset(b)`: every element of set `a` is in set `b` (SPEC 9.2).
fn subset(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("subset", 2, 2, &[])?;
    match (args.pos[0].view(), args.pos[1].view()) {
        (V::Set(a), V::Set(b)) => Ok(Value::bool(a.is_subset(b))),
        (V::Set(_), other) | (other, _) => Err(type_err(&args, "subset", "two sets", other)),
    }
}

/// Each member of the collection given to `union` or `intersection` must be
/// a set or a rangeset, even when there is only one of them.
fn set_member(args: &Args, fname: &str, s: &Value) -> Result<()> {
    match s.view() {
        V::Set(_) | V::RangeSet(_) => Ok(()),
        other => Err(type_err(args, fname, "sets or rangesets", other)),
    }
}

/// `sets.union`: everything in any of the sets (SPEC 9.2). The union of no
/// sets is the empty set.
fn union(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("union", 1, 1, &[])?;
    let mut out: Option<Value> = None;
    for s in it.to_vec(&args.pos[0])? {
        set_member(&args, "union", &s)?;
        out = Some(match out {
            None => s,
            Some(acc) => binary(BinOp::BitOr, &acc, &s).map_err(|m| args.err(m))?,
        });
    }
    Ok(out.unwrap_or(Value::set(ValueSet::new())))
}

/// `sets.intersection`: what is in every one of the sets. There must be at
/// least one set.
fn intersection(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("intersection", 1, 1, &[])?;
    let mut out: Option<Value> = None;
    for s in it.to_vec(&args.pos[0])? {
        set_member(&args, "intersection", &s)?;
        out = Some(match out {
            None => s,
            Some(acc) => binary(BinOp::BitAnd, &acc, &s).map_err(|m| args.err(m))?,
        });
    }
    out.ok_or_else(|| args.err("`intersection` needs at least one set"))
}

/// `points.bounds`: for tuples, the (min, max) of each coordinate; for
/// ints, (min, max). `none` when there are no points.
fn bounds(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("bounds", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let Some(first) = items.first() else {
        return Ok(Value::NONE);
    };
    let extreme = |vals: &mut dyn Iterator<Item = Value>| -> Result<Value> {
        let mut lo: Option<Value> = None;
        let mut hi: Option<Value> = None;
        for v in vals {
            let smaller = match &lo {
                None => true,
                Some(l) => compare(&v, l).map_err(|m| args.err(m))? == Ordering::Less,
            };
            let larger = match &hi {
                None => true,
                Some(h) => compare(&v, h).map_err(|m| args.err(m))? == Ordering::Greater,
            };
            if smaller {
                lo = Some(v.clone());
            }
            if larger {
                hi = Some(v);
            }
        }
        Ok(Value::tuple(vec![
            lo.unwrap_or(Value::NONE),
            hi.unwrap_or(Value::NONE),
        ]))
    };
    match first.view() {
        V::Tuple(t) => {
            let dims = t.len();
            let mut out = Vec::with_capacity(dims);
            for d in 0..dims {
                let mut column = Vec::with_capacity(items.len());
                for p in &items {
                    match p.view() {
                        V::Tuple(q) if q.len() == dims => column.push(q[d].clone()),
                        _ => {
                            return Err(args.err(format!(
                                "`bounds` needs points of the same length, found {}",
                                short(p)
                            )));
                        }
                    }
                }
                out.push(extreme(&mut column.into_iter())?);
            }
            Ok(Value::tuple(out))
        }
        _ => extreme(&mut items.into_iter()),
    }
}

fn uniq(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("uniq", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut seen = FxHashSet::default();
    Ok(list_of(
        items
            .into_iter()
            .filter(|x| seen.insert(x.clone()))
            .collect(),
    ))
}

/// `s.repeat(n)` or `xs.repeat(n)`, for strings and lists only.
fn repeat(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("repeat", 2, 2, &[])?;
    let len = match args.pos[0].view() {
        V::Str(s) => s.len(),
        V::List(xs) => xs.len(),
        other => return Err(type_err(&args, "repeat", "a str or list", other)),
    };
    let n = match args.pos[1].view() {
        V::Int(n) if n.signum() < 0 => {
            return Err(args.err(format!("`repeat` needs a count of 0 or more, found {n}")));
        }
        // Any number of copies of nothing is nothing.
        V::Int(_) if len == 0 => return Ok(args.pos[0].clone()),
        V::Int(n) => n,
        other => return Err(type_err(&args, "repeat", "an int count", other)),
    };
    // The result is allocated once: it is often the largest value in a
    // program. One too big to allocate is an error, not a crash.
    let too_long = || args.err(format!("`repeat` result is too long: {n} copies"));
    let n = n
        .as_i64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(too_long)?;
    let total = len.checked_mul(n).ok_or_else(too_long)?;
    let item_bytes = match args.pos[0].view() {
        V::Str(_) => 1,
        _ => std::mem::size_of::<Value>(),
    };
    if (total as u64).saturating_mul(item_bytes as u64) > crate::MAX_VALUE_BYTES {
        return Err(too_long());
    }
    match args.pos[0].view() {
        V::Str(s) => {
            let mut out = String::new();
            out.try_reserve_exact(total).map_err(|_| too_long())?;
            for _ in 0..n {
                out.push_str(s);
            }
            Ok(Value::str(&out))
        }
        V::List(xs) => {
            let mut out = Vec::new();
            out.try_reserve_exact(total).map_err(|_| too_long())?;
            for _ in 0..n {
                out.extend(xs.iter().cloned());
            }
            Ok(Value::list(out))
        }
        other => Err(type_err(&args, "repeat", "a str or list", other)),
    }
}

fn flatten(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("flatten", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut out = Vec::new();
    for x in items {
        for y in it.to_vec(&x).map_err(|e| e.at(args.span))? {
            out.push(y);
        }
    }
    Ok(Value::list(out))
}

fn group_by(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("group_by", 2, 3, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut groups: OrdMap<Value, Vec<Value>> = OrdMap::new();
    for x in items {
        let k = it.call(&args.pos[1], vec![x.clone()], args.span)?;
        let v = match args.pos.get(2) {
            Some(f) => it.call(f, vec![x], args.span)?,
            None => x,
        };
        groups.entry(k).or_default().push(v);
    }
    let mut out = ValueMap::new();
    for (k, group) in groups {
        value::map_insert(&mut out, k, Value::list(group)).map_err(|m| args.err(m))?;
    }
    Ok(Value::map(out))
}

/// Running totals per key for `counts` and `tally`. Totals are kept in a
/// hash map and sorted into a map once at the end, which is much cheaper
/// than a lookup and an insert in the persistent map for every item.
struct Totals {
    sums: FxHashMap<Value, Value>,
    first: Option<Value>,
}

impl Totals {
    fn new() -> Totals {
        Totals {
            sums: FxHashMap::default(),
            first: None,
        }
    }

    /// Adds `n` to the total for `k`. Keys are checked against the first
    /// key, as a map checks them against its existing keys.
    fn add(&mut self, k: Value, n: &Value, args: &Args) -> Result<()> {
        value::check_key_compatible(self.first.as_ref(), &k).map_err(|m| args.err(m))?;
        if self.first.is_none() {
            self.first = Some(k.clone());
        }
        match self.sums.get_mut(&k) {
            Some(total) => {
                *total = binary(BinOp::Add, total, n).map_err(|m| args.err(m))?;
            }
            None => {
                self.sums.insert(k, n.clone());
            }
        }
        Ok(())
    }

    fn into_map(self) -> Value {
        let mut entries: Vec<(Value, Value)> = self.sums.into_iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        Value::map(entries.into_iter().collect())
    }
}

fn counts(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("counts", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut out = Totals::new();
    let one = Value::int(1);
    for x in items {
        out.add(x, &one, &args)?;
    }
    Ok(out.into_map())
}

fn tally(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("tally", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    let mut out = Totals::new();
    for x in items {
        match x.view() {
            V::Tuple(t) if t.len() == 2 => out.add(t[0].clone(), &t[1], &args)?,
            _ => {
                return Err(args.err(format!(
                    "`tally` expects (key, n) pairs, found {}",
                    short(&x)
                )));
            }
        }
    }
    Ok(out.into_map())
}

/// Index combinations of size `k` from `n`, in lexicographic order.
fn index_combos(n: usize, k: usize, mut f: impl FnMut(&[usize])) {
    if k > n {
        return;
    }
    let mut idx: Vec<usize> = (0..k).collect();
    loop {
        f(&idx);
        let Some(i) = (0..k).rev().find(|&i| idx[i] != i + n - k) else {
            return;
        };
        idx[i] += 1;
        for j in i + 1..k {
            idx[j] = idx[j - 1] + 1;
        }
    }
}

fn subsets(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("subsets", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    if items.len() > 24 {
        return Err(args.err(format!("`subsets` of {} elements is too many", items.len())));
    }
    let mut out = Vec::new();
    for k in 0..=items.len() {
        index_combos(items.len(), k, |idx| {
            out.push(list_of(idx.iter().map(|&i| items[i].clone()).collect()))
        });
    }
    Ok(Value::list(out))
}

fn combos(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("combos", 2, 2, &[])?;
    let k = usize_arg(&args, 1, "combos")?;
    let items = it.to_vec(&args.pos[0])?;
    let mut out = Vec::new();
    index_combos(items.len(), k, |idx| {
        out.push(Value::tuple(
            idx.iter().map(|&i| items[i].clone()).collect(),
        ))
    });
    Ok(Value::list(out))
}

fn perms(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("perms", 1, 1, &[])?;
    let items = it.to_vec(&args.pos[0])?;
    if items.len() > 10 {
        return Err(args.err(format!("`perms` of {} elements is too many", items.len())));
    }
    let mut out = Vec::new();
    let mut idx: Vec<usize> = (0..items.len()).collect();
    loop {
        out.push(Value::tuple(
            idx.iter().map(|&i| items[i].clone()).collect(),
        ));
        // Next permutation in lexicographic order.
        let Some(i) = (1..idx.len()).rev().find(|&i| idx[i - 1] < idx[i]) else {
            break;
        };
        let j = (i..idx.len())
            .rev()
            .find(|&j| idx[j] > idx[i - 1])
            .unwrap_or(i);
        idx.swap(i - 1, j);
        idx[i..].reverse();
    }
    Ok(Value::list(out))
}

fn transpose(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("transpose", 1, 1, &[])?;
    if let V::Grid(g) = args.pos[0].view() {
        return Ok(Value::grid_rc(std::rc::Rc::new(
            super::grids::transpose_grid(g),
        )));
    }
    let rows = it.to_vec(&args.pos[0])?;
    let strings = rows.iter().all(|r| matches!(r.view(), V::Str(_))) && !rows.is_empty();
    let rows: Vec<Vec<Value>> = rows.iter().map(|r| it.to_vec(r)).collect::<Result<_>>()?;
    let width = rows.first().map_or(0, Vec::len);
    if rows.iter().any(|r| r.len() != width) {
        return Err(args.err("`transpose` needs rows of equal length"));
    }
    let cols = (0..width).map(|c| rows.iter().map(move |r| r[c].clone()));
    Ok(if strings {
        list_of(
            cols.map(|col| Value::from_str_rc(col.map(|v| v.display()).collect::<String>().into()))
                .collect(),
        )
    } else {
        list_of(cols.map(|col| list_of(col.collect())).collect())
    })
}

fn list(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("list", 0, 1, &[])?;
    match args.pos.first() {
        None => Ok(Value::list(Vec::new())),
        Some(x) => Ok(list_of(it.to_vec(x)?)),
    }
}

fn tuple(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("tuple", 0, 1, &[])?;
    match args.pos.first() {
        None => Ok(Value::tuple(Vec::new())),
        Some(x) => Ok(Value::tuple(it.to_vec(x)?)),
    }
}

/// `set()`, `set(xs)`, and the update `m.set(k, v)` / `g.set(p, v)`.
fn set(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("set", 0, 3, &[])?;
    // Update the map, grid or list in place when nothing else holds it.
    if let [coll, _, _] = args.pos.as_slice()
        && let V::Map(_) | V::Grid(_) | V::List(_) = coll.view()
    {
        let span = args.span;
        let mut pos = args.pos.into_iter();
        let (Some(mut coll), Some(k), Some(v)) = (pos.next(), pos.next(), pos.next()) else {
            return Err(Error::new("internal: expected three arguments", span));
        };
        match coll.make_mut() {
            Some(Heap::Map(m)) => value::map_insert(m, k, v).map_err(|e| Error::new(e, span))?,
            Some(Heap::Grid(g)) => {
                super::grids::set_cell(g, &k, v).map_err(|e| Error::new(e, span))?
            }
            Some(Heap::List(items)) => {
                let Some(i) = k.as_int() else {
                    return Err(Error::new(
                        format!(
                            "`set` expects an int index into a list, found {}",
                            k.type_name()
                        ),
                        span,
                    ));
                };
                let len = items.len();
                let idx = i
                    .as_i64()
                    .and_then(|i| resolve_index(i, len))
                    .ok_or_else(|| {
                        Error::new(
                            format!("index {i} is out of range for a list of length {len}"),
                            span,
                        )
                    })?;
                items.set_mut(idx, v);
            }
            _ => return Err(Error::new("internal: expected a collection", span)),
        }
        return Ok(coll);
    }
    // A tuple or string: an updated copy.
    if let [t, i, _] = args.pos.as_slice()
        && let (V::Tuple(items), V::Int(i)) = (t.view(), i.view())
    {
        let len = items.len();
        let idx = i
            .as_i64()
            .and_then(|i| resolve_index(i, len))
            .ok_or_else(|| {
                args.err(format!(
                    "index {i} is out of range for a tuple of length {len}"
                ))
            })?;
        let mut out = items.to_vec();
        out[idx] = args.pos[2].clone();
        return Ok(Value::tuple(out));
    }
    if let [t, i, x] = args.pos.as_slice()
        && let (V::Str(text), V::Int(i)) = (t.view(), i.view())
    {
        let mut chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let idx = i
            .as_i64()
            .and_then(|i| resolve_index(i, len))
            .ok_or_else(|| {
                args.err(format!(
                    "index {i} is out of range for a str of length {len}"
                ))
            })?;
        let V::Char(c) = x.view() else {
            return Err(type_err(&args, "set", "a char to put in a str", x));
        };
        chars[idx] = c;
        return Ok(Value::str(&chars.into_iter().collect::<String>()));
    }
    match args.pos.as_slice() {
        [] => Ok(Value::set(ValueSet::new())),
        [x] => {
            let mut out = ValueSet::new();
            for v in it.to_vec(x)? {
                value::set_insert(&mut out, v).map_err(|m| args.err(m))?;
            }
            Ok(Value::set(out))
        }
        [other, _, _] => Err(type_err(
            &args,
            "set",
            "a map, list or grid to update",
            other,
        )),
        _ => Err(args.err("`set` takes 0, 1 or 3 arguments")),
    }
}

fn join(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("join", 1, 2, &[])?;
    let sep = match args.pos.get(1).map(Value::view) {
        None => String::new(),
        Some(V::Str(s)) => s.to_string(),
        Some(V::Char(c)) => c.to_string(),
        Some(other) => return Err(type_err(&args, "join", "a str separator", other)),
    };
    let items = it.to_vec(&args.pos[0])?;
    Ok(Value::from_str_rc(
        items
            .iter()
            .map(Value::display)
            .collect::<Vec<_>>()
            .join(&sep)
            .into(),
    ))
}

fn map_arg<'a>(args: &'a Args, fname: &str) -> Result<&'a ValueMap> {
    match args.pos[0].view() {
        V::Map(m) => Ok(m),
        other => Err(type_err(args, fname, "a map", other)),
    }
}

fn keys(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("keys", 1, 1, &[])?;
    Ok(Value::list(
        map_arg(&args, "keys")?
            .keys()
            .cloned()
            .collect::<crate::value::List>(),
    ))
}

fn values(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("values", 1, 1, &[])?;
    Ok(Value::list(
        map_arg(&args, "values")?
            .values()
            .cloned()
            .collect::<crate::value::List>(),
    ))
}

fn items(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("items", 1, 1, &[])?;
    Ok(Value::list(
        map_arg(&args, "items")?
            .iter()
            .map(|(k, v)| Value::pair(k.clone(), v.clone()))
            .collect::<crate::value::List>(),
    ))
}

fn add(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("add", 2, 2, &[])?;
    let span = args.span;
    let mut pos = args.pos.into_iter();
    let (Some(mut coll), Some(x)) = (pos.next(), pos.next()) else {
        return Err(Error::new("internal: expected two arguments", span));
    };
    // Updated in place when nothing else holds the set.
    match coll.make_mut() {
        Some(Heap::Set(s)) => value::set_insert(s, x).map_err(|m| Error::new(m, span))?,
        _ => {
            return Err(Error::new(
                format!("`add` expects a set, found {}", coll.type_name()),
                span,
            ));
        }
    }
    Ok(coll)
}

fn remove(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("remove", 2, 2, &[])?;
    let span = args.span;
    let mut pos = args.pos.into_iter();
    let (Some(mut coll), Some(x)) = (pos.next(), pos.next()) else {
        return Err(Error::new("internal: expected two arguments", span));
    };
    // Updated in place when nothing else holds the set or map.
    match coll.make_mut() {
        Some(Heap::Set(s)) => {
            s.remove(&x);
        }
        Some(Heap::Map(m)) => {
            m.remove(&x);
        }
        _ => {
            return Err(Error::new(
                format!("`remove` expects a set or map, found {}", coll.type_name()),
                span,
            ));
        }
    }
    Ok(coll)
}

/// `get(k)` / `get(k, default)`: like indexing, but missing gives a default.
fn get(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("get", 2, 3, &[])?;
    let default = args.pos.get(2).cloned().unwrap_or(Value::NONE);
    let (coll, k) = (&args.pos[0], &args.pos[1]);
    let found = match (coll.view(), k.view()) {
        (V::Map(m), _) => m.get(k).cloned(),
        (V::Grid(g), _) => super::grids::position(g, k).map(|i| g.cells[i].clone()),
        (V::List(_) | V::Tuple(_) | V::Str(_), V::Int(Int::Small(i))) => index_int(coll, i).ok(),
        (V::List(_) | V::Tuple(_) | V::Str(_), V::Int(_)) => None,
        (V::List(_) | V::Tuple(_) | V::Str(_), other) => {
            return Err(Error::new(
                format!(
                    "`get` on a {} needs an int index, found {}",
                    coll.type_name(),
                    other.type_name()
                ),
                args.span,
            ));
        }
        (other, _) => {
            return Err(type_err(
                &args,
                "get",
                "a map, list, tuple, str or grid",
                other,
            ));
        }
    };
    Ok(found.unwrap_or(default))
}
