//! Runtime values (SPEC 4). All values are immutable and cheap to clone.
//!
//! A [`Value`] is either an immediate (`none`, a bool, a char, a small int,
//! `inf`, `-inf`) or a reference-counted [`Heap`] object. Code reads a value
//! through [`Value::view`], which borrows, or takes it apart with
//! [`Value::unpack`], which moves the contents out when the value is the
//! only reference to its object (so updates at a last use happen in place,
//! SPEC 4.1).
//!
//! `Value` implements `Eq`, `Ord` and `Hash` structurally so it can be used
//! as an `imbl` map key or memo key. The `Ord` impl is total (across types it
//! orders by a type tag) and is for internal use only; user-visible ordering
//! goes through [`compare`], which reports cross-type comparisons as errors
//! (SPEC 4.1).

use crate::ast::{FnDef, Lambda};
use crate::eval::{Frame, Generator, Memo};
use crate::int::Int;
pub use crate::list::List;
use word::{Kind, Word};

#[allow(unsafe_code)]
pub mod word;
use imbl::{OrdMap, OrdSet};
use num_bigint::BigInt;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt::{self, Write};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// A Tinsel value: one tagged word (see [`word`]).
#[derive(Clone)]
pub struct Value(Word);

/// The objects behind heap values.
///
/// `repr(u8)` gives the enum a defined layout (a tag byte first), so that
/// compiled code can read a few simple objects itself (see [`layout`]).
#[derive(Clone)]
#[repr(u8)]
pub enum Heap {
    Big(Rc<BigInt>) = 0,
    /// An `i64` outside the fixnum range.
    Wide(i64) = 1,
    Str(Rc<str>) = 2,
    Tuple(Tup) = layout::TUPLE,
    List(List) = 4,
    Set(OrdSet<Value>) = 5,
    Map(OrdMap<Value, Value>) = 6,
    Range(Rc<Range>) = 7,
    Func(Rc<Func>) = 8,
    Gen(Rc<RefCell<Generator>>) = 9,
    Grid(Rc<Grid>) = 10,
    RangeSet(Rc<RangeSet>) = 11,
    /// An object of the IR engines that is never a Tinsel value.
    Internal(Internal) = layout::INTERNAL,
}

/// What compiled code needs to know to read heap objects itself: tags
/// (fixed by the `repr(u8)` enums) and byte offsets (measured, since Rust
/// doesn't promise where a variant's fields go).
pub mod layout {
    use super::{Heap, Internal, Tup, Value};

    pub const TUPLE: u8 = 3;
    pub const INTERNAL: u8 = 12;
    pub const TUP_TWO: u8 = 0;
    pub const TUP_THREE: u8 = 1;
    pub const TUP_OTHER: u8 = 2;
    pub const INTERNAL_REC: u8 = 0;

    /// Offsets from the start of a [`Heap`] object.
    #[derive(Debug, Clone, Copy)]
    pub struct Offsets {
        /// The `Tup` of a tuple (its tag byte comes first).
        pub tup: i32,
        /// The items of a `Tup::Two` and a `Tup::Three`.
        pub two: i32,
        pub three: i32,
        /// The data pointer and length of a `Tup::Other`'s boxed items.
        pub other_ptr: i32,
        pub other_len: i32,
        /// The `Internal` of an internal object.
        pub internal: i32,
    }

    fn offset<T>(base: &Heap, field: *const T) -> i32 {
        (field.addr() - std::ptr::from_ref(base).addr()) as i32
    }

    /// Measures the offsets on sample objects.
    pub fn offsets() -> Offsets {
        let two = Heap::Tuple(Tup::Two([Value::NONE, Value::NONE]));
        let three = Heap::Tuple(Tup::Three([Value::NONE, Value::NONE, Value::NONE]));
        let (Heap::Tuple(t2 @ Tup::Two(a2)), Heap::Tuple(Tup::Three(a3))) = (&two, &three) else {
            unreachable!("built as tuples above")
        };
        let long = Heap::Tuple(Tup::Other(vec![Value::NONE; 4].into_boxed_slice()));
        let Heap::Tuple(Tup::Other(b)) = &long else {
            unreachable!("built as a long tuple above")
        };
        let boxed = offset(&long, std::ptr::from_ref(b));
        let (p, l) = super::word::boxed_slice_layout();
        let acc = Heap::Internal(Internal::Acc(crate::ir::interp::Acc::List(Vec::new())));
        let Heap::Internal(x) = &acc else {
            unreachable!("built as internal above")
        };
        Offsets {
            tup: offset(&two, t2),
            two: offset(&two, a2.as_ptr()),
            three: offset(&three, a3.as_ptr()),
            other_ptr: boxed + p,
            other_len: boxed + l,
            internal: offset(&acc, x),
        }
    }
}

/// What the IR interpreter and compiled code keep in variables besides
/// values: `where` records, iterators and accumulators. The lowering never
/// lets one reach a Tinsel value, so library code never sees one.
#[derive(Clone)]
#[repr(u8)]
pub enum Internal {
    Rec(crate::ir::interp::Record) = layout::INTERNAL_REC,
    Iter(Rc<RefCell<crate::eval::ValIter>>) = 1,
    Acc(crate::ir::interp::Acc) = 2,
}

/// A tuple's items, stored inline when there are two or three of them
/// (positions and the like), so reading one costs a single indirection.
/// Only [`Tup::new`] makes one, so a tuple of two or three items is always
/// `Two` or `Three`; compiled code relies on this.
#[derive(Clone)]
#[repr(u8)]
pub enum Tup {
    Two([Value; 2]) = layout::TUP_TWO,
    Three([Value; 3]) = layout::TUP_THREE,
    Other(Box<[Value]>) = layout::TUP_OTHER,
}

impl Tup {
    fn new(items: Vec<Value>) -> Tup {
        match <[Value; 3]>::try_from(items) {
            Ok(a) => Tup::Three(a),
            Err(items) => match <[Value; 2]>::try_from(items) {
                Ok(a) => Tup::Two(a),
                Err(items) => Tup::Other(items.into_boxed_slice()),
            },
        }
    }

    pub fn as_slice(&self) -> &[Value] {
        match self {
            Tup::Two(a) => a,
            Tup::Three(a) => a,
            Tup::Other(a) => a,
        }
    }

    pub fn into_vec(self) -> Vec<Value> {
        match self {
            Tup::Two(a) => a.into(),
            Tup::Three(a) => a.into(),
            Tup::Other(a) => a.into_vec(),
        }
    }
}

/// A value seen by reference: what [`Value::view`] returns.
/// A tuple's items, as [`V::Tuple`] gives them: borrowed from a heap tuple,
/// or unpacked from a packed pair (see [`word`]). Either way they read as
/// a slice.
#[derive(Clone)]
pub enum Items<'a> {
    Slice(&'a [Value]),
    Pair(word::PairItems),
}

impl std::ops::Deref for Items<'_> {
    type Target = [Value];
    fn deref(&self) -> &[Value] {
        match self {
            Items::Slice(s) => s,
            Items::Pair(p) => p.as_slice(),
        }
    }
}

#[derive(Clone)]
pub enum V<'a> {
    None,
    Bool(bool),
    /// Ints are given by value; cloning a big one copies only a pointer.
    Int(Int),
    Char(char),
    Str(&'a Rc<str>),
    Tuple(Items<'a>),
    List(&'a List),
    Set(&'a OrdSet<Value>),
    Map(&'a OrdMap<Value, Value>),
    Range(&'a Rc<Range>),
    Func(&'a Rc<Func>),
    Gen(&'a Rc<RefCell<Generator>>),
    Grid(&'a Rc<Grid>),
    RangeSet(&'a Rc<RangeSet>),
    Internal(&'a Internal),
}

/// A value taken apart: what [`Value::unpack`] returns. A heap object's
/// contents are moved out when the value held the only reference, and
/// cloned (cheaply: every payload is shared or persistent) otherwise.
pub enum Own {
    None,
    Bool(bool),
    Int(Int),
    Char(char),
    Str(Rc<str>),
    Tuple(Tup),
    List(List),
    Set(OrdSet<Value>),
    Map(OrdMap<Value, Value>),
    Range(Rc<Range>),
    Func(Rc<Func>),
    Gen(Rc<RefCell<Generator>>),
    Grid(Rc<Grid>),
    RangeSet(Rc<RangeSet>),
    Internal(Internal),
}

/// `start..end` with an inclusive end, or `start..` when `end` is `None`.
/// Exclusive ranges are normalised to inclusive ones when created, and
/// `exclusive` records that one was written with `..<`, so that it
/// displays as written (SPEC 10). That can't work for an end of `inf` or
/// `-inf`, which `- 1` leaves unchanged, so there the flag also excludes the
/// end ([`Range::excludes_end`]): `inf..<inf` is empty. Otherwise equality,
/// ordering and hashing ignore it, so `0..<5 == 0..4`. They compare
/// elements, not spelling ([`Range::key`]).
#[derive(Debug, Clone)]
pub struct Range {
    pub start: Int,
    pub end: Option<Int>,
    pub exclusive: bool,
}

impl PartialEq for Range {
    fn eq(&self, o: &Range) -> bool {
        self.key() == o.key()
    }
}

impl Eq for Range {}

impl std::hash::Hash for Range {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.key().hash(h);
    }
}

impl PartialOrd for Range {
    fn partial_cmp(&self, o: &Range) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Range {
    fn cmp(&self, o: &Range) -> Ordering {
        self.key().cmp(&o.key())
    }
}

impl Range {
    /// What equality, ordering and hashing compare: the elements, so every
    /// empty range has the same key (`None`), and an inclusive end of `inf`
    /// is the same as no end (`1..inf == 1..`).
    fn key(&self) -> Option<(&Int, Option<&Int>, bool)> {
        if self.is_empty() {
            return None;
        }
        let end = self
            .end
            .as_ref()
            .filter(|e| self.excludes_end() || !matches!(e, Int::Inf(true)));
        Some((&self.start, end, self.excludes_end()))
    }

    /// Whether `end` itself is left out: an exclusive range ending at `inf`
    /// or `-inf`. (A finite exclusive end was already moved down by one.)
    pub fn excludes_end(&self) -> bool {
        self.exclusive && self.end.as_ref().is_some_and(Int::is_inf)
    }

    pub fn contains(&self, i: &Int) -> bool {
        *i >= self.start
            && self
                .end
                .as_ref()
                .is_none_or(|e| i < e || (i == e && !self.excludes_end()))
    }

    /// Whether the range has no elements (its end is below its start).
    pub fn is_empty(&self) -> bool {
        self.end
            .as_ref()
            .is_some_and(|e| *e < self.start || (*e == self.start && self.excludes_end()))
    }

    /// The end, unless the range never ends: `a..` and `a..inf` alike.
    pub fn finite_end(&self) -> Option<&Int> {
        self.end.as_ref().filter(|e| !matches!(e, Int::Inf(true)))
    }

    /// Number of elements, or `None` if the range is infinite: it never
    /// ends, or (not being empty) starts at `-inf` or `inf`.
    pub fn len(&self) -> Option<Int> {
        if self.is_empty() {
            return Some(Int::Small(0));
        }
        if self.start.is_inf() {
            return None;
        }
        let end = self.finite_end()?;
        end.sub(&self.start).ok()?.add(&Int::Small(1)).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    /// Row-major cells.
    pub cells: Vec<Value>,
}

/// A set of ints as sorted, disjoint, inclusive spans, merged so that no two
/// touch (SPEC 9.8). Spans are always finite.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RangeSet {
    pub spans: Vec<(Int, Int)>,
}

fn succ(i: &Int) -> Int {
    i.add(&Int::Small(1)).unwrap_or_else(|_| i.clone())
}

fn pred(i: &Int) -> Int {
    i.sub(&Int::Small(1)).unwrap_or_else(|_| i.clone())
}

impl RangeSet {
    /// Sorts and merges any finite spans; empty ones are dropped.
    pub fn from_spans(mut spans: Vec<(Int, Int)>) -> RangeSet {
        spans.retain(|(lo, hi)| lo <= hi);
        spans.sort();
        let mut out: Vec<(Int, Int)> = Vec::with_capacity(spans.len());
        for (lo, hi) in spans {
            match out.last_mut() {
                Some(last) if lo <= succ(&last.1) => {
                    if hi > last.1 {
                        last.1 = hi;
                    }
                }
                _ => out.push((lo, hi)),
            }
        }
        RangeSet { spans: out }
    }

    pub fn contains(&self, x: &Int) -> bool {
        let i = self.spans.partition_point(|(lo, _)| lo <= x);
        i > 0 && *x <= self.spans[i - 1].1
    }

    /// How many ints the set contains.
    pub fn len(&self) -> Int {
        let mut n = Int::Small(0);
        for (lo, hi) in &self.spans {
            if let Ok(d) = hi
                .sub(lo)
                .and_then(|d| d.add(&Int::Small(1)))
                .and_then(|d| n.add(&d))
            {
                n = d;
            }
        }
        n
    }

    pub fn union(&self, other: &RangeSet) -> RangeSet {
        RangeSet::from_spans(self.spans.iter().chain(&other.spans).cloned().collect())
    }

    pub fn intersection(&self, other: &RangeSet) -> RangeSet {
        let (a, b) = (&self.spans, &other.spans);
        let (mut i, mut j) = (0, 0);
        let mut out = Vec::new();
        while i < a.len() && j < b.len() {
            let lo = a[i].0.clone().max(b[j].0.clone());
            let hi = a[i].1.clone().min(b[j].1.clone());
            if lo <= hi {
                out.push((lo, hi));
            }
            if a[i].1 < b[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        RangeSet { spans: out }
    }

    pub fn difference(&self, other: &RangeSet) -> RangeSet {
        let b = &other.spans;
        let mut j = 0;
        let mut out = Vec::new();
        for (lo, hi) in &self.spans {
            let mut cur = lo.clone();
            while j < b.len() && b[j].1 < cur {
                j += 1;
            }
            let mut k = j;
            while k < b.len() && b[k].0 <= *hi && cur <= *hi {
                if b[k].0 > cur {
                    out.push((cur.clone(), pred(&b[k].0)));
                }
                cur = cur.max(succ(&b[k].1));
                k += 1;
            }
            if cur <= *hi {
                out.push((cur, hi.clone()));
            }
        }
        RangeSet { spans: out }
    }
}

pub enum Func {
    User {
        def: Rc<FnDef>,
        env: Option<Rc<Frame>>,
        memo: Option<Rc<Memo>>,
    },
    Lambda {
        lam: Rc<Lambda>,
        env: Rc<Frame>,
    },
    Builtin(usize),
    /// A closure of the IR interpreter.
    Ir(crate::ir::interp::Closure),
}

impl Func {
    pub fn name(&self) -> String {
        match self {
            Func::User { def, .. } => def.name.to_string(),
            Func::Lambda { .. } => "<lambda>".into(),
            Func::Builtin(id) => crate::builtins::BUILTINS[*id].name.to_string(),
            Func::Ir(c) => c.name.to_string(),
        }
    }
}

impl Heap {
    fn into_own(self) -> Own {
        match self {
            Heap::Big(b) => Own::Int(Int::Big(b)),
            Heap::Wide(i) => Own::Int(Int::Small(i)),
            Heap::Str(s) => Own::Str(s),
            Heap::Tuple(t) => Own::Tuple(t),
            Heap::List(l) => Own::List(l),
            Heap::Set(s) => Own::Set(s),
            Heap::Map(m) => Own::Map(m),
            Heap::Range(r) => Own::Range(r),
            Heap::Func(f) => Own::Func(f),
            Heap::Gen(g) => Own::Gen(g),
            Heap::Grid(g) => Own::Grid(g),
            Heap::RangeSet(r) => Own::RangeSet(r),
            Heap::Internal(x) => Own::Internal(x),
        }
    }

    fn cloned(&self) -> Own {
        match self {
            Heap::Big(b) => Own::Int(Int::Big(b.clone())),
            Heap::Wide(i) => Own::Int(Int::Small(*i)),
            Heap::Str(s) => Own::Str(s.clone()),
            Heap::Tuple(t) => Own::Tuple(t.clone()),
            Heap::List(l) => Own::List(l.clone()),
            Heap::Set(s) => Own::Set(s.clone()),
            Heap::Map(m) => Own::Map(m.clone()),
            Heap::Range(r) => Own::Range(r.clone()),
            Heap::Func(f) => Own::Func(f.clone()),
            Heap::Gen(g) => Own::Gen(g.clone()),
            Heap::Grid(g) => Own::Grid(g.clone()),
            Heap::RangeSet(r) => Own::RangeSet(r.clone()),
            Heap::Internal(x) => Own::Internal(x.clone()),
        }
    }
}

/// Whether a new heap object gets the watch flag (see [`word`]): `where`
/// records and the closures that capture them, which reference cycles can
/// run through.
fn watched(h: &Heap) -> bool {
    match h {
        Heap::Internal(Internal::Rec(_)) => true,
        Heap::Func(f) => matches!(&**f, Func::Ir(c) if c.watch),
        _ => false,
    }
}

/// Whether dropping a reference to the watched object `h` needs the cycle
/// check now (`ir::interp::release_cycle`).
fn may_close_cycle(h: &Heap) -> bool {
    crate::ir::interp::may_close_cycle(h)
}

/// Called by [`Word`]'s `drop` for such an object, with its word and
/// reference count, before releasing the reference.
fn release_cycle(h: &Heap, bits: u64, count: usize) {
    crate::ir::interp::release_cycle(h, bits, count);
}

fn heap(h: Heap) -> Value {
    Value(Word::heap(Rc::new(h)))
}

impl Value {
    pub const NONE: Value = Value(Word::NONE);
    pub const TRUE: Value = Value(Word::bool(true));
    pub const FALSE: Value = Value(Word::bool(false));

    pub const fn bool(b: bool) -> Value {
        Value(Word::bool(b))
    }

    pub const fn char(c: char) -> Value {
        Value(Word::char(c))
    }

    #[inline]
    pub fn int(i: i64) -> Value {
        match Word::fixnum(i) {
            Some(w) => Value(w),
            None => heap(Heap::Wide(i)),
        }
    }

    pub fn from_int(i: Int) -> Value {
        match i {
            Int::Small(i) => Value::int(i),
            Int::Big(b) => heap(Heap::Big(b)),
            Int::Inf(p) => Value(Word::inf(p)),
        }
    }

    pub fn str(s: &str) -> Value {
        heap(Heap::Str(s.into()))
    }

    pub fn from_str_rc(s: Rc<str>) -> Value {
        heap(Heap::Str(s))
    }

    pub fn string(s: String) -> Value {
        heap(Heap::Str(s.into()))
    }

    pub fn tuple(items: Vec<Value>) -> Value {
        if let [a, b] = items.as_slice()
            && let Some(p) = Value::packed(a, b)
        {
            return p;
        }
        heap(Heap::Tuple(Tup::new(items)))
    }

    /// `(a, b)` as a packed pair, if both are small enough ints.
    #[inline]
    fn packed(a: &Value, b: &Value) -> Option<Value> {
        Word::pair(a.0.as_fix()?, b.0.as_fix()?).map(Value)
    }

    /// A tuple of the items drained from `items`, which keeps its
    /// allocation for reuse.
    pub fn tuple_drain(items: &mut Vec<Value>) -> Value {
        let t = match items.len() {
            2 | 3 => {
                let mut d = items.drain(..);
                match (d.next(), d.next(), d.next()) {
                    (Some(a), Some(b), None) => return Value::pair(a, b),
                    (Some(a), Some(b), Some(c)) => Tup::Three([a, b, c]),
                    _ => Tup::Other(Box::new([])),
                }
            }
            _ => Tup::Other(items.drain(..).collect()),
        };
        heap(Heap::Tuple(t))
    }

    pub fn from_tup(t: Tup) -> Value {
        match t {
            Tup::Two([a, b]) => Value::pair(a, b),
            t => heap(Heap::Tuple(t)),
        }
    }

    pub fn pair(a: Value, b: Value) -> Value {
        Value::packed(&a, &b).unwrap_or_else(|| heap(Heap::Tuple(Tup::Two([a, b]))))
    }

    pub fn list(items: impl Into<List>) -> Value {
        heap(Heap::List(items.into()))
    }

    pub fn set(s: OrdSet<Value>) -> Value {
        heap(Heap::Set(s))
    }

    pub fn map(m: OrdMap<Value, Value>) -> Value {
        heap(Heap::Map(m))
    }

    pub fn range(r: Range) -> Value {
        heap(Heap::Range(Rc::new(r)))
    }

    pub fn range_rc(r: Rc<Range>) -> Value {
        heap(Heap::Range(r))
    }

    pub fn func(f: Func) -> Value {
        heap(Heap::Func(Rc::new(f)))
    }

    pub fn func_rc(f: Rc<Func>) -> Value {
        heap(Heap::Func(f))
    }

    pub fn generator(g: Generator) -> Value {
        heap(Heap::Gen(Rc::new(RefCell::new(g))))
    }

    pub fn gen_rc(g: Rc<RefCell<Generator>>) -> Value {
        heap(Heap::Gen(g))
    }

    pub fn grid(g: Grid) -> Value {
        heap(Heap::Grid(Rc::new(g)))
    }

    pub fn grid_rc(g: Rc<Grid>) -> Value {
        heap(Heap::Grid(g))
    }

    pub fn rangeset(r: RangeSet) -> Value {
        heap(Heap::RangeSet(Rc::new(r)))
    }

    pub fn rangeset_rc(r: Rc<RangeSet>) -> Value {
        heap(Heap::RangeSet(r))
    }

    pub fn internal(x: Internal) -> Value {
        heap(Heap::Internal(x))
    }

    pub fn as_internal(&self) -> Option<&Internal> {
        match self.0.kind() {
            Kind::Heap(Heap::Internal(x)) => Some(x),
            _ => None,
        }
    }

    /// The value, by reference.
    #[inline]
    pub fn view(&self) -> V<'_> {
        match self.0.kind() {
            Kind::Fix(i) => V::Int(Int::Small(i)),
            Kind::None => V::None,
            Kind::Bool(b) => V::Bool(b),
            Kind::Char(c) => V::Char(c),
            Kind::Inf(p) => V::Int(Int::Inf(p)),
            Kind::Pair(x, y) => V::Tuple(Items::Pair(word::PairItems::new(x, y))),
            Kind::Heap(h) => match h {
                Heap::Big(b) => V::Int(Int::Big(b.clone())),
                Heap::Wide(i) => V::Int(Int::Small(*i)),
                Heap::Str(s) => V::Str(s),
                Heap::Tuple(t) => V::Tuple(Items::Slice(t.as_slice())),
                Heap::List(l) => V::List(l),
                Heap::Set(s) => V::Set(s),
                Heap::Map(m) => V::Map(m),
                Heap::Range(r) => V::Range(r),
                Heap::Func(f) => V::Func(f),
                Heap::Gen(g) => V::Gen(g),
                Heap::Grid(g) => V::Grid(g),
                Heap::RangeSet(r) => V::RangeSet(r),
                Heap::Internal(x) => V::Internal(x),
            },
        }
    }

    /// The value taken apart, moving its contents out when this is the
    /// only reference to them.
    pub fn unpack(self) -> Own {
        match self.0.into_rc() {
            Ok(h) => match Rc::try_unwrap(h) {
                Ok(h) => h.into_own(),
                Err(h) => h.cloned(),
            },
            Err(w) => match w.kind() {
                Kind::Fix(i) => Own::Int(Int::Small(i)),
                Kind::None | Kind::Heap(_) => Own::None,
                Kind::Bool(b) => Own::Bool(b),
                Kind::Char(c) => Own::Char(c),
                Kind::Inf(p) => Own::Int(Int::Inf(p)),
                Kind::Pair(x, y) => Own::Tuple(Tup::Two([Value::int(x), Value::int(y)])),
            },
        }
    }

    /// The `i64` this value holds, if it is an int that fits in one.
    #[inline]
    pub fn as_small(&self) -> Option<i64> {
        match self.0.kind() {
            Kind::Fix(i) => Some(i),
            Kind::Heap(Heap::Wide(i)) => Some(*i),
            _ => None,
        }
    }

    /// The bool this value holds, if it is one.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.0.kind() {
            Kind::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// The int this value holds, if it is one.
    pub fn as_int(&self) -> Option<Int> {
        match self.view() {
            V::Int(i) => Some(i),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        self.0.bits() == Word::NONE.bits()
    }

    /// The heap object behind this value, for an update in place. If
    /// something else also refers to it, this value first gets its own copy
    /// (a cheap one: every payload is shared or persistent).
    pub fn make_mut(&mut self) -> Option<&mut Heap> {
        self.0.make_mut()
    }

    /// An identity for this value's heap object and how many values refer
    /// to it, or `None` for an unboxed value. For the evaluator's cycle
    /// check.
    pub fn heap_refs(&self) -> Option<(u64, usize)> {
        Some((self.0.bits(), self.0.ref_count()?))
    }

    /// Whether two values are the same object (or equal immediates).
    pub fn same(&self, o: &Value) -> bool {
        self.0.bits() == o.0.bits() || (self.0.is_unboxed() && self == o)
    }

    pub fn type_name(&self) -> &'static str {
        self.view().type_name()
    }

    fn tag(&self) -> u8 {
        self.view().tag()
    }

    /// Display form, as produced by `str(x)` and string interpolation.
    pub fn display(&self) -> String {
        match self.view() {
            V::Str(s) => s.to_string(),
            V::Char(c) => c.to_string(),
            _ => self.repr(),
        }
    }

    /// Form used inside collections: strings and chars are quoted.
    pub fn repr(&self) -> String {
        let mut s = String::new();
        self.write_repr(&mut s);
        s
    }

    fn write_repr(&self, out: &mut String) {
        fn seq<'a>(
            out: &mut String,
            items: impl Iterator<Item = &'a Value>,
            open: &str,
            close: &str,
        ) {
            out.push_str(open);
            for (i, v) in items.enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                v.write_repr(out);
            }
            out.push_str(close);
        }
        match self.view() {
            V::None => out.push_str("none"),
            V::Bool(b) => out.push_str(if b { "true" } else { "false" }),
            V::Int(i) => {
                let _ = write!(out, "{i}");
            }
            V::Char(c) => {
                let _ = write!(out, "{c:?}");
            }
            V::Str(s) => {
                let _ = write!(out, "{:?}", &**s);
            }
            V::Tuple(items) => {
                if items.len() == 1 {
                    out.push('(');
                    items[0].write_repr(out);
                    out.push_str(",)");
                } else {
                    seq(out, items.iter(), "(", ")");
                }
            }
            V::List(items) => seq(out, items.iter(), "[", "]"),
            V::Set(items) => {
                if items.is_empty() {
                    out.push_str("set()");
                } else {
                    seq(out, items.iter(), "{", "}");
                }
            }
            V::Map(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write_repr(out);
                    out.push_str(": ");
                    v.write_repr(out);
                }
                out.push('}');
            }
            V::Range(r) => {
                let _ = write!(out, "{}..", r.start);
                match &r.end {
                    // The end as written: one past the last element.
                    Some(e) if r.exclusive => {
                        let _ = write!(out, "<{}", succ(e));
                    }
                    Some(e) => {
                        let _ = write!(out, "{e}");
                    }
                    None => {}
                }
            }
            V::Func(f) => {
                let _ = write!(out, "<function {}>", f.name());
            }
            V::Gen(_) => out.push_str("<generator>"),
            V::Grid(g) => {
                let _ = write!(out, "<grid {}x{}>", g.width, g.height);
            }
            V::RangeSet(r) => {
                out.push_str("rangeset([");
                for (i, (lo, hi)) in r.spans.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    let _ = write!(out, "{lo}..{hi}");
                }
                out.push_str("])");
            }
            V::Internal(_) => out.push_str("<internal>"),
        }
    }
}

impl V<'_> {
    pub fn type_name(&self) -> &'static str {
        match self {
            V::None => "none",
            V::Bool(_) => "bool",
            V::Int(_) => "int",
            V::Char(_) => "char",
            V::Str(_) => "str",
            V::Tuple(_) => "tuple",
            V::List(_) => "list",
            V::Set(_) => "set",
            V::Map(_) => "map",
            V::Range(_) => "range",
            V::Func(_) => "function",
            V::Gen(_) => "generator",
            V::Grid(_) => "grid",
            V::RangeSet(_) => "rangeset",
            V::Internal(_) => "internal",
        }
    }

    fn tag(&self) -> u8 {
        match self {
            V::None => 0,
            V::Bool(_) => 1,
            V::Int(_) => 2,
            V::Char(_) => 3,
            V::Str(_) => 4,
            V::Tuple(_) => 5,
            V::List(_) => 6,
            V::Set(_) => 7,
            V::Map(_) => 8,
            V::Range(_) => 9,
            V::Func(_) => 10,
            V::Gen(_) => 11,
            V::Grid(_) => 12,
            V::RangeSet(_) => 13,
            V::Internal(_) => 14,
        }
    }
}

impl From<Int> for Value {
    fn from(i: Int) -> Value {
        Value::from_int(i)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::bool(b)
    }
}

impl From<Own> for Value {
    fn from(o: Own) -> Value {
        match o {
            Own::None => Value::NONE,
            Own::Bool(b) => Value::bool(b),
            Own::Int(i) => Value::from_int(i),
            Own::Char(c) => Value::char(c),
            Own::Str(s) => heap(Heap::Str(s)),
            Own::Tuple(t) => Value::from_tup(t),
            Own::List(l) => heap(Heap::List(l)),
            Own::Set(s) => heap(Heap::Set(s)),
            Own::Map(m) => heap(Heap::Map(m)),
            Own::Range(r) => heap(Heap::Range(r)),
            Own::Func(f) => heap(Heap::Func(f)),
            Own::Gen(g) => heap(Heap::Gen(g)),
            Own::Grid(g) => heap(Heap::Grid(g)),
            Own::RangeSet(r) => heap(Heap::RangeSet(r)),
            Own::Internal(x) => heap(Heap::Internal(x)),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
    }
}

/// `a.cmp(b)` for the elements of two tuples or lists, comparing small
/// ints directly. Positions such as `(x, y)` are the most common map and
/// set keys, so this is on the hot path of most lookups.
#[inline]
fn cmp_items<'a>(
    a: impl Iterator<Item = &'a Value>,
    b: impl Iterator<Item = &'a Value>,
) -> Ordering {
    let mut b = b;
    for x in a {
        let Some(y) = b.next() else {
            return Ordering::Greater;
        };
        let o = match (x.0.as_fix(), y.0.as_fix()) {
            (Some(p), Some(q)) => p.cmp(&q),
            _ => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    if b.next().is_some() {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// `a == b` for the elements of two tuples, comparing small ints directly.
#[inline]
fn eq_items(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| match (x.as_small(), y.as_small()) {
                (Some(p), Some(q)) => p == q,
                _ => x == y,
            })
}

impl PartialEq for Value {
    #[inline]
    fn eq(&self, o: &Value) -> bool {
        // Unboxed values are equal exactly when their words are, and never
        // equal a boxed one (an int is boxed only outside the fixnums).
        if self.0.bits() == o.0.bits() {
            return true;
        }
        if self.0.is_unboxed() || o.0.is_unboxed() {
            return false;
        }
        match (self.view(), o.view()) {
            (V::Tuple(a), V::Tuple(b)) => eq_items(&a, &b),
            (V::None, V::None) => true,
            (V::Bool(a), V::Bool(b)) => a == b,
            (V::Int(a), V::Int(b)) => a == b,
            (V::Char(a), V::Char(b)) => a == b,
            (V::Str(a), V::Str(b)) => a == b,
            (V::List(a), V::List(b)) => a == b,
            (V::Set(a), V::Set(b)) => a == b,
            (V::Map(a), V::Map(b)) => a == b,
            (V::Range(a), V::Range(b)) => a == b,
            (V::Func(a), V::Func(b)) => Rc::ptr_eq(a, b),
            (V::Gen(a), V::Gen(b)) => Rc::ptr_eq(a, b),
            (V::Grid(a), V::Grid(b)) => a == b,
            (V::RangeSet(a), V::RangeSet(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Ord for Value {
    /// Total order for internal use (map keys, memo tables).
    #[inline]
    fn cmp(&self, o: &Value) -> Ordering {
        if let (Some(a), Some(b)) = (self.0.as_fix(), o.0.as_fix()) {
            return a.cmp(&b);
        }
        if let (Some(a), Some(b)) = (self.0.pair_order(), o.0.pair_order()) {
            return a.cmp(&b);
        }
        self.cmp_other(o)
    }
}

impl Value {
    /// `cmp` when the two aren't both small ints.
    #[inline(never)]
    fn cmp_other(&self, o: &Value) -> Ordering {
        // Positions, the commonest keys: pairs of small ints, packed or not.
        if let (Kind::Pair(x0, x1), Kind::Pair(y0, y1)) = (self.0.kind(), o.0.kind()) {
            return (x0, x1).cmp(&(y0, y1));
        }
        if let (
            Kind::Heap(Heap::Tuple(Tup::Two([a0, a1]))),
            Kind::Heap(Heap::Tuple(Tup::Two([b0, b1]))),
        ) = (self.0.kind(), o.0.kind())
            && let (Some(x0), Some(x1)) = (a0.0.as_fix(), a1.0.as_fix())
            && let (Some(y0), Some(y1)) = (b0.0.as_fix(), b1.0.as_fix())
        {
            return (x0, x1).cmp(&(y0, y1));
        }
        match (self.view(), o.view()) {
            (V::Tuple(a), V::Tuple(b)) => cmp_items(a.iter(), b.iter()),
            (V::None, V::None) => Ordering::Equal,
            (V::Bool(a), V::Bool(b)) => a.cmp(&b),
            (V::Int(a), V::Int(b)) => a.cmp(&b),
            (V::Char(a), V::Char(b)) => a.cmp(&b),
            (V::Str(a), V::Str(b)) => a.cmp(b),
            (V::List(a), V::List(b)) => cmp_items(a.iter(), b.iter()),
            (V::Set(a), V::Set(b)) => a.iter().cmp(b.iter()),
            (V::Map(a), V::Map(b)) => a.iter().cmp(b.iter()),
            (V::Range(a), V::Range(b)) => a.cmp(b),
            (V::Func(a), V::Func(b)) => Rc::as_ptr(a).cast::<()>().cmp(&Rc::as_ptr(b).cast::<()>()),
            (V::Gen(a), V::Gen(b)) => Rc::as_ptr(a).cast::<()>().cmp(&Rc::as_ptr(b).cast::<()>()),
            (V::Grid(a), V::Grid(b)) => a.cmp(b),
            (V::RangeSet(a), V::RangeSet(b)) => a.cmp(b),
            (a, b) => a.tag().cmp(&b.tag()),
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, o: &Value) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Hash for Value {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let v = self.view();
        v.tag().hash(h);
        match v {
            V::None => {}
            V::Bool(b) => b.hash(h),
            V::Int(i) => i.hash(h),
            V::Char(c) => c.hash(h),
            V::Str(s) => s.hash(h),
            V::Tuple(t) => t.hash(h),
            V::List(l) => l.hash(h),
            V::Set(s) => s.hash(h),
            V::Map(m) => m.hash(h),
            V::Range(r) => r.hash(h),
            V::Func(f) => Rc::as_ptr(f).cast::<()>().hash(h),
            V::Gen(g) => Rc::as_ptr(g).cast::<()>().hash(h),
            V::Grid(g) => g.hash(h),
            V::RangeSet(r) => r.hash(h),
            // Never a key: equal only to itself (by pointer, as functions).
            V::Internal(_) => {}
        }
    }
}

/// User-visible ordering (SPEC 4.1): only between values of the same type.
pub fn compare(a: &Value, b: &Value) -> Result<Ordering, String> {
    if let (Some(x), Some(y)) = (a.0.as_fix(), b.0.as_fix()) {
        return Ok(x.cmp(&y));
    }
    match (a.view(), b.view()) {
        (V::Bool(x), V::Bool(y)) => Ok(x.cmp(&y)),
        (V::Int(x), V::Int(y)) => Ok(x.cmp(&y)),
        (V::Char(x), V::Char(y)) => Ok(x.cmp(&y)),
        (V::Str(x), V::Str(y)) => Ok(x.cmp(y)),
        (V::Tuple(x), V::Tuple(y)) => compare_seq(x.iter(), y.iter()),
        (V::List(x), V::List(y)) => compare_seq(x.iter(), y.iter()),
        (V::Set(x), V::Set(y)) => compare_seq(x.iter(), y.iter()),
        (V::Map(x), V::Map(y)) => {
            let mut xi = x.iter();
            let mut yi = y.iter();
            loop {
                match (xi.next(), yi.next()) {
                    (None, None) => return Ok(Ordering::Equal),
                    (None, Some(_)) => return Ok(Ordering::Less),
                    (Some(_), None) => return Ok(Ordering::Greater),
                    (Some((k1, v1)), Some((k2, v2))) => {
                        // Like a pair: the values are compared only when
                        // the keys are equal, so unequal keys decide even
                        // when the values can't be ordered.
                        let o = match compare(k1, k2)? {
                            Ordering::Equal => compare(v1, v2)?,
                            o => o,
                        };
                        if o != Ordering::Equal {
                            return Ok(o);
                        }
                    }
                }
            }
        }
        (x, y) if x.tag() == y.tag() => Err(format!("{} values are not ordered", x.type_name())),
        (x, y) => Err(format!(
            "cannot order {} against {}",
            x.type_name(),
            y.type_name()
        )),
    }
}

fn compare_seq<'a>(
    mut xs: impl Iterator<Item = &'a Value>,
    mut ys: impl Iterator<Item = &'a Value>,
) -> Result<Ordering, String> {
    loop {
        match (xs.next(), ys.next()) {
            (None, None) => return Ok(Ordering::Equal),
            (None, Some(_)) => return Ok(Ordering::Less),
            (Some(_), None) => return Ok(Ordering::Greater),
            (Some(x), Some(y)) => {
                let o = compare(x, y)?;
                if o != Ordering::Equal {
                    return Ok(o);
                }
            }
        }
    }
}

/// Checks that `key` can join a map or set whose smallest key is `existing`
/// (SPEC 4.1: keys in one map or set have the same type). Within a type,
/// the internal `Ord` gives a fixed order even where `<` is an error.
pub fn check_key_compatible(existing: Option<&Value>, key: &Value) -> Result<(), String> {
    if let V::Func(_) | V::Gen(_) = key.view() {
        return Err(format!(
            "a {} cannot be a map key or set element",
            key.type_name()
        ));
    }
    match existing {
        Some(e) => same_key_type(e, key),
        None => Ok(()),
    }
}

/// Keys in one map or set must have the same type (SPEC 4.1).
pub fn same_key_type(a: &Value, b: &Value) -> Result<(), String> {
    if a.tag() == b.tag() {
        Ok(())
    } else {
        Err(format!(
            "cannot mix {} and {} keys in one map or set",
            a.type_name(),
            b.type_name()
        ))
    }
}

/// Inserts into a set, checking key compatibility.
pub fn set_insert(set: &mut OrdSet<Value>, v: Value) -> Result<(), String> {
    check_key_compatible(set.get_min(), &v)?;
    set.insert(v);
    Ok(())
}

/// Inserts into a map, checking key compatibility.
pub fn map_insert(map: &mut OrdMap<Value, Value>, k: Value, v: Value) -> Result<(), String> {
    check_key_compatible(map.get_min().map(|(k, _)| k), &k)?;
    map.insert(k, v);
    Ok(())
}

/// Anything with a Tinsel type name: a value or a view of one. Lets error
/// helpers take either.
pub trait Typed {
    fn type_name(&self) -> &'static str;
}

impl Typed for Value {
    fn type_name(&self) -> &'static str {
        Value::type_name(self)
    }
}

impl Typed for V<'_> {
    fn type_name(&self) -> &'static str {
        V::type_name(self)
    }
}

impl<T: Typed + ?Sized> Typed for &T {
    fn type_name(&self) -> &'static str {
        (**self).type_name()
    }
}
