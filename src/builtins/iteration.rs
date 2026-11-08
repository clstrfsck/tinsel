//! Iteration and cycles (SPEC 9.4).

use super::{Args, Builtin, fixed, func, type_err};
use crate::error::Result;
use crate::eval::{Generator, Interp};
use crate::fxhash::FxHashMap;
use crate::int::Int;
use crate::value::{V, Value};

pub const BUILTINS: &[Builtin] = &[
    fixed("iterate", 2, iterate),
    fixed("fixpoint", 2, fixpoint),
    func("find_cycle", find_cycle),
];

/// `iterate(x, f)` is a lazy generator. `iterate(x, f, times: n)` applies
/// `f` n times, skipping repeated cycles of states arithmetically.
fn iterate(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("iterate", 2, 2, &["times"])?;
    let (x, f) = (args.pos[0].clone(), args.pos[1].clone());
    let Some(times) = args.named("times") else {
        return Ok(Value::generator(Generator::iterate(x, f)));
    };
    // Any size of int: with a cycle, even 10 ** 30 steps take no time.
    let n = match times.view() {
        V::Int(n) if n.signum() >= 0 && !n.is_inf() => n.clone(),
        V::Int(n) => {
            return Err(args.err(format!(
                "`iterate` needs `times` to be 0 or more, found {n}"
            )));
        }
        other => return Err(type_err(&args, "iterate", "an int for `times`", other)),
    };
    let int_err = |m: String| args.err(m);
    let mut seen: FxHashMap<Value, usize> = FxHashMap::default();
    let mut states: Vec<Value> = Vec::new();
    let mut cur = x;
    let mut step = 0usize;
    while Int::Small(step as i64) < n {
        if let Some(&prev) = seen.get(&cur) {
            // States prev..step repeat forever: skip whole cycles.
            let period = Int::Small((step - prev) as i64);
            let left = n.sub(&Int::Small(step as i64)).map_err(int_err)?;
            let offset = left.rem(&period).map_err(int_err)?.as_i64().unwrap_or(0) as usize;
            return Ok(states[prev + offset].clone());
        }
        seen.insert(cur.clone(), step);
        states.push(cur.clone());
        cur = it.call(&f, vec![cur], args.span)?;
        step += 1;
    }
    Ok(cur)
}

fn fixpoint(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("fixpoint", 2, 2, &[])?;
    let mut cur = args.pos[0].clone();
    loop {
        let next = it.call(&args.pos[1], vec![cur.clone()], args.span)?;
        if next == cur {
            return Ok(cur);
        }
        cur = next;
    }
}

/// `xs.find_cycle(key: k)` returns `(start, period, prefix)`.
fn find_cycle(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("find_cycle", 1, 1, &["key"])?;
    let key = args.named("key").cloned();
    let mut iter = it.iter(&args.pos[0]).map_err(|e| e.at(args.span))?;
    let mut seen: FxHashMap<Value, usize> = FxHashMap::default();
    let mut items = Vec::new();
    while let Some(x) = iter.next(it)? {
        let k = match &key {
            Some(f) => it.call(f, vec![x.clone()], args.span)?,
            None => x.clone(),
        };
        let i = items.len();
        items.push(x);
        if let Some(&start) = seen.get(&k) {
            return Ok(Value::tuple(vec![
                Value::int(start as i64),
                Value::int((i - start) as i64),
                Value::list(items.into_iter().collect::<crate::value::List>()),
            ]));
        }
        seen.insert(k, i);
    }
    Err(args.err("`find_cycle` reached the end of the sequence without finding a repeat"))
}
