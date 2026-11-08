//! Intervals (SPEC 9.8).

use super::{Args, Builtin, func, type_err};
use crate::error::Result;
use crate::eval::{Interp, short};
use crate::value::{Range, RangeSet, V, Value};
use std::rc::Rc;

pub const BUILTINS: &[Builtin] = &[func("rangeset", rangeset), func("ranges", ranges)];

/// `rangeset(xs)` from finite ranges and ints, or `rangeset()`.
fn rangeset(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("rangeset", 0, 1, &[])?;
    let mut spans = Vec::new();
    if let Some(xs) = args.pos.first() {
        for x in it.to_vec(xs)? {
            match x.view() {
                V::Int(i) if !i.is_inf() => spans.push((i.clone(), i.clone())),
                V::Range(r) => match &r.end {
                    Some(end) if !end.is_inf() && !r.start.is_inf() => {
                        spans.push((r.start.clone(), end.clone()))
                    }
                    _ => {
                        return Err(args.err(format!(
                            "`rangeset` needs finite ranges, found {}",
                            short(&x)
                        )));
                    }
                },
                other => {
                    return Err(args.err(format!(
                        "`rangeset` expects ranges and ints, found {}",
                        other.type_name()
                    )));
                }
            }
        }
    }
    Ok(Value::rangeset_rc(Rc::new(RangeSet::from_spans(spans))))
}

fn ranges(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("ranges", 1, 1, &[])?;
    let V::RangeSet(r) = args.pos[0].view() else {
        return Err(type_err(&args, "ranges", "a rangeset", &args.pos[0]));
    };
    let out = r.spans.iter().map(|(lo, hi)| {
        Value::range_rc(Rc::new(Range {
            start: lo.clone(),
            end: Some(hi.clone()),
            exclusive: false,
        }))
    });
    Ok(Value::list(out.collect::<Vec<_>>()))
}
