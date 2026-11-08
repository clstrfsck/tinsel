//! The mid-level IR for native compilation (`reference/native-plan.md`,
//! section 3).
//!
//! A program becomes a [`Module`] of [`Function`]s. Each function is a list
//! of basic blocks in A-normal form: every intermediate value is a numbered
//! variable, assigned once, and control flow is explicit jumps between
//! blocks that take parameters (close to SSA).
//!
//! - **Closures** are explicit. A function's free variables are its
//!   *captures*, copied into the closure when it is made.
//! - **`where` blocks** become a *record* made once per call of the
//!   enclosing clause. It holds a lazy cell for each constant, the memo
//!   tables of the block's memoized functions, a link to the enclosing
//!   record, and copies of the clause's variables that the block's
//!   definitions use. Every `where` definition captures just its record.
//! - **Top-level constants** are lazy cells of the run, read with
//!   [`Op::Global`].
//! - **Patterns** compile to tests and branches.
//! - **Tail calls** are terminators, and a function's tail calls to itself
//!   are jumps back to its entry block.
//! - **Last uses** are marked on every operand (see [`live`]), so a value
//!   held only once can be updated in place.
//!
//! [`interp`] runs the IR. It's there to check the lowering against the
//! whole test corpus before any machine code exists.

pub mod cse;
pub mod interp;
pub mod live;
pub mod lower;
pub mod simplify;

use crate::ast::{BinOp, CmpOp, Name, StrSeg, TypeName};
use crate::error::Span;
use crate::value::Value;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

pub type Var = u32;
pub type BlockId = u32;
pub type FuncId = u32;

/// A variable read by an instruction. `last` is set by [`live`] when this
/// is the variable's last use on its path: the value is moved out, not
/// copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Use {
    pub var: Var,
    pub last: bool,
}

impl Use {
    pub fn new(var: Var) -> Use {
        Use { var, last: false }
    }
}

pub struct Module {
    pub funcs: Vec<Function>,
    /// One per global, in the program's global order.
    pub globals: Vec<GlobalDef>,
    /// The cells of top-level constants: one per definition, which may
    /// bind several globals.
    pub consts: Vec<Cell>,
    /// Memo tables for top-level functions, one set per run.
    pub memos: usize,
    pub layouts: Vec<Layout>,
    /// Each check's function, by the check's span start.
    pub checks: HashMap<usize, FuncId>,
    /// For each `example`: the `with` overrides, as (global, cell).
    pub withs: Vec<Vec<(usize, Cell)>>,
}

#[derive(Debug, Clone, Copy)]
pub enum GlobalDef {
    /// Binding `index` of constant cell `cell` (`None` when it binds one
    /// name and so holds the value itself rather than a tuple).
    Const {
        cell: usize,
        index: Option<usize>,
    },
    Func(FuncId),
}

/// A lazy constant: the function computing it, and its names for cycle
/// reports.
#[derive(Debug, Clone)]
pub struct Cell {
    pub thunk: FuncId,
    pub describe: Rc<str>,
}

/// The shape of a `where` record.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// Copies of the enclosing clause's variables.
    pub vals: usize,
    pub cells: Vec<Cell>,
    pub memos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    /// A named function (top-level or `where`).
    Named,
    Lambda,
    /// The body of a constant.
    Thunk,
    /// A `test`, `example` or `answers` expression.
    Check,
}

/// Where a memoized function keeps its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoAt {
    /// Table `i` of the run (top-level functions).
    Global(usize),
    /// Table `i` of the record that is the closure's capture 0 (`where`
    /// functions).
    Record(usize),
}

pub struct Function {
    pub name: Name,
    pub kind: FnKind,
    pub params: u32,
    pub caps: u32,
    pub memo: Option<MemoAt>,
    /// Set for `@memo`, which turns off the large-key warning.
    pub memo_annotated: bool,
    /// The `FnDef::id` of a named function, for the large-key warning.
    pub def_id: Option<usize>,
    /// The definition, for messages.
    pub span: Span,
    pub blocks: Vec<Block>,
    pub nvars: u32,
}

pub struct Block {
    pub params: Vec<Var>,
    pub insts: Vec<Inst>,
    pub term: Term,
    /// Variables that are live on some edge into this block but not in it:
    /// released on entry. Set by [`live`].
    pub drops: Vec<Var>,
}

pub struct Inst {
    /// `None` when the result isn't used.
    pub dst: Option<Var>,
    pub op: Op,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum FmtPart {
    Lit(Rc<str>),
    Val(Use),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccKind {
    List,
    Set,
    Map,
}

#[derive(Debug, Clone)]
pub enum Op {
    Const(Value),
    /// A tail call of the running function, made as a jump back to its
    /// entry block. It has no result; it only counts the call, at the
    /// instruction's span, for stack traces (SPEC 8.3).
    SelfTail,
    /// Capture `i` of the running closure.
    Cap(u32),
    /// A global's value; a constant is computed on first use.
    Global(usize),
    /// A library function as a value, or the value of a library constant
    /// such as `input`.
    BuiltinValue(usize),

    // --- Records and closures
    /// A `where` record: its enclosing record and copied values.
    NewRecord {
        layout: usize,
        parent: Option<Use>,
        vals: Vec<Use>,
    },
    RecParent(Use),
    RecVal(Use, u32),
    /// The value of cell `cell` of a record, computed on first use.
    Force(Use, u32),
    MakeClosure {
        func: FuncId,
        caps: Vec<Use>,
    },

    // --- Calls
    Call {
        f: Use,
        args: Vec<Use>,
        named: Vec<(Name, Use)>,
    },
    /// A call to a known function with the right number of arguments.
    CallKnown {
        func: FuncId,
        caps: Vec<Use>,
        args: Vec<Use>,
    },
    CallBuiltin {
        id: usize,
        args: Vec<Use>,
        named: Vec<(Name, Use)>,
    },

    // --- Operators
    Bin(BinOp, Use, Use),
    /// A comparison, giving a bool.
    Cmp(CmpOp, Use, Use),
    Neg(Use),
    Not(Use),
    BitNot(Use),
    /// The value itself if it's a bool; otherwise an error saying `what`
    /// needs one.
    CheckBool(Use, &'static str),
    /// The result of the predicate passed to library function `fname`,
    /// which must be a bool (as the library function itself checks).
    CheckPred(Use, &'static str),
    /// An element read by library function `fname` with no predicate
    /// (`any`, `all`), which must be a bool.
    CheckElem(Use, &'static str),
    /// Whether a library call over `coll` with function `f` can run as a
    /// loop here: `f` is a function and `coll` a list, tuple, set, map,
    /// string or finite range (so the call isn't lazy and can't fail on
    /// the collection).
    Loopable {
        coll: Use,
        f: Use,
    },
    Tuple(Vec<Use>),
    List(Vec<Use>),
    Set(Vec<Use>),
    Map(Vec<(Use, Use)>),
    Format(Vec<FmtPart>),
    Index(Use, Use),
    /// `x.0`
    IndexInt(Use, i64),
    Slice {
        coll: Use,
        lo: Option<Use>,
        hi: Option<Use>,
        inclusive: bool,
    },
    Range {
        lo: Use,
        hi: Option<Use>,
        inclusive: bool,
    },

    // --- Pattern tests (all give bools) and parts
    IsType(Use, TypeName),
    EqConst(Use, Value),
    /// A tuple of exactly `n` items.
    IsTuple(Use, usize),
    /// A list of exactly `n` items, or at least `n` if `at_least`.
    IsList {
        v: Use,
        n: usize,
        at_least: bool,
    },
    /// Item `i` of a tuple or list known to have it.
    Item(Use, usize),
    /// Item `i` from the end of a list known to have it (0 is the last).
    ItemBack(Use, usize),
    /// A list without its first `before` and last `after` items.
    Middle {
        v: Use,
        before: usize,
        after: usize,
    },
    /// A string pattern: `none` if it doesn't match, otherwise a tuple of
    /// the captured values.
    MatchStr(Use, Rc<[StrSeg]>),

    // --- Iteration
    /// An iterator over a collection. `finite` makes an infinite range an
    /// error.
    Iter {
        coll: Use,
        finite: bool,
    },
    /// An empty accumulator; for a list, room for as many items as the
    /// collection in the hint has (if it is a list, tuple or range).
    NewAcc(AccKind, Option<Use>),
    AccPush(Use, Use),
    AccInsert(Use, Use, Use),
    AccDone(Use),
    /// A generator giving the items of `f(x)` for each `x` of `src`.
    FlatMap {
        src: Use,
        f: Use,
    },
}

#[derive(Debug, Clone)]
pub enum Callee {
    Value(Use),
    Known { func: FuncId, caps: Vec<Use> },
}

#[derive(Debug, Clone)]
pub enum Fail {
    /// No clause of the named function matches the arguments.
    NoClause(Name),
    NoArm,
    /// A constant's value doesn't match its pattern (the pattern's source).
    ConstPattern(Rc<str>),
    LambdaArg(Rc<str>),
    Msg(Rc<str>),
}

#[derive(Debug, Clone)]
pub enum Term {
    Jump(BlockId, Vec<Use>),
    /// `cond` is known to be a bool.
    If {
        cond: Use,
        then: BlockId,
        els: BlockId,
    },
    Return(Use),
    TailCall {
        callee: Callee,
        args: Vec<Use>,
        span: Span,
    },
    /// The next item of an iterator, passed to `some` as its parameter, or
    /// `none` when there are no more.
    Next {
        iter: Use,
        some: BlockId,
        none: BlockId,
        span: Span,
    },
    Fail {
        fail: Fail,
        vals: Vec<Use>,
        span: Span,
    },
    /// Not yet filled in, during lowering.
    Unset,
}

impl Op {
    /// Calls `f` on every operand, in evaluation order.
    pub fn uses_mut(&mut self, f: &mut dyn FnMut(&mut Use)) {
        match self {
            Op::Const(_) | Op::Cap(_) | Op::Global(_) | Op::BuiltinValue(_) | Op::SelfTail => {}
            Op::NewAcc(_, hint) => {
                if let Some(u) = hint {
                    f(u);
                }
            }
            Op::NewRecord { parent, vals, .. } => {
                if let Some(p) = parent {
                    f(p);
                }
                vals.iter_mut().for_each(f);
            }
            Op::RecParent(u)
            | Op::RecVal(u, _)
            | Op::Force(u, _)
            | Op::Neg(u)
            | Op::Not(u)
            | Op::BitNot(u)
            | Op::CheckBool(u, _)
            | Op::CheckPred(u, _)
            | Op::CheckElem(u, _)
            | Op::IndexInt(u, _)
            | Op::IsType(u, _)
            | Op::EqConst(u, _)
            | Op::IsTuple(u, _)
            | Op::IsList { v: u, .. }
            | Op::Item(u, _)
            | Op::ItemBack(u, _)
            | Op::Middle { v: u, .. }
            | Op::MatchStr(u, _)
            | Op::Iter { coll: u, .. }
            | Op::AccDone(u) => f(u),
            Op::MakeClosure { caps, .. } => caps.iter_mut().for_each(f),
            Op::Call { f: g, args, named } => {
                f(g);
                args.iter_mut().for_each(&mut *f);
                named.iter_mut().for_each(|(_, u)| f(u));
            }
            Op::CallKnown { caps, args, .. } => {
                caps.iter_mut().for_each(&mut *f);
                args.iter_mut().for_each(f);
            }
            Op::CallBuiltin { args, named, .. } => {
                args.iter_mut().for_each(&mut *f);
                named.iter_mut().for_each(|(_, u)| f(u));
            }
            Op::Bin(_, a, b) | Op::Cmp(_, a, b) | Op::Index(a, b) | Op::AccPush(a, b) => {
                f(a);
                f(b);
            }
            Op::FlatMap { src, f: g } | Op::Loopable { coll: src, f: g } => {
                f(src);
                f(g);
            }
            Op::AccInsert(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            Op::Tuple(xs) | Op::List(xs) | Op::Set(xs) => xs.iter_mut().for_each(f),
            Op::Map(kvs) => kvs.iter_mut().for_each(|(k, v)| {
                f(k);
                f(v);
            }),
            Op::Format(parts) => parts.iter_mut().for_each(|p| {
                if let FmtPart::Val(u) = p {
                    f(u);
                }
            }),
            Op::Slice { coll, lo, hi, .. } => {
                f(coll);
                if let Some(u) = lo {
                    f(u);
                }
                if let Some(u) = hi {
                    f(u);
                }
            }
            Op::Range { lo, hi, .. } => {
                f(lo);
                if let Some(u) = hi {
                    f(u);
                }
            }
        }
    }

    /// Calls `f` on every operand, in evaluation order (read-only).
    pub fn uses(&self, f: &mut dyn FnMut(Use)) {
        match self {
            Op::Const(_) | Op::Cap(_) | Op::Global(_) | Op::BuiltinValue(_) | Op::SelfTail => {}
            Op::NewAcc(_, hint) => {
                if let Some(u) = hint {
                    f(*u);
                }
            }
            Op::NewRecord { parent, vals, .. } => {
                if let Some(p) = parent {
                    f(*p);
                }
                vals.iter().for_each(|u| f(*u));
            }
            Op::RecParent(u)
            | Op::RecVal(u, _)
            | Op::Force(u, _)
            | Op::Neg(u)
            | Op::Not(u)
            | Op::BitNot(u)
            | Op::CheckBool(u, _)
            | Op::CheckPred(u, _)
            | Op::CheckElem(u, _)
            | Op::IndexInt(u, _)
            | Op::IsType(u, _)
            | Op::EqConst(u, _)
            | Op::IsTuple(u, _)
            | Op::IsList { v: u, .. }
            | Op::Item(u, _)
            | Op::ItemBack(u, _)
            | Op::Middle { v: u, .. }
            | Op::MatchStr(u, _)
            | Op::Iter { coll: u, .. }
            | Op::AccDone(u) => f(*u),
            Op::MakeClosure { caps, .. } => caps.iter().for_each(|u| f(*u)),
            Op::Call { f: g, args, named } => {
                f(*g);
                args.iter().for_each(|u| f(*u));
                named.iter().for_each(|(_, u)| f(*u));
            }
            Op::CallKnown { caps, args, .. } => {
                caps.iter().for_each(|u| f(*u));
                args.iter().for_each(|u| f(*u));
            }
            Op::CallBuiltin { args, named, .. } => {
                args.iter().for_each(|u| f(*u));
                named.iter().for_each(|(_, u)| f(*u));
            }
            Op::Bin(_, a, b) | Op::Cmp(_, a, b) | Op::Index(a, b) | Op::AccPush(a, b) => {
                f(*a);
                f(*b);
            }
            Op::FlatMap { src, f: g } | Op::Loopable { coll: src, f: g } => {
                f(*src);
                f(*g);
            }
            Op::AccInsert(a, b, c) => {
                f(*a);
                f(*b);
                f(*c);
            }
            Op::Tuple(xs) | Op::List(xs) | Op::Set(xs) => xs.iter().for_each(|u| f(*u)),
            Op::Map(kvs) => kvs.iter().for_each(|(k, v)| {
                f(*k);
                f(*v);
            }),
            Op::Format(parts) => parts.iter().for_each(|p| {
                if let FmtPart::Val(u) = p {
                    f(*u);
                }
            }),
            Op::Slice { coll, lo, hi, .. } => {
                f(*coll);
                if let Some(u) = lo {
                    f(*u);
                }
                if let Some(u) = hi {
                    f(*u);
                }
            }
            Op::Range { lo, hi, .. } => {
                f(*lo);
                if let Some(u) = hi {
                    f(*u);
                }
            }
        }
    }
}

impl Term {
    pub fn uses_mut(&mut self, f: &mut dyn FnMut(&mut Use)) {
        match self {
            Term::Jump(_, args) => args.iter_mut().for_each(f),
            Term::If { cond, .. } => f(cond),
            Term::Return(u) => f(u),
            Term::TailCall { callee, args, .. } => {
                match callee {
                    Callee::Value(u) => f(u),
                    Callee::Known { caps, .. } => caps.iter_mut().for_each(&mut *f),
                }
                args.iter_mut().for_each(f);
            }
            Term::Next { iter, .. } => f(iter),
            Term::Fail { vals, .. } => vals.iter_mut().for_each(f),
            Term::Unset => {}
        }
    }

    /// Calls `f` on every operand, in evaluation order (read-only).
    pub fn uses(&self, f: &mut dyn FnMut(Use)) {
        match self {
            Term::Jump(_, args) => args.iter().for_each(|u| f(*u)),
            Term::If { cond, .. } => f(*cond),
            Term::Return(u) => f(*u),
            Term::TailCall { callee, args, .. } => {
                match callee {
                    Callee::Value(u) => f(*u),
                    Callee::Known { caps, .. } => caps.iter().for_each(|u| f(*u)),
                }
                args.iter().for_each(|u| f(*u));
            }
            Term::Next { iter, .. } => f(*iter),
            Term::Fail { vals, .. } => vals.iter().for_each(|u| f(*u)),
            Term::Unset => {}
        }
    }

    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Term::Jump(b, _) => vec![*b],
            Term::If { then, els, .. } => vec![*then, *els],
            Term::Next { some, none, .. } => vec![*some, *none],
            Term::Return(_) | Term::TailCall { .. } | Term::Fail { .. } | Term::Unset => vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Printing, for debugging and `tinsel ir`.

impl fmt::Display for Use {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}{}", self.var, if self.last { "!" } else { "" })
    }
}

fn list<T: fmt::Display>(xs: &[T]) -> String {
    xs.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, func) in self.funcs.iter().enumerate() {
            writeln!(
                f,
                "fn{i} {} ({:?}, {} params, {} caps{}):",
                func.name,
                func.kind,
                func.params,
                func.caps,
                match func.memo {
                    Some(m) => format!(", memo {m:?}"),
                    None => String::new(),
                }
            )?;
            for (b, block) in func.blocks.iter().enumerate() {
                let params: Vec<String> = block.params.iter().map(|v| format!("v{v}")).collect();
                write!(f, "  b{b}({}):", params.join(", "))?;
                if !block.drops.is_empty() {
                    let drops: Vec<String> = block.drops.iter().map(|v| format!("v{v}")).collect();
                    write!(f, " drop {}", drops.join(", "))?;
                }
                writeln!(f)?;
                for inst in &block.insts {
                    match inst.dst {
                        Some(d) => writeln!(f, "    v{d} = {}", inst.op)?,
                        None => writeln!(f, "    {}", inst.op)?,
                    }
                }
                writeln!(f, "    {}", block.term)?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::Const(v) => write!(f, "const {}", v.repr()),
            Op::Cap(i) => write!(f, "cap {i}"),
            Op::Global(i) => write!(f, "global {i}"),
            Op::BuiltinValue(i) => write!(f, "builtin {}", crate::builtins::BUILTINS[*i].name),
            Op::NewRecord {
                layout,
                parent,
                vals,
            } => {
                let p = parent.map_or("-".to_string(), |p| p.to_string());
                write!(f, "record L{layout} parent {p} [{}]", list(vals))
            }
            Op::RecParent(u) => write!(f, "parent {u}"),
            Op::RecVal(u, i) => write!(f, "recval {u}.{i}"),
            Op::Force(u, i) => write!(f, "force {u}.{i}"),
            Op::MakeClosure { func, caps } => write!(f, "closure fn{func} [{}]", list(caps)),
            Op::Call { f: g, args, named } => {
                write!(f, "call {g}({})", list(args))?;
                for (n, u) in named {
                    write!(f, " {n}: {u}")?;
                }
                Ok(())
            }
            Op::CallKnown { func, caps, args } => {
                write!(f, "call fn{func}[{}]({})", list(caps), list(args))
            }
            Op::CallBuiltin { id, args, named } => {
                write!(
                    f,
                    "call {}({})",
                    crate::builtins::BUILTINS[*id].name,
                    list(args)
                )?;
                for (n, u) in named {
                    write!(f, " {n}: {u}")?;
                }
                Ok(())
            }
            Op::Bin(op, a, b) => write!(f, "{a} {} {b}", op.symbol()),
            Op::Cmp(op, a, b) => write!(f, "{a} {} {b}", op.symbol()),
            Op::Neg(u) => write!(f, "-{u}"),
            Op::Not(u) => write!(f, "not {u}"),
            Op::BitNot(u) => write!(f, "~{u}"),
            Op::CheckBool(u, what) => write!(f, "bool {u} ({what})"),
            Op::CheckPred(u, fname) => write!(f, "pred {u} ({fname})"),
            Op::CheckElem(u, fname) => write!(f, "elem {u} ({fname})"),
            Op::SelfTail => write!(f, "selftail"),
            Op::Loopable { coll, f: g } => write!(f, "loopable {coll} {g}"),
            Op::Tuple(xs) => write!(f, "({})", list(xs)),
            Op::List(xs) => write!(f, "[{}]", list(xs)),
            Op::Set(xs) => write!(f, "{{{}}}", list(xs)),
            Op::Map(kvs) => {
                let kv: Vec<String> = kvs.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                write!(f, "{{{}}}", kv.join(", "))
            }
            Op::Format(parts) => {
                write!(f, "format")?;
                for p in parts {
                    match p {
                        FmtPart::Lit(s) => write!(f, " {s:?}")?,
                        FmtPart::Val(u) => write!(f, " {u}")?,
                    }
                }
                Ok(())
            }
            Op::Index(a, b) => write!(f, "{a}[{b}]"),
            Op::IndexInt(a, i) => write!(f, "{a}.{i}"),
            Op::Slice {
                coll,
                lo,
                hi,
                inclusive,
            } => {
                let lo = lo.map_or(String::new(), |u| u.to_string());
                let hi = hi.map_or(String::new(), |u| u.to_string());
                let dots = if *inclusive { ".." } else { "..<" };
                write!(f, "{coll}[{lo}{dots}{hi}]")
            }
            Op::Range { lo, hi, inclusive } => {
                let hi = hi.map_or(String::new(), |u| u.to_string());
                let dots = if *inclusive { ".." } else { "..<" };
                write!(f, "{lo}{dots}{hi}")
            }
            Op::IsType(u, t) => write!(f, "is {u} {t:?}"),
            Op::EqConst(u, v) => write!(f, "{u} == {}", v.repr()),
            Op::IsTuple(u, n) => write!(f, "is {u} tuple/{n}"),
            Op::IsList { v, n, at_least } => {
                write!(f, "is {v} list/{}{n}", if *at_least { ">=" } else { "" })
            }
            Op::Item(u, i) => write!(f, "item {u}[{i}]"),
            Op::ItemBack(u, i) => write!(f, "item {u}[-{}]", i + 1),
            Op::Middle { v, before, after } => write!(f, "middle {v}[{before}..-{after}]"),
            Op::MatchStr(u, _) => write!(f, "matchstr {u}"),
            Op::Iter { coll, finite } => {
                write!(f, "iter {coll}{}", if *finite { " finite" } else { "" })
            }
            Op::NewAcc(k, hint) => match hint {
                Some(u) => write!(f, "acc {k:?} sized {u}"),
                None => write!(f, "acc {k:?}"),
            },
            Op::AccPush(a, x) => write!(f, "push {a} {x}"),
            Op::AccInsert(a, k, v) => write!(f, "insert {a} {k} {v}"),
            Op::AccDone(a) => write!(f, "done {a}"),
            Op::FlatMap { src, f: g } => write!(f, "flatmap {src} {g}"),
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Jump(b, args) => write!(f, "jump b{b}({})", list(args)),
            Term::If { cond, then, els } => write!(f, "if {cond} b{then} else b{els}"),
            Term::Return(u) => write!(f, "return {u}"),
            Term::TailCall { callee, args, .. } => match callee {
                Callee::Value(u) => write!(f, "tailcall {u}({})", list(args)),
                Callee::Known { func, caps } => {
                    write!(f, "tailcall fn{func}[{}]({})", list(caps), list(args))
                }
            },
            Term::Next {
                iter, some, none, ..
            } => write!(f, "next {iter} b{some} else b{none}"),
            Term::Fail { fail, vals, .. } => write!(f, "fail {fail:?} ({})", list(vals)),
            Term::Unset => write!(f, "<unset>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Engine, compile};

    fn module(src: &str) -> Module {
        let prog = compile(src.to_string()).expect("compiles");
        lower::lower(&prog).expect("lowers")
    }

    fn func<'m>(m: &'m Module, name: &str) -> &'m Function {
        m.funcs
            .iter()
            .find(|f| &*f.name == name)
            .expect("function exists")
    }

    fn tail_calls(f: &Function) -> usize {
        f.blocks
            .iter()
            .filter(|b| matches!(b.term, Term::TailCall { .. }))
            .count()
    }

    /// `part1` on the IR interpreter, shown.
    fn run(src: &str) -> String {
        let prog = compile(src.to_string()).expect("compiles");
        let engine = Engine::ir(&prog).expect("lowers");
        let mut it = engine.interp(&prog, Err("no input".into()), None);
        match it.global_by_name("part1", Span::default()) {
            Some(Ok(v)) => v.repr(),
            Some(Err(e)) => format!("error: {}", e.msg),
            None => "undefined".into(),
        }
    }

    /// `part1` on the tree-walker, shown.
    fn run_tree(src: &str) -> String {
        run_on(src, |_| Engine::Tree)
    }

    /// `part1` compiled to native code, shown.
    fn run_native(src: &str) -> String {
        run_on(src, |p| Engine::native(p).expect("compiles natively"))
    }

    fn run_on(src: &str, engine: impl Fn(&crate::resolve::Program) -> Engine) -> String {
        let prog = compile(src.to_string()).expect("compiles");
        let mut it = engine(&prog).interp(&prog, Err("no input".into()), None);
        match it.global_by_name("part1", Span::default()) {
            Some(Ok(v)) => v.repr(),
            Some(Err(e)) => format!("error: {}", e.msg),
            None => "undefined".into(),
        }
    }

    #[test]
    fn library_loops_match_the_library() {
        // `filter` and friends become loops in the IR; results and error
        // messages must be the library's.
        for body in [
            "[1, 2, 3].filter(_ > 1)",
            "{3, 1, 2}.map(_ * 2)",
            "\"abca\".count(_ == 'a')",
            "[(1, 2), (3, 4)].map((a, b) => a + b)",
            "(1..5).first(_ > 2)",
            "[1, 2].filter(_ + 1)",
            "[1].count(x => x)",
            "[1].any(x => none)",
            "{1: 2}.all(_ => \"no\")",
            "\"ab\".first(_ => 1)",
            "[1, 2].map(x => 1 / (x - 2))",
            "[1].filter(3)",
            "5.filter(_ > 1)",
            "(1..).filter(_ > 1).take(2)",
            "[3, 1, 2].min_by(x => -x)",
            "{1: 0, 2: 0}.max_by(_ % 2)",
            "[].min_by(_ + 1)",
            "[1, \"a\"].min_by(x => x)",
            "[(1, 'a'), (1, 'b')].max_by(.0)",
            "(1..4).fold(0, (a, x) => a + x)",
            "\"abc\".scan(\"\", (s, c) => s ++ c)",
            "[1, 2].fold([], (acc, x) => acc ++ [x])",
            "[1, 2].fold(0, x => x)",
        ] {
            let src = format!("part1 = {body}");
            let expected = run_tree(&src);
            assert_eq!(run(&src), expected, "IR: {body}");
            assert_eq!(run_native(&src), expected, "native: {body}");
        }
    }

    #[test]
    fn repeats_are_computed_once() {
        let m = module("f(m, k) = m[k] + m[k] * m[k]");
        let indexes = func(&m, "f")
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i.op, Op::Index(..)))
            .count();
        assert_eq!(indexes, 1);
    }

    #[test]
    fn cse_keeps_results() {
        // Reusing results must never show: calls (which may give a fresh,
        // single-pass generator) and closures (compared by identity) are
        // never merged, branches don't share, and errors stay put.
        for src in [
            "g() = (x for x in 1..3)\npart1 = (g().first, g().first, list(g()))",
            "f(n) = (x => x + n) == (x => x + n)\npart1 = f(1)",
            "f(m, k) = if k > 0 then m[k] else m[k] + 1\npart1 = (f({1: 2}, 1), f({0: 5}, 0))",
            "f(m, k) = if k > 5 then m[k] else 0\npart1 = (f({}, 1), f({}, 9))",
            "f(xs) = (xs[0] + xs[0], xs.len, xs[0])\npart1 = (f([4]), f([]))",
            "f(t) = (t.0, t.1, t.0 + t.1, t.0)\npart1 = f((2, 3))",
            "f(s) = (s | {1}, s | {1}, s - {1})\npart1 = f({1, 2})",
        ] {
            assert_eq!(run(src), run_tree(src), "IR: {src}");
            assert_eq!(run_native(src), run_tree(src), "native: {src}");
        }
    }

    #[test]
    fn loops_and_small_builtins_match() {
        // Loops over ranges, lists and tuples keep their state in machine
        // variables in compiled code; `min`, `max` and `abs` of small ints
        // are inline. All must match the tree-walker.
        for body in [
            "[x * 2 for x in 1..5]",
            "[x for x in 5..1]",
            "[x for x in 1..<1]",
            "[x for x in (2 ** 61 - 3)..(2 ** 61 + 1)]",
            "[x for x in (-2 ** 61)..(-2 ** 61 + 2)]",
            "[(x, y) for x in [1, 2] for y in (3, 4)]",
            "[c for c in \"héllo\"]",
            "[x for x in {3, 1, 2}] ++ [k for k in {2: 0, 1: 0}]",
            "[x for x in []]",
            "[1, 2, 3, 4].first(_ > 2)",
            "(1..1000000).any(_ == 3)",
            "[x for x in 1..]",
            "[x for x in 5]",
            "(min(3, 3), max(-1, 2), min(2, -5), max(7, 7))",
            "(abs(-7), abs(0), abs(-2 ** 61), abs(2 ** 61 - 1), abs(-2 ** 70))",
            "(min(1, \"a\"))",
            "abs(\"x\")",
            "([1, 2, 3, 4][1..], [1, 2, 3, 4][..<2], \"hello\"[1..3], [1, 2][5..])",
            "[1, 2, 3][\"a\"..]",
            "(\"abc\".chars, \"héllo\".chars, \"\".chars)",
            "[x for x in 1..100000 if x % 50000 == 0]",
            "[y for x in [1, 2] for y in [x, x, x]]",
            "[x for x in {2, 1}] ++ [k for k in {\"b\": 1}] ++ [c for c in \"xy\"]",
            "[x for x in (y * 2 for y in 1..3)]",
            "[1, 2].map(_ + 1) ++ (1..3).filter(_ > 1) ++ [3, 1].scan(0, (a, x) => a + x)",
        ] {
            let src = format!("part1 = {body}");
            let expected = run_tree(&src);
            assert_eq!(run(&src), expected, "IR: {body}");
            assert_eq!(run_native(&src), expected, "native: {body}");
        }
    }

    #[test]
    fn self_tail_calls_are_loops() {
        let m = module("count(n, acc) = if n == 0 then acc else count(n - 1, acc + 1)");
        let f = func(&m, "count");
        assert_eq!(tail_calls(f), 0);
        assert!(
            f.blocks
                .iter()
                .any(|b| matches!(b.term, Term::Jump(0, ref a) if a.len() == 2))
        );
        // So a million iterations run without a million calls.
        assert_eq!(
            run(
                "count(n, acc) = if n == 0 then acc else count(n - 1, acc + 1)\npart1 = count(1000000, 0)"
            ),
            "1000000"
        );
    }

    #[test]
    fn where_functions_loop_too() {
        let m =
            module("f(n) = go(n, 0)\n  where\n    go(0, a) = a\n    go(k, a) = go(k - 1, a + n)");
        assert_eq!(tail_calls(func(&m, "go")), 0);
    }

    #[test]
    fn memoized_self_tail_calls_stay_calls() {
        // `fib` is memoized (a recursive call not in tail position), so
        // each call's result must reach the memo table.
        let src = "fib(n) = if n < 2 then n else fib(n - 1) + fib(n - 2)\n\
                   f(n) = if n == 0 then fib(30) else f(n - 1)";
        let m = module(src);
        assert!(func(&m, "fib").memo.is_some());
        assert!(func(&m, "f").memo.is_none());
        assert_eq!(run(&format!("{src}\npart1 = f(3)")), "832040");
    }

    #[test]
    fn last_uses_are_moves() {
        let m = module("f(xs) = xs.set(0, xs.len)");
        let f = func(&m, "f");
        let mut uses = Vec::new();
        for b in &f.blocks {
            for i in &b.insts {
                if let Op::CallBuiltin { args, .. } = &i.op {
                    uses.extend(args.iter().map(|u| (u.var, u.last)));
                }
            }
        }
        // `xs.len` copies `xs`; `set` then takes it.
        assert!(uses.contains(&(0, false)));
        assert!(uses.contains(&(0, true)));
    }

    #[test]
    fn where_constants_are_lazy_and_shared() {
        let src = "f(n) = if n > 0 then a + b else 0\n  where\n    (a, b) = (n, n * 2)\n    boom = 1 / 0\npart1 = (f(2), f(0))";
        assert_eq!(run(src), "(6, 0)");
    }

    #[test]
    fn constant_cycles_are_reported() {
        let r = run("part1 = a\na = b + 1\nb = a + 1");
        assert!(r.contains("depends on itself"), "{r}");
        let r = run("part1 = f(1)\nf(n) = x\n  where\n    x = y\n    y = x");
        assert!(r.contains("depends on itself"), "{r}");
    }

    #[test]
    fn generators_are_lazy() {
        let src = "part1 = (x * x for x in (1..) if x % 3 == 0).take(3)";
        assert_eq!(run(src), "[9, 36, 81]");
        // Nothing runs until the generator is read.
        assert_eq!(run("part1 = ((1 / 0 for x in [1]), 5).1"), "5");
    }
}
