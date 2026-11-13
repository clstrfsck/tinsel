//! `where` constants as plain locals, where that is the same thing.
//!
//! Lowering gives each `where` block a record: its constants' cells, each
//! computed on first use by a thunk (a function capturing the record), and
//! copies of the values the thunks read. That is what laziness needs in
//! general, but it costs an allocation, a call through the runtime and a
//! check per use, on every call of the function.
//!
//! Often none of that is needed. When nothing but the function itself and
//! the cells' own thunks use the record, and each cell has a use that comes
//! before all its others on every path, the cell can be computed right
//! there, as an ordinary local, by inlining its thunk at that use. It is
//! then computed exactly where it was before (so a constant that isn't
//! reached still isn't computed, and an error happens at the same point,
//! with the same message), and every later use reads the local.
//!
//! A use inside another cell's thunk becomes a use where that thunk is
//! inlined, so cells are handled before the cells they use: by then, all
//! of a cell's uses are in the function. If any cell has no use that comes
//! first on every path, the record is left as it is. So are records that
//! anything else captures (a `where` function or a lambda), records whose
//! cells depend on each other in a cycle (an error the runtime reports),
//! and thunks with anything unusual in them.

use super::cse::dominators;
use super::{Block, Function, Inst, Layout, Op, Term, Var};
use std::collections::HashMap;

/// Turns qualifying `where` records into locals, in every function.
pub fn module(funcs: &mut [Function], layouts: &[Layout]) {
    for fi in 0..funcs.len() {
        let records: Vec<(Var, usize)> = funcs[fi]
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match (&i.op, i.dst) {
                (Op::NewRecord { layout, .. }, Some(r)) => Some((r, *layout)),
                _ => None,
            })
            .collect();
        let mut changed = false;
        for (r, layout) in records {
            let thunks: Vec<Function> = layouts[layout]
                .cells
                .iter()
                .map(|c| copy(&funcs[c.thunk as usize]))
                .collect();
            if let Some((blocks, nvars)) = unrecord(&funcs[fi], r, &thunks) {
                funcs[fi].blocks = blocks;
                funcs[fi].nvars = nvars;
                changed = true;
            }
        }
        if changed {
            let f = &mut funcs[fi];
            super::cse::function(f);
            super::cse::gather_items(f);
            super::simplify::function(f);
            super::live::function(f);
        }
    }
}

fn copy(f: &Function) -> Function {
    Function {
        name: f.name.clone(),
        kind: f.kind,
        params: f.params,
        caps: f.caps,
        memo: f.memo,
        memo_annotated: f.memo_annotated,
        def_id: f.def_id,
        span: f.span,
        blocks: f.blocks.clone(),
        nvars: f.nvars,
    }
}

/// What a thunk does with its record (capture 0): which cells it forces,
/// or `None` if it does anything else with it.
fn thunk_forces(t: &Function) -> Option<Vec<u32>> {
    if t.params != 0 || t.caps != 1 || !t.blocks.first()?.params.is_empty() {
        return None;
    }
    let mut rec: Vec<Var> = Vec::new();
    for i in t.blocks.iter().flat_map(|b| &b.insts) {
        match (&i.op, i.dst) {
            (Op::Cap(0), Some(d)) => rec.push(d),
            (Op::Cap(_), _) | (Op::SelfTail, _) => return None,
            _ => {}
        }
    }
    let mut forces = Vec::new();
    let mut ok = true;
    for b in &t.blocks {
        if matches!(b.term, Term::TailCall { .. }) {
            return None;
        }
        for i in &b.insts {
            match &i.op {
                Op::Force(u, k) if rec.contains(&u.var) => forces.push(*k),
                Op::RecVal(u, _) if rec.contains(&u.var) => {}
                op => op.uses(&mut |u| ok &= !rec.contains(&u.var)),
            }
        }
        b.term.uses(&mut |u| ok &= !rec.contains(&u.var));
    }
    ok.then_some(forces)
}

/// The cells in an order where each comes before the cells it forces, or
/// `None` if they force each other in a cycle.
fn users_first(deps: &[Vec<u32>]) -> Option<Vec<usize>> {
    let n = deps.len();
    let mut users = vec![0usize; n];
    for ds in deps {
        for &d in ds {
            users[d as usize] += 1;
        }
    }
    let mut ready: Vec<usize> = (0..n).filter(|&k| users[k] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(k) = ready.pop() {
        order.push(k);
        for &d in &deps[k] {
            users[d as usize] -= 1;
            if users[d as usize] == 0 {
                ready.push(d as usize);
            }
        }
    }
    (order.len() == n).then_some(order)
}

/// Whether instruction `a` (block, index) runs before `b` on every path.
fn dominates(idom: &[Option<usize>], a: (usize, usize), b: (usize, usize)) -> bool {
    if a.0 == b.0 {
        return a.1 <= b.1;
    }
    let mut x = b.0;
    loop {
        match idom[x] {
            None => return false,
            Some(p) if p == x => return false,
            Some(p) if p == a.0 => return true,
            Some(p) => x = p,
        }
    }
}

/// Whether block `site` can run again, for the same record, without
/// passing through block `made` (where the record is made, which would
/// make a new one): whether it is on a cycle that avoids `made`. A
/// constant first used there, in a loop such as a comprehension's, is
/// computed once by its record but would be computed on every pass as a
/// local.
fn repeats(blocks: &[Block], site: usize, made: usize) -> bool {
    if site == made {
        return false;
    }
    let mut seen = vec![false; blocks.len()];
    let mut work: Vec<usize> = blocks[site]
        .term
        .successors()
        .iter()
        .map(|&b| b as usize)
        .collect();
    while let Some(b) = work.pop() {
        if b == site {
            return true;
        }
        if b == made || seen[b] {
            continue;
        }
        seen[b] = true;
        work.extend(blocks[b].term.successors().iter().map(|&s| s as usize));
    }
    false
}

/// `f`'s blocks and variable count with record `r` turned into locals, or
/// `None` if it doesn't qualify.
fn unrecord(f: &Function, r: Var, thunks: &[Function]) -> Option<(Vec<Block>, u32)> {
    // The record's copied values; nothing in `f` may use it but its cells.
    let mut vals: Vec<Var> = Vec::new();
    let mut ok = true;
    for b in &f.blocks {
        for i in &b.insts {
            match &i.op {
                Op::NewRecord { vals: vs, .. } if i.dst == Some(r) => {
                    vals = vs.iter().map(|u| u.var).collect();
                }
                Op::Force(u, _) | Op::RecVal(u, _) if u.var == r => {}
                op => op.uses(&mut |u| ok &= u.var != r),
            }
        }
        b.term.uses(&mut |u| ok &= u.var != r);
    }
    if !ok {
        return None;
    }
    let deps: Vec<Vec<u32>> = thunks.iter().map(thunk_forces).collect::<Option<_>>()?;
    let order = users_first(&deps)?;

    let mut blocks = f.blocks.clone();
    let mut nvars = f.nvars;
    let mut rename: HashMap<Var, Var> = HashMap::new();
    for k in order {
        let sites: Vec<(usize, usize)> = blocks
            .iter()
            .enumerate()
            .flat_map(|(bi, b)| {
                b.insts
                    .iter()
                    .enumerate()
                    .filter_map(move |(ii, i)| match &i.op {
                        Op::Force(u, c) if u.var == r && *c as usize == k => Some((bi, ii)),
                        _ => None,
                    })
            })
            .collect();
        if sites.is_empty() {
            continue; // Never used: never computed.
        }
        let idom = dominators(&blocks);
        let &site = sites
            .iter()
            .find(|&&a| sites.iter().all(|&b| dominates(&idom, a, b)))?;
        let made = blocks.iter().position(|b| {
            b.insts
                .iter()
                .any(|i| i.dst == Some(r) && matches!(i.op, Op::NewRecord { .. }))
        })?;
        if repeats(&blocks, site.0, made) {
            return None;
        }
        // The local: the first use's result, or a new variable if only the
        // later uses read it.
        let local = match blocks[site.0].insts[site.1].dst {
            Some(v) => v,
            None => {
                nvars += 1;
                nvars - 1
            }
        };
        // The later uses read the local. They come after the first use, so
        // removing them doesn't move it.
        let mut later: Vec<(usize, usize)> = sites.into_iter().filter(|&s| s != site).collect();
        later.sort_by(|a, b| b.cmp(a));
        for (bi, ii) in later {
            if let Some(d) = blocks[bi].insts.remove(ii).dst {
                rename.insert(d, local);
            }
        }
        inline(&mut blocks, &mut nvars, site, local, r, &thunks[k]);
    }

    // The copied values are the variables themselves; then the record goes.
    for b in &mut blocks {
        b.insts.retain(|i| match (&i.op, i.dst) {
            (Op::RecVal(u, n), d) if u.var == r => {
                if let (Some(d), Some(&v)) = (d, vals.get(*n as usize)) {
                    rename.insert(d, v);
                }
                false
            }
            (Op::NewRecord { .. }, Some(d)) => d != r,
            _ => true,
        });
    }
    let resolve = |mut v: Var| {
        while let Some(&w) = rename.get(&v) {
            v = w;
        }
        v
    };
    let mut leftover = false;
    for b in &mut blocks {
        for i in &mut b.insts {
            i.op.uses_mut(&mut |u| {
                u.var = resolve(u.var);
                u.last = false;
                leftover |= u.var == r;
            });
        }
        b.term.uses_mut(&mut |u| {
            u.var = resolve(u.var);
            u.last = false;
            leftover |= u.var == r;
        });
        b.drops.clear();
    }
    // Every use of the record should be gone; if not, keep it as it was.
    (!leftover).then_some((blocks, nvars))
}

/// Replaces the force at `site` with the thunk's body, its result in
/// `local`. The rest of the block moves to a new block that the body's
/// returns jump to. The thunk's record reads become reads of `r`, handled
/// by the caller.
fn inline(
    blocks: &mut Vec<Block>,
    nvars: &mut u32,
    site: (usize, usize),
    local: Var,
    r: Var,
    thunk: &Function,
) {
    let base = *nvars;
    *nvars += thunk.nvars;
    let cont = blocks.len();
    let first = cont + 1;
    let rec: Vec<Var> = thunk
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|i| matches!(i.op, Op::Cap(0)).then_some(i.dst).flatten())
        .collect();
    let var = |v: Var| if rec.contains(&v) { r } else { base + v };
    let block = |b: super::BlockId| (first as u32) + b;

    // The block up to the force jumps into the body; the rest of it, after
    // the force, becomes the block the body returns to.
    let b = &mut blocks[site.0];
    let mut rest = b.insts.split_off(site.1);
    rest.remove(0); // The force itself.
    let term = std::mem::replace(&mut b.term, Term::Jump(block(0), Vec::new()));
    blocks.push(Block {
        params: vec![local],
        insts: rest,
        term,
        drops: Vec::new(),
    });
    for tb in &thunk.blocks {
        let insts: Vec<Inst> = tb
            .insts
            .iter()
            .filter(|i| !matches!(i.op, Op::Cap(0)))
            .map(|i| {
                let mut op = i.op.clone();
                op.uses_mut(&mut |u| {
                    u.var = var(u.var);
                    u.last = false;
                });
                Inst {
                    dst: i.dst.map(var),
                    op,
                    span: i.span,
                }
            })
            .collect();
        let mut term = match &tb.term {
            Term::Return(u) => Term::Jump(cont as u32, vec![*u]),
            Term::Jump(b, args) => Term::Jump(block(*b), args.clone()),
            Term::If { cond, then, els } => Term::If {
                cond: *cond,
                then: block(*then),
                els: block(*els),
            },
            Term::Next {
                iter,
                some,
                none,
                span,
            } => Term::Next {
                iter: *iter,
                some: block(*some),
                none: block(*none),
                span: *span,
            },
            other => other.clone(),
        };
        term.uses_mut(&mut |u| {
            u.var = var(u.var);
            u.last = false;
        });
        blocks.push(Block {
            params: tb.params.iter().map(|&p| var(p)).collect(),
            insts,
            term,
            drops: Vec::new(),
        });
    }
}
