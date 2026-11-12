//! Tree-walking evaluator (SPEC 8).
//!
//! - Frames hold slots for parameters, pattern bindings and `where`
//!   definitions. `where` constants are lazy thunks in their slots.
//! - Calls in tail position return [`Flow::Tail`] to the call loop, which
//!   runs them without growing the native stack (a trampoline).
//! - Non-tail calls go through `stacker::maybe_grow`, so deep recursion
//!   works.

use crate::ast::*;
use crate::builtins::{self, Args, BUILTINS};
use crate::error::{Error, Result, Span};
use crate::fxhash::FxHashMap;
use crate::int::Int;
use crate::resolve::{GlobalDef, Program};
use crate::value::{self, Func, Heap, List, Own, Range, V, Value, compare};
use crate::value::{ValueMap, ValueSet};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

const STACK_RED_ZONE: usize = 256 * 1024;
const STACK_GROW: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Frames

#[derive(Clone)]
pub enum Slot {
    Empty,
    Val(Value),
    /// A lazy `where` constant.
    Thunk(Rc<ConstDef>),
    Evaluating,
    /// A `where` function, with its per-call memo table if memoized.
    LocalFn(Rc<FnDef>, Option<Rc<Memo>>),
}

pub struct Frame {
    slots: RefCell<Vec<Slot>>,
    parent: Option<Rc<Frame>>,
}

impl Frame {
    pub fn new(size: usize, parent: Option<Rc<Frame>>) -> Rc<Frame> {
        Rc::new(Frame {
            slots: RefCell::new(vec![Slot::Empty; size]),
            parent,
        })
    }

    pub fn set(&self, slot: usize, v: Slot) {
        if let Some(s) = self.slots.borrow_mut().get_mut(slot) {
            *s = v;
        }
    }

    fn get(&self, slot: usize) -> Slot {
        self.slots
            .borrow()
            .get(slot)
            .cloned()
            .unwrap_or(Slot::Empty)
    }
}

#[derive(Default)]
pub struct Memo {
    pub(crate) table: RefCell<FxHashMap<Vec<Value>, Value>>,
}

// ---------------------------------------------------------------------------
// Generators

pub struct Generator {
    kind: GenKind,
    /// Generators are single-pass: only one operation may read one (SPEC 4).
    used: bool,
}

enum GenKind {
    Comp {
        comp: Rc<Comp>,
        root: Rc<Frame>,
        levels: Vec<GenLevel>,
        started: bool,
    },
    Iterate {
        cur: Value,
        f: Value,
        first: bool,
    },
    /// `map`, `filter`, `enumerate` and `zip` over a generator or an
    /// infinite range: computed one element at a time (SPEC 9.2).
    Map {
        src: ValIter,
        f: Value,
        span: Span,
    },
    Filter {
        src: ValIter,
        f: Value,
        span: Span,
    },
    Enumerate {
        src: ValIter,
        next: i64,
    },
    Zip {
        srcs: Vec<ValIter>,
    },
    /// The items of `f(x)` for each `x` of `src`, in turn: how the IR
    /// lowers a generator comprehension.
    FlatMap {
        src: ValIter,
        f: Value,
        cur: Option<ValIter>,
        span: Span,
    },
    /// Stands in for one of the above while it computes an element, so
    /// that reading the generator again from inside is an error, not a
    /// panic.
    Busy,
}

struct GenLevel {
    clause: usize,
    iter: ValIter,
    parent: Rc<Frame>,
}

impl Generator {
    fn new(kind: GenKind) -> Generator {
        Generator { kind, used: false }
    }

    pub fn map(src: ValIter, f: Value, span: Span) -> Generator {
        Generator::new(GenKind::Map { src, f, span })
    }

    pub fn filter(src: ValIter, f: Value, span: Span) -> Generator {
        Generator::new(GenKind::Filter { src, f, span })
    }

    pub fn enumerate(src: ValIter) -> Generator {
        Generator::new(GenKind::Enumerate { src, next: 0 })
    }

    pub fn zip(srcs: Vec<ValIter>) -> Generator {
        Generator::new(GenKind::Zip { srcs })
    }

    pub fn flat_map(src: ValIter, f: Value, span: Span) -> Generator {
        Generator::new(GenKind::FlatMap {
            src,
            f,
            cur: None,
            span,
        })
    }

    pub fn iterate(start: Value, f: Value) -> Generator {
        Generator {
            kind: GenKind::Iterate {
                cur: start,
                f,
                first: true,
            },
            used: false,
        }
    }
}

/// An iterator over the elements of any iterable value. It walks the
/// collection lazily rather than copying it, so stopping early (`first`,
/// `any`, `index`, ...) costs only the elements visited.
pub enum ValIter {
    /// A list, consumed from a (cheap, shared) clone.
    List {
        items: List,
        pos: usize,
    },
    /// A tuple value, and the next position in it.
    Tuple {
        tuple: Value,
        pos: usize,
    },
    /// Set elements or map keys in order, copied out in chunks (see
    /// `Chunks`).
    Set(Chunks<ValueSet>),
    MapKeys(Chunks<ValueMap>),
    Range {
        cur: Int,
        end: Option<Int>,
    },
    Chars {
        s: Rc<str>,
        pos: usize,
    },
    Gen(Rc<RefCell<Generator>>),
    /// The ints of a rangeset: the span being read, and the next int in it.
    RangeSet {
        set: Rc<value::RangeSet>,
        span: usize,
        next: Option<Int>,
    },
}

/// Iteration over a sorted set or map's elements. Each refill copies the
/// next chunk of elements with one range search, so a whole pass is close
/// to linear. Chunks start small and double, so stopping early (`any`,
/// `index`, `take`) copies at most about twice the elements visited.
pub struct Chunks<C> {
    coll: C,
    buf: Vec<Value>,
    pos: usize,
    size: usize,
    done: bool,
}

const FIRST_CHUNK: usize = 32;
const MAX_CHUNK: usize = 4096;

impl<C> Chunks<C> {
    fn new(coll: C) -> Chunks<C> {
        Chunks {
            coll,
            buf: Vec::new(),
            pos: 0,
            size: FIRST_CHUNK,
            done: false,
        }
    }

    /// The next element. `fill(coll, after, n, out)` appends up to `n`
    /// elements following `after` (from the start if `None`) to `out`.
    fn next(&mut self, fill: impl Fn(&C, Option<&Value>, usize, &mut Vec<Value>)) -> Option<Value> {
        if self.pos == self.buf.len() {
            if self.done {
                return None;
            }
            let after = self.buf.pop();
            self.buf.clear();
            fill(&self.coll, after.as_ref(), self.size, &mut self.buf);
            if self.buf.len() < self.size {
                self.done = true;
            }
            self.size = (self.size * 2).min(MAX_CHUNK);
            self.pos = 0;
        }
        let len = self.buf.len();
        let slot = self.buf.get_mut(self.pos)?;
        self.pos += 1;
        // The last element of a chunk stays put: the next refill starts
        // after it. The others are moved out rather than cloned.
        Some(if self.pos == len {
            slot.clone()
        } else {
            std::mem::replace(slot, Value::NONE)
        })
    }
}

impl ValIter {
    pub fn next(&mut self, it: &mut Interp) -> Result<Option<Value>> {
        match self {
            ValIter::List { items, pos } => {
                let v = items.get(*pos).cloned();
                *pos += 1;
                Ok(v)
            }
            ValIter::Tuple { tuple, pos } => {
                let V::Tuple(items) = tuple.view() else {
                    return Ok(None);
                };
                let v = items.get(*pos).cloned();
                *pos += 1;
                Ok(v)
            }
            ValIter::Set(c) => Ok(c.next(|set, after, n, out| match after {
                None => out.extend(set.iter().take(n).cloned()),
                Some(l) => out.extend(set.iter_after(l).take(n).cloned()),
            })),
            ValIter::MapKeys(c) => Ok(c.next(|map, after, n, out| match after {
                None => out.extend(map.keys().take(n).cloned()),
                Some(l) => out.extend(map.iter_after(l).take(n).map(|(k, _)| k.clone())),
            })),
            ValIter::Range { cur, end } => {
                if end.as_ref().is_some_and(|e| *cur > *e) {
                    return Ok(None);
                }
                let v = cur.clone();
                *cur = cur.add(&Int::Small(1)).map_err(Error::msg)?;
                Ok(Some(Value::from_int(v)))
            }
            ValIter::Chars { s, pos } => match s[*pos..].chars().next() {
                Some(c) => {
                    *pos += c.len_utf8();
                    Ok(Some(Value::char(c)))
                }
                None => Ok(None),
            },
            ValIter::Gen(g) => it.gen_next(g),
            ValIter::RangeSet { set, span, next } => {
                while let Some((lo, hi)) = set.spans.get(*span) {
                    let cur = next.take().unwrap_or_else(|| lo.clone());
                    if cur <= *hi {
                        *next = Some(cur.add(&Int::Small(1)).map_err(Error::msg)?);
                        return Ok(Some(Value::from_int(cur)));
                    }
                    *span += 1;
                }
                Ok(None)
            }
        }
    }

    pub fn is_infinite(&self) -> bool {
        match self {
            ValIter::Range { end, .. } => end.as_ref().is_none_or(Int::is_inf),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Interpreter

type NamedArgs = Vec<(Name, Value)>;

/// Result of evaluating an expression in tail position.
pub enum Flow {
    Val(Value),
    Tail(Value, Vec<Value>, Span),
}

enum GSlot {
    Thunk(Rc<ConstDef>, usize),
    Evaluating,
    Val(Value),
}

pub struct Interp<'p> {
    pub prog: &'p Program,
    globals: Vec<GSlot>,
    /// The puzzle input, or why there isn't one.
    pub input: std::result::Result<Rc<str>, String>,
    /// Set when the program asked for `input` and there wasn't one, so that
    /// checks needing the real input can be skipped rather than failed.
    pub input_missing: bool,
    /// Constants currently being evaluated, for cycle reports.
    const_stack: Vec<Rc<ConstDef>>,
    builtin_values: Vec<Option<Value>>,
    /// Frames from finished calls that nothing captured, kept for reuse so
    /// that most calls don't allocate.
    frame_pool: Vec<Rc<Frame>>,
    /// Emptied argument vectors, kept for reuse.
    arg_pool: Vec<Vec<Value>>,
    /// Closed lambdas (`Lambda::closed`), built on first use.
    closed_lambdas: Vec<Option<Value>>,
    /// Set when the program runs on the IR interpreter instead
    /// (`crate::ir::interp`).
    pub ir: Option<Box<crate::ir::interp::Runtime>>,
}

/// A memo key holding more values than this gets a warning (SPEC 8.1).
pub const MEMO_WARN_SIZE: usize = 1000;

/// Whether `v` holds at most `budget` values, counting every value inside
/// collections. Stops as soon as the budget runs out, so it costs no more
/// than hashing the value would.
fn within(v: &Value, budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    match v.view() {
        V::Tuple(xs) => xs.iter().all(|x| within(x, budget)),
        V::List(xs) => xs.iter().all(|x| within(x, budget)),
        V::Set(s) => s.iter().all(|x| within(x, budget)),
        V::Map(m) => m
            .iter()
            .all(|(k, x)| within(k, budget) && within(x, budget)),
        V::Grid(g) => g.cells.iter().all(|x| within(x, budget)),
        V::RangeSet(r) => {
            let n = r.spans.len();
            *budget = budget.saturating_sub(n);
            *budget > 0 || n == 0
        }
        _ => true,
    }
}

/// How many spare frames to keep.
const FRAME_POOL_MAX: usize = 256;

impl<'p> Interp<'p> {
    pub fn new(
        prog: &'p Program,
        input: std::result::Result<Rc<str>, String>,
        overrides: &[(Name, Rc<ConstDef>, Span)],
    ) -> Interp<'p> {
        let mut globals: Vec<GSlot> = prog
            .globals
            .iter()
            .map(|g| match g {
                GlobalDef::Const { def, slot } => GSlot::Thunk(def.clone(), *slot),
                GlobalDef::Func(def) => {
                    let memo = prog.memo[def.id].then(|| Rc::new(Memo::default()));
                    GSlot::Val(Value::func_rc(Rc::new(Func::User {
                        def: def.clone(),
                        env: None,
                        memo,
                    })))
                }
            })
            .collect();
        for (name, def, _) in overrides {
            if let Some(i) = prog.global(name) {
                globals[i] = GSlot::Thunk(def.clone(), 0);
            }
        }
        Interp {
            prog,
            globals,
            input,
            input_missing: false,
            const_stack: Vec::new(),
            builtin_values: vec![None; BUILTINS.len()],
            frame_pool: Vec::new(),
            arg_pool: Vec::new(),
            closed_lambdas: vec![None; prog.closed_lambdas],
            ir: None,
        }
    }

    /// A lambda that uses no enclosing locals, built once. Its environment
    /// is an empty frame, since the body never looks outside its own.
    fn closed_lambda(&mut self, lam: &Rc<Lambda>, id: usize) -> Value {
        if let Some(Some(v)) = self.closed_lambdas.get(id) {
            return v.clone();
        }
        let v = Value::func_rc(Rc::new(Func::Lambda {
            lam: lam.clone(),
            env: Frame::new(0, None),
        }));
        if let Some(slot) = self.closed_lambdas.get_mut(id) {
            *slot = Some(v.clone());
        }
        v
    }

    /// An empty vector for a call's arguments.
    fn arg_vec(&mut self) -> Vec<Value> {
        self.arg_pool.pop().unwrap_or_default()
    }

    /// Returns a call's argument vector to the pool.
    fn recycle_args(&mut self, mut args: Vec<Value>) {
        if self.arg_pool.len() < FRAME_POOL_MAX && args.capacity() <= 16 {
            args.clear();
            self.arg_pool.push(args);
        }
    }

    /// A frame with `size` empty slots, reusing a pooled one if possible.
    fn new_frame(&mut self, size: usize, parent: Option<Rc<Frame>>) -> Rc<Frame> {
        if let Some(mut frame) = self.frame_pool.pop()
            && let Some(f) = Rc::get_mut(&mut frame)
        {
            f.parent = parent;
            f.slots.get_mut().resize(size, Slot::Empty);
            return frame;
        }
        Frame::new(size, parent)
    }

    /// Puts a finished frame back in the pool, if nothing else holds it.
    /// A frame captured by a lambda, generator or `where` function is left
    /// to be freed normally when its last holder goes.
    fn recycle_frame(&mut self, mut frame: Rc<Frame>) {
        let Some(f) = Rc::get_mut(&mut frame) else {
            return;
        };
        // Free the contents now, as dropping the frame would have.
        let parent = f.parent.take();
        f.slots.get_mut().clear();
        drop(parent);
        if self.frame_pool.len() < FRAME_POOL_MAX {
            self.frame_pool.push(frame);
        }
    }

    /// `release_frame`, then recycling.
    fn finish_frame(&mut self, frame: Rc<Frame>) {
        release_frame(&frame);
        self.recycle_frame(frame);
    }

    /// Value of a top-level name, if it exists.
    pub fn global_by_name(&mut self, name: &str, span: Span) -> Option<Result<Value>> {
        let i = self.prog.global(name)?;
        if self.ir.is_some() {
            return Some(crate::ir::interp::global(self, i, span));
        }
        Some(self.global(i, span))
    }

    fn cycle_error(&self, name: &str, span: Span) -> Error {
        let described: Vec<String> = self.const_stack.iter().map(|c| c.describe()).collect();
        let start = described
            .iter()
            .position(|c| c.split(", ").any(|n| n == name))
            .unwrap_or(0);
        let mut chain: Vec<&str> = described[start..].iter().map(String::as_str).collect();
        chain.push(name);
        Error::new(
            format!(
                "constant `{name}` depends on itself: {}",
                chain.join(" -> ")
            ),
            span,
        )
    }

    #[inline(never)]
    fn global(&mut self, i: usize, span: Span) -> Result<Value> {
        let def = match &self.globals[i] {
            GSlot::Val(v) => return Ok(v.clone()),
            GSlot::Evaluating => return Err(self.cycle_error(&self.prog.global_names[i], span)),
            GSlot::Thunk(def, _) => def.clone(),
        };
        // All globals bound by this definition (more than one for patterns).
        let members: Vec<(usize, usize)> = self
            .globals
            .iter()
            .enumerate()
            .filter_map(|(j, g)| match g {
                GSlot::Thunk(d, s) if Rc::ptr_eq(d, &def) => Some((j, *s)),
                _ => None,
            })
            .collect();
        for &(j, _) in &members {
            self.globals[j] = GSlot::Evaluating;
        }
        let frame = Frame::new(def.frame_size, None);
        self.init_wheres(&def.wheres, &frame);
        self.const_stack.push(def.clone());
        let result = self.eval(&def.body, &frame);
        self.const_stack.pop();
        let result = result.and_then(|v| {
            if match_pattern(&def.pat, &v, &frame) {
                Ok(())
            } else {
                Err(Error::new(
                    format!(
                        "expected a value matching `{}`, found {}",
                        self.source_of(def.pat.span),
                        short(&v)
                    ),
                    def.pat.span,
                ))
            }
        });
        if let Err(e) = result {
            for &(j, s) in &members {
                self.globals[j] = GSlot::Thunk(def.clone(), s);
            }
            return Err(e);
        }
        for &(j, s) in &members {
            let v = match frame.get(s) {
                Slot::Val(v) => v,
                _ => Value::NONE,
            };
            self.globals[j] = GSlot::Val(v);
        }
        match &self.globals[i] {
            GSlot::Val(v) => Ok(v.clone()),
            _ => Err(Error::new("internal: global not set", span)),
        }
    }

    fn init_wheres(&self, w: &Wheres, frame: &Rc<Frame>) {
        for c in &w.consts {
            c.pat
                .bindings(&mut |b| frame.set(b.slot, Slot::Thunk(c.clone())));
            self.init_wheres(&c.wheres, frame);
        }
        for (f, slot) in &w.fns {
            let memo = self.prog.memo[f.id].then(|| Rc::new(Memo::default()));
            frame.set(*slot, Slot::LocalFn(f.clone(), memo));
        }
    }

    #[inline(never)]
    fn local(
        &mut self,
        frame: &Rc<Frame>,
        depth: usize,
        slot: usize,
        name: &str,
        span: Span,
    ) -> Result<Value> {
        let mut f = frame;
        for _ in 0..depth {
            f = f
                .parent
                .as_ref()
                .ok_or_else(|| Error::new("internal: bad frame depth", span))?;
        }
        // The common case: copy the value out without copying the slot.
        if let Some(Slot::Val(v)) = f.slots.borrow().get(slot) {
            return Ok(v.clone());
        }
        match f.get(slot) {
            Slot::Val(v) => Ok(v),
            Slot::Thunk(def) => {
                let f = f.clone();
                self.force_local(&f, &def, slot, span)
            }
            Slot::Evaluating => Err(self.cycle_error(name, span)),
            Slot::LocalFn(def, memo) => Ok(Value::func_rc(Rc::new(Func::User {
                def,
                env: Some(f.clone()),
                memo,
            }))),
            Slot::Empty => Err(Error::new(
                format!("internal: `{name}` used before it was bound"),
                span,
            )),
        }
    }

    /// A local's last use (see `moves`): takes its value out of the frame,
    /// so that the frame no longer shares it.
    #[inline(never)]
    fn take_local(
        &mut self,
        frame: &Rc<Frame>,
        slot: usize,
        name: &str,
        span: Span,
    ) -> Result<Value> {
        if let Some(s) = frame.slots.borrow_mut().get_mut(slot)
            && let Slot::Val(_) = s
            && let Slot::Val(v) = std::mem::replace(s, Slot::Empty)
        {
            return Ok(v);
        }
        // A `where` constant not yet evaluated, or a `where` function.
        let v = self.local(frame, 0, slot, name, span)?;
        if let Some(s) = frame.slots.borrow_mut().get_mut(slot)
            && let Slot::Val(_) = s
        {
            *s = Slot::Empty;
        }
        Ok(v)
    }

    fn force_local(
        &mut self,
        frame: &Rc<Frame>,
        def: &Rc<ConstDef>,
        slot: usize,
        span: Span,
    ) -> Result<Value> {
        let mut slots = Vec::new();
        def.pat.bindings(&mut |b| slots.push(b.slot));
        for &s in &slots {
            frame.set(s, Slot::Evaluating);
        }
        self.const_stack.push(def.clone());
        let result = self.eval(&def.body, frame);
        self.const_stack.pop();
        let result = result.and_then(|v| {
            if match_pattern(&def.pat, &v, frame) {
                Ok(())
            } else {
                Err(Error::new(
                    format!(
                        "expected a value matching `{}`, found {}",
                        self.source_of(def.pat.span),
                        short(&v)
                    ),
                    def.pat.span,
                ))
            }
        });
        if let Err(e) = result {
            for &s in &slots {
                frame.set(s, Slot::Thunk(def.clone()));
            }
            return Err(e);
        }
        match frame.get(slot) {
            Slot::Val(v) => Ok(v),
            _ => Err(Error::new("internal: local constant not set", span)),
        }
    }

    /// The source text of a span, for quoting patterns in errors.
    fn source_of(&self, span: Span) -> &str {
        self.prog.src.get(span.start..span.end).unwrap_or("?")
    }

    #[inline(never)]
    pub(crate) fn builtin_value(&mut self, id: usize, span: Span) -> Result<Value> {
        let b = &BUILTINS[id];
        if b.constant {
            return (b.f)(
                self,
                Args {
                    pos: Vec::new(),
                    named: Vec::new(),
                    span,
                },
            );
        }
        if let Some(v) = &self.builtin_values[id] {
            return Ok(v.clone());
        }
        let v = Value::func_rc(Rc::new(Func::Builtin(id)));
        self.builtin_values[id] = Some(v.clone());
        Ok(v)
    }

    // --- Expressions ----------------------------------------------------------

    /// Evaluates an expression.
    ///
    /// This is a thin dispatcher: every non-trivial case lives in its own
    /// `#[inline(never)]` function. Rust reserves stack for the locals of
    /// every arm of a `match`, so one big function would make each level of
    /// Tinsel recursion pay for the largest case (SPEC 8.2).
    pub fn eval(&mut self, e: &Expr, frame: &Rc<Frame>) -> Result<Value> {
        match &e.kind {
            ExprKind::Lit(v) => Ok(v.clone()),
            ExprKind::Local {
                depth,
                slot,
                name,
                moved,
            } => {
                if *moved {
                    self.take_local(frame, *slot, name, e.span)
                } else {
                    self.local(frame, *depth, *slot, name, e.span)
                }
            }
            ExprKind::Global(i) => self.global(*i, e.span),
            ExprKind::Call { func, args, .. } => self.eval_call(func, args, frame, e.span),
            ExprKind::Binary(op, a, b) => self.eval_binary(*op, a, b, frame, e.span),
            ExprKind::Compare(first, rest) => self.eval_compare(first, rest, frame, e.span),
            ExprKind::If(..) | ExprKind::Match(..) => self.eval_branch(e, frame),
            _ => self.eval_other(e, frame),
        }
    }

    #[inline(never)]
    fn eval_call(
        &mut self,
        func: &Expr,
        args: &[Arg],
        frame: &Rc<Frame>,
        span: Span,
    ) -> Result<Value> {
        if let ExprKind::Builtin(id) = func.kind
            && !BUILTINS[id].constant
        {
            return self.direct_builtin_call(id, args, frame, span);
        }
        if let Some((f, pos)) = self.known_user_call(func, args, frame)? {
            return self.call_named(f, pos, Vec::new(), span);
        }
        let (f, pos, named) = self.eval_call_parts(func, args, frame)?;
        self.call_named(f, pos, named, span)
    }

    /// For a call to a top-level function with the right number of
    /// positional arguments: the function and the evaluated arguments.
    #[inline(never)]
    fn known_user_call(
        &mut self,
        func: &Expr,
        args: &[Arg],
        frame: &Rc<Frame>,
    ) -> Result<Option<(Value, Vec<Value>)>> {
        let ExprKind::Global(i) = func.kind else {
            return Ok(None);
        };
        let f = match &self.globals[i] {
            GSlot::Val(f)
                if matches!(f.view(), V::Func(rc)
                        if matches!(&**rc, Func::User { def, .. } if def.arity == args.len()))
                    && args.iter().all(|a| a.name.is_none()) =>
            {
                f.clone()
            }
            _ => return Ok(None),
        };
        let mut pos = self.arg_vec();
        for a in args {
            pos.push(self.eval(&a.value, frame)?);
        }
        Ok(Some((f, pos)))
    }

    /// A call to a library function named directly, as in `xs.len` or
    /// `seen.set(k, v)`. The function is known statically, so this skips
    /// making a function value and the general dispatch. It also skips the
    /// stack check: a library function that calls back into Tinsel code does
    /// so through `call`, which makes that check itself.
    #[inline(never)]
    fn direct_builtin_call(
        &mut self,
        id: usize,
        args: &[Arg],
        frame: &Rc<Frame>,
        span: Span,
    ) -> Result<Value> {
        let mut pos = self.arg_vec();
        let mut named = Vec::new();
        for a in args {
            let v = self.eval(&a.value, frame)?;
            match &a.name {
                Some(n) => named.push((n.clone(), v)),
                None => pos.push(v),
            }
        }
        let b = &BUILTINS[id];
        if let Some(n) = b.arity {
            spread(&mut pos, n);
        }
        (b.f)(self, Args { pos, named, span }).map_err(|e| e.at(span))
    }

    #[inline(never)]
    fn eval_binary(
        &mut self,
        op: BinOp,
        a: &Expr,
        b: &Expr,
        frame: &Rc<Frame>,
        span: Span,
    ) -> Result<Value> {
        let av = self.eval(a, frame)?;
        let bv = self.eval(b, frame)?;
        if let (V::Int(Int::Small(x)), V::Int(Int::Small(y))) = (av.view(), bv.view())
            && let Some(r) = small_int_op(op, x, y)
        {
            return Ok(Value::from_int(Int::Small(r)));
        }
        binary_owned(op, av, bv).map_err(|m| Error::new(m, span))
    }

    #[inline(never)]
    fn eval_compare(
        &mut self,
        first: &Expr,
        rest: &[(CmpOp, Expr)],
        frame: &Rc<Frame>,
        span: Span,
    ) -> Result<Value> {
        let mut left = self.eval(first, frame)?;
        for (op, x) in rest {
            let right = self.eval(x, frame)?;
            let holds = match (left.view(), right.view()) {
                (V::Int(Int::Small(a)), V::Int(Int::Small(b))) => match op {
                    CmpOp::Eq => a == b,
                    CmpOp::Ne => a != b,
                    CmpOp::Lt => a < b,
                    CmpOp::Le => a <= b,
                    CmpOp::Gt => a > b,
                    CmpOp::Ge => a >= b,
                    CmpOp::In | CmpOp::NotIn => self
                        .compare_op(*op, &left, &right)
                        .map_err(|m| Error::new(m, span))?,
                },
                _ => self
                    .compare_op(*op, &left, &right)
                    .map_err(|m| Error::new(m, span))?,
            };
            if !holds {
                return Ok(Value::bool(false));
            }
            left = right;
        }
        Ok(Value::bool(true))
    }

    #[inline(never)]
    fn eval_branch(&mut self, e: &Expr, frame: &Rc<Frame>) -> Result<Value> {
        let branch = self.select_branch(e, frame)?;
        self.eval(branch, frame)
    }

    /// The less common cases.
    #[inline(never)]
    fn eval_other(&mut self, e: &Expr, frame: &Rc<Frame>) -> Result<Value> {
        match &e.kind {
            ExprKind::Builtin(id) => self.builtin_value(*id, e.span),
            ExprKind::Interp(parts) => {
                let mut s = String::new();
                for p in parts {
                    match p {
                        InterpPart::Lit(l) => s.push_str(l),
                        InterpPart::Expr(x) => s.push_str(&self.eval(x, frame)?.display()),
                    }
                }
                Ok(Value::from_str_rc(s.into()))
            }
            ExprKind::Tuple(xs) => {
                let mut items = self.arg_vec();
                for x in xs {
                    items.push(self.eval(x, frame)?);
                }
                let t = Value::tuple_drain(&mut items);
                self.recycle_args(items);
                Ok(t)
            }
            ExprKind::List(xs) => {
                let mut v = Vec::new();
                for x in xs {
                    v.push(self.eval(x, frame)?);
                }
                Ok(Value::list(v))
            }
            ExprKind::Set(xs) => {
                let mut s = ValueSet::new();
                for x in xs {
                    let v = self.eval(x, frame)?;
                    value::set_insert(&mut s, v).map_err(|m| Error::new(m, x.span))?;
                }
                Ok(Value::set(s))
            }
            ExprKind::Map(entries) => {
                let mut m = ValueMap::new();
                for (k, v) in entries {
                    let kv = self.eval(k, frame)?;
                    let vv = self.eval(v, frame)?;
                    value::map_insert(&mut m, kv, vv).map_err(|msg| Error::new(msg, k.span))?;
                }
                Ok(Value::map(m))
            }
            ExprKind::Comp(comp) => self.comprehension(comp, frame, e.span),
            ExprKind::TupleIndex(x, i) => {
                let v = self.eval(x, frame)?;
                builtins::collections::index_int(&v, *i as i64).map_err(|m| Error::new(m, e.span))
            }
            ExprKind::Index(x, idx) => {
                let v = self.eval(x, frame)?;
                if let ExprKind::Range { lo, hi, inclusive } = &idx.kind {
                    let lo = match lo {
                        Some(lo) => Some(self.eval(lo, frame)?),
                        None => None,
                    };
                    let hi = match hi {
                        Some(hi) => Some(self.eval(hi, frame)?),
                        None => None,
                    };
                    return builtins::collections::slice(&v, lo, hi, *inclusive)
                        .map_err(|m| Error::new(m, e.span));
                }
                let i = self.eval(idx, frame)?;
                builtins::collections::index(&v, &i).map_err(|m| Error::new(m, e.span))
            }
            ExprKind::Unary(op, x) => {
                let v = self.eval(x, frame)?;
                match op {
                    UnOp::Neg => negate(&v).map_err(|m| Error::new(m, e.span)),
                    UnOp::Not => match v.view() {
                        V::Bool(b) => Ok(Value::bool(!b)),
                        other => Err(Error::new(
                            format!("`not` needs a bool, found {}", other.type_name()),
                            e.span,
                        )),
                    },
                    UnOp::BitNot => match v.view() {
                        V::Int(n) if !n.is_inf() => n
                            .neg()
                            .sub(&Int::Small(1))
                            .map(Value::from_int)
                            .map_err(|m| Error::new(m, e.span)),
                        other => Err(Error::new(
                            format!("`~` needs an int, found {}", other.type_name()),
                            e.span,
                        )),
                    },
                }
            }
            ExprKind::And(a, b) => {
                if !self.eval_bool(a, frame, "`and`")? {
                    return Ok(Value::bool(false));
                }
                Ok(Value::bool(self.eval_bool(b, frame, "`and`")?))
            }
            ExprKind::Or(a, b) => {
                if self.eval_bool(a, frame, "`or`")? {
                    return Ok(Value::bool(true));
                }
                Ok(Value::bool(self.eval_bool(b, frame, "`or`")?))
            }
            ExprKind::Coalesce(a, b) => match self.eval(a, frame)? {
                v if v.is_none() => self.eval(b, frame),
                v => Ok(v),
            },
            ExprKind::Lambda(lam) => match lam.closed {
                Some(id) => Ok(self.closed_lambda(lam, id)),
                None => Ok(Value::func_rc(Rc::new(Func::Lambda {
                    lam: lam.clone(),
                    env: frame.clone(),
                }))),
            },
            ExprKind::Range { lo, hi, inclusive } => {
                let Some(lo) = lo else {
                    return Err(Error::new(
                        "a range needs a start (open-start ranges are only for slicing)",
                        e.span,
                    ));
                };
                let start = self.eval_int(lo, frame, "range start")?;
                let end = match hi {
                    None => None,
                    Some(hi) => {
                        let h = self.eval_int(hi, frame, "range end")?;
                        Some(if *inclusive {
                            h
                        } else {
                            h.sub(&Int::Small(1)).map_err(|m| Error::new(m, e.span))?
                        })
                    }
                };
                Ok(Value::range_rc(Rc::new(Range {
                    start,
                    end,
                    exclusive: hi.is_some() && !*inclusive,
                })))
            }
            ExprKind::Name(n) => Err(Error::new(
                format!("internal: unresolved name `{n}`"),
                e.span,
            )),
            // Handled by `eval`.
            ExprKind::Lit(_)
            | ExprKind::Local { .. }
            | ExprKind::Global(_)
            | ExprKind::Call { .. }
            | ExprKind::Binary(..)
            | ExprKind::Compare(..)
            | ExprKind::If(..)
            | ExprKind::Match(..) => self.eval(e, frame),
        }
    }

    /// Evaluates `e` in tail position: `if` and `match` branches are
    /// followed in a loop, and a call to a Tinsel function is returned to
    /// the trampoline instead of being made.
    fn eval_tail_loop(&mut self, mut e: &Expr, frame: &Rc<Frame>) -> Result<Flow> {
        loop {
            match &e.kind {
                ExprKind::Call { func, args, .. } => {
                    return self.tail_call(func, args, frame, e.span);
                }
                ExprKind::If(..) | ExprKind::Match(..) => e = self.select_branch(e, frame)?,
                _ => return Ok(Flow::Val(self.eval(e, frame)?)),
            }
        }
    }

    #[inline(never)]
    fn tail_call(
        &mut self,
        func: &Expr,
        args: &[Arg],
        frame: &Rc<Frame>,
        span: Span,
    ) -> Result<Flow> {
        if let ExprKind::Builtin(id) = func.kind
            && !BUILTINS[id].constant
        {
            return Ok(Flow::Val(self.direct_builtin_call(id, args, frame, span)?));
        }
        if let Some((f, pos)) = self.known_user_call(func, args, frame)? {
            return Ok(Flow::Tail(f, pos, span));
        }
        let (f, pos, named) = self.eval_call_parts(func, args, frame)?;
        let is_user = matches!(f.view(), V::Func(rc) if !matches!(**rc, Func::Builtin(_)));
        if is_user && named.is_empty() {
            Ok(Flow::Tail(f, pos, span))
        } else {
            Ok(Flow::Val(self.call_named(f, pos, named, span)?))
        }
    }

    /// For `if` and `match`, picks the branch to evaluate (binding the
    /// match arm's pattern variables).
    #[inline(never)]
    fn select_branch<'e>(&mut self, e: &'e Expr, frame: &Rc<Frame>) -> Result<&'e Expr> {
        match &e.kind {
            ExprKind::If(c, a, b) => Ok(if self.eval_bool(c, frame, "`if`")? {
                a
            } else {
                b
            }),
            ExprKind::Match(scrutinee, arms) => {
                let v = self.eval(scrutinee, frame)?;
                for arm in arms {
                    if !match_pattern(&arm.pat, &v, frame) {
                        continue;
                    }
                    if let Some(g) = &arm.guard
                        && !self.eval_bool(g, frame, "a `when` guard")?
                    {
                        continue;
                    }
                    return Ok(&arm.body);
                }
                Err(Error::new(
                    format!("no match arm matches {}", short(&v)),
                    e.span,
                ))
            }
            _ => Ok(e),
        }
    }

    #[inline(never)]
    fn eval_call_parts(
        &mut self,
        func: &Expr,
        args: &[Arg],
        frame: &Rc<Frame>,
    ) -> Result<(Value, Vec<Value>, NamedArgs)> {
        let f = self.eval(func, frame)?;
        let mut pos = self.arg_vec();
        let mut named = Vec::new();
        for a in args {
            let v = self.eval(&a.value, frame)?;
            match &a.name {
                Some(n) => named.push((n.clone(), v)),
                None => pos.push(v),
            }
        }
        Ok((f, pos, named))
    }

    pub fn eval_bool(&mut self, e: &Expr, frame: &Rc<Frame>, what: &str) -> Result<bool> {
        match (self.eval(e, frame)?).view() {
            V::Bool(b) => Ok(b),
            other => Err(Error::new(
                format!("{what} needs a bool, found {}", other.type_name()),
                e.span,
            )),
        }
    }

    fn eval_int(&mut self, e: &Expr, frame: &Rc<Frame>, what: &str) -> Result<Int> {
        match (self.eval(e, frame)?).view() {
            V::Int(i) => Ok(i),
            other => Err(Error::new(
                format!("{what} must be an int, found {}", other.type_name()),
                e.span,
            )),
        }
    }

    #[inline(never)]
    pub(crate) fn compare_op(
        &mut self,
        op: CmpOp,
        a: &Value,
        b: &Value,
    ) -> std::result::Result<bool, String> {
        let ord = |a, b| compare(a, b);
        Ok(match op {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => ord(a, b)? == Ordering::Less,
            CmpOp::Le => ord(a, b)? != Ordering::Greater,
            CmpOp::Gt => ord(a, b)? == Ordering::Greater,
            CmpOp::Ge => ord(a, b)? != Ordering::Less,
            CmpOp::In => self.contains(b, a)?,
            CmpOp::NotIn => !self.contains(b, a)?,
        })
    }

    /// `x in coll` (SPEC 9.2).
    pub fn contains(&mut self, coll: &Value, x: &Value) -> std::result::Result<bool, String> {
        Ok(match coll.view() {
            V::List(items) => items.contains(x),
            V::Tuple(items) => items.contains(x),
            V::Set(s) => s.contains(x),
            V::Map(m) => m.contains_key(x),
            V::Range(r) => matches!(x.view(), V::Int(i) if r.contains(&i)),
            V::RangeSet(r) => matches!(x.view(), V::Int(i) if r.contains(&i)),
            V::Str(s) => match x.view() {
                V::Char(c) => s.contains(c),
                V::Str(sub) => s.contains(&**sub),
                other => return Err(format!("cannot look for {} in a str", other.type_name())),
            },
            V::Grid(g) => builtins::grids::position(g, x).is_some(),
            V::Gen(_) => {
                let mut it = self.iter(coll).map_err(Error::into_msg)?;
                while let Some(v) = it.next(self).map_err(Error::into_msg)? {
                    if v == *x {
                        return Ok(true);
                    }
                }
                false
            }
            other => {
                return Err(format!(
                    "`in` needs a collection, found {}",
                    other.type_name()
                ));
            }
        })
    }

    // --- Calls ------------------------------------------------------------------

    pub fn call(&mut self, f: &Value, args: Vec<Value>, span: Span) -> Result<Value> {
        self.call_named(f.clone(), args, Vec::new(), span)
    }

    pub fn call_named(
        &mut self,
        f: Value,
        args: Vec<Value>,
        named: Vec<(Name, Value)>,
        span: Span,
    ) -> Result<Value> {
        // IR closures run in the IR engines, which check the stack
        // themselves; this avoids the trampoline below.
        if named.is_empty()
            && self.ir.is_some()
            && matches!(f.view(), V::Func(rc) if matches!(&**rc, Func::Ir(_)))
        {
            return crate::ir::interp::call_value(self, f, args, span);
        }
        // The same check as `stacker::maybe_grow`, but without its closure
        // plumbing on the common path, which cost ~600 bytes per call.
        stacker::maybe_grow(STACK_RED_ZONE, STACK_GROW, || {
            self.call_loop(f, args, named, span)
        })
    }

    /// The trampoline. Only this loop and `eval_tail` stay on the native
    /// stack while a function body runs; everything else about a call
    /// happens in `prepare_call`, whose frame is gone by then.
    fn call_loop(
        &mut self,
        mut f: Value,
        mut args: Vec<Value>,
        named: NamedArgs,
        mut span: Span,
    ) -> Result<Value> {
        // Memo tables waiting for the result of a chain of tail calls; every
        // function in the chain has the same result.
        let mut pending: Vec<(Rc<Memo>, Vec<Value>)> = Vec::new();
        let mut named = named;
        // The previous frame in a chain of tail calls. Its lambdas may have
        // been in use by the tail call, so it is checked for cycles one step
        // later. Holding only one keeps loops in constant space.
        let mut previous: Option<Rc<Frame>> = None;
        // For the stack trace: where this chain of tail calls started, the
        // function called there (`None` for a lambda), and how many tail
        // calls followed.
        let start = span;
        let mut first: Option<Option<Rc<FnDef>>> = None;
        let mut tail_calls = 0;
        loop {
            let prepared = self.prepare_call(f, args, named, span, &mut pending);
            let prepared = prepared.map_err(|e| chain_entry(e, &first, start, tail_calls))?;
            let (func, body, frame) = match prepared {
                Prepared::Done(v) => {
                    if let Some(p) = previous {
                        self.finish_frame(p);
                    }
                    return Ok(finish(pending, v));
                }
                Prepared::Run(func, body, frame) => (func, body, frame),
            };
            if first.is_none() {
                first = Some(match &*func {
                    Func::User { def, .. } => Some(def.clone()),
                    _ => None,
                });
            }
            let result = match (&*func, body) {
                (Func::User { def, .. }, Some(i)) => {
                    self.eval_tail_loop(&def.clauses[i].body, &frame)
                }
                (Func::Lambda { lam, .. }, None) => self.eval_tail_loop(&lam.body, &frame),
                _ => Err(Error::new("internal: bad call", span)),
            };
            let flow = match result {
                Ok(flow) => flow,
                Err(e) => {
                    let e = with_trace(e, &func, span);
                    return Err(chain_entry(e, &first, start, tail_calls));
                }
            };
            drop(func);
            match flow {
                Flow::Val(v) => {
                    self.finish_frame(frame);
                    if let Some(p) = previous {
                        self.finish_frame(p);
                    }
                    return Ok(finish(pending, v));
                }
                Flow::Tail(f2, a2, s2) => {
                    if let Some(p) = previous.replace(frame) {
                        self.finish_frame(p);
                    }
                    f = f2;
                    args = a2;
                    named = Vec::new();
                    span = s2;
                    tail_calls += 1;
                }
            }
        }
    }

    /// Everything about a call except running the body: checks, tuple
    /// spreading, the memo lookup, and choosing a clause. Builtins run here.
    #[inline(never)]
    fn prepare_call(
        &mut self,
        f: Value,
        mut args: Vec<Value>,
        named: NamedArgs,
        span: Span,
        pending: &mut Vec<(Rc<Memo>, Vec<Value>)>,
    ) -> Result<Prepared> {
        let func = match f.view() {
            V::Func(rc) => rc,
            other => {
                return Err(Error::new(
                    format!(
                        "expected a function to call, found {}",
                        with_article(other.type_name())
                    ),
                    span,
                ));
            }
        };
        if !named.is_empty() && !matches!(**func, Func::Builtin(_)) {
            return Err(Error::new(
                "named arguments are only supported by library functions",
                span,
            ));
        }
        match &**func {
            Func::Builtin(id) => {
                if let Some(n) = BUILTINS[*id].arity {
                    spread(&mut args, n);
                }
                let v = (BUILTINS[*id].f)(
                    self,
                    Args {
                        pos: args,
                        named,
                        span,
                    },
                )
                .map_err(|e| e.at(span))?;
                Ok(Prepared::Done(v))
            }
            Func::User { def, env, memo } => {
                spread(&mut args, def.arity);
                if args.len() != def.arity {
                    return Err(Error::new(
                        format!(
                            "`{}` takes {} arguments, but was given {}",
                            def.name,
                            def.arity,
                            args.len()
                        ),
                        span,
                    ));
                }
                // Functions in the key compare by identity, which is sound
                // because the key keeps them alive. A call with a generator
                // in its arguments isn't cached: a cache hit would hide the
                // error from reading it a second time (SPEC 8.1).
                if let Some(m) = memo
                    && !args.iter().any(contains_generator)
                {
                    if def.annotation != Some(Annotation::Memo) {
                        self.check_memo_key(def, &args);
                    }
                    if let Some(v) = m.table.borrow().get(&args) {
                        return Ok(Prepared::Done(v.clone()));
                    }
                    pending.push((m.clone(), args.clone()));
                }
                let (clause, frame) = self
                    .choose_clause(def, env, &mut args, span)
                    .map_err(|e| with_trace(e, func, span))?;
                self.recycle_args(args);
                Ok(Prepared::Run(func.clone(), Some(clause), frame))
            }
            Func::Ir(_) => Ok(Prepared::Done(crate::ir::interp::call_closure(
                self, func, args, span,
            )?)),
            Func::Lambda { lam, env } => {
                spread(&mut args, lam.params.len());
                if args.len() != lam.params.len() {
                    return Err(Error::new(
                        format!(
                            "lambda takes {} arguments, but was given {}",
                            lam.params.len(),
                            args.len()
                        ),
                        span,
                    ));
                }
                let frame = self.new_frame(lam.frame_size, Some(env.clone()));
                if all_simple(&lam.params) {
                    bind_simple(&lam.params, &mut args, &frame);
                    self.recycle_args(args);
                    return Ok(Prepared::Run(func.clone(), None, frame));
                }
                for (p, a) in lam.params.iter().zip(&args) {
                    if !match_pattern(p, a, &frame) {
                        return Err(Error::new(
                            format!(
                                "expected a lambda argument matching `{}`, found {}",
                                self.source_of(p.span),
                                short(a)
                            ),
                            span,
                        ));
                    }
                }
                self.recycle_args(args);
                Ok(Prepared::Run(func.clone(), None, frame))
            }
        }
    }

    /// Warns, once per function, when a function memoized automatically is
    /// called with a large key (SPEC 8.1): hashing it on every call can cost
    /// far more than the memo saves, as when a recursive function threads a
    /// growing simulation state through calls that never repeat.
    fn check_memo_key(&self, def: &FnDef, args: &[Value]) {
        self.check_memo_key_for(def.id, &def.name, def.span, args);
    }

    /// `check_memo_key` for function `id`, named `name`, defined at `span`.
    pub(crate) fn check_memo_key_for(&self, id: usize, name: &str, span: Span, args: &[Value]) {
        if self.prog.memo_warned.borrow().contains(&id) {
            return;
        }
        let mut budget = MEMO_WARN_SIZE;
        if args.iter().all(|a| within(a, &mut budget)) {
            return;
        }
        self.prog.memo_warned.borrow_mut().insert(id);
        let at = crate::runner::location(
            &self.prog.path,
            &self.prog.src,
            &self.prog.files,
            span.start,
        );
        eprintln!(
            "{at}: warning: `{}` is memoized automatically, but was called with arguments \
             holding more than {MEMO_WARN_SIZE} values, which are hashed on every call. \
             Add @nomemo if its calls never repeat, or @memo to keep memoizing without \
             this warning.",
            name
        );
    }

    /// The first clause whose patterns and guard match, and its frame.
    fn choose_clause(
        &mut self,
        def: &FnDef,
        env: &Option<Rc<Frame>>,
        args: &mut Vec<Value>,
        span: Span,
    ) -> Result<(usize, Rc<Frame>)> {
        for (i, clause) in def.clauses.iter().enumerate() {
            let frame = self.new_frame(clause.frame_size, env.clone());
            // A clause of plain names and no guard always matches, so its
            // arguments can be moved into place rather than copied.
            if clause.guard.is_none() && all_simple(&clause.params) {
                bind_simple(&clause.params, args, &frame);
                self.init_wheres(&clause.wheres, &frame);
                return Ok((i, frame));
            }
            if !clause
                .params
                .iter()
                .zip(args.iter())
                .all(|(p, a)| match_pattern(p, a, &frame))
            {
                self.recycle_frame(frame);
                continue;
            }
            self.init_wheres(&clause.wheres, &frame);
            if let Some(g) = &clause.guard
                && !self.eval_bool(g, &frame, "a `when` guard")?
            {
                self.finish_frame(frame);
                continue;
            }
            return Ok((i, frame));
        }
        let shown: Vec<String> = args.iter().map(short).collect();
        Err(Error::new(
            format!("no clause of `{}` matches ({})", def.name, shown.join(", ")),
            span,
        ))
    }

    // --- Iteration -----------------------------------------------------------------

    /// Iterator over the elements of `v` (map keys for maps).
    pub fn iter(&mut self, v: &Value) -> Result<ValIter> {
        Ok(match v.view() {
            V::List(items) => ValIter::List {
                items: items.clone(),
                pos: 0,
            },
            V::Tuple(_) => ValIter::Tuple {
                tuple: v.clone(),
                pos: 0,
            },
            V::Set(s) => ValIter::Set(Chunks::new(s.clone())),
            V::Map(m) => ValIter::MapKeys(Chunks::new(m.clone())),
            // A range from `-inf` (or `inf`) has no first element to start
            // from, unless it is empty.
            V::Range(r) if r.is_empty() => ValIter::Range {
                cur: Int::Small(1),
                end: Some(Int::Small(0)),
            },
            V::Range(r) if r.start.is_inf() => {
                return Err(Error::msg(format!(
                    "cannot iterate over a range that starts at {}",
                    r.start
                )));
            }
            V::Range(r) => ValIter::Range {
                cur: r.start.clone(),
                end: r.end.clone(),
            },
            V::Str(s) => ValIter::Chars {
                s: s.clone(),
                pos: 0,
            },
            V::RangeSet(r) => ValIter::RangeSet {
                set: r.clone(),
                span: 0,
                next: None,
            },
            V::Gen(g) => {
                let mut state = g.borrow_mut();
                if state.used {
                    return Err(Error::msg(
                        "this generator has already been read; generators are single-pass \
                         (collect it with `list()` to use it more than once)",
                    ));
                }
                state.used = true;
                ValIter::Gen(g.clone())
            }
            other => {
                return Err(Error::msg(format!(
                    "cannot iterate over {}",
                    with_article(other.type_name())
                )));
            }
        })
    }

    /// All elements of a finite iterable.
    pub fn to_vec(&mut self, v: &Value) -> Result<Vec<Value>> {
        match v.view() {
            V::List(items) => return Ok(items.iter().cloned().collect()),
            V::Tuple(items) => return Ok(items.to_vec()),
            V::Set(s) => return Ok(s.iter().cloned().collect()),
            V::Map(m) => return Ok(m.keys().cloned().collect()),
            _ => {}
        }
        let mut it = self.iter(v)?;
        if it.is_infinite() {
            return Err(Error::msg(
                "this needs a finite collection, but the range is infinite",
            ));
        }
        let mut out = Vec::new();
        while let Some(x) = it.next(self)? {
            out.push(x);
        }
        Ok(out)
    }

    fn comprehension(&mut self, comp: &Rc<Comp>, frame: &Rc<Frame>, span: Span) -> Result<Value> {
        match comp.kind {
            CompKind::Gen => Ok(Value::generator(Generator {
                kind: GenKind::Comp {
                    comp: comp.clone(),
                    root: frame.clone(),
                    levels: Vec::new(),
                    started: false,
                },
                used: false,
            })),
            CompKind::List => {
                let mut out = Vec::new();
                self.comp_loop(comp, 0, frame, &mut |it, f| {
                    out.push(it.eval(&comp.body, f)?);
                    Ok(())
                })?;
                Ok(Value::list(out))
            }
            CompKind::Set => {
                let mut out = ValueSet::new();
                self.comp_loop(comp, 0, frame, &mut |it, f| {
                    let v = it.eval(&comp.body, f)?;
                    value::set_insert(&mut out, v).map_err(|m| Error::new(m, comp.body.span))
                })?;
                Ok(Value::set(out))
            }
            CompKind::Map => {
                let value_expr = comp
                    .value
                    .as_ref()
                    .ok_or_else(|| Error::new("internal: map comprehension", span))?;
                let mut out = ValueMap::new();
                self.comp_loop(comp, 0, frame, &mut |it, f| {
                    let k = it.eval(&comp.body, f)?;
                    let v = it.eval(value_expr, f)?;
                    value::map_insert(&mut out, k, v).map_err(|m| Error::new(m, comp.body.span))
                })?;
                Ok(Value::map(out))
            }
        }
    }

    fn comp_loop(
        &mut self,
        comp: &Comp,
        i: usize,
        frame: &Rc<Frame>,
        out: &mut dyn FnMut(&mut Interp<'p>, &Rc<Frame>) -> Result<()>,
    ) -> Result<()> {
        match comp.clauses.get(i) {
            None => out(self, frame),
            Some(CompClause::If(c)) => {
                if self.eval_bool(c, frame, "a comprehension `if`")? {
                    self.comp_loop(comp, i + 1, frame, out)?;
                }
                Ok(())
            }
            Some(CompClause::For {
                pat,
                iter,
                frame_size,
            }) => {
                let coll = self.eval(iter, frame)?;
                let mut it = self.iter(&coll).map_err(|e| e.at(iter.span))?;
                if it.is_infinite() {
                    return Err(Error::new(
                        "cannot build a collection from an infinite range",
                        iter.span,
                    ));
                }
                while let Some(item) = it.next(self)? {
                    let f = self.new_frame(*frame_size, Some(frame.clone()));
                    if match_pattern(pat, &item, &f) {
                        self.comp_loop(comp, i + 1, &f, out)?;
                    }
                    self.recycle_frame(f);
                }
                Ok(())
            }
        }
    }

    /// The next element of a lazy `map`, `filter`, `enumerate` or `zip`.
    fn adapter_next(&mut self, kind: &mut GenKind) -> Result<Option<Value>> {
        match kind {
            GenKind::Map { src, f, span } => match src.next(self)? {
                Some(x) => self.call(f, vec![x], *span).map(Some),
                None => Ok(None),
            },
            GenKind::Filter { src, f, span } => loop {
                let Some(x) = src.next(self)? else {
                    return Ok(None);
                };
                match (self.call(f, vec![x.clone()], *span)?).view() {
                    V::Bool(true) => return Ok(Some(x)),
                    V::Bool(false) => {}
                    other => {
                        return Err(Error::new(
                            format!(
                                "the predicate passed to `filter` must return a bool, but returned {}",
                                other.type_name()
                            ),
                            *span,
                        ));
                    }
                }
            },
            GenKind::Enumerate { src, next } => match src.next(self)? {
                Some(x) => {
                    let i = *next;
                    *next += 1;
                    Ok(Some(Value::pair(Value::int(i), x)))
                }
                None => Ok(None),
            },
            GenKind::Zip { srcs } => {
                let mut row = Vec::with_capacity(srcs.len());
                for src in srcs.iter_mut() {
                    match src.next(self)? {
                        Some(x) => row.push(x),
                        None => return Ok(None),
                    }
                }
                Ok(Some(Value::tuple(row)))
            }
            GenKind::FlatMap { src, f, cur, span } => loop {
                if let Some(c) = cur {
                    if let Some(x) = c.next(self)? {
                        return Ok(Some(x));
                    }
                    *cur = None;
                }
                let Some(x) = src.next(self)? else {
                    return Ok(None);
                };
                let items = self.call(f, vec![x], *span)?;
                *cur = Some(self.iter(&items).map_err(|e| e.at(*span))?);
            },
            GenKind::Busy => Err(Error::msg(
                "a generator was read while computing its own next element",
            )),
            GenKind::Comp { .. } | GenKind::Iterate { .. } => {
                Err(Error::msg("internal: not a lazy adapter"))
            }
        }
    }

    pub fn gen_next(&mut self, g: &Rc<RefCell<Generator>>) -> Result<Option<Value>> {
        let Ok(mut gen_ref) = g.try_borrow_mut() else {
            return Err(Error::msg(
                "a generator was read while computing its own next element",
            ));
        };
        if let GenKind::Map { .. }
        | GenKind::Filter { .. }
        | GenKind::Enumerate { .. }
        | GenKind::Zip { .. }
        | GenKind::FlatMap { .. }
        | GenKind::Busy = gen_ref.kind
        {
            // Take the state out while user code runs.
            let mut kind = std::mem::replace(&mut gen_ref.kind, GenKind::Busy);
            drop(gen_ref);
            let result = self.adapter_next(&mut kind);
            if let Ok(mut gen_ref) = g.try_borrow_mut() {
                gen_ref.kind = kind;
            }
            return result;
        }
        match &mut gen_ref.kind {
            GenKind::Iterate { cur, f, first } => {
                if *first {
                    *first = false;
                } else {
                    let f = f.clone();
                    let next = self.call(&f, vec![cur.clone()], Span::default())?;
                    *cur = next;
                }
                Ok(Some(cur.clone()))
            }
            GenKind::Comp {
                comp,
                root,
                levels,
                started,
            } => {
                let comp = comp.clone();
                let mut pending = if *started {
                    None
                } else {
                    *started = true;
                    Some((0usize, root.clone()))
                };
                loop {
                    if let Some((mut i, frame)) = pending.take() {
                        loop {
                            match comp.clauses.get(i) {
                                None => return self.eval(&comp.body, &frame).map(Some),
                                Some(CompClause::If(c)) => {
                                    if self.eval_bool(c, &frame, "a comprehension `if`")? {
                                        i += 1;
                                        continue;
                                    }
                                    break;
                                }
                                Some(CompClause::For { iter, .. }) => {
                                    let coll = self.eval(iter, &frame)?;
                                    let it = self.iter(&coll).map_err(|e| e.at(iter.span))?;
                                    levels.push(GenLevel {
                                        clause: i,
                                        iter: it,
                                        parent: frame,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                    let Some(level) = levels.last_mut() else {
                        return Ok(None);
                    };
                    match level.iter.next(self)? {
                        None => {
                            levels.pop();
                        }
                        Some(item) => {
                            if let Some(CompClause::For {
                                pat, frame_size, ..
                            }) = comp.clauses.get(level.clause)
                            {
                                let f = Frame::new(*frame_size, Some(level.parent.clone()));
                                if match_pattern(pat, &item, &f) {
                                    pending = Some((level.clause + 1, f));
                                }
                            }
                        }
                    }
                }
            }
            // Handled above.
            _ => Err(Error::msg("internal: unexpected generator state")),
        }
    }
}

fn finish(pending: Vec<(Rc<Memo>, Vec<Value>)>, v: Value) -> Value {
    for (m, key) in pending {
        m.table.borrow_mut().insert(key, v.clone());
    }
    v
}

/// Called when a call's body has finished. If the frame is kept alive only
/// by lambdas and generators stored in its own slots (for example a `where`
/// constant holding a lambda), it is garbage that `Rc` can't free, so its
/// slots are cleared to break the cycle.
///
/// This is safe because it only happens when those slot values account for
/// every other reference to the frame: nothing else can ever read the slots
/// again. A reference this can't see (a returned or memoized lambda, a
/// generator nested in a tuple) makes the counts disagree, and the frame is
/// left alone. That can leak, but never breaks a live frame.
#[inline(never)]
fn release_frame(frame: &Rc<Frame>) {
    let others = Rc::strong_count(frame) - 1;
    if others == 0 {
        return;
    }
    let refs_to = |f: &Rc<Frame>| usize::from(Rc::ptr_eq(f, frame));
    // Each distinct lambda or generator in the slots: (pointer, how many
    // slots hold it, its own reference count, its references to the frame).
    let mut found: Vec<(u64, usize, usize, usize)> = Vec::new();
    {
        let Ok(slots) = frame.slots.try_borrow() else {
            return;
        };
        for slot in slots.iter() {
            let Slot::Val(v) = slot else { continue };
            // A value counts as held only by its holders if its payload is
            // held only by this one heap object; otherwise it's never freed
            // here (a leak at worst, never a dangling reference).
            let Some((ptr, count)) = v.heap_refs() else {
                continue;
            };
            let (count, refs) = match v.view() {
                V::Func(rc) => {
                    let refs = match &**rc {
                        Func::Lambda { env, .. } => refs_to(env),
                        Func::User { env: Some(env), .. } => refs_to(env),
                        _ => 0,
                    };
                    let count = if Rc::strong_count(rc) == 1 {
                        count
                    } else {
                        usize::MAX
                    };
                    (count, refs)
                }
                V::Gen(g) => {
                    let Ok(state) = g.try_borrow() else { return };
                    let refs = match &state.kind {
                        GenKind::Comp { root, levels, .. } => {
                            refs_to(root) + levels.iter().map(|l| refs_to(&l.parent)).sum::<usize>()
                        }
                        GenKind::Iterate { .. }
                        | GenKind::Map { .. }
                        | GenKind::Filter { .. }
                        | GenKind::Enumerate { .. }
                        | GenKind::Zip { .. }
                        | GenKind::FlatMap { .. }
                        | GenKind::Busy => 0,
                    };
                    let count = if Rc::strong_count(g) == 1 {
                        count
                    } else {
                        usize::MAX
                    };
                    (count, refs)
                }
                _ => continue,
            };
            match found.iter_mut().find(|f| f.0 == ptr) {
                Some(f) => f.1 += 1,
                None => found.push((ptr, 1, count, refs)),
            }
        }
    }
    // Every holder of the frame is one of these values, and every holder of
    // those that refer to the frame is a slot of the frame: the whole cycle
    // is unreachable. (A value that doesn't refer to the frame can't keep
    // it alive, wherever else it is held.)
    let internal: usize = found.iter().map(|f| f.3).sum();
    let only_in_slots = found
        .iter()
        .all(|&(_, in_slots, count, refs)| refs == 0 || in_slots == count);
    if internal == others && only_in_slots {
        // Move the slots out before dropping them, so no borrow is held
        // while other frames are freed.
        let slots = std::mem::take(&mut *frame.slots.borrow_mut());
        drop(slots);
    }
}

/// What `prepare_call` decided.
enum Prepared {
    /// The call's result is already known (a builtin, or a memo hit).
    Done(Value),
    /// Run this function's body (the clause index, for named functions) in
    /// this frame.
    Run(Rc<Func>, Option<usize>, Rc<Frame>),
}

/// Adds a stack-trace entry for a failed call.
#[cold]
fn with_trace(mut e: Error, func: &Func, span: Span) -> Error {
    let name = match func {
        Func::User { def, .. } => def.name.to_string(),
        _ => "<lambda>".to_string(),
    };
    e.push_trace(&name, span, 0);
    e
}

/// After an error in a chain of tail calls, adds the call that started the
/// chain, noting how many tail calls followed it.
#[cold]
fn chain_entry(
    mut e: Error,
    first: &Option<Option<Rc<FnDef>>>,
    start: Span,
    tail_calls: usize,
) -> Error {
    if tail_calls > 0
        && let Some(def) = first
    {
        let name = def.as_ref().map_or("<lambda>", |d| &*d.name);
        e.push_trace(name, start, tail_calls);
    }
    e
}

/// Whether a value is, or holds anywhere inside it, a generator.
pub(crate) fn contains_generator(v: &Value) -> bool {
    match v.view() {
        V::Gen(_) => true,
        V::Tuple(items) => items.iter().any(contains_generator),
        V::List(items) => items.iter().any(contains_generator),
        V::Set(items) => items.iter().any(contains_generator),
        V::Map(m) => m
            .iter()
            .any(|(k, v)| contains_generator(k) || contains_generator(v)),
        V::Grid(g) => g.cells.iter().any(contains_generator),
        _ => false,
    }
}

/// Tuple spreading (SPEC 5.2).
pub(crate) fn spread(args: &mut Vec<Value>, arity: usize) {
    if arity >= 2
        && args.len() == 1
        && let V::Tuple(t) = args[0].view()
        && t.len() == arity
    {
        *args = t.to_vec();
    }
}

/// "an int", "a str", ...
pub fn with_article(type_name: &str) -> String {
    let vowel = type_name.starts_with(['a', 'e', 'i', 'o', 'u']);
    format!("{} {type_name}", if vowel { "an" } else { "a" })
}

/// A value shortened for error messages.
pub fn short(v: &Value) -> String {
    let s = v.repr();
    if s.chars().count() > 60 {
        format!("{}...", s.chars().take(57).collect::<String>())
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Pattern matching (SPEC 6)

/// `x op y` for two small ints, when the result is a small int too.
/// `None` sends the operation to the general `binary` (overflow, division
/// by zero, and everything else).
#[inline]
pub(crate) fn small_int_op(op: BinOp, x: i64, y: i64) -> Option<i64> {
    match op {
        BinOp::Add => x.checked_add(y),
        BinOp::Sub => x.checked_sub(y),
        BinOp::Mul => x.checked_mul(y),
        // Floor division and a remainder with the divisor's sign (SPEC 5.1).
        BinOp::Div => {
            let q = x.checked_div(y)?;
            Some(if (x % y != 0) && ((x < 0) != (y < 0)) {
                q - 1
            } else {
                q
            })
        }
        BinOp::Mod => {
            let r = x.checked_rem(y)?;
            Some(if r != 0 && ((r < 0) != (y < 0)) {
                r + y
            } else {
                r
            })
        }
        BinOp::BitAnd => Some(x & y),
        BinOp::BitOr => Some(x | y),
        BinOp::Xor => Some(x ^ y),
        _ => None,
    }
}

/// Whether every pattern is a plain name or `_`, so it matches anything.
fn all_simple(params: &[Pattern]) -> bool {
    params
        .iter()
        .all(|p| matches!(p.kind, PatKind::Bind(_) | PatKind::Wild))
}

/// Binds plain-name parameters by moving the arguments out of `args`.
fn bind_simple(params: &[Pattern], args: &mut Vec<Value>, frame: &Frame) {
    let mut slots = frame.slots.borrow_mut();
    for (p, a) in params.iter().zip(args.drain(..)) {
        if let PatKind::Bind(b) = &p.kind
            && let Some(s) = slots.get_mut(b.slot)
        {
            *s = Slot::Val(a);
        }
    }
}

pub fn match_pattern(p: &Pattern, v: &Value, frame: &Frame) -> bool {
    match_into(p, v, &mut frame.slots.borrow_mut())
}

/// Matches `v` against `p`, binding into `slots`.
fn match_into(p: &Pattern, v: &Value, slots: &mut [Slot]) -> bool {
    fn set(slots: &mut [Slot], slot: usize, val: Value) {
        if let Some(s) = slots.get_mut(slot) {
            *s = Slot::Val(val);
        }
    }
    match &p.kind {
        PatKind::Wild => true,
        PatKind::Bind(b) => {
            set(slots, b.slot, v.clone());
            true
        }
        PatKind::Typed(b, ty) => {
            let ok = match ty {
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
            };
            if ok && let Some(b) = b {
                set(slots, b.slot, v.clone());
            }
            ok
        }
        PatKind::Lit(l) => v == l,
        PatKind::Tuple(ps) => match v.view() {
            V::Tuple(items) => {
                items.len() == ps.len()
                    && ps
                        .iter()
                        .zip(items.iter())
                        .all(|(p, x)| match_into(p, x, slots))
            }
            _ => false,
        },
        PatKind::List {
            before,
            rest,
            after,
        } => {
            let V::List(items) = v.view() else {
                return false;
            };
            let fixed = before.len() + after.len();
            match rest {
                None if items.len() != fixed => return false,
                Some(_) if items.len() < fixed => return false,
                _ => {}
            }
            for (i, p) in before.iter().enumerate() {
                if !match_into(p, &items[i], slots) {
                    return false;
                }
            }
            let tail_start = items.len() - after.len();
            for (i, p) in after.iter().enumerate() {
                if !match_into(p, &items[tail_start + i], slots) {
                    return false;
                }
            }
            if let Some(Some(b)) = rest {
                let middle = items.slice(before.len(), tail_start);
                set(slots, b.slot, Value::list(middle));
            }
            true
        }
        PatKind::Str(segs) => {
            let V::Str(s) = v.view() else { return false };
            let mut binds = Vec::new();
            if !match_segs(segs, s, 0, 0, &mut binds) {
                return false;
            }
            for (slot, val) in binds {
                set(slots, slot, val);
            }
            true
        }
    }
}

pub(crate) fn match_segs(
    segs: &[StrSeg],
    s: &str,
    pos: usize,
    i: usize,
    binds: &mut Vec<(usize, Value)>,
) -> bool {
    let Some(seg) = segs.get(i) else {
        return pos == s.len();
    };
    let rest = &s[pos..];
    let bind = |binds: &mut Vec<(usize, Value)>, b: &Option<Binding>, v: Value| {
        if let Some(b) = b {
            binds.push((b.slot, v));
        }
    };
    match seg {
        StrSeg::Lit(l) => {
            rest.starts_with(l.as_str()) && match_segs(segs, s, pos + l.len(), i + 1, binds)
        }
        StrSeg::Cap {
            bind: b,
            kind: CapKind::Str,
        } => {
            if i + 1 == segs.len() {
                bind(binds, b, Value::str(rest));
                return true;
            }
            // Shortest first.
            let mut ends: Vec<usize> = rest.char_indices().map(|(j, _)| j).collect();
            ends.push(rest.len());
            if let Some(StrSeg::Lit(l)) = segs.get(i + 1) {
                ends.retain(|&j| rest[j..].starts_with(l.as_str()));
            }
            for j in ends {
                let mark = binds.len();
                bind(binds, b, Value::str(&rest[..j]));
                if match_segs(segs, s, pos + j, i + 1, binds) {
                    return true;
                }
                binds.truncate(mark);
            }
            false
        }
        StrSeg::Cap {
            bind: b,
            kind: CapKind::Int,
        } => {
            // An optional sign, - or +, then digits (SPEC 6.1).
            let bytes = rest.as_bytes();
            let negative = bytes.first() == Some(&b'-');
            let sign = usize::from(negative || bytes.first() == Some(&b'+'));
            let digits = bytes[sign..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits == 0 {
                return false;
            }
            let len = sign + digits;
            let Some(n) = Int::parse(&rest[sign..len]) else {
                return false;
            };
            let n = if negative { n.neg() } else { n };
            let mark = binds.len();
            bind(binds, b, Value::from_int(n));
            if match_segs(segs, s, pos + len, i + 1, binds) {
                return true;
            }
            binds.truncate(mark);
            false
        }
        StrSeg::Cap {
            bind: b,
            kind: CapKind::Char,
        } => {
            let Some(c) = rest.chars().next() else {
                return false;
            };
            let mark = binds.len();
            bind(binds, b, Value::char(c));
            if match_segs(segs, s, pos + c.len_utf8(), i + 1, binds) {
                return true;
            }
            binds.truncate(mark);
            false
        }
        StrSeg::Cap {
            bind: b,
            kind: CapKind::Word,
        } => {
            let len = rest
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                .count();
            if len == 0 {
                return false;
            }
            let mark = binds.len();
            bind(binds, b, Value::str(&rest[..len]));
            if match_segs(segs, s, pos + len, i + 1, binds) {
                return true;
            }
            binds.truncate(mark);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Operators (SPEC 5.1, 5.3)

pub(crate) fn negate(v: &Value) -> std::result::Result<Value, String> {
    match v.view() {
        V::Int(i) => Ok(Value::from_int(i.neg())),
        V::Tuple(items) => Ok(Value::tuple(
            items
                .iter()
                .map(negate)
                .collect::<std::result::Result<_, _>>()?,
        )),
        other => Err(format!("cannot negate a {}", other.type_name())),
    }
}

fn int_op(op: BinOp, a: &Int, b: &Int) -> std::result::Result<Int, String> {
    match op {
        BinOp::Add => a.add(b),
        BinOp::Sub => a.sub(b),
        BinOp::Mul => a.mul(b),
        BinOp::Div => a.div(b),
        BinOp::Mod => a.rem(b),
        BinOp::Pow => a.pow(b),
        BinOp::Shl => a.shl(b),
        BinOp::Shr => a.shr(b),
        BinOp::BitAnd => a.bitand(b),
        BinOp::BitOr => a.bitor(b),
        BinOp::Xor => a.bitxor(b),
        BinOp::Concat | BinOp::Dot(_) => Err(format!("`{}` is not an int operator", op.symbol())),
    }
}

fn op_error(op: BinOp, a: &Value, b: &Value) -> String {
    let hint = match (op, a.view(), b.view()) {
        (BinOp::Add, V::List(_), V::List(_)) | (BinOp::Add, V::Str(_), V::Str(_)) => {
            " (use `++` to concatenate)"
        }
        _ => "",
    };
    format!(
        "cannot apply `{}` to {} and {}{hint}",
        op.symbol(),
        a.type_name(),
        b.type_name()
    )
}

#[inline(never)]
pub fn binary(op: BinOp, a: &Value, b: &Value) -> std::result::Result<Value, String> {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            match (a.view(), b.view()) {
                (V::Int(x), V::Int(y)) => int_op(op, &x, &y).map(Value::from_int),
                (V::Tuple(x), V::Tuple(y)) => {
                    // Positions and directions: pairs of small ints.
                    if let ([a0, a1], [b0, b1]) = (&*x, &*y)
                        && let (Some(p0), Some(p1)) = (a0.as_small(), a1.as_small())
                        && let (Some(q0), Some(q1)) = (b0.as_small(), b1.as_small())
                        && let (Some(r0), Some(r1)) =
                            (small_int_op(op, p0, q0), small_int_op(op, p1, q1))
                    {
                        return Ok(Value::pair(Value::int(r0), Value::int(r1)));
                    }
                    if x.len() != y.len() {
                        return Err(format!(
                            "cannot apply `{}` to tuples of different lengths ({} and {})",
                            op.symbol(),
                            x.len(),
                            y.len()
                        ));
                    }
                    let items = x
                        .iter()
                        .zip(y.iter())
                        .map(|(p, q)| binary(op, p, q))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok(Value::tuple(items))
                }
                (V::Tuple(x), V::Int(_)) => {
                    let items = x
                        .iter()
                        .map(|p| binary(op, p, b))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok(Value::tuple(items))
                }
                (V::Int(_), V::Tuple(y)) => {
                    let items = y
                        .iter()
                        .map(|q| binary(op, a, q))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok(Value::tuple(items))
                }
                (V::RangeSet(x), V::RangeSet(y)) if op == BinOp::Sub => {
                    Ok(Value::rangeset_rc(Rc::new(x.difference(y))))
                }
                (V::Set(x), V::Set(y)) if op == BinOp::Sub => {
                    Ok(Value::set(set_difference(x.clone(), y)))
                }
                _ => Err(op_error(op, a, b)),
            }
        }
        BinOp::Pow | BinOp::Shl | BinOp::Shr => match (a.view(), b.view()) {
            (V::Int(x), V::Int(y)) => int_op(op, &x, &y).map(Value::from_int),
            _ => Err(op_error(op, a, b)),
        },
        BinOp::BitAnd | BinOp::BitOr | BinOp::Xor => match (a.view(), b.view()) {
            (V::Int(x), V::Int(y)) => int_op(op, &x, &y).map(Value::from_int),
            (V::Set(x), V::Set(y)) => {
                if let (Some(p), Some(q)) = (x.get_min(), y.get_min()) {
                    value::same_key_type(p, q)?;
                }
                Ok(Value::set(match op {
                    BinOp::BitAnd => set_intersection(x, y),
                    BinOp::BitOr => set_union(x.clone(), y.clone()),
                    _ => x.clone().symmetric_difference(y),
                }))
            }
            (V::RangeSet(x), V::RangeSet(y)) if op != BinOp::Xor => {
                Ok(Value::rangeset_rc(Rc::new(if op == BinOp::BitOr {
                    x.union(y)
                } else {
                    x.intersection(y)
                })))
            }
            (V::Map(x), V::Map(y)) if op == BinOp::BitOr => {
                if let (Some((p, _)), Some((q, _))) = (x.get_min(), y.get_min()) {
                    value::same_key_type(p, q)?;
                }
                // Values from the right-hand map win.
                Ok(Value::map(map_merge(x.clone(), y)))
            }
            _ => Err(op_error(op, a, b)),
        },
        BinOp::Concat => match (a.view(), b.view()) {
            (V::Str(x), V::Str(y)) => Ok(Value::from_str_rc(format!("{x}{y}").into())),
            (V::List(x), V::List(y)) => Ok(Value::list(x.clone().concat(y.clone()))),
            _ => Err(op_error(op, a, b)),
        },
        BinOp::Dot(cmp) => {
            let elem = |p: &Value, q: &Value| -> std::result::Result<Value, String> {
                let r = match cmp {
                    CmpOp::Eq => p == q,
                    CmpOp::Ne => p != q,
                    CmpOp::Lt => compare(p, q)? == Ordering::Less,
                    CmpOp::Le => compare(p, q)? != Ordering::Greater,
                    CmpOp::Gt => compare(p, q)? == Ordering::Greater,
                    CmpOp::Ge => compare(p, q)? != Ordering::Less,
                    CmpOp::In | CmpOp::NotIn => return Err("internal: dotted `in`".into()),
                };
                Ok(Value::bool(r))
            };
            match (a.view(), b.view()) {
                (V::Tuple(x), V::Tuple(y)) => {
                    if x.len() != y.len() {
                        return Err(format!(
                            "cannot apply `{}` to tuples of different lengths ({} and {})",
                            op.symbol(),
                            x.len(),
                            y.len()
                        ));
                    }
                    Ok(Value::tuple(
                        x.iter()
                            .zip(y.iter())
                            .map(|(p, q)| elem(p, q))
                            .collect::<std::result::Result<_, _>>()?,
                    ))
                }
                (V::Tuple(x), _) => Ok(Value::tuple(
                    x.iter()
                        .map(|p| elem(p, b))
                        .collect::<std::result::Result<_, _>>()?,
                )),
                (_, V::Tuple(y)) => Ok(Value::tuple(
                    y.iter()
                        .map(|q| elem(a, q))
                        .collect::<std::result::Result<_, _>>()?,
                )),
                _ => Err(format!(
                    "`{}` needs a tuple operand, found {} and {}",
                    op.symbol(),
                    a.type_name(),
                    b.type_name()
                )),
            }
        }
    }
}

// The `imbl` set operations walk their second argument, so `small - large`
// would cost O(large). These walk whichever side makes the work
// proportional to the smaller set.

/// `x - y`. When `x` is the larger set it is updated in place if nothing
/// else refers to it.
fn set_difference(x: ValueSet, y: &ValueSet) -> ValueSet {
    if x.len() <= y.len() {
        x.iter().filter(|v| !y.contains(v)).cloned().collect()
    } else {
        let mut out = x;
        for v in y.iter() {
            out.remove(v);
        }
        out
    }
}

fn set_intersection(x: &ValueSet, y: &ValueSet) -> ValueSet {
    let (small, large) = if x.len() <= y.len() { (x, y) } else { (y, x) };
    small
        .iter()
        .filter(|v| large.contains(v))
        .cloned()
        .collect()
}

/// `x | y`, inserting the smaller set into the larger, which is updated in
/// place if nothing else refers to it.
fn set_union(x: ValueSet, y: ValueSet) -> ValueSet {
    let (small, mut out) = if x.len() <= y.len() { (x, y) } else { (y, x) };
    for v in small.iter() {
        out.insert(v.clone());
    }
    out
}

/// `x | y` on maps; values from `y` win. `x` is updated in place if
/// nothing else refers to it.
fn map_merge(x: ValueMap, y: &ValueMap) -> ValueMap {
    let mut out = x;
    for (k, v) in y.iter() {
        out.insert(k.clone(), v.clone());
    }
    out
}

/// `binary` for operands the caller owns, so that a set, map or list that
/// nothing else refers to can be updated in place rather than copied.
pub(crate) fn binary_owned(
    op: BinOp,
    mut a: Value,
    b: Value,
) -> std::result::Result<Value, String> {
    let in_place = match (op, a.view(), b.view()) {
        (BinOp::BitOr, V::Set(x), V::Set(y)) => {
            if let (Some(p), Some(q)) = (x.get_min(), y.get_min()) {
                value::same_key_type(p, q)?;
            }
            true
        }
        (BinOp::BitOr, V::Map(x), V::Map(y)) => {
            if let (Some((p, _)), Some((q, _))) = (x.get_min(), y.get_min()) {
                value::same_key_type(p, q)?;
            }
            true
        }
        (BinOp::Sub, V::Set(_), V::Set(_)) | (BinOp::Concat, V::List(_), V::List(_)) => true,
        _ => false,
    };
    if !in_place {
        return binary(op, &a, &b);
    }
    match (a.make_mut(), b.unpack()) {
        (Some(Heap::Set(x)), Own::Set(y)) if op == BinOp::BitOr => {
            *x = set_union(std::mem::take(x), y);
        }
        (Some(Heap::Set(x)), Own::Set(y)) => *x = set_difference(std::mem::take(x), &y),
        (Some(Heap::Map(x)), Own::Map(y)) => *x = map_merge(std::mem::take(x), &y),
        (Some(Heap::List(x)), Own::List(y)) => *x = std::mem::take(x).concat(y),
        _ => return Err(format!("internal: `{}` in place", op.symbol())),
    }
    Ok(a)
}

#[cfg(test)]
mod fast_path_tests {
    use super::*;

    /// The small-int fast path must agree with the general `binary`
    /// whenever it gives an answer.
    #[test]
    fn small_int_ops_match_binary() {
        let edge = [
            0,
            1,
            -1,
            2,
            -2,
            3,
            -3,
            7,
            -7,
            100,
            -100,
            i64::MAX,
            i64::MIN,
            i64::MAX - 1,
            i64::MIN + 1,
        ];
        let ops = [
            BinOp::Add,
            BinOp::Sub,
            BinOp::Mul,
            BinOp::Div,
            BinOp::Mod,
            BinOp::BitAnd,
            BinOp::BitOr,
            BinOp::Xor,
        ];
        for op in ops {
            for &x in &edge {
                for &y in &edge {
                    let Some(fast) = small_int_op(op, x, y) else {
                        continue;
                    };
                    let slow = binary(op, &Value::int(x), &Value::int(y));
                    assert!(
                        matches!(&slow, Ok(s) if s.as_small() == Some(fast)),
                        "{x} {} {y}: fast {fast}, general {:?}",
                        op.symbol(),
                        slow.map(|v| v.repr())
                    );
                }
            }
        }
    }
}
