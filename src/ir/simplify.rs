//! Three clean-ups after common-subexpression elimination (`cse`), before
//! liveness (`live`):
//!
//! - **`x in xs.indices`.** A call to `indices` whose only use is an `in`
//!   or `not in` right after it becomes one [`Op::InIndices`], which for a
//!   list or tuple is a bounds check rather than a range built and then
//!   searched (as in an interpreter's `pc not in program.indices`).
//! - **Tuples only taken apart.** A tuple built only to be matched, as in
//!   `match (a, b)`, whose every use is a size test or an item, isn't
//!   built: each test is known, and each item is the value it was built
//!   from.
//! - **Unused results.** An instruction that can't fail and whose result
//!   nothing reads is dropped, such as the item a `_` in a pattern takes.
//!
//! None changes a result or an error: only instructions that can't fail
//! are removed, the fused `in` fails only as the call would (with its
//! span), and nothing else moves.

use super::{Function, Op, Use, Value, Var};
use crate::ast::CmpOp;

pub fn function(f: &mut Function) {
    in_indices(f);
    tuples(f);
    unused(f);
}

/// Whether `op` can't fail. `Item`, `ItemBack` and `Middle` are only made
/// after a test that the value has the item.
fn infallible(op: &Op) -> bool {
    matches!(
        op,
        Op::Const(_)
            | Op::Cap(_)
            | Op::RecParent(_)
            | Op::RecVal(..)
            | Op::IsType(..)
            | Op::EqConst(..)
            | Op::IsTuple(..)
            | Op::IsList { .. }
            | Op::Item(..)
            | Op::ItemBack(..)
            | Op::Middle { .. }
            | Op::Tuple(_)
            | Op::List(_)
    )
}

/// `v = indices(xs)` followed by `x in v` (or `not in`), with no other
/// use of `v`, becomes `x in xs.indices`. The two are adjacent, so no
/// other error can come between them; `in` against a range can't fail.
fn in_indices(f: &mut Function) {
    let Some(id) = crate::builtins::lookup("indices") else {
        return;
    };
    let mut count = vec![0u32; f.nvars as usize];
    for block in &f.blocks {
        for inst in &block.insts {
            inst.op.uses(&mut |u| count[u.var as usize] += 1);
        }
        block.term.uses(&mut |u| count[u.var as usize] += 1);
    }
    for block in &mut f.blocks {
        let mut i = 0;
        while i + 1 < block.insts.len() {
            let fused = match (&block.insts[i], &block.insts[i + 1].op) {
                (call, Op::Cmp(op @ (CmpOp::In | CmpOp::NotIn), x, r))
                    if call.dst == Some(r.var) && x.var != r.var && count[r.var as usize] == 1 =>
                {
                    match &call.op {
                        Op::CallBuiltin { id: b, args, named }
                            if *b == id && args.len() == 1 && named.is_empty() =>
                        {
                            Some(Op::InIndices(*x, args[0], *op == CmpOp::NotIn))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(op) = fused {
                let call = block.insts.remove(i);
                block.insts[i].op = op;
                block.insts[i].span = call.span;
            }
            i += 1;
        }
    }
}

/// Tuples only tested and taken apart are never built.
fn tuples(f: &mut Function) {
    let n = f.nvars as usize;
    // The operands of each variable made by `Op::Tuple`.
    let mut parts: Vec<Option<Vec<Var>>> = vec![None; n];
    for block in &f.blocks {
        for inst in &block.insts {
            if let (Some(d), Op::Tuple(xs)) = (inst.dst, &inst.op) {
                parts[d as usize] = Some(xs.iter().map(|u| u.var).collect());
            }
        }
    }
    if parts.iter().all(Option::is_none) {
        return;
    }
    // A tuple qualifies if it is only tested for its size and taken items
    // from. An item past its length is guarded by a size test that is now
    // known to fail, as in an arm for a longer tuple, so it never runs.
    let mut ok: Vec<bool> = parts.iter().map(Option::is_some).collect();
    let escape = |v: Var, ok: &mut Vec<bool>| ok[v as usize] = false;
    for block in &f.blocks {
        for inst in &block.insts {
            match &inst.op {
                Op::IsTuple(_, _) | Op::Item(_, _) => {}
                op => op.uses(&mut |u| escape(u.var, &mut ok)),
            }
        }
        block.term.uses(&mut |u| escape(u.var, &mut ok));
    }
    if !ok.iter().any(|&b| b) {
        return;
    }
    // Rewrite: tests become constants, items the values themselves.
    let mut subst: Vec<Option<Var>> = vec![None; n];
    for block in &mut f.blocks {
        for inst in &mut block.insts {
            match &inst.op {
                Op::IsTuple(u, k) if ok[u.var as usize] => {
                    let len = parts[u.var as usize].as_ref().map_or(0, Vec::len);
                    inst.op = Op::Const(Value::bool(len == *k));
                }
                Op::Item(u, i) if ok[u.var as usize] => {
                    match parts[u.var as usize].as_ref().and_then(|p| p.get(*i)) {
                        Some(&x) => {
                            if let Some(d) = inst.dst {
                                subst[d as usize] = Some(x);
                            }
                        }
                        // Never reached; a constant keeps its result
                        // defined until `unused` drops it.
                        None => inst.op = Op::Const(Value::NONE),
                    }
                }
                _ => {}
            }
        }
    }
    let resolve = |mut v: Var| {
        while let Some(w) = subst[v as usize] {
            v = w;
        }
        v
    };
    for block in &mut f.blocks {
        block.insts.retain(|inst| match (&inst.op, inst.dst) {
            (Op::Item(u, _), _) if ok[u.var as usize] => false,
            (Op::Tuple(_), Some(d)) if ok[d as usize] => false,
            _ => true,
        });
        for inst in &mut block.insts {
            inst.op.uses_mut(&mut |u: &mut Use| u.var = resolve(u.var));
        }
        block
            .term
            .uses_mut(&mut |u: &mut Use| u.var = resolve(u.var));
    }
}

/// Drops instructions that can't fail and whose results nothing reads,
/// until there are none.
fn unused(f: &mut Function) {
    let mut count = vec![0u32; f.nvars as usize];
    for block in &f.blocks {
        for inst in &block.insts {
            inst.op.uses(&mut |u| count[u.var as usize] += 1);
        }
        block.term.uses(&mut |u| count[u.var as usize] += 1);
    }
    loop {
        let mut changed = false;
        for block in &mut f.blocks {
            block.insts.retain(|inst| {
                let dead = infallible(&inst.op) && inst.dst.is_none_or(|d| count[d as usize] == 0);
                if dead {
                    inst.op.uses(&mut |u| count[u.var as usize] -= 1);
                    changed = true;
                }
                !dead
            });
        }
        if !changed {
            return;
        }
    }
}
