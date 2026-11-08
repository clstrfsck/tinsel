//! The boundary between compiled code and Rust: the runtime entry points
//! that compiled code calls, and the calls from Rust into compiled code.
//!
//! This is one of the modules where `reference/native-plan.md` (section 4)
//! allows `unsafe`. Compiled code and Rust exchange values as raw words
//! ([`Value::into_raw`]), and the context as a pointer. The rules every
//! entry point follows:
//!
//! - **Words.** Each word argument is either *consumed* (the entry point
//!   takes over its reference, with [`Value::from_raw`]) or *borrowed*
//!   (read through [`borrow`], which never releases it). Each entry
//!   point's comment says which. A result word is always owned by the
//!   caller. The code generator (`codegen.rs`) follows the same comments.
//! - **Argument arrays.** `argv` points at `n` words in the caller's stack
//!   frame, where `n` is known from the site or function: the code
//!   generator writes exactly that many before the call.
//! - **The context.** Compiled code passes back the `&mut Ctx` it was
//!   given, unchanged. It refers to the [`Interp`] of the Rust call that
//!   entered compiled code, which isn't used until that call returns.
//! - **Errors.** An entry point never panics or unwinds. A failure stores
//!   the error in the context and returns [`ERR`]; compiled code returns it
//!   straight up to [`run`], which turns it back into an `Err`.
//!
//! Debug builds check that every word received looks valid
//! ([`Value::from_raw`]).

use super::{Native, Site};
use crate::ast::{BinOp, CmpOp, TypeName};
use crate::builtins::{Args, BUILTINS};
use crate::error::{Error, Result, Span};
use crate::eval::Interp;
use crate::ir::Term;
use crate::ir::interp::{Forced, force_begin, force_end};
use crate::ir::interp::{exec, fail_error, internal, memo_key_ok, memo_table, next_item};
use crate::value::Heap;
use crate::value::word::sentinel::{DONE, ERR, MISS};
use crate::value::{Func, V, Value};
use std::mem::ManuallyDrop;
use std::rc::Rc;

/// The `site` of a call from Rust: its span is the context's. It fits in 32
/// bits, as a function's `site` parameter keeps a count of tail calls in
/// its high half ([`rt_trace`]).
pub const ENTRY: u64 = u32::MAX as u64;

/// Compiled code needs this much stack below the limit for the Rust it
/// calls. Rust code that recurses deeper grows the stack itself.
const MARGIN: usize = 1024 * 1024;
/// A call into compiled code with less stack than this left moves to a new
/// stack of `SEGMENT` bytes (address space; pages are used only as needed).
const RED_ZONE: usize = 64 * 1024 * 1024;
const SEGMENT: usize = 4 * 1024 * 1024 * 1024;

/// What compiled code passes to every function and runtime entry point:
/// one per call from Rust into compiled code.
#[repr(C)]
pub struct Ctx<'a, 'p> {
    /// The lowest stack address compiled code may use. Function prologues
    /// read it at offset 0.
    pub limit: usize,
    it: *mut Interp<'p>,
    native: &'a Native,
    /// The span of the Rust call, for errors at [`ENTRY`].
    span: Span,
    pending: Option<Error>,
}

impl<'p> Ctx<'_, 'p> {
    fn fail(&mut self, e: Error) -> u64 {
        self.pending = Some(e);
        ERR
    }

    fn site(&self, site: u64) -> &Site {
        &self.native.sites[site as usize]
    }

    /// The span of `site`, which may be [`ENTRY`].
    fn span_of(&self, site: u64) -> Span {
        if site == ENTRY {
            self.span
        } else {
            self.site(site).span
        }
    }

    fn interp(&mut self) -> &mut Interp<'p> {
        // SAFETY: `it` came from the `&mut Interp` that `run` was given,
        // and `run` doesn't use that borrow until compiled code returns.
        // Compiled code is single-threaded and calls one entry point at a
        // time, each of which takes this context mutably, so this is the
        // only live reference. Re-entry from Rust (a library function
        // calling a closure) makes a new context from this reborrow.
        unsafe { &mut *self.it }
    }
}

/// The owned words in `argv[..n]`, as values. Any not read are released.
struct Words<'a> {
    words: &'a [u64],
    next: usize,
}

/// The `n` words at `argv`.
///
/// # Safety
///
/// `argv` points at `n` initialised words, which stay put while the result
/// is used.
unsafe fn words<'a>(argv: *const u64, n: usize) -> &'a [u64] {
    if n == 0 {
        return &[];
    }
    // SAFETY: by this function's contract.
    unsafe { std::slice::from_raw_parts(argv, n) }
}

impl<'a> Words<'a> {
    /// # Safety
    ///
    /// `argv` points at `n` words that the caller hands over.
    unsafe fn new(argv: *const u64, n: usize) -> Words<'a> {
        Words {
            // SAFETY: by this function's contract.
            words: unsafe { words(argv, n) },
            next: 0,
        }
    }
}

impl Iterator for Words<'_> {
    type Item = Value;

    fn next(&mut self) -> Option<Value> {
        let w = *self.words.get(self.next)?;
        self.next += 1;
        // SAFETY: the caller handed over each word once (`Words::new`), and
        // `next` passes each one on once.
        Some(unsafe { Value::from_raw(w) })
    }
}

impl Drop for Words<'_> {
    fn drop(&mut self) {
        for v in self.by_ref() {
            drop(v);
        }
    }
}

/// A borrowed word, as a value that must not be dropped.
///
/// # Safety
///
/// `w` must be a valid word that its owner keeps for the whole borrow.
unsafe fn borrow(w: u64) -> ManuallyDrop<Value> {
    // SAFETY: by this function's contract; `ManuallyDrop` stops the value
    // releasing the owner's reference.
    ManuallyDrop::new(unsafe { Value::from_raw(w) })
}

/// The captures of the closure word `clo`: a closure's, or for a
/// function that captures just a record and was given the record itself
/// (`super::record_clo`), that record; none for `none`.
fn caps(clo: &Value) -> &[Value] {
    match clo.view() {
        V::Func(f) => match &**f {
            Func::Ir(c) => &c.caps,
            _ => &[],
        },
        V::Internal(_) => std::slice::from_ref(clo),
        _ => &[],
    }
}

/// The types of `IsType`, numbered for compiled code.
pub(super) const TYPES: [TypeName; 13] = [
    TypeName::Int,
    TypeName::Bool,
    TypeName::Char,
    TypeName::Str,
    TypeName::None,
    TypeName::Tuple,
    TypeName::List,
    TypeName::Set,
    TypeName::Map,
    TypeName::Range,
    TypeName::RangeSet,
    TypeName::Grid,
    TypeName::Function,
];

/// The comparisons, numbered for compiled code.
pub(super) const CMPS: [CmpOp; 8] = [
    CmpOp::Eq,
    CmpOp::Ne,
    CmpOp::Lt,
    CmpOp::Le,
    CmpOp::Gt,
    CmpOp::Ge,
    CmpOp::In,
    CmpOp::NotIn,
];

// ---------------------------------------------------------------------------
// Entry points. All words are `u64`; so are the context, site and counts,
// which keeps their Cranelift signatures uniform.

/// Takes a new reference to `w` (borrowed).
extern "C" fn rt_dup(w: u64) {
    // SAFETY: compiled code passes a word it owns.
    let v = unsafe { borrow(w) };
    let _ = Value::clone(&v).into_raw();
}

/// Releases `w` (consumed).
extern "C" fn rt_drop(w: u64) {
    // SAFETY: compiled code hands over a word it owns.
    drop(unsafe { Value::from_raw(w) });
}

/// Runs the instruction at `site` ([`exec`]) on the operands in `argv`
/// (consumed), as the closure `clo` (borrowed).
extern "C" fn rt_op(cx: &mut Ctx<'_, '_>, clo: u64, site: u64, argv: *const u64) -> u64 {
    let native = cx.native;
    let s = &native.sites[site as usize];
    // SAFETY: the code generator writes `s.n` owned operands at `argv`.
    let ops = unsafe { Words::new(argv, s.n as usize) };
    // SAFETY: `clo` is the running function's own closure word.
    let clo = unsafe { borrow(clo) };
    let m = &*native.module;
    let inst = &m.funcs[s.func as usize].blocks[s.block as usize].insts[s.inst as usize];
    prof::count(&inst.op);
    match exec(cx.interp(), m, caps(&clo), &inst.op, ops, inst.span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// The next item of the iterator `iter` (borrowed), or [`DONE`].
extern "C" fn rt_next(cx: &mut Ctx<'_, '_>, site: u64, iter: u64) -> u64 {
    let span = cx.site(site).span;
    // SAFETY: compiled code keeps the iterator while it reads from it.
    let iter = unsafe { borrow(iter) };
    match next_item(cx.interp(), &iter, span) {
        Ok(Some(v)) => v.into_raw(),
        Ok(None) => DONE,
        Err(e) => cx.fail(e),
    }
}

/// The error of the [`Term::Fail`] at `site`, with its values in `argv`
/// (consumed). Always returns [`ERR`].
extern "C" fn rt_fail(cx: &mut Ctx<'_, '_>, site: u64, argv: *const u64) -> u64 {
    let native = cx.native;
    let s = &native.sites[site as usize];
    // SAFETY: the code generator writes `s.n` owned words at `argv`.
    let vals: Vec<Value> = unsafe { Words::new(argv, s.n as usize) }.collect();
    let e = match &native.module.funcs[s.func as usize].blocks[s.block as usize].term {
        Term::Fail { fail, span, .. } => fail_error(fail, &vals, *span),
        _ => internal("unexpected terminator", s.span),
    };
    cx.fail(e)
}

/// Adds function `func`, called at `site`, to the pending error's trace.
///
/// `site` and `chain` are the function's parameters. The low half of `site`
/// is where the function was called, by a call or the last tail call; the
/// high half counts the tail calls since the chain started, if it was
/// reached by any. Then `chain` holds where the chain started (low half)
/// and the function called there (high half). `tails` more tail calls were
/// made as jumps within the function, the last at site `last`. As in the
/// tree-walker, the trace names the function, called at the last tail
/// call, then the call that started the chain with how many followed.
extern "C" fn rt_trace(
    cx: &mut Ctx<'_, '_>,
    func: u64,
    site: u64,
    chain: u64,
    tails: u64,
    last: u64,
) {
    let before = site >> 32;
    let at = if tails > 0 { last } else { site & ENTRY };
    let span = cx.span_of(at);
    let native = cx.native;
    let name = &native.module.funcs[func as usize].name;
    let count = before + tails;
    let (start, first) = if before > 0 {
        (chain & ENTRY, chain >> 32)
    } else {
        (site & ENTRY, func)
    };
    let start = cx.span_of(start);
    let first = &native.module.funcs[first as usize].name;
    if let Some(e) = &mut cx.pending {
        e.push_trace(name, span, 0);
        if count > 0 {
            e.push_trace(first, start, count as usize);
        }
    }
}

/// The error for running out of stack in a function called at `site`.
extern "C" fn rt_stack(cx: &mut Ctx<'_, '_>, site: u64) -> u64 {
    let span = cx.span_of(site & ENTRY);
    cx.fail(Error::new(
        "stack overflow: the recursion is too deep".to_string(),
        span,
    ))
}

/// A closure of function `func`, capturing the `n` words in `argv`
/// (consumed).
extern "C" fn rt_closure(cx: &mut Ctx<'_, '_>, func: u64, n: u64, argv: *const u64) -> u64 {
    let native = cx.native;
    // SAFETY: the code generator writes `n` owned words at `argv`.
    let caps: Vec<Value> = unsafe { Words::new(argv, n as usize) }.collect();
    crate::ir::interp::closure_value(cx.interp(), &native.module, func as u32, caps).into_raw()
}

/// The compiled code to tail-call for `f` (borrowed) with `n` arguments,
/// or 0 if `f` isn't a compiled closure taking exactly `n`.
extern "C" fn rt_tail_target(cx: &mut Ctx<'_, '_>, f: u64, n: u64) -> u64 {
    // SAFETY: compiled code owns `f` for the duration of the call.
    let f = unsafe { borrow(f) };
    if let V::Func(rc) = f.view()
        && let Func::Ir(c) = &**rc
        && cx.native.module.funcs[c.func as usize].params as u64 == n
    {
        return cx.native.code[c.func as usize] as u64;
    }
    0
}

/// Calls the function in `argv[0]` with the rest of `argv` (all consumed),
/// for the tail call at `site` when it can't be a compiled tail call.
extern "C" fn rt_call_value(cx: &mut Ctx<'_, '_>, site: u64, argv: *const u64) -> u64 {
    let s = cx.site(site);
    let (n, span) = (s.n as usize, s.span);
    // SAFETY: the code generator writes `n` owned words at `argv`.
    let mut words = unsafe { Words::new(argv, n) };
    let Some(f) = words.next() else {
        return cx.fail(internal("call without a function", span));
    };
    let args = words.collect();
    match cx.interp().call_named(f, args, Vec::new(), span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// The memoized result of function `func` for the arguments in `argv`
/// (borrowed), called as `clo` (borrowed), or [`MISS`].
extern "C" fn rt_memo_get(cx: &mut Ctx<'_, '_>, clo: u64, func: u64, argv: *const u64) -> u64 {
    let native = cx.native;
    let f = &native.module.funcs[func as usize];
    // SAFETY: `argv` holds the function's `params` arguments, which the
    // caller keeps for the duration of the call.
    let raw = unsafe { words(argv, f.params as usize) };
    let args: Vec<Value> = raw
        .iter()
        // SAFETY: as above; each is cloned, taking a new reference.
        .map(|&w| Value::clone(&*unsafe { borrow(w) }))
        .collect();
    // SAFETY: the caller owns `clo` for the duration of the call.
    let clo = unsafe { borrow(clo) };
    let Some(at) = f.memo else {
        return MISS;
    };
    if !memo_key_ok(cx.interp(), f, &args) {
        return MISS;
    }
    match memo_table(cx.interp(), at, caps(&clo), f.span) {
        Ok(t) => {
            let hit = t.table.borrow().get(&args).cloned();
            hit.map_or(MISS, Value::into_raw)
        }
        Err(_) => MISS,
    }
}

/// Stores `r` (borrowed) as the result of function `func` for the
/// arguments in `argv` (consumed), called as `clo` (borrowed).
extern "C" fn rt_memo_put(cx: &mut Ctx<'_, '_>, clo: u64, func: u64, argv: *const u64, r: u64) {
    let native = cx.native;
    let f = &native.module.funcs[func as usize];
    // SAFETY: the code generator writes the function's `params` owned
    // arguments at `argv`.
    let args: Vec<Value> = unsafe { Words::new(argv, f.params as usize) }.collect();
    // SAFETY: the caller owns `clo` for the duration of the call.
    let clo = unsafe { borrow(clo) };
    // SAFETY: likewise `r`.
    let r = unsafe { borrow(r) };
    let Some(at) = f.memo else {
        return;
    };
    if args.iter().any(crate::eval::contains_generator) {
        return;
    }
    if let Ok(t) = memo_table(cx.interp(), at, caps(&clo), f.span) {
        t.table.borrow_mut().insert(args, Value::clone(&r));
    }
}

// --- Direct entry points for common instructions. Unlike `rt_op`, these
// borrow their operands and need no lookup of the instruction.

/// Capture `i` of the running function's closure `clo` (borrowed).
extern "C" fn rt_cap(clo: u64, i: u64) -> u64 {
    // SAFETY: `clo` is the running function's own closure word.
    let clo = unsafe { borrow(clo) };
    caps(&clo)
        .get(i as usize)
        .cloned()
        .unwrap_or(Value::NONE)
        .into_raw()
}

/// Value `i` of the record `rec` (borrowed).
extern "C" fn rt_recval(rec: u64, i: u64) -> u64 {
    // SAFETY: compiled code owns `rec` for the duration of the call.
    let rec = unsafe { borrow(rec) };
    crate::ir::interp::rec_val(&rec, i as usize)
        .unwrap_or(Value::NONE)
        .into_raw()
}

/// The parent of the record `rec` (borrowed).
extern "C" fn rt_recparent(rec: u64) -> u64 {
    // SAFETY: compiled code owns `rec` for the duration of the call.
    let rec = unsafe { borrow(rec) };
    crate::ir::interp::rec_parent(&rec)
        .unwrap_or(Value::NONE)
        .into_raw()
}

/// Cell `k` of the record `rec` (borrowed) if it has its value; otherwise
/// [`MISS`], having marked the cell as being computed: compiled code then
/// runs its thunk ([`rt_thunk_code`]) and stores the result
/// ([`rt_forced`]).
extern "C" fn rt_force(cx: &mut Ctx<'_, '_>, site: u64, rec: u64, k: u64) -> u64 {
    let span = cx.site(site).span;
    let native = cx.native;
    // SAFETY: compiled code owns `rec` for the duration of the call.
    let rec = unsafe { borrow(rec) };
    match force_begin(cx.interp(), &native.module, &rec, k as usize, span) {
        Ok(Forced::Done(v)) => v.into_raw(),
        Ok(Forced::Run(_)) => MISS,
        Err(e) => cx.fail(e),
    }
}

/// The compiled thunk of cell `k` of the record `rec` (borrowed).
extern "C" fn rt_thunk_code(cx: &mut Ctx<'_, '_>, rec: u64, k: u64) -> u64 {
    // SAFETY: compiled code owns `rec` for the duration of the call.
    let rec = unsafe { borrow(rec) };
    let native = cx.native;
    match crate::ir::interp::cell_thunk(&native.module, &rec, k as usize) {
        Some(f) => native.code[f as usize] as u64,
        None => 0,
    }
}

/// Stores `r` (borrowed; or `ERR`) as cell `k` of `rec` (borrowed), and
/// returns `r`.
extern "C" fn rt_forced(cx: &mut Ctx<'_, '_>, rec: u64, k: u64, r: u64) -> u64 {
    // SAFETY: compiled code owns `rec` for the duration of the call.
    let rec = unsafe { borrow(rec) };
    if r == ERR {
        force_end(cx.interp(), &rec, k as usize, None);
        return ERR;
    }
    // SAFETY: compiled code owns `r`, a value, for the duration of the call.
    let v = unsafe { borrow(r) };
    force_end(cx.interp(), &rec, k as usize, Some(&v));
    r
}

/// Item `i` of the tuple or list `v` (borrowed).
extern "C" fn rt_item(cx: &mut Ctx<'_, '_>, site: u64, v: u64, i: u64) -> u64 {
    // SAFETY: compiled code owns `v` for the duration of the call.
    let v = unsafe { borrow(v) };
    match crate::ir::interp::item(&v, i as usize, cx.site(site).span) {
        Ok(x) => x.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// Whether `v` (borrowed) is a tuple of `n` items.
extern "C" fn rt_is_tuple(v: u64, n: u64) -> u64 {
    // SAFETY: compiled code owns `v` for the duration of the call.
    let v = unsafe { borrow(v) };
    Value::bool(matches!(v.view(), V::Tuple(t) if t.len() as u64 == n)).into_raw()
}

/// Whether `v` (borrowed) is a list of `n` items, or at least `n`.
extern "C" fn rt_is_list(v: u64, n: u64, at_least: u64) -> u64 {
    // SAFETY: compiled code owns `v` for the duration of the call.
    let v = unsafe { borrow(v) };
    let n = n as usize;
    Value::bool(match v.view() {
        V::List(l) if at_least != 0 => l.len() >= n,
        V::List(l) => l.len() == n,
        _ => false,
    })
    .into_raw()
}

/// Whether `v` (borrowed) has type `TYPES[ty]`.
extern "C" fn rt_is_type(v: u64, ty: u64) -> u64 {
    // SAFETY: compiled code owns `v` for the duration of the call.
    let v = unsafe { borrow(v) };
    let ty = TYPES[ty as usize % TYPES.len()];
    Value::bool(crate::ir::interp::type_is(&v, ty)).into_raw()
}

/// `a[i]` (both borrowed).
extern "C" fn rt_index(cx: &mut Ctx<'_, '_>, site: u64, a: u64, i: u64) -> u64 {
    // SAFETY: compiled code owns `a` and `i` for the duration of the call.
    let a = unsafe { borrow(a) };
    // SAFETY: as above, for `i`.
    let i = unsafe { borrow(i) };
    match crate::builtins::collections::index(&a, &i) {
        Ok(x) => x.into_raw(),
        Err(m) => {
            let span = cx.site(site).span;
            cx.fail(Error::new(m, span))
        }
    }
}

/// `a.i` for a constant int `i` (`a` borrowed).
extern "C" fn rt_index_int(cx: &mut Ctx<'_, '_>, site: u64, a: u64, i: u64) -> u64 {
    // SAFETY: compiled code owns `a` for the duration of the call.
    let a = unsafe { borrow(a) };
    match crate::builtins::collections::index_int(&a, i as i64) {
        Ok(x) => x.into_raw(),
        Err(m) => {
            let span = cx.site(site).span;
            cx.fail(Error::new(m, span))
        }
    }
}

/// `a op b` for the comparison `CMPS[op]` (both borrowed).
extern "C" fn rt_cmp(cx: &mut Ctx<'_, '_>, site: u64, op: u64, a: u64, b: u64) -> u64 {
    // SAFETY: compiled code owns `a` and `b` for the duration of the call.
    let a = unsafe { borrow(a) };
    // SAFETY: as above, for `b`.
    let b = unsafe { borrow(b) };
    let op = CMPS[op as usize % CMPS.len()];
    match cx.interp().compare_op(op, &a, &b) {
        Ok(r) => Value::bool(r).into_raw(),
        Err(m) => {
            let span = cx.site(site).span;
            cx.fail(Error::new(m, span))
        }
    }
}

/// A tuple of the `n` words in `argv` (consumed).
extern "C" fn rt_tuple(argv: *const u64, n: u64) -> u64 {
    // SAFETY: the code generator writes `n` owned words at `argv`.
    let items: Vec<Value> = unsafe { Words::new(argv, n as usize) }.collect();
    Value::tuple(items).into_raw()
}

/// The value of global `i`.
extern "C" fn rt_global(cx: &mut Ctx<'_, '_>, site: u64, i: u64) -> u64 {
    let span = cx.site(site).span;
    match crate::ir::interp::global(cx.interp(), i as usize, span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// A record of layout `layout`, with the parent `argv[0]` if `parent`,
/// then `n` values (all consumed).
extern "C" fn rt_record(
    cx: &mut Ctx<'_, '_>,
    layout: u64,
    parent: u64,
    argv: *const u64,
    n: u64,
) -> u64 {
    // SAFETY: the code generator writes `n` owned words at `argv`, plus
    // the parent first if there is one.
    let mut words = unsafe { Words::new(argv, n as usize + usize::from(parent != 0)) };
    let parent = if parent != 0 { words.next() } else { None };
    let vals = words.collect();
    crate::ir::interp::new_record(&cx.native.module, layout as usize, parent, vals).into_raw()
}

/// The binary operators, numbered for compiled code.
pub(super) const BINS: [BinOp; 12] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Mod,
    BinOp::Pow,
    BinOp::Concat,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::BitAnd,
    BinOp::BitOr,
    BinOp::Xor,
];

/// `a op b` for `BINS[op]` (both consumed, so `|`, `-` and `++` can update
/// `a` in place).
extern "C" fn rt_bin(cx: &mut Ctx<'_, '_>, site: u64, op: u64, a: u64, b: u64) -> u64 {
    // SAFETY: compiled code hands over `a` and `b`.
    let a = unsafe { Value::from_raw(a) };
    // SAFETY: as above, for `b`.
    let b = unsafe { Value::from_raw(b) };
    let op = BINS[op as usize % BINS.len()];
    if let (Some(x), Some(y)) = (a.as_small(), b.as_small())
        && let Some(r) = crate::eval::small_int_op(op, x, y)
    {
        return Value::int(r).into_raw();
    }
    match crate::eval::binary_owned(op, a, b) {
        Ok(v) => v.into_raw(),
        Err(m) => {
            let span = cx.site(site).span;
            cx.fail(Error::new(m, span))
        }
    }
}

/// Library function `id` on the `n` arguments in `argv` (consumed), with
/// no named arguments.
extern "C" fn rt_builtin(
    cx: &mut Ctx<'_, '_>,
    site: u64,
    id: u64,
    argv: *const u64,
    n: u64,
) -> u64 {
    // SAFETY: the code generator writes `n` owned words at `argv`.
    let pos: Vec<Value> = unsafe { Words::new(argv, n as usize) }.collect();
    let span = cx.site(site).span;
    match call_builtin(cx.interp(), id as usize, pos, span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// Library function `id` on `pos`, as `exec` calls it.
fn call_builtin(it: &mut Interp<'_>, id: usize, mut pos: Vec<Value>, span: Span) -> Result<Value> {
    let b = &BUILTINS[id];
    if let Some(n) = b.arity {
        crate::eval::spread(&mut pos, n);
    }
    (b.f)(
        it,
        Args {
            pos,
            named: Vec::new(),
            span,
        },
    )
    .map_err(|e| e.at(span))
}

/// `len(v)` (`v` borrowed), for the library's `len` (builtin `id`).
extern "C" fn rt_len(cx: &mut Ctx<'_, '_>, site: u64, id: u64, v: u64) -> u64 {
    // SAFETY: compiled code owns `v` for the duration of the call.
    let v = unsafe { borrow(v) };
    let n = match v.view() {
        V::List(x) => Some(x.len()),
        V::Tuple(x) => Some(x.len()),
        V::Set(x) => Some(x.len()),
        V::Map(x) => Some(x.len()),
        V::Str(s) if s.is_ascii() => Some(s.len()),
        _ => None,
    };
    if let Some(n) = n {
        return Value::int(n as i64).into_raw();
    }
    let span = cx.site(site).span;
    match call_builtin(cx.interp(), id as usize, vec![Value::clone(&v)], span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// `get(coll, k, default)` (all borrowed), for the library's `get`
/// (builtin `id`); `get(coll, k)` passes `none` as the default, which is
/// what the library function gives when there is none.
extern "C" fn rt_get(
    cx: &mut Ctx<'_, '_>,
    site: u64,
    id: u64,
    coll: u64,
    k: u64,
    default: u64,
) -> u64 {
    // SAFETY: compiled code owns `coll`, `k` and `default` for the
    // duration of the call.
    let coll = unsafe { borrow(coll) };
    // SAFETY: as above, for `k`.
    let k = unsafe { borrow(k) };
    // SAFETY: as above, for `default`.
    let default = unsafe { borrow(default) };
    match coll.view() {
        V::Map(m) => {
            return m
                .get(&*k)
                .cloned()
                .unwrap_or_else(|| Value::clone(&default))
                .into_raw();
        }
        V::List(l) => {
            if let Some(i) = k.as_small()
                && let Ok(i) = usize::try_from(i)
            {
                return l
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| Value::clone(&default))
                    .into_raw();
            }
        }
        _ => {}
    }
    let span = cx.site(site).span;
    let args = vec![
        Value::clone(&coll),
        Value::clone(&k),
        Value::clone(&default),
    ];
    match call_builtin(cx.interp(), id as usize, args, span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// `set(coll, k, x)` (all consumed), for the library's `set` (builtin
/// `id`): in place when nothing else holds `coll`.
extern "C" fn rt_set(cx: &mut Ctx<'_, '_>, site: u64, id: u64, coll: u64, k: u64, x: u64) -> u64 {
    // SAFETY: compiled code hands over `coll`, `k` and `x`.
    let mut coll = unsafe { Value::from_raw(coll) };
    // SAFETY: as above.
    let k = unsafe { Value::from_raw(k) };
    // SAFETY: as above.
    let x = unsafe { Value::from_raw(x) };
    let span = cx.site(site).span;
    let fast = match coll.view() {
        V::List(l) => k
            .as_small()
            .and_then(|i| usize::try_from(i).ok())
            .is_some_and(|i| i < l.len()),
        V::Map(_) => true,
        _ => false,
    };
    if fast {
        match coll.make_mut() {
            Some(Heap::List(l)) => {
                if let Some(i) = k.as_small().and_then(|i| usize::try_from(i).ok()) {
                    l.set_mut(i, x);
                }
                return coll.into_raw();
            }
            Some(Heap::Map(m)) => {
                return match crate::value::map_insert(m, k, x) {
                    Ok(()) => coll.into_raw(),
                    Err(e) => cx.fail(Error::new(e, span)),
                };
            }
            _ => return cx.fail(internal("set", span)),
        }
    }
    match call_builtin(cx.interp(), id as usize, vec![coll, k, x], span) {
        Ok(v) => v.into_raw(),
        Err(e) => cx.fail(e),
    }
}

/// A list of the `n` words in `argv` (consumed).
extern "C" fn rt_list(argv: *const u64, n: u64) -> u64 {
    // SAFETY: the code generator writes `n` owned words at `argv`.
    let raw = unsafe { words(argv, n as usize) };
    // SAFETY: each word is handed over once, and taken here once.
    let items = raw.iter().map(|&w| unsafe { Value::from_raw(w) });
    Value::list(crate::list::List::from_exact(items)).into_raw()
}

/// An empty accumulator of kind `k` (0 list, 1 set, 2 map); a list has
/// room for as many items as `hint` (borrowed) has (`interp::size_hint`).
extern "C" fn rt_acc(k: u64, hint: u64) -> u64 {
    use crate::ir::interp::Acc;
    // SAFETY: compiled code owns `hint` (or it is `none`) for the call.
    let hint = unsafe { borrow(hint) };
    let acc = match k {
        0 => Acc::List(Vec::with_capacity(crate::ir::interp::size_hint(&hint))),
        1 => Acc::Set(imbl::OrdSet::new()),
        _ => Acc::Map(imbl::OrdMap::new()),
    };
    Value::internal(crate::value::Internal::Acc(acc)).into_raw()
}

/// Adds `x` to the list or set accumulator `acc` (both consumed); gives
/// the accumulator back.
extern "C" fn rt_push(cx: &mut Ctx<'_, '_>, site: u64, acc: u64, x: u64) -> u64 {
    use crate::ir::interp::Acc;
    use crate::value::Internal;
    // SAFETY: compiled code hands over `acc` and `x`.
    let mut acc = unsafe { Value::from_raw(acc) };
    // SAFETY: as above.
    let x = unsafe { Value::from_raw(x) };
    let span = cx.site(site).span;
    match acc.make_mut() {
        Some(Heap::Internal(Internal::Acc(Acc::List(l)))) => l.push(x),
        Some(Heap::Internal(Internal::Acc(Acc::Set(s)))) => {
            if let Err(e) = crate::value::set_insert(s, x) {
                return cx.fail(Error::new(e, span));
            }
        }
        _ => return cx.fail(internal("push to a non-list, non-set", span)),
    }
    acc.into_raw()
}

/// The collection an accumulator (consumed) built.
extern "C" fn rt_done(acc: u64) -> u64 {
    use crate::ir::interp::Acc;
    use crate::value::{Internal, Own};
    // SAFETY: compiled code hands over the accumulator.
    let acc = unsafe { Value::from_raw(acc) };
    match acc.unpack() {
        Own::Internal(Internal::Acc(Acc::List(l))) => Value::list(l),
        Own::Internal(Internal::Acc(Acc::Set(s))) => Value::set(s),
        Own::Internal(Internal::Acc(Acc::Map(m))) => Value::map(m),
        _ => Value::NONE,
    }
    .into_raw()
}

/// An iterator over `coll` (borrowed); with `finite`, an infinite range
/// is an error.
extern "C" fn rt_iter(cx: &mut Ctx<'_, '_>, site: u64, coll: u64, finite: u64) -> u64 {
    // SAFETY: compiled code owns `coll` for the duration of the call.
    let coll = unsafe { borrow(coll) };
    let span = cx.site(site).span;
    match cx.interp().iter(&coll) {
        Ok(i) if finite != 0 && i.is_infinite() => cx.fail(Error::new(
            "cannot build a collection from an infinite range",
            span,
        )),
        Ok(i) => Value::internal(crate::value::Internal::Iter(Rc::new(
            std::cell::RefCell::new(i),
        )))
        .into_raw(),
        Err(e) => cx.fail(e.at(span)),
    }
}

/// Whether a library call over `coll` with `f` (both borrowed) can be a
/// loop (`Op::Loopable`).
extern "C" fn rt_loopable(coll: u64, f: u64) -> u64 {
    // SAFETY: compiled code owns `coll` for the duration of the call.
    let coll = unsafe { borrow(coll) };
    // SAFETY: as above, for `f`.
    let f = unsafe { borrow(f) };
    Value::bool(crate::ir::interp::loopable(&coll, &f)).into_raw()
}

/// The list `v` (borrowed) without its first `before` and last `after`
/// items (`Op::Middle`).
extern "C" fn rt_middle(cx: &mut Ctx<'_, '_>, site: u64, v: u64, before: u64, after: u64) -> u64 {
    // SAFETY: compiled code owns `v` for the duration of the call.
    let v = unsafe { borrow(v) };
    match crate::ir::interp::middle(&v, before as usize, after as usize) {
        Some(m) => m.into_raw(),
        None => {
            let span = cx.site(site).span;
            cx.fail(internal("middle of a list", span))
        }
    }
}

/// `coll[lo..hi]` (all borrowed). Bit 0 of `flags` says there is a `lo`,
/// bit 1 a `hi`, bit 2 that the slice is inclusive (`..`, not `..<`).
extern "C" fn rt_slice(
    cx: &mut Ctx<'_, '_>,
    site: u64,
    coll: u64,
    lo: u64,
    hi: u64,
    flags: u64,
) -> u64 {
    // SAFETY: compiled code owns `coll` for the duration of the call.
    let coll = unsafe { borrow(coll) };
    // SAFETY: as above, for `lo` (a value; `none` when absent).
    let lo = unsafe { borrow(lo) };
    // SAFETY: as above, for `hi`.
    let hi = unsafe { borrow(hi) };
    let lo = (flags & 1 != 0).then(|| Value::clone(&lo));
    let hi = (flags & 2 != 0).then(|| Value::clone(&hi));
    match crate::builtins::collections::slice(&coll, lo, hi, flags & 4 != 0) {
        Ok(v) => v.into_raw(),
        Err(m) => {
            let span = cx.site(site).span;
            cx.fail(Error::new(m, span))
        }
    }
}

/// Starts iterating over `coll` (borrowed), for a loop whose state
/// compiled code keeps itself. Writes three words at `out`: a word for the
/// loop to hold (owned), and two numbers; returns the mode:
///
/// - 1: a range of small ints: `none`, then the first and last int;
/// - 2: a list or tuple: the collection, then 0 and its length (read with
///   [`rt_seq_get`]);
/// - 0: anything else: the runtime's iterator ([`rt_next`]).
///
/// With `finite`, an infinite range is an error, as for `Op::Iter`.
extern "C" fn rt_iter_init(
    cx: &mut Ctx<'_, '_>,
    site: u64,
    coll: u64,
    finite: u64,
    out: *mut u64,
) -> u64 {
    use crate::int::Int;
    use crate::value::word::{FIX_MAX, FIX_MIN};
    // SAFETY: compiled code owns `coll` for the duration of the call.
    let coll = unsafe { borrow(coll) };
    let (mode, w, a, b) = match coll.view() {
        V::Range(r)
            if matches!((&r.start, &r.end), (Int::Small(s), Some(Int::Small(e)))
                if (FIX_MIN..FIX_MAX).contains(s) && (FIX_MIN..FIX_MAX).contains(e)) =>
        {
            let (Int::Small(s), Some(Int::Small(e))) = (&r.start, &r.end) else {
                return cx.fail(internal("range", cx.site(site).span));
            };
            (1, Value::NONE, *s as u64, *e as u64)
        }
        V::List(l) => (2, Value::clone(&coll), 0, l.len() as u64),
        V::Tuple(t) => (2, Value::clone(&coll), 0, t.len() as u64),
        _ => {
            let span = cx.site(site).span;
            match cx.interp().iter(&coll) {
                Ok(i) if finite != 0 && i.is_infinite() => {
                    return cx.fail(Error::new(
                        "cannot build a collection from an infinite range",
                        span,
                    ));
                }
                Ok(i) => (
                    0,
                    Value::internal(crate::value::Internal::Iter(Rc::new(
                        std::cell::RefCell::new(i),
                    ))),
                    0,
                    0,
                ),
                Err(e) => return cx.fail(e.at(span)),
            }
        }
    };
    // SAFETY: `out` is a three-word stack slot of the caller's (`codegen`'s
    // `iter_slot`), which nothing else uses during this call.
    let slot = unsafe { std::slice::from_raw_parts_mut(out, 3) };
    slot.copy_from_slice(&[w.into_raw(), a, b]);
    mode
}

/// Item `i` of the list or tuple `seq` (borrowed), for loops started by
/// [`rt_iter_init`] in mode 2.
extern "C" fn rt_seq_get(seq: u64, i: u64) -> u64 {
    // SAFETY: the loop holds `seq` while it runs.
    let seq = unsafe { borrow(seq) };
    let i = i as usize;
    match seq.view() {
        V::List(l) => l.get(i).cloned(),
        V::Tuple(t) => t.get(i).cloned(),
        _ => None,
    }
    .unwrap_or(Value::NONE)
    .into_raw()
}

/// Debug builds only: adjusts the live heap word count by `d` (as an `i64`)
/// for references compiled code counted inline.
extern "C" fn rt_live(d: u64) {
    crate::value::word::live::count(d as i64);
}

/// The runtime entry points, for the JIT's symbol table: (name, address,
/// parameter count, whether it returns a word).
pub(super) fn symbols() -> Vec<(&'static str, *const u8, usize, bool)> {
    vec![
        ("rt_dup", rt_dup as *const u8, 1, false),
        ("rt_drop", rt_drop as *const u8, 1, false),
        ("rt_op", rt_op as *const u8, 4, true),
        ("rt_next", rt_next as *const u8, 3, true),
        ("rt_fail", rt_fail as *const u8, 3, true),
        ("rt_trace", rt_trace as *const u8, 6, false),
        ("rt_stack", rt_stack as *const u8, 2, true),
        ("rt_closure", rt_closure as *const u8, 4, true),
        ("rt_tail_target", rt_tail_target as *const u8, 3, true),
        ("rt_call_value", rt_call_value as *const u8, 3, true),
        ("rt_memo_get", rt_memo_get as *const u8, 4, true),
        ("rt_memo_put", rt_memo_put as *const u8, 5, false),
        ("rt_cap", rt_cap as *const u8, 2, true),
        ("rt_recval", rt_recval as *const u8, 2, true),
        ("rt_recparent", rt_recparent as *const u8, 1, true),
        ("rt_force", rt_force as *const u8, 4, true),
        ("rt_thunk_code", rt_thunk_code as *const u8, 3, true),
        ("rt_forced", rt_forced as *const u8, 4, true),
        ("rt_item", rt_item as *const u8, 4, true),
        ("rt_is_tuple", rt_is_tuple as *const u8, 2, true),
        ("rt_is_list", rt_is_list as *const u8, 3, true),
        ("rt_is_type", rt_is_type as *const u8, 2, true),
        ("rt_index", rt_index as *const u8, 4, true),
        ("rt_index_int", rt_index_int as *const u8, 4, true),
        ("rt_cmp", rt_cmp as *const u8, 5, true),
        ("rt_tuple", rt_tuple as *const u8, 2, true),
        ("rt_global", rt_global as *const u8, 3, true),
        ("rt_record", rt_record as *const u8, 5, true),
        ("rt_bin", rt_bin as *const u8, 5, true),
        ("rt_builtin", rt_builtin as *const u8, 5, true),
        ("rt_len", rt_len as *const u8, 4, true),
        ("rt_get", rt_get as *const u8, 6, true),
        ("rt_set", rt_set as *const u8, 6, true),
        ("rt_list", rt_list as *const u8, 2, true),
        ("rt_acc", rt_acc as *const u8, 2, true),
        ("rt_push", rt_push as *const u8, 4, true),
        ("rt_done", rt_done as *const u8, 1, true),
        ("rt_iter", rt_iter as *const u8, 4, true),
        ("rt_live", rt_live as *const u8, 1, false),
        ("rt_loopable", rt_loopable as *const u8, 2, true),
        ("rt_middle", rt_middle as *const u8, 5, true),
        ("rt_slice", rt_slice as *const u8, 6, true),
        ("rt_iter_init", rt_iter_init as *const u8, 5, true),
        ("rt_seq_get", rt_seq_get as *const u8, 2, true),
    ]
}

// ---------------------------------------------------------------------------
// Calls from Rust

/// The lowest address compiled code may take the stack to, on the current
/// stack.
fn stack_limit() -> usize {
    let here = 0u8;
    let sp = std::ptr::addr_of!(here).addr();
    match stacker::remaining_stack() {
        Some(left) => sp.saturating_sub(left) + MARGIN,
        None => 0,
    }
}

/// The signature of the entry shims (`codegen::shim`): context, code,
/// closure, site, arguments.
type Shim = extern "C" fn(*mut Ctx<'_, '_>, *const u8, u64, u64, *const u64) -> u64;

/// Runs `clo`, an IR closure of `native`, on `args` (already checked
/// against its parameters), called at `span`.
pub fn run(
    it: &mut Interp<'_>,
    native: &Rc<Native>,
    clo: Value,
    args: Vec<Value>,
    span: Span,
) -> Result<Value> {
    let V::Func(rc) = clo.view() else {
        return Err(internal("not a function", span));
    };
    let Func::Ir(c) = &**rc else {
        return Err(internal("not an IR closure", span));
    };
    let func = c.func as usize;
    let params = native.module.funcs[func].params as usize;
    if args.len() != params {
        return Err(internal("wrong argument count", span));
    }
    let code = native.code[func];
    let shim = native.shims[params];
    // A function capturing just its record is given the record itself.
    let clo = if super::record_clo(&native.module.funcs[func]) && c.caps.len() == 1 {
        c.caps[0].clone()
    } else {
        clo
    };
    let mut buf = [0u64; 8];
    let mut long = Vec::new();
    let argv: &mut [u64] = if params <= buf.len() {
        &mut buf[..params]
    } else {
        long.resize(params, 0);
        &mut long
    };
    for (w, a) in argv.iter_mut().zip(args) {
        *w = a.into_raw();
    }
    let clo = clo.into_raw();
    let mut call = || {
        let mut cx = Ctx {
            limit: stack_limit(),
            it,
            native,
            span,
            pending: None,
        };
        // SAFETY: `shims[params]` was compiled (`codegen::shim`) with the
        // `Shim` signature, for functions of `params` parameters, and
        // `code` is such a function; `native` (borrowed for this whole
        // call) owns the memory of both. The shim passes on the context,
        // closure and arguments, whose words it is given here, and `argv`
        // holds the `params` words it reads.
        let enter = unsafe { std::mem::transmute::<*const u8, Shim>(shim) };
        let r = enter(&mut cx, code, clo, ENTRY, argv.as_ptr());
        if r == ERR {
            return Err(cx
                .pending
                .take()
                .unwrap_or_else(|| internal("error without a message", span)));
        }
        // SAFETY: compiled code returns an owned word, or `ERR`.
        Ok(unsafe { Value::from_raw(r) })
    };
    // Grow the stack only when it's short, without `maybe_grow`'s closure
    // plumbing on the common path.
    match stacker::remaining_stack() {
        Some(left) if left >= RED_ZONE => call(),
        _ => stacker::grow(SEGMENT, call),
    }
}

/// `TIN_PROF=1`: counts the instructions that go through the generic
/// [`rt_op`] path, by kind, and prints the commonest after `tinsel run`.
/// For finding what to give a direct path next.
pub mod prof {
    use crate::ir::Op;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static COUNTS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    }

    fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("TIN_PROF").is_some())
    }

    pub(super) fn count(op: &Op) {
        if !on() {
            return;
        }
        let k = match op {
            Op::CallBuiltin { id, .. } => {
                format!("call {}", crate::builtins::BUILTINS[*id].name)
            }
            Op::Bin(o, _, _) => format!("binary {}", o.symbol()),
            Op::Cmp(o, _, _) => format!("compare {}", o.symbol()),
            other => {
                let shown = other.to_string();
                shown.split_whitespace().next().unwrap_or("?").to_string()
            }
        };
        COUNTS.with(|c| *c.borrow_mut().entry(k).or_default() += 1);
    }

    /// Prints the counts (only with `TIN_PROF`).
    pub fn dump() {
        if !on() {
            return;
        }
        COUNTS.with(|c| {
            let mut v: Vec<_> = c.borrow().iter().map(|(k, n)| (*n, k.clone())).collect();
            v.sort_by(|a, b| b.cmp(a));
            for (n, k) in v.iter().take(25) {
                eprintln!("{n:>12} {k}");
            }
        });
    }
}
