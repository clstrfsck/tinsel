//! Liveness: marks each operand that is its variable's last use, and the
//! variables each block releases on entry.
//!
//! A variable is live at a point if some path from there reads it. An
//! operand is a last use when its variable isn't live after the instruction
//! (and isn't read again later in the same instruction). `Block::drops`
//! lists what a predecessor still holds that this block never reads, plus
//! this block's own parameters that nothing reads.
//!
//! The drops are per block, the union over its predecessors, so a block may
//! list a variable that one particular edge never set. The IR interpreter
//! doesn't mind (releasing an empty register does nothing), but code
//! generation will need them per edge.

use super::{Block, Function, Var};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Bits(Vec<u64>);

impl Bits {
    pub(crate) fn new(n: u32) -> Bits {
        Bits(vec![0; (n as usize).div_ceil(64)])
    }
    pub(crate) fn has(&self, v: Var) -> bool {
        self.0[(v / 64) as usize] & (1 << (v % 64)) != 0
    }
    pub(crate) fn set(&mut self, v: Var) {
        self.0[(v / 64) as usize] |= 1 << (v % 64);
    }
    pub(crate) fn clear(&mut self, v: Var) {
        self.0[(v / 64) as usize] &= !(1 << (v % 64));
    }
    pub(crate) fn union(&mut self, other: &Bits) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            let n = *a | b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = Var> + '_ {
        self.0.iter().enumerate().flat_map(|(i, w)| {
            (0..64)
                .filter(move |b| w & (1 << b) != 0)
                .map(move |b| (i * 64 + b) as Var)
        })
    }
}

/// The variables live at the start of `block` (after its parameters are
/// bound), given those live at its end. With `mark`, also sets the `last`
/// flags and drops unused results.
fn scan(block: &mut Block, live_out: &Bits, mark: bool) -> (Bits, Bits) {
    let mut live = live_out.clone();
    let mut term_uses: Vec<Var> = Vec::new();
    block.term.uses_mut(&mut |u| term_uses.push(u.var));
    // Terminator operands, last first.
    let mut flags = Vec::new();
    for &v in term_uses.iter().rev() {
        flags.push(!live.has(v));
        live.set(v);
    }
    if mark {
        let mut i = flags.len();
        block.term.uses_mut(&mut |u| {
            i -= 1;
            u.last = flags[i];
        });
    }
    let before_term = live.clone();
    for inst in block.insts.iter_mut().rev() {
        if let Some(d) = inst.dst {
            if mark && !live.has(d) {
                inst.dst = None;
            }
            live.clear(d);
        }
        let mut uses: Vec<Var> = Vec::new();
        inst.op.uses_mut(&mut |u| uses.push(u.var));
        let mut flags = Vec::new();
        for &v in uses.iter().rev() {
            flags.push(!live.has(v));
            live.set(v);
        }
        if mark {
            let mut i = flags.len();
            inst.op.uses_mut(&mut |u| {
                i -= 1;
                u.last = flags[i];
            });
        }
    }
    (live, before_term)
}

/// The variables live at the start of each block of `f`, not counting the
/// block's parameters. Reads only the operands, not their `last` flags.
pub(crate) fn live_in(f: &Function) -> Vec<Bits> {
    let n = f.blocks.len();
    let empty = Bits::new(f.nvars);
    let mut live_in = vec![empty.clone(); n];
    loop {
        let mut changed = false;
        for b in (0..n).rev() {
            let mut live = empty.clone();
            for s in f.blocks[b].term.successors() {
                live.union(&live_in[s as usize]);
            }
            let block = &f.blocks[b];
            block.term.uses(&mut |u| live.set(u.var));
            for inst in block.insts.iter().rev() {
                if let Some(d) = inst.dst {
                    live.clear(d);
                }
                inst.op.uses(&mut |u| live.set(u.var));
            }
            for &p in &block.params {
                live.clear(p);
            }
            if live != live_in[b] {
                live_in[b] = live;
                changed = true;
            }
        }
        if !changed {
            return live_in;
        }
    }
}

pub fn function(f: &mut Function) {
    let n = f.blocks.len();
    let empty = Bits::new(f.nvars);
    let live_in = live_in(f);
    let succs: Vec<Vec<usize>> = f
        .blocks
        .iter()
        .map(|b| b.term.successors().iter().map(|&s| s as usize).collect())
        .collect();
    let mut drops = vec![empty.clone(); n];
    for b in 0..n {
        let mut out = empty.clone();
        for &s in &succs[b] {
            out.union(&live_in[s]);
        }
        let (start, before_term) = scan(&mut f.blocks[b], &out, true);
        // Parameters nothing reads.
        for &p in &f.blocks[b].params {
            if !start.has(p) {
                drops[b].set(p);
            }
        }
        // What this block still holds at its end, less what the
        // terminator moves out.
        let mut held = before_term;
        f.blocks[b].term.uses_mut(&mut |u| {
            if u.last {
                held.clear(u.var);
            }
        });
        for &s in &succs[b] {
            let mut d = held.clone();
            for v in live_in[s].iter() {
                d.clear(v);
            }
            drops[s].union(&d);
        }
    }
    for (b, d) in drops.into_iter().enumerate() {
        f.blocks[b].drops = d.iter().collect();
    }
}
