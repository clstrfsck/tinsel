//! An interpreter for the IR, used to check the lowering against the test
//! corpus (`TIN_ENGINE=ir`, or `tinsel run --ir`), and the runtime that compiled code shares
//! with it (`crate::native`).
//!
//! It runs inside the tree-walker's [`Interp`], so the library functions
//! work unchanged: an IR closure is a [`Func::Ir`] value, and when a library
//! function calls one, `Interp::call` comes back here. Each call gets a
//! register per IR variable; a last use moves the value out of its
//! register, so a value held once can be updated in place, as in the
//! evaluator.
//!
//! Every register holds a [`Value`]. `where` records, iterators and
//! accumulators are [`Internal`] values, which the lowering never lets
//! reach Tinsel code. What each instruction does is [`exec`], which
//! compiled code calls too, so the two engines can't disagree about it.
//!
//! When the program has been compiled (`Runtime::native`), [`run`] hands
//! each call to the compiled code instead of interpreting it.
//!
//! A `where` record whose cell holds a closure capturing that same record
//! is a reference cycle; [`release_cycle`] frees it once it's unreachable.

use super::*;
use crate::builtins::{Args, BUILTINS};
use crate::error::{Error, Result};
use crate::eval::{
    Generator, Interp, Memo, binary_owned, contains_generator, match_segs, negate, short,
    small_int_op, spread,
};
use crate::int::Int;
use crate::value::{self, Func, Heap, Internal, Range, V};
use crate::value::{ValueMap, ValueSet};
use std::cell::RefCell;

const STACK_RED_ZONE: usize = 256 * 1024;
const STACK_GROW: usize = 16 * 1024 * 1024;

pub struct Closure {
    pub func: FuncId,
    pub name: Name,
    /// The captured values; a `where` function's capture 0 is its record.
    pub caps: Vec<Value>,
    /// Whether a reference cycle can run through this closure: it captures
    /// a record and could be stored in one's cell (so not a constant's
    /// thunk). See [`release_cycle`].
    pub watch: bool,
}

/// A `where` record (see the module docs of `ir`).
#[derive(Clone)]
pub struct Record {
    parent: Option<Value>,
    layout: usize,
    vals: Vec<Value>,
    cells: Vec<RefCell<State>>,
    memos: Vec<Rc<Memo>>,
    /// Set once a cell holds a closure capturing this record: the only way
    /// it can be part of a cycle that [`release_cycle`] frees.
    cyclic: std::cell::Cell<bool>,
}

#[derive(Clone)]
enum State {
    Pending,
    Evaluating,
    Done(Value),
}

/// A comprehension's collection while it is being built.
#[derive(Clone)]
pub enum Acc {
    List(Vec<Value>),
    Set(ValueSet),
    Map(ValueMap),
}

/// The IR program's state for one run.
pub struct Runtime {
    pub(crate) module: Rc<Module>,
    globals: Vec<GlobalDef>,
    cells: Vec<(Cell, State)>,
    memos: Vec<Rc<Memo>>,
    /// Closures of functions without captures, made once, as values.
    plain: Vec<Option<Value>>,
    /// Constants being computed, for cycle reports.
    const_stack: Vec<Rc<str>>,
    /// The compiled program, when running natively.
    pub(crate) native: Option<Rc<crate::native::Native>>,
}

/// Switches `it` to running `module`, for the `example` block `example`
/// (with its `with` overrides) or the real input. With `native`, calls run
/// the compiled code.
pub fn install(
    it: &mut Interp<'_>,
    module: Rc<Module>,
    native: Option<Rc<crate::native::Native>>,
    example: Option<usize>,
) {
    let mut globals = module.globals.clone();
    let mut cells: Vec<(Cell, State)> = module
        .consts
        .iter()
        .map(|c| (c.clone(), State::Pending))
        .collect();
    if let Some(withs) = example.and_then(|e| module.withs.get(e)) {
        for (g, cell) in withs {
            cells.push((cell.clone(), State::Pending));
            globals[*g] = GlobalDef::Const {
                cell: cells.len() - 1,
                index: None,
            };
        }
    }
    it.ir = Some(Box::new(Runtime {
        globals,
        cells,
        memos: (0..module.memos)
            .map(|_| Rc::new(Memo::default()))
            .collect(),
        plain: vec![None; module.funcs.len()],
        const_stack: Vec::new(),
        native,
        module,
    }));
}

fn rt<'a>(it: &'a mut Interp<'_>) -> &'a mut Runtime {
    it.ir.as_deref_mut().expect("the IR runtime is installed")
}

pub(crate) fn internal(msg: &str, span: Span) -> Error {
    Error::new(format!("internal: {msg}"), span)
}

/// The record a value holds.
fn record(v: &Value) -> Option<&Record> {
    match v.as_internal() {
        Some(Internal::Rec(r)) => Some(r),
        _ => None,
    }
}

/// The value of global `i`.
pub fn global(it: &mut Interp<'_>, i: usize, span: Span) -> Result<Value> {
    let m = rt(it).module.clone();
    match rt(it).globals[i] {
        GlobalDef::Func(f) => Ok(plain_value(it, &m, f)),
        GlobalDef::Const { cell, index } => {
            let v = force_global(it, &m, cell, span)?;
            match index {
                Some(k) => item(&v, k, span),
                None => Ok(v),
            }
        }
    }
}

/// Runs the check whose span starts at `start`.
pub fn run_check(it: &mut Interp<'_>, start: usize, span: Span) -> Result<Value> {
    let m = rt(it).module.clone();
    let f = *m
        .checks
        .get(&start)
        .ok_or_else(|| internal("unknown check", span))?;
    let clo = plain(it, &m, f);
    run(it, &m, clo, Vec::new(), span)
}

/// Called by `Interp::call` for a [`Func::Ir`].
pub fn call_closure(
    it: &mut Interp<'_>,
    f: &Rc<Func>,
    mut args: Vec<Value>,
    span: Span,
) -> Result<Value> {
    let m = rt(it).module.clone();
    let Func::Ir(c) = &**f else {
        return Err(internal("not an IR closure", span));
    };
    check_args(&m, c, &mut args, span)?;
    run(it, &m, f.clone(), args, span)
}

/// Calls the function value `f`, an IR closure (from `Interp::call`).
pub fn call_value(
    it: &mut Interp<'_>,
    f: Value,
    mut args: Vec<Value>,
    span: Span,
) -> Result<Value> {
    let m = rt(it).module.clone();
    let V::Func(rc) = f.view() else {
        return Err(internal("not a function", span));
    };
    let Func::Ir(c) = &**rc else {
        return Err(internal("not an IR closure", span));
    };
    check_args(&m, c, &mut args, span)?;
    if let Some(n) = rt(it).native.clone() {
        return crate::native::run(it, &n, f, args, span);
    }
    let rc = rc.clone();
    drop(f);
    run(it, &m, rc, args, span)
}

/// Tuple spreading and the argument count, for a call through a value.
pub(crate) fn check_args(m: &Module, c: &Closure, args: &mut Vec<Value>, span: Span) -> Result<()> {
    let f = &m.funcs[c.func as usize];
    let n = f.params as usize;
    spread(args, n);
    if args.len() == n {
        return Ok(());
    }
    let msg = match f.kind {
        FnKind::Lambda => format!("lambda takes {n} arguments, but was given {}", args.len()),
        _ => format!(
            "`{}` takes {n} arguments, but was given {}",
            f.name,
            args.len()
        ),
    };
    Err(Error::new(msg, span))
}

/// The closure of a function without captures, as a value (made once).
fn plain_value(it: &mut Interp<'_>, m: &Module, f: FuncId) -> Value {
    let r = rt(it);
    if let Some(c) = &r.plain[f as usize] {
        return c.clone();
    }
    let c = Value::func_rc(Rc::new(Func::Ir(Closure {
        func: f,
        name: m.funcs[f as usize].name.clone(),
        caps: Vec::new(),
        watch: false,
    })));
    r.plain[f as usize] = Some(c.clone());
    c
}

/// The closure of a function without captures.
fn plain(it: &mut Interp<'_>, m: &Module, f: FuncId) -> Rc<Func> {
    match plain_value(it, m, f).view() {
        V::Func(rc) => rc.clone(),
        _ => unreachable!("plain_value makes functions"),
    }
}

/// A closure of `func` as a value: the shared one when there are no
/// captures.
pub(crate) fn closure_value(
    it: &mut Interp<'_>,
    m: &Module,
    func: FuncId,
    caps: Vec<Value>,
) -> Value {
    if caps.is_empty() {
        return plain_value(it, m, func);
    }
    Value::func_rc(closure(it, m, func, caps))
}

/// A closure of `func`: the shared one when there are no captures.
pub(crate) fn closure(it: &mut Interp<'_>, m: &Module, func: FuncId, caps: Vec<Value>) -> Rc<Func> {
    if caps.is_empty() {
        return plain(it, m, func);
    }
    let watch =
        m.funcs[func as usize].kind != FnKind::Thunk && caps.iter().any(|x| record(x).is_some());
    Rc::new(Func::Ir(Closure {
        func,
        name: m.funcs[func as usize].name.clone(),
        caps,
        watch,
    }))
}

fn cycle_error(stack: &[Rc<str>], name: &str, span: Span) -> Error {
    let start = stack
        .iter()
        .position(|c| c.split(", ").any(|n| n == name))
        .unwrap_or(0);
    let mut chain: Vec<&str> = stack[start..].iter().map(|s| &**s).collect();
    chain.push(name);
    Error::new(
        format!(
            "constant `{name}` depends on itself: {}",
            chain.join(" -> ")
        ),
        span,
    )
}

fn force_global(it: &mut Interp<'_>, m: &Module, cell: usize, span: Span) -> Result<Value> {
    let (c, state) = &rt(it).cells[cell];
    let c = c.clone();
    match state {
        State::Done(v) => return Ok(v.clone()),
        State::Evaluating => return Err(cycle_error(&rt(it).const_stack, &c.describe, span)),
        State::Pending => {}
    }
    rt(it).cells[cell].1 = State::Evaluating;
    rt(it).const_stack.push(c.describe.clone());
    let clo = plain(it, m, c.thunk);
    let r = run(it, m, clo, Vec::new(), span);
    rt(it).const_stack.pop();
    rt(it).cells[cell].1 = match &r {
        Ok(v) => State::Done(v.clone()),
        Err(_) => State::Pending,
    };
    r
}

/// The value of cell `k` of the record `recv`, computed on first use.
fn force_cell(
    it: &mut Interp<'_>,
    m: &Module,
    recv: &Value,
    k: usize,
    span: Span,
) -> Result<Value> {
    let thunk = match force_begin(it, m, recv, k, span)? {
        Forced::Done(v) => return Ok(v),
        Forced::Run(thunk) => thunk,
    };
    let clo = closure(it, m, thunk, vec![recv.clone()]);
    let r = run(it, m, clo, Vec::new(), span);
    force_end(it, recv, k, r.as_ref().ok());
    r
}

/// What [`force_begin`] found.
pub(crate) enum Forced {
    Done(Value),
    /// The cell is now being computed: run this thunk (capturing the
    /// record), then call [`force_end`].
    Run(FuncId),
}

/// The first half of forcing cell `k` of record `recv`: its value if it
/// has one, or the thunk to compute it, after marking it as being
/// computed (so a cycle is an error).
pub(crate) fn force_begin(
    it: &mut Interp<'_>,
    m: &Module,
    recv: &Value,
    k: usize,
    span: Span,
) -> Result<Forced> {
    let rec = record(recv).ok_or_else(|| internal("not a record", span))?;
    let cell = &m.layouts[rec.layout].cells[k];
    match &*rec.cells[k].borrow() {
        State::Done(v) => return Ok(Forced::Done(v.clone())),
        State::Evaluating => return Err(cycle_error(&rt(it).const_stack, &cell.describe, span)),
        State::Pending => {}
    }
    *rec.cells[k].borrow_mut() = State::Evaluating;
    rt(it).const_stack.push(cell.describe.clone());
    Ok(Forced::Run(cell.thunk))
}

/// The second half: stores the thunk's value, or makes the cell pending
/// again after an error (`None`).
pub(crate) fn force_end(it: &mut Interp<'_>, recv: &Value, k: usize, v: Option<&Value>) {
    rt(it).const_stack.pop();
    let Some(rec) = record(recv) else { return };
    if let Some(v) = v
        && let V::Func(f) = v.view()
        && let Func::Ir(c) = &**f
        && c.caps.iter().any(|x| x.raw_bits() == recv.raw_bits())
    {
        rec.cyclic.set(true);
    }
    *rec.cells[k].borrow_mut() = match v {
        Some(v) => State::Done(v.clone()),
        None => State::Pending,
    };
}

/// The thunk of cell `k` of the record `recv`.
pub(crate) fn cell_thunk(m: &Module, recv: &Value, k: usize) -> Option<FuncId> {
    let rec = record(recv)?;
    Some(m.layouts[rec.layout].cells.get(k)?.thunk)
}

/// Value `i` copied into the record `recv`.
pub(crate) fn rec_val(recv: &Value, i: usize) -> Option<Value> {
    record(recv).and_then(|r| r.vals.get(i).cloned())
}

/// The parent of the record `recv`.
pub(crate) fn rec_parent(recv: &Value) -> Option<Value> {
    record(recv).and_then(|r| r.parent.clone())
}

/// A new record of layout `layout`.
pub(crate) fn new_record(
    m: &Module,
    layout: usize,
    parent: Option<Value>,
    vals: Vec<Value>,
) -> Value {
    let l = &m.layouts[layout];
    Value::internal(Internal::Rec(Record {
        parent,
        layout,
        vals,
        cells: (0..l.cells.len())
            .map(|_| RefCell::new(State::Pending))
            .collect(),
        memos: (0..l.memos).map(|_| Rc::new(Memo::default())).collect(),
        cyclic: std::cell::Cell::new(false),
    }))
}

// ---------------------------------------------------------------------------
// Reference cycles through records

/// Whether dropping a reference to `h` needs [`release_cycle`]: `h` is a
/// record, or a closure capturing one, that a cycle may run through.
#[inline]
pub(crate) fn may_close_cycle(h: &Heap) -> bool {
    match h {
        Heap::Internal(Internal::Rec(r)) => r.cyclic.get(),
        Heap::Func(f) => match &**f {
            Func::Ir(c) => {
                c.watch
                    && c.caps
                        .iter()
                        .any(|x| record(x).is_some_and(|r| r.cyclic.get()))
            }
            _ => false,
        },
        _ => false,
    }
}

/// Called just before a reference to `h` is dropped, when `h` (word
/// `bits`, `count` references in all, counting the one being dropped) is a
/// record or a function.
///
/// A record's cell can hold a closure that captures the record itself (a
/// `where` lambda that calls itself or a sibling): a reference cycle. Once
/// nothing outside the cycle refers to the record, its cells are cleared,
/// which frees the lot. This is the IR engines' version of the evaluator's
/// `release_frame`, checked whenever a reference to the record, or to a
/// closure capturing it, goes. It is conservative: anything it can't
/// account for counts as an outside reference, so the worst case is a
/// leak. Clearing a cell that was still reachable could only make it be
/// computed again.
pub(crate) fn release_cycle(h: &Heap, bits: u64, count: usize) {
    match h {
        Heap::Internal(Internal::Rec(r)) if count > 1 => {
            collect(r, bits, count - 1, Dying { bits, func: None });
        }
        Heap::Func(f) => {
            let Func::Ir(c) = &**f else { return };
            // The closure itself is released when this was its last word,
            // which drops a reference to the shared `Rc<Func>`.
            let func = (count == 1).then_some(Rc::as_ptr(f));
            if count == 1 && Rc::strong_count(f) == 1 {
                // Freed outright; its captures are dropped (and checked) in turn.
                return;
            }
            for cap in &c.caps {
                if let (Some(r), Some((rbits, n))) = (record(cap), cap.heap_refs()) {
                    collect(r, rbits, n, Dying { bits, func });
                }
            }
        }
        _ => {}
    }
}

/// The reference about to be dropped: a word, and the function whose
/// `Rc` loses a reference with it, if any.
#[derive(Clone, Copy)]
struct Dying {
    bits: u64,
    func: Option<*const Func>,
}

/// Run a closure's `Rc<Func>` through a value's drop, so that the cycle
/// check sees it go (see [`release_cycle`]).
fn release_closure(f: Rc<Func>) {
    let shared_with_records = Rc::strong_count(&f) > 1
        && matches!(&*f, Func::Ir(c) if c.caps.iter().any(|x| record(x).is_some()));
    if shared_with_records {
        drop(Value::func_rc(f));
    }
}

/// Clears the cells of record `r` (word `rbits`) if its `others` remaining
/// references all come from closures held only in its own cells, once the
/// reference `dying` has gone.
fn collect(r: &Record, rbits: u64, others: usize, dying: Dying) {
    if others == 0 || !r.cyclic.get() {
        return;
    }
    // Each distinct closure in the cells: (word, cells holding it,
    // references left to it, its references to the record).
    let mut found: Vec<(u64, usize, usize, usize)> = Vec::new();
    for cell in &r.cells {
        let Ok(state) = cell.try_borrow() else {
            return;
        };
        let State::Done(v) = &*state else { continue };
        let Some((vbits, mut n)) = v.heap_refs() else {
            continue;
        };
        let V::Func(rc) = v.view() else { continue };
        let Func::Ir(c) = &**rc else { continue };
        if vbits == dying.bits {
            n -= 1;
        }
        let mut strong = Rc::strong_count(rc);
        if vbits != dying.bits && dying.func == Some(Rc::as_ptr(rc)) {
            strong -= 1;
        }
        if strong != 1 {
            n = usize::MAX;
        }
        let refs = c.caps.iter().filter(|x| x.raw_bits() == rbits).count();
        match found.iter_mut().find(|f| f.0 == vbits) {
            Some(f) => f.1 += 1,
            None => found.push((vbits, 1, n, refs)),
        }
    }
    // The record is unreachable if every reference to it comes from these
    // closures, and each closure that refers to it is held only here. (A
    // closure that doesn't refer to it can't keep it alive.)
    let internal: usize = found.iter().map(|f| f.3).sum();
    if internal != others || found.iter().any(|f| f.3 > 0 && f.1 != f.2) {
        return;
    }
    // Moved out first, so no borrow is held while they are freed.
    let mut taken = Vec::new();
    for cell in &r.cells {
        if let Ok(mut state) = cell.try_borrow_mut() {
            taken.push(std::mem::replace(&mut *state, State::Pending));
        }
    }
    drop(taken);
}

// ---------------------------------------------------------------------------
// Memo tables

/// The memo table at `at`, for a closure with captures `caps`.
pub(crate) fn memo_table(
    it: &mut Interp<'_>,
    at: MemoAt,
    caps: &[Value],
    span: Span,
) -> Result<Rc<Memo>> {
    match at {
        MemoAt::Global(i) => Ok(rt(it).memos[i].clone()),
        MemoAt::Record(i) => caps
            .first()
            .and_then(record)
            .map(|r| r.memos[i].clone())
            .ok_or_else(|| internal("memo table", span)),
    }
}

/// Whether a call with these arguments is memoized (SPEC 8.1), after the
/// large-key warning.
pub(crate) fn memo_key_ok(it: &mut Interp<'_>, f: &Function, args: &[Value]) -> bool {
    if args.iter().any(contains_generator) {
        return false;
    }
    if !f.memo_annotated
        && let Some(id) = f.def_id
    {
        it.check_memo_key_for(id, &f.name, f.span, args);
    }
    true
}

// ---------------------------------------------------------------------------
// Calls

/// Runs closure `clo` (arguments already checked), following tail calls.
pub(crate) fn run(
    it: &mut Interp<'_>,
    m: &Module,
    clo: Rc<Func>,
    args: Vec<Value>,
    span: Span,
) -> Result<Value> {
    if let Some(n) = rt(it).native.clone() {
        return crate::native::run(it, &n, Value::func_rc(clo), args, span);
    }
    stacker::maybe_grow(STACK_RED_ZONE, STACK_GROW, || {
        run_loop(it, m, clo, args, span)
    })
}

fn run_loop(
    it: &mut Interp<'_>,
    m: &Module,
    mut clo: Rc<Func>,
    mut args: Vec<Value>,
    mut span: Span,
) -> Result<Value> {
    // Memo tables waiting for the result of a chain of tail calls.
    let mut pending: Vec<(Rc<Memo>, Vec<Value>)> = Vec::new();
    let finish = |pending: Vec<(Rc<Memo>, Vec<Value>)>, v: Value| {
        for (memo, key) in pending {
            memo.table.borrow_mut().insert(key, v.clone());
        }
        v
    };
    // For the stack trace: the first traced function of this chain of tail
    // calls and where it was called, and how many tail calls followed.
    let mut first: Option<(&str, Span)> = None;
    let mut tail_calls = 0;
    loop {
        let Func::Ir(c) = &*clo else {
            return Err(internal("not an IR closure", span));
        };
        let f = &m.funcs[c.func as usize];
        if first.is_none() && traced(f) {
            first = Some((&f.name, span));
            tail_calls = 0;
        }
        if let Some(at) = f.memo
            && memo_key_ok(it, f, &args)
        {
            let table = memo_table(it, at, &c.caps, span)?;
            // Copied out first: `finish` may add to this same table.
            let hit = table.table.borrow().get(&args).cloned();
            if let Some(v) = hit {
                return Ok(finish(pending, v));
            }
            pending.push((table, args.clone()));
        }
        let mut tails = SelfTails::default();
        let result = body(it, m, f, c, args, &mut tails);
        tail_calls += tails.count;
        if let Some(s) = tails.last {
            span = s;
        }
        let flow = match result {
            Ok(flow) => flow,
            Err(mut e) => {
                // As the tree-walker does: the function that failed, called
                // at the last tail call, then the call that started the
                // chain with the number of tail calls that followed.
                if traced(f) {
                    e.push_trace(&f.name, span, 0, None);
                }
                if tail_calls > 0
                    && let Some((name, start)) = first
                {
                    e.push_trace(name, start, tail_calls, None);
                }
                return Err(e);
            }
        };
        match flow {
            Flow::Return(v) => {
                release_closure(clo);
                return Ok(finish(pending, v));
            }
            Flow::Tail(f2, a2, s2) => {
                release_closure(std::mem::replace(&mut clo, f2));
                args = a2;
                span = s2;
                tail_calls += 1;
            }
        }
    }
}

enum Flow {
    Return(Value),
    Tail(Rc<Func>, Vec<Value>, Span),
}

struct Regs(Vec<Option<Value>>);

impl Regs {
    /// Reads an operand: moved out on its last use, otherwise copied.
    fn val(&mut self, u: Use, span: Span) -> Result<Value> {
        let r = &mut self.0[u.var as usize];
        let v = if u.last { r.take() } else { r.clone() };
        v.ok_or_else(|| internal(&format!("v{} is empty", u.var), span))
    }

    fn vals(&mut self, us: &[Use], span: Span) -> Result<Vec<Value>> {
        us.iter().map(|&u| self.val(u, span)).collect()
    }
}

/// Whether errors leaving `f` name it in the stack trace.
fn traced(f: &Function) -> bool {
    matches!(f.kind, FnKind::Named | FnKind::Lambda)
}

/// Tail calls made by a run of [`body`] as jumps (`Op::SelfTail`): how many,
/// and where the last one was.
#[derive(Default)]
struct SelfTails {
    count: usize,
    last: Option<Span>,
}

fn body(
    it: &mut Interp<'_>,
    m: &Module,
    f: &Function,
    c: &Closure,
    args: Vec<Value>,
    tails: &mut SelfTails,
) -> Result<Flow> {
    let mut regs = Regs(vec![None; f.nvars as usize]);
    for (&p, a) in f.blocks[0].params.iter().zip(args) {
        regs.0[p as usize] = Some(a);
    }
    let mut b = 0;
    let mut operands = Vec::new();
    loop {
        let block = &f.blocks[b];
        for &d in &block.drops {
            regs.0[d as usize] = None;
        }
        for inst in &block.insts {
            if let Op::SelfTail = inst.op {
                tails.count += 1;
                tails.last = Some(inst.span);
                continue;
            }
            operands.clear();
            let mut bad = None;
            inst.op.uses(&mut |u| match regs.val(u, inst.span) {
                Ok(v) => operands.push(v),
                Err(e) => bad = Some(e),
            });
            if let Some(e) = bad {
                return Err(e);
            }
            let r = exec(it, m, &c.caps, &inst.op, operands.drain(..), inst.span)?;
            if let Some(d) = inst.dst {
                regs.0[d as usize] = Some(r);
            }
        }
        match &block.term {
            Term::Jump(to, args) => {
                let vals: Vec<Option<Value>> =
                    args.iter().map(|&u| regs.val(u, f.span).ok()).collect();
                for (&p, v) in f.blocks[*to as usize].params.iter().zip(vals) {
                    regs.0[p as usize] = v;
                }
                b = *to as usize;
            }
            Term::If { cond, then, els } => {
                b = match (regs.val(*cond, f.span)?).view() {
                    V::Bool(true) => *then as usize,
                    V::Bool(false) => *els as usize,
                    _ => return Err(internal("branch on a non-bool", f.span)),
                };
            }
            Term::Return(u) => return Ok(Flow::Return(regs.val(*u, f.span)?)),
            Term::TailCall { callee, args, span } => {
                let callee = match callee {
                    Callee::Value(u) => regs.val(*u, *span)?,
                    Callee::Known { func, caps } => {
                        let caps = regs.vals(caps, *span)?;
                        let args = regs.vals(args, *span)?;
                        let clo = closure(it, m, *func, caps);
                        return Ok(Flow::Tail(clo, args, *span));
                    }
                };
                let mut args = regs.vals(args, *span)?;
                match callee.view() {
                    V::Func(rc) if matches!(&**rc, Func::Ir(_)) => {
                        if let Func::Ir(c) = &**rc {
                            check_args(m, c, &mut args, *span)?;
                        }
                        return Ok(Flow::Tail(rc.clone(), args, *span));
                    }
                    _ => {
                        return Ok(Flow::Return(it.call_named(
                            callee,
                            args,
                            Vec::new(),
                            *span,
                        )?));
                    }
                }
            }
            Term::Next {
                iter,
                some,
                none,
                span,
            } => {
                let i = regs.val(*iter, *span)?;
                match next_item(it, &i, *span)? {
                    Some(x) => {
                        let p = f.blocks[*some as usize].params[0];
                        regs.0[p as usize] = Some(x);
                        b = *some as usize;
                    }
                    None => b = *none as usize,
                }
            }
            Term::Fail { fail, vals, span } => {
                let vals = regs.vals(vals, *span)?;
                return Err(fail_error(fail, &vals, *span));
            }
            Term::Unset => return Err(internal("unset terminator", f.span)),
        }
    }
}

/// The next item of the iterator `iter`, for [`Term::Next`].
pub(crate) fn next_item(it: &mut Interp<'_>, iter: &Value, span: Span) -> Result<Option<Value>> {
    match iter.as_internal() {
        Some(Internal::Iter(i)) => i.borrow_mut().next(it),
        _ => Err(internal("not an iterator", span)),
    }
}

/// The error of a [`Term::Fail`].
pub(crate) fn fail_error(fail: &Fail, vals: &[Value], span: Span) -> Error {
    let shown = || vals.iter().map(short).collect::<Vec<_>>().join(", ");
    let msg = match fail {
        Fail::NoClause(name) => format!("no clause of `{name}` matches ({})", shown()),
        Fail::NoArm => format!("no match arm matches {}", shown()),
        Fail::ConstPattern(src) => {
            format!("expected a value matching `{src}`, found {}", shown())
        }
        Fail::LambdaArg(src) => {
            format!(
                "expected a lambda argument matching `{src}`, found {}",
                shown()
            )
        }
        Fail::Msg(msg) => msg.to_string(),
    };
    Error::new(msg, span)
}

pub(crate) fn item(v: &Value, i: usize, span: Span) -> Result<Value> {
    match v.view() {
        V::Tuple(xs) => xs.get(i).cloned(),
        V::List(xs) => xs.get(i).cloned(),
        _ => None,
    }
    .ok_or_else(|| internal("item", span))
}

pub(crate) fn type_is(v: &Value, ty: TypeName) -> bool {
    match ty {
        TypeName::Int => matches!(v.view(), V::Int(_)),
        TypeName::Bool => matches!(v.view(), V::Bool(_)),
        TypeName::Char => matches!(v.view(), V::Char(_)),
        TypeName::Str => matches!(v.view(), V::Str(_)),
        TypeName::None => v.is_none(),
        TypeName::Tuple => matches!(v.view(), V::Tuple(_)),
        TypeName::List => matches!(v.view(), V::List(_)),
        TypeName::Set => matches!(v.view(), V::Set(_)),
        TypeName::Map => matches!(v.view(), V::Map(_)),
        TypeName::Range => matches!(v.view(), V::Range(_)),
        TypeName::RangeSet => matches!(v.view(), V::RangeSet(_)),
        TypeName::Grid => matches!(v.view(), V::Grid(_)),
        TypeName::Function => matches!(v.view(), V::Func(_)),
    }
}

fn int_operand(v: Value, what: &str, span: Span) -> Result<Int> {
    match v.view() {
        V::Int(i) => Ok(i),
        other => Err(Error::new(
            format!("{what} must be an int, found {}", other.type_name()),
            span,
        )),
    }
}

/// How many items to make room for in a list built from `coll`
/// ([`Op::NewAcc`]): its length if it is a list, tuple or finite range of
/// small ints, up to a limit (filters may keep far fewer).
pub(crate) fn size_hint(coll: &Value) -> usize {
    const MOST: usize = 1024;
    let n = match coll.view() {
        V::List(l) => l.len(),
        V::Tuple(t) => t.len(),
        V::Range(r) => match (&r.start, &r.end) {
            (Int::Small(s), Some(Int::Small(e))) if e >= s => {
                usize::try_from(e.saturating_sub(*s).saturating_add(1)).unwrap_or(MOST)
            }
            _ => 0,
        },
        _ => 0,
    };
    n.min(MOST)
}

/// [`Op::Loopable`]: whether a library call over `coll` with `f` can be a
/// loop.
pub(crate) fn loopable(coll: &Value, f: &Value) -> bool {
    let finite = |r: &Range| !r.start.is_inf() && r.end.as_ref().is_some_and(|e| !e.is_inf());
    matches!(f.view(), V::Func(_))
        && match coll.view() {
            V::List(_) | V::Tuple(_) | V::Set(_) | V::Map(_) | V::Str(_) => true,
            V::Range(r) => finite(r),
            _ => false,
        }
}

/// [`Op::Middle`]: the list `v` without its first `before` and last
/// `after` items (known to have them).
pub(crate) fn middle(v: &Value, before: usize, after: usize) -> Option<Value> {
    match v.view() {
        V::List(l) if before + after <= l.len() => {
            Some(Value::list(l.slice(before, l.len() - after)))
        }
        _ => None,
    }
}

/// Runs one instruction, `op`, on its operands: owned values, in the order
/// of [`Op::uses`]. `caps` are the running closure's captures.
pub(crate) fn exec(
    it: &mut Interp<'_>,
    m: &Module,
    caps: &[Value],
    op: &Op,
    mut ops: impl Iterator<Item = Value>,
    span: Span,
) -> Result<Value> {
    let at = |msg: String| Error::new(msg, span);
    let mut next = || ops.next().ok_or_else(|| internal("missing operand", span));
    Ok(match op {
        Op::Const(x) => x.clone(),
        // Counted by `body`, which never passes it here.
        Op::SelfTail => Value::NONE,
        Op::Cap(i) => caps
            .get(*i as usize)
            .cloned()
            .ok_or_else(|| internal("capture", span))?,
        Op::Global(i) => global(it, *i, span)?,
        Op::BuiltinValue(id) => it.builtin_value(*id, span)?,
        Op::NewRecord {
            layout,
            parent,
            vals,
        } => {
            let parent = match parent {
                Some(_) => Some(next()?),
                None => None,
            };
            let vals = (0..vals.len()).map(|_| next()).collect::<Result<_>>()?;
            new_record(m, *layout, parent, vals)
        }
        Op::RecParent(_) => {
            rec_parent(&next()?).ok_or_else(|| internal("record has no parent", span))?
        }
        Op::RecVal(_, i) => {
            rec_val(&next()?, *i as usize).ok_or_else(|| internal("record value", span))?
        }
        Op::Force(_, k) => force_cell(it, m, &next()?, *k as usize, span)?,
        Op::MakeClosure { func, caps } => {
            let caps = (0..caps.len()).map(|_| next()).collect::<Result<_>>()?;
            // A closure without captures is made once, so that it is the
            // same value each time (as the evaluator's closed lambdas).
            closure_value(it, m, *func, caps)
        }
        Op::Call { args, named, .. } => {
            let f = next()?;
            let args = (0..args.len()).map(|_| next()).collect::<Result<_>>()?;
            let mut nv = Vec::new();
            for (n, _) in named {
                nv.push((n.clone(), next()?));
            }
            it.call_named(f, args, nv, span)?
        }
        Op::CallKnown { func, caps, args } => {
            let caps = (0..caps.len()).map(|_| next()).collect::<Result<_>>()?;
            let args = (0..args.len()).map(|_| next()).collect::<Result<_>>()?;
            let clo = closure(it, m, *func, caps);
            run(it, m, clo, args, span)?
        }
        Op::CallBuiltin { id, args, named } => {
            let mut pos: Vec<Value> = (0..args.len()).map(|_| next()).collect::<Result<_>>()?;
            let mut nv = Vec::new();
            for (n, _) in named {
                nv.push((n.clone(), next()?));
            }
            let b = &BUILTINS[*id];
            if let Some(n) = b.arity {
                spread(&mut pos, n);
            }
            (b.f)(
                it,
                Args {
                    pos,
                    named: nv,
                    span,
                },
            )
            .map_err(|e| e.at(span))?
        }
        Op::Bin(o, _, _) => {
            let a = next()?;
            let b = next()?;
            if let (V::Int(Int::Small(x)), V::Int(Int::Small(y))) = (a.view(), b.view())
                && let Some(r) = small_int_op(*o, x, y)
            {
                return Ok(Value::from_int(Int::Small(r)));
            }
            binary_owned(*o, a, b).map_err(at)?
        }
        Op::Cmp(o, _, _) => {
            let a = next()?;
            let b = next()?;
            Value::bool(it.compare_op(*o, &a, &b).map_err(at)?)
        }
        Op::Neg(_) => negate(&next()?).map_err(at)?,
        Op::Not(_) => match next()?.view() {
            V::Bool(b) => Value::bool(!b),
            other => {
                return Err(at(format!(
                    "`not` needs a bool, found {}",
                    other.type_name()
                )));
            }
        },
        Op::BitNot(_) => match next()?.view() {
            V::Int(n) if !n.is_inf() => Value::from_int(n.neg().sub(&Int::Small(1)).map_err(at)?),
            other => return Err(at(format!("`~` needs an int, found {}", other.type_name()))),
        },
        Op::CheckBool(_, what) => match next()?.view() {
            V::Bool(b) => Value::bool(b),
            other => {
                return Err(at(format!(
                    "{what} needs a bool, found {}",
                    other.type_name()
                )));
            }
        },
        Op::CheckPred(_, fname) => match next()?.view() {
            V::Bool(b) => Value::bool(b),
            other => {
                return Err(at(format!(
                    "the predicate passed to `{fname}` must return a bool, but returned {}",
                    other.type_name()
                )));
            }
        },
        Op::CheckElem(_, fname) => match next()?.view() {
            V::Bool(b) => Value::bool(b),
            other => {
                return Err(at(format!(
                    "`{fname}` with no argument needs bool elements, found {}",
                    other.type_name()
                )));
            }
        },
        Op::Loopable { .. } => {
            let coll = next()?;
            let f = next()?;
            Value::bool(loopable(&coll, &f))
        }
        Op::Tuple(xs) => Value::tuple((0..xs.len()).map(|_| next()).collect::<Result<_>>()?),
        Op::List(xs) => Value::list((0..xs.len()).map(|_| next()).collect::<Result<Vec<_>>>()?),
        Op::Set(xs) => {
            let mut s = ValueSet::new();
            for _ in 0..xs.len() {
                value::set_insert(&mut s, next()?).map_err(at)?;
            }
            Value::set(s)
        }
        Op::Map(kvs) => {
            let mut out = ValueMap::new();
            for _ in 0..kvs.len() {
                let k = next()?;
                let x = next()?;
                value::map_insert(&mut out, k, x).map_err(at)?;
            }
            Value::map(out)
        }
        Op::Format(parts) => {
            let mut s = String::new();
            for p in parts {
                match p {
                    FmtPart::Lit(l) => s.push_str(l),
                    FmtPart::Val(_) => s.push_str(&next()?.display()),
                }
            }
            Value::from_str_rc(s.into())
        }
        Op::Index(_, _) => {
            let a = next()?;
            let i = next()?;
            crate::builtins::collections::index(&a, &i).map_err(at)?
        }
        Op::IndexInt(_, i) => crate::builtins::collections::index_int(&next()?, *i).map_err(at)?,
        Op::Slice {
            lo, hi, inclusive, ..
        } => {
            let coll = next()?;
            let lo = match lo {
                Some(_) => Some(next()?),
                None => None,
            };
            let hi = match hi {
                Some(_) => Some(next()?),
                None => None,
            };
            crate::builtins::collections::slice(&coll, lo, hi, *inclusive).map_err(at)?
        }
        Op::Range { hi, inclusive, .. } => {
            let start = int_operand(next()?, "range start", span)?;
            let end = match hi {
                None => None,
                Some(_) => {
                    let h = int_operand(next()?, "range end", span)?;
                    Some(if *inclusive {
                        h
                    } else {
                        h.sub(&Int::Small(1)).map_err(at)?
                    })
                }
            };
            Value::range_rc(Rc::new(Range {
                start,
                end,
                exclusive: hi.is_some() && !*inclusive,
            }))
        }
        Op::IsType(_, ty) => Value::bool(type_is(&next()?, *ty)),
        Op::EqConst(_, x) => Value::bool(next()? == *x),
        Op::IsTuple(_, n) => Value::bool(matches!(next()?.view(), V::Tuple(t) if t.len() == *n)),
        Op::IsList { n, at_least, .. } => Value::bool(match next()?.view() {
            V::List(l) if *at_least => l.len() >= *n,
            V::List(l) => l.len() == *n,
            _ => false,
        }),
        Op::Item(_, i) => item(&next()?, *i, span)?,
        Op::ItemBack(_, i) => match next()?.view() {
            V::List(l) if *i < l.len() => l.get(l.len() - 1 - i).cloned().unwrap_or(Value::NONE),
            _ => return Err(internal("item from the end", span)),
        },
        Op::Middle { before, after, .. } => {
            middle(&next()?, *before, *after).ok_or_else(|| internal("middle of a list", span))?
        }
        Op::MatchStr(_, segs) => {
            let x = next()?;
            let V::Str(s) = x.view() else {
                return Ok(Value::NONE);
            };
            let mut binds = Vec::new();
            if !match_segs(segs, s, 0, 0, &mut binds) {
                return Ok(Value::NONE);
            }
            Value::tuple(binds.into_iter().map(|(_, x)| x).collect())
        }
        Op::Iter { finite, .. } => {
            let coll = next()?;
            let i = it.iter(&coll).map_err(|e| e.at(span))?;
            if *finite && i.is_infinite() {
                return Err(at("cannot build a collection from an infinite range".into()));
            }
            Value::internal(Internal::Iter(Rc::new(RefCell::new(i))))
        }
        Op::NewAcc(k, hint) => {
            let room = match hint {
                Some(_) => size_hint(&next()?),
                None => 0,
            };
            Value::internal(Internal::Acc(match k {
                AccKind::List => Acc::List(Vec::with_capacity(room)),
                AccKind::Set => Acc::Set(ValueSet::new()),
                AccKind::Map => Acc::Map(ValueMap::new()),
            }))
        }
        Op::AccPush(_, _) => {
            let mut a = next()?;
            let x = next()?;
            match a.make_mut() {
                Some(Heap::Internal(Internal::Acc(Acc::List(l)))) => l.push(x),
                Some(Heap::Internal(Internal::Acc(Acc::Set(s)))) => {
                    value::set_insert(s, x).map_err(at)?
                }
                _ => return Err(internal("push to a non-list, non-set", span)),
            }
            a
        }
        Op::AccInsert(_, _, _) => {
            let mut a = next()?;
            let k = next()?;
            let x = next()?;
            match a.make_mut() {
                Some(Heap::Internal(Internal::Acc(Acc::Map(mp)))) => {
                    value::map_insert(mp, k, x).map_err(at)?
                }
                _ => return Err(internal("insert into a non-map", span)),
            }
            a
        }
        Op::AccDone(_) => match next()?.unpack() {
            value::Own::Internal(Internal::Acc(Acc::List(l))) => Value::list(l),
            value::Own::Internal(Internal::Acc(Acc::Set(s))) => Value::set(s),
            value::Own::Internal(Internal::Acc(Acc::Map(mp))) => Value::map(mp),
            _ => return Err(internal("not an accumulator", span)),
        },
        Op::FlatMap { .. } => {
            let src = next()?;
            let f = next()?;
            let i = it.iter(&src).map_err(|e| e.at(span))?;
            Value::generator(Generator::flat_map(i, f, span))
        }
    })
}
