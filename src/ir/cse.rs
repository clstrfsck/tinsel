//! Common-subexpression elimination: an instruction that repeats one
//! already computed, on the same operands, on every path to it, is
//! dropped, and its uses read the earlier result.
//!
//! Tinsel is pure, so this never changes a result. Only instructions whose
//! value depends on nothing but their operands take part ([`pure`]):
//! calls don't (a call may return a generator, and each call gives a new,
//! single-pass one), closures don't (they compare by identity), and neither
//! do iterators and accumulators (they change). An earlier instruction is
//! reused only where it *dominates* the repeat, so it has certainly run,
//! and run without an error, by then: errors happen exactly where they did.
//!
//! It runs before liveness (`live`), which then finds the last uses of the
//! surviving variables.

use super::{Block, Function, Op, Var};
use std::collections::HashMap;

/// Whether `op` can be reused: its value depends only on its operands.
fn pure(op: &Op) -> bool {
    matches!(
        op,
        Op::Cap(_)
            | Op::Global(_)
            | Op::RecParent(_)
            | Op::RecVal(..)
            | Op::Force(..)
            | Op::Bin(..)
            | Op::Cmp(..)
            | Op::Neg(_)
            | Op::Not(_)
            | Op::BitNot(_)
            | Op::CheckBool(..)
            | Op::CheckPred(..)
            | Op::CheckElem(..)
            | Op::Tuple(_)
            | Op::List(_)
            | Op::Set(_)
            | Op::Map(_)
            | Op::Format(_)
            | Op::Index(..)
            | Op::InIndices(..)
            | Op::IndexInt(..)
            | Op::Slice { .. }
            | Op::Range { .. }
            | Op::IsType(..)
            | Op::EqConst(..)
            | Op::IsTuple(..)
            | Op::IsList { .. }
            | Op::Item(..)
            | Op::ItemBack(..)
            | Op::Middle { .. }
            | Op::Loopable { .. }
    )
}

/// The immediate dominator of each block (block 0 for itself; `None` for
/// blocks that can't be reached), by the iterative algorithm of Cooper,
/// Harvey and Kennedy.
pub(crate) fn dominators(blocks: &[Block]) -> Vec<Option<usize>> {
    let n = blocks.len();
    // Reverse postorder from block 0.
    let mut order = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
    seen[0] = true;
    while let Some((b, i)) = stack.pop() {
        let succs = blocks[b].term.successors();
        if let Some(&s) = succs.get(i) {
            stack.push((b, i + 1));
            let s = s as usize;
            if !seen[s] {
                seen[s] = true;
                stack.push((s, 0));
            }
        } else {
            order.push(b);
        }
    }
    order.reverse();
    let mut rank = vec![usize::MAX; n];
    for (i, &b) in order.iter().enumerate() {
        rank[b] = i;
    }
    let mut preds = vec![Vec::new(); n];
    for (b, block) in blocks.iter().enumerate() {
        for s in block.term.successors() {
            preds[s as usize].push(b);
        }
    }
    let mut idom: Vec<Option<usize>> = vec![None; n];
    idom[0] = Some(0);
    let intersect = |idom: &[Option<usize>], mut a: usize, mut b: usize| {
        while a != b {
            while rank[a] > rank[b] {
                a = idom[a].unwrap_or(0);
            }
            while rank[b] > rank[a] {
                b = idom[b].unwrap_or(0);
            }
        }
        a
    };
    loop {
        let mut changed = false;
        for &b in order.iter().skip(1) {
            let mut new: Option<usize> = None;
            for &p in &preds[b] {
                if idom[p].is_none() {
                    continue;
                }
                new = Some(match new {
                    None => p,
                    Some(q) => intersect(&idom, p, q),
                });
            }
            if new.is_some() && idom[b] != new {
                idom[b] = new;
                changed = true;
            }
        }
        if !changed {
            return idom;
        }
    }
}

/// Follows `subst` to the variable that stands for `v`.
fn resolve(subst: &[Option<Var>], mut v: Var) -> Var {
    while let Some(w) = subst[v as usize] {
        v = w;
    }
    v
}

pub fn function(f: &mut Function) {
    let n = f.blocks.len();
    let idom = dominators(&f.blocks);
    let mut children = vec![Vec::new(); n];
    for (b, d) in idom.iter().enumerate().skip(1) {
        if let Some(d) = *d {
            children[d].push(b);
        }
    }
    let mut subst: Vec<Option<Var>> = vec![None; f.nvars as usize];
    let mut table: HashMap<String, Var> = HashMap::new();
    // Each entry: the table entries a block added, undone when the walk
    // leaves its subtree of the dominator tree.
    enum Step {
        Enter(usize),
        Leave(Vec<(String, Option<Var>)>),
    }
    let mut stack = vec![Step::Enter(0)];
    while let Some(step) = stack.pop() {
        let b = match step {
            Step::Leave(undo) => {
                for (k, old) in undo.into_iter().rev() {
                    match old {
                        Some(v) => table.insert(k, v),
                        None => table.remove(&k),
                    };
                }
                continue;
            }
            Step::Enter(b) => b,
        };
        let mut undo = Vec::new();
        for inst in &mut f.blocks[b].insts {
            inst.op.uses_mut(&mut |u| u.var = resolve(&subst, u.var));
            let Some(d) = inst.dst else { continue };
            if !pure(&inst.op) {
                continue;
            }
            // The printed form names the operands and every immediate;
            // the variant tells apart forms that print alike (`{}`).
            let key = format!("{:?} {}", std::mem::discriminant(&inst.op), inst.op);
            match table.get(&key) {
                Some(&e) => subst[d as usize] = Some(e),
                None => {
                    undo.push((key.clone(), None));
                    table.insert(key, d);
                }
            }
        }
        f.blocks[b]
            .term
            .uses_mut(&mut |u| u.var = resolve(&subst, u.var));
        stack.push(Step::Leave(undo));
        for &c in children[b].iter().rev() {
            stack.push(Step::Enter(c));
        }
    }
    // Drop the repeats; their results were renamed above.
    for block in &mut f.blocks {
        block
            .insts
            .retain(|i| i.dst.is_none_or(|d| subst[d as usize].is_none()));
    }
}

/// Moves the items taken from one `where` pattern's value up to the first
/// of them, in each block, so the value (a tuple of the pattern's names) is
/// let go at once rather than at the last name's first use.
///
/// Otherwise the tuple keeps its items shared in between: in
/// `store(m, …) = (mem.set(…), ip + n, rb) where (mem, ip, rb) = m`, it
/// held `mem` through the `set`, so the map was copied instead of updated
/// in place. Such items can't fail (the value is always that tuple), and
/// nothing else moves, so this changes no result and no error.
pub fn gather_items(f: &mut Function) {
    let forced: Vec<bool> = {
        let mut v = vec![false; f.nvars as usize];
        for block in &f.blocks {
            for inst in &block.insts {
                if let (Some(d), Op::Force(..)) = (inst.dst, &inst.op) {
                    v[d as usize] = true;
                }
            }
        }
        v
    };
    for block in &mut f.blocks {
        let mut insts: Vec<Option<super::Inst>> = std::mem::take(&mut block.insts)
            .into_iter()
            .map(Some)
            .collect();
        let mut out = Vec::with_capacity(insts.len());
        for i in 0..insts.len() {
            let Some(inst) = insts[i].take() else {
                continue;
            };
            let src = match &inst.op {
                Op::Item(u, _) if forced[u.var as usize] => Some(u.var),
                _ => None,
            };
            out.push(inst);
            let Some(x) = src else { continue };
            for later in &mut insts[i + 1..] {
                if let Some(Op::Item(u, _)) = later.as_ref().map(|l| &l.op)
                    && u.var == x
                    && let Some(l) = later.take()
                {
                    out.push(l);
                }
            }
        }
        block.insts = out;
    }
}
