//! The one-word representation of a [`Value`](super::Value)
//! (`reference/native-plan.md`, section 2.3).
//!
//! Layout (the low two bits are the tag):
//!
//! - `…0`: a fixnum, the int `w >> 1`, for ints in [`FIX_MIN`]..=[`FIX_MAX`].
//! - `…01`: a pointer to the [`Heap`] object inside an `Rc<Heap>`, plus 1.
//!   `Rc` allocations are at least 8-aligned, so the low bits are free. Bit
//!   2 is the *watch* flag, fixed when the object is made: set for objects
//!   a reference cycle can run through (see `ir::interp::release_cycle`),
//!   so dropping any other word needn't look at its object.
//! - `…11`: an immediate: `none`, a bool, a char, `inf`/`-inf`, or a pair
//!   of small ints. Bits 2–4 say which, and the payload (the bool, the
//!   char's code point, the sign of `inf`, or the pair) is in bits 8 and
//!   up. A pair `(x, y)` keeps `y` in bits 8–35 and `x` in bits 36–63, each
//!   a 28-bit signed int; a pair of ints in [`PAIR_MIN`]..=[`PAIR_MAX`] is
//!   always stored this way, never as a heap tuple, so equal pairs have
//!   equal words.
//!
//! This is one of the modules where the plan allows `unsafe` (section 4).
//! Its invariant:
//!
//! **Every word with tag `01` was made from `Rc::into_raw` of an
//! `Rc<Heap>` and owns one strong count of it.**
//!
//! Safe code can't break it: [`Word`] has no constructor from a raw `u64`,
//! and every function here that makes a pointer word takes the count it
//! stores from an `Rc` it was given. No `Weak` reference to a heap object
//! is ever made, so a strong count of 1 means the word is the only
//! reference.
//!
//! Words will cross into generated code as plain integers, so pointers use
//! exposed provenance: [`tag_ptr`] exposes it and [`Word::ptr`] picks it up
//! again. Strict provenance isn't possible across that boundary. Miri runs
//! with `-Zmiri-permissive-provenance` (`.cargo/config.toml`) for this
//! reason:
//!
//! ```text
//! cargo +nightly miri test --lib value::word
//! ```

use super::Heap;
use std::mem::ManuallyDrop;
use std::rc::Rc;

const TAG_MASK: u64 = 0b11;
const WATCH: u64 = 0b100;
/// The bits of a pointer word that aren't the address.
const PTR_BITS: u64 = TAG_MASK | WATCH;
const PTR_TAG: u64 = 0b01;
const IMM_TAG: u64 = 0b11;

const KIND_MASK: u64 = 0b11100;
const KIND_NONE: u64 = 0b00000;
const KIND_BOOL: u64 = 0b00100;
const KIND_CHAR: u64 = 0b01000;
const KIND_INF: u64 = 0b01100;
const KIND_PAIR: u64 = 0b10000;

/// The range of each int of a packed pair.
pub const PAIR_MIN: i64 = -(1 << 27);
pub const PAIR_MAX: i64 = (1 << 27) - 1;
/// Where a packed pair's ints are, for compiled code: `x` is the word
/// shifted right (arithmetically) by `PAIR_X_SHIFT`; `y` is the word
/// shifted left by `PAIR_Y_UP` and then right by `PAIR_X_SHIFT`.
pub const PAIR_X_SHIFT: u32 = 36;
pub const PAIR_Y_UP: u32 = 28;
/// The tag and kind bits of a packed pair (the low byte).
pub const PAIR_LOW: u64 = IMM_TAG | KIND_PAIR;
const PAYLOAD_SHIFT: u32 = 8;

/// The smallest fixnum. Fixnums hold 62-bit ints, as in the phase 0 spike,
/// so `n << 1` can't overflow and generated code has room to spare.
pub const FIX_MIN: i64 = -(1 << 61);
/// The largest fixnum.
pub const FIX_MAX: i64 = (1 << 61) - 1;

/// A tagged word that owns its reference, if it has one.
pub struct Word(u64);

/// What a word holds.
pub enum Kind<'a> {
    Fix(i64),
    None,
    Bool(bool),
    Char(char),
    /// `true` is `inf`, `false` is `-inf`.
    Inf(bool),
    /// A packed pair of small ints.
    Pair(i64, i64),
    Heap(&'a Heap),
}

/// The word for an `Rc` pointer, exposing its provenance (see the module
/// comment).
fn tag_ptr(p: *const Heap, watch: bool) -> u64 {
    let a = p.expose_provenance() as u64;
    debug_assert_eq!(a & PTR_BITS, 0, "an Rc allocation is 8-aligned");
    a | PTR_TAG | if watch { WATCH } else { 0 }
}

const fn imm(kind: u64, payload: u64) -> Word {
    Word(IMM_TAG | kind | (payload << PAYLOAD_SHIFT))
}

impl Word {
    pub const NONE: Word = imm(KIND_NONE, 0);

    pub const fn bool(b: bool) -> Word {
        imm(KIND_BOOL, b as u64)
    }

    pub const fn char(c: char) -> Word {
        imm(KIND_CHAR, c as u64)
    }

    pub const fn inf(positive: bool) -> Word {
        imm(KIND_INF, positive as u64)
    }

    /// The fixnum for `n`, if it is in range.
    #[inline]
    pub const fn fixnum(n: i64) -> Option<Word> {
        if FIX_MIN <= n && n <= FIX_MAX {
            Some(Word((n << 1) as u64))
        } else {
            None
        }
    }

    /// The packed pair `(x, y)`, if both are in range.
    #[inline]
    pub const fn pair(x: i64, y: i64) -> Option<Word> {
        if PAIR_MIN <= x && x <= PAIR_MAX && PAIR_MIN <= y && y <= PAIR_MAX {
            let xs = (x as u64) << PAIR_X_SHIFT;
            let ys = ((y as u64) << PAIR_X_SHIFT) >> PAIR_Y_UP;
            Some(Word(xs | ys | PAIR_LOW))
        } else {
            None
        }
    }

    /// A word holding `h`, taking over its strong count.
    pub fn heap(h: Rc<Heap>) -> Word {
        live::count(1);
        let watch = super::watched(&h);
        Word(tag_ptr(Rc::into_raw(h), watch))
    }

    #[inline]
    fn ptr(&self) -> Option<*const Heap> {
        (self.0 & TAG_MASK == PTR_TAG)
            .then(|| std::ptr::with_exposed_provenance((self.0 & !PTR_BITS) as usize))
    }

    /// The fixnum this word holds, if it is one.
    #[inline]
    pub fn as_fix(&self) -> Option<i64> {
        (self.0 & 1 == 0).then_some((self.0 as i64) >> 1)
    }

    /// For a packed pair, a number that orders pairs as `(x, y)` tuples
    /// order: `x` is already signed in the top bits, and flipping the sign
    /// bit of `y`'s field makes that field order as an unsigned number.
    #[inline]
    pub fn pair_order(&self) -> Option<i64> {
        (self.0 & 0xff == PAIR_LOW).then_some((self.0 ^ (1 << (PAIR_X_SHIFT - 1))) as i64)
    }

    /// The packed pair whose [`Word::pair_order`] is `n`.
    #[inline]
    pub fn from_pair_order(n: i64) -> Word {
        let w = Word(n as u64 ^ (1 << (PAIR_X_SHIFT - 1)));
        debug_assert!(w.0 & 0xff == PAIR_LOW);
        w
    }

    /// Whether the word is an immediate or a fixnum, so equal words mean
    /// equal values and different words different values.
    #[inline]
    pub fn is_unboxed(&self) -> bool {
        self.0 & TAG_MASK != PTR_TAG
    }

    /// For a fixnum, char, bool or `none`: the order of its type and its
    /// value, as values of those types order (`none`, then bools, then
    /// ints, then chars, as `V::tag` numbers them). `None` for anything
    /// else, including `inf` and packed pairs. Read from the bits alone.
    #[inline]
    pub fn simple_order(&self) -> Option<(u8, i64)> {
        if self.0 & 1 == 0 {
            return Some((2, (self.0 as i64) >> 1));
        }
        if self.0 & TAG_MASK == PTR_TAG {
            return None;
        }
        let payload = self.0 >> PAYLOAD_SHIFT;
        match self.0 & KIND_MASK {
            KIND_NONE => Some((0, 0)),
            KIND_BOOL => Some((1, i64::from(payload != 0))),
            KIND_CHAR => Some((3, payload as i64)),
            _ => None,
        }
    }

    /// The raw bits, for comparisons between unboxed words and for pointer
    /// identity.
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn kind(&self) -> Kind<'_> {
        if self.0 & 1 == 0 {
            return Kind::Fix((self.0 as i64) >> 1);
        }
        if let Some(p) = self.ptr() {
            // SAFETY: by the module invariant, a pointer word owns a strong
            // count of a live `Rc<Heap>`, which keeps the object alive for
            // as long as `self` is borrowed. The only `&mut Heap` this
            // module hands out needs `&mut self` (`make_mut`), so none can
            // exist while this shared borrow does.
            return Kind::Heap(unsafe { &*p });
        }
        let payload = self.0 >> PAYLOAD_SHIFT;
        match self.0 & KIND_MASK {
            KIND_NONE => Kind::None,
            KIND_BOOL => Kind::Bool(payload != 0),
            KIND_CHAR => Kind::Char(char::from_u32(payload as u32).unwrap_or('\0')),
            KIND_PAIR => Kind::Pair(
                (self.0 as i64) >> PAIR_X_SHIFT,
                ((self.0 << PAIR_Y_UP) as i64) >> PAIR_X_SHIFT,
            ),
            _ => Kind::Inf(payload != 0),
        }
    }

    /// The heap object, as an `Rc`, if the word holds one; otherwise the
    /// word itself.
    pub fn into_rc(self) -> Result<Rc<Heap>, Word> {
        let Some(p) = self.ptr() else {
            return Err(self);
        };
        let _ = ManuallyDrop::new(self);
        live::count(-1);
        // SAFETY: the word owned this strong count (module invariant), and
        // `ManuallyDrop` above stops the word releasing it too.
        Ok(unsafe { Rc::from_raw(p) })
    }

    /// The heap object, for an update in place. If anything else refers
    /// to it, this word first gets its own copy (`Rc::make_mut`).
    pub fn make_mut(&mut self) -> Option<&mut Heap> {
        let p = self.ptr()?;
        if self.ref_count() == Some(1) {
            let q: *mut Heap = std::ptr::with_exposed_provenance_mut(p.addr());
            // SAFETY: the word holds the only strong count of the object
            // (module invariant), and no `Weak` references are ever made,
            // so nothing else can reach it; the result borrows `self`
            // mutably for as long as it lives.
            return Some(unsafe { &mut *q });
        }
        let p_watch = self.0 & WATCH != 0;
        // Hand the count to `rc` for the update, leaving an immediate here,
        // so that a panic inside `make_mut` can't release it twice.
        self.0 = Word::NONE.0;
        // SAFETY: the word owned this strong count (module invariant), and
        // has just stopped referring to it.
        let mut rc = unsafe { Rc::from_raw(p) };
        Rc::make_mut(&mut rc);
        // The count passes back to the word.
        // A copy is the same kind of object, so it keeps the watch flag.
        self.0 = tag_ptr(Rc::into_raw(rc), p_watch);
        let q: *mut Heap = std::ptr::with_exposed_provenance_mut((self.0 & !PTR_BITS) as usize);
        // SAFETY: `make_mut` left the object with a strong count of 1, held
        // by this word, and no `Weak` references (none are ever made). The
        // result borrows `self` mutably, so nothing else can reach the
        // object while it lives.
        Some(unsafe { &mut *q })
    }

    /// The cycle check for a word about to be dropped (out of line, to keep
    /// `drop` small).
    #[inline(never)]
    fn release_cycle(&self, p: *const Heap) {
        // SAFETY: `self` still owns its strong count (module invariant), so
        // the object is alive for this borrow.
        let h = unsafe { &*p };
        if super::may_close_cycle(h)
            && let Some(n) = self.ref_count()
        {
            super::release_cycle(h, self.0, n);
        }
    }

    /// How many words refer to this word's heap object, if it has one.
    pub fn ref_count(&self) -> Option<usize> {
        let p = self.ptr()?;
        // SAFETY: the word owns a strong count (module invariant); the
        // `ManuallyDrop` keeps this temporary `Rc` from releasing it.
        let rc = ManuallyDrop::new(unsafe { Rc::from_raw(p) });
        Some(Rc::strong_count(&rc))
    }
}

/// Words that compiled code uses as signals, never as values: immediates
/// of kind `none` with a nonzero payload, which no [`Word`] constructor
/// makes. `ERR` means a pending error, `DONE` an exhausted iterator and
/// `MISS` a memo table without the key.
pub mod sentinel {
    use super::{IMM_TAG, KIND_NONE, PAYLOAD_SHIFT};
    pub const ERR: u64 = IMM_TAG | KIND_NONE | (1 << PAYLOAD_SHIFT);
    pub const DONE: u64 = IMM_TAG | KIND_NONE | (2 << PAYLOAD_SHIFT);
    pub const MISS: u64 = IMM_TAG | KIND_NONE | (3 << PAYLOAD_SHIFT);
}

/// Whether `bits` could be a word: a fixnum, a pointer, or an immediate
/// that `kind` understands. For debug checks at the native boundary.
fn plausible(bits: u64) -> bool {
    match bits & TAG_MASK {
        IMM_TAG => match bits & KIND_MASK {
            KIND_NONE => bits == Word::NONE.0,
            KIND_BOOL => bits >> PAYLOAD_SHIFT <= 1 && bits & 0xe0 == 0,
            KIND_PAIR => bits & 0xe0 == 0,
            KIND_CHAR | KIND_INF => bits & 0xe0 == 0,
            _ => false,
        },
        PTR_TAG => bits & !PTR_BITS != 0,
        _ => true,
    }
}

/// Where a `Box<[Value]>` keeps its data pointer and length: byte offsets
/// within the box's two words. Rust doesn't promise their order, so it is
/// measured on a sample, for compiled code that reads long tuples.
pub fn boxed_slice_layout() -> (i32, i32) {
    let b: Box<[super::Value]> = vec![super::Value::NONE; 7].into_boxed_slice();
    let data = b.as_ptr().addr();
    assert_eq!(std::mem::size_of::<Box<[super::Value]>>(), 16);
    let at = std::ptr::from_ref(&b).cast::<[*const u8; 2]>();
    // SAFETY: `at` points at the box itself, two initialised words (its
    // size is checked above), read here as pointers without being used as
    // such; the box is untouched and dropped normally.
    let words = unsafe { at.read() };
    if words[0].addr() == data && words[1].addr() == 7 {
        (0, 8)
    } else {
        assert!(
            words[1].addr() == data && words[0].addr() == 7,
            "a slice box is (data, length)"
        );
        (8, 0)
    }
}

/// Where an `Rc<Heap>`'s strong count is, as a byte offset from the
/// pointer in its word, so compiled code can count references inline.
/// Rust doesn't promise it, so it is measured; `None` if no candidate
/// behaves like the count (compiled code then calls the runtime).
pub fn rc_count_offset() -> Option<i32> {
    let read = |rc: &Rc<Heap>, off: isize| {
        let p = Rc::as_ptr(rc).cast::<u8>();
        // SAFETY: `Rc` keeps its two counts just before the value, in the
        // same allocation, which `rc` keeps alive, so `off` (-16 or -8)
        // stays in bounds.
        let at = unsafe { p.offset(off) }.cast::<usize>();
        // SAFETY: as above; a plain read of an initialised word, which
        // changes nothing.
        unsafe { at.read() }
    };
    let rc = Rc::new(Heap::Wide(0));
    let before = [read(&rc, -16), read(&rc, -8)];
    let rc2 = Rc::clone(&rc);
    let after = [read(&rc, -16), read(&rc, -8)];
    drop(rc2);
    let back = [read(&rc, -16), read(&rc, -8)];
    [-16, -8]
        .into_iter()
        .zip(0..)
        .find(|&(_, i)| before[i] == 1 && after[i] == 2 && back[i] == 1)
        .map(|(off, _)| off)
}

/// The two ints of a packed pair as values, for reading as a slice
/// ([`super::Items`]). They are fixnums, which own nothing, so they need no
/// dropping; `ManuallyDrop` spares every view of a tuple the drop code.
#[derive(Clone)]
pub struct PairItems([ManuallyDrop<super::Value>; 2]);

impl PairItems {
    /// From the ints of a packed pair, which are always fixnums.
    #[inline]
    pub fn new(x: i64, y: i64) -> PairItems {
        debug_assert!((PAIR_MIN..=PAIR_MAX).contains(&x) && (PAIR_MIN..=PAIR_MAX).contains(&y));
        PairItems([
            ManuallyDrop::new(super::Value(Word((x << 1) as u64))),
            ManuallyDrop::new(super::Value(Word((y << 1) as u64))),
        ])
    }

    pub fn as_slice(&self) -> &[super::Value] {
        let items: &[ManuallyDrop<super::Value>] = &self.0;
        // SAFETY: `ManuallyDrop<T>` is `repr(transparent)` over `T`, so a
        // slice of one has the layout of a slice of the other; the borrow
        // keeps `self` alive. Reading the values through `&` is fine, and
        // nothing here drops them.
        unsafe { &*(std::ptr::from_ref(items) as *const [super::Value]) }
    }
}

impl super::Value {
    /// The value's bits, handing its reference (if any) to the caller, for
    /// compiled code. [`Value::from_raw`](super::Value::from_raw) takes it
    /// back.
    pub fn into_raw(self) -> u64 {
        let w = ManuallyDrop::new(self);
        w.0.0
    }

    /// The value's bits, without giving up anything: for comparing unboxed
    /// values, and for constants that compiled code refers to while the
    /// value is kept alive elsewhere.
    pub fn raw_bits(&self) -> u64 {
        self.0.0
    }

    /// The value whose reference `bits` holds.
    ///
    /// # Safety
    ///
    /// `bits` must have come from [`into_raw`](Self::into_raw), or be the
    /// bits of an unboxed value, and the reference it carries must not be
    /// used again: the result owns it.
    pub unsafe fn from_raw(bits: u64) -> super::Value {
        debug_assert!(plausible(bits), "not a word: {bits:#x}");
        super::Value(Word(bits))
    }
}

impl Clone for Word {
    #[inline]
    fn clone(&self) -> Word {
        if let Some(p) = self.ptr() {
            live::count(1);
            // SAFETY: `p` came from `Rc::into_raw` and its count is at least
            // one, held by `self` (module invariant). The new word owns the
            // count added here.
            unsafe { Rc::increment_strong_count(p) };
        }
        Word(self.0)
    }
}

impl Drop for Word {
    #[inline]
    fn drop(&mut self) {
        if let Some(p) = self.ptr() {
            if self.0 & WATCH != 0 {
                // Before releasing: the object stays alive during the check.
                self.release_cycle(p);
            }
            live::count(-1);
            // SAFETY: `self` owns one strong count (module invariant) and is
            // being dropped, so the count is released exactly once.
            unsafe { Rc::decrement_strong_count(p) };
        }
    }
}

/// Live heap words, counted in debug builds only, so that tests can check
/// for leaks and double frees (section 4).
pub mod live {
    use std::cell::Cell;

    thread_local! {
        static LIVE: Cell<i64> = const { Cell::new(0) };
    }

    #[inline]
    pub fn count(d: i64) {
        if cfg!(debug_assertions) {
            LIVE.with(|n| n.set(n.get() + d));
        }
    }

    /// How many words referring to heap objects exist on this thread (in
    /// debug builds; always 0 in release builds).
    pub fn words() -> i64 {
        LIVE.with(Cell::get)
    }
}

/// The global allocator: mimalloc, called as plain `mi_malloc` for the
/// usual alignments.
///
/// The `mimalloc` crate passes every allocation to `mi_malloc_aligned`.
/// In mimalloc 3, when such a call misses its fast path and the size isn't
/// a power of two, it over-allocates from the next size class up, so the
/// objects of one size end up spread over two classes, and pages keep
/// being emptied, handed back and fetched again (with an `madvise` each
/// time). 2023 day 22 spent 45% of its native run time doing that.
/// Every mimalloc block is at least 8-aligned, so alignments up to 8 (all
/// of Tinsel's own allocations) need only `mi_malloc`; larger ones still
/// go through the crate. Every block is freed with `mi_free` either way.
///
/// Miri can't call into mimalloc's C code, so under Miri the program keeps
/// the default allocator.
#[cfg(not(miri))]
mod alloc {
    use mimalloc::MiMalloc;
    use std::alloc::{GlobalAlloc, Layout};

    #[global_allocator]
    static GLOBAL: Alloc = Alloc;

    struct Alloc;

    /// The alignment every mimalloc block has (a pointer's).
    const PLAIN_ALIGN: usize = 8;

    // SAFETY: these are mimalloc's own functions, linked in by the
    // `mimalloc` crate, declared with their C signatures.
    unsafe extern "C" {
        fn mi_malloc(size: usize) -> *mut u8;
        fn mi_zalloc(size: usize) -> *mut u8;
        fn mi_realloc(p: *mut u8, size: usize) -> *mut u8;
    }

    // SAFETY: each method returns a block of at least the size asked for,
    // aligned as asked (mimalloc's blocks are all 8-aligned, and larger
    // alignments go to `MiMalloc`), or null when out of memory; blocks
    // from either path are freed by `mi_free`, which is what
    // `MiMalloc::dealloc` calls, and grown by `mi_realloc` only when they
    // came from the plain path (the alignment is fixed for a block).
    unsafe impl GlobalAlloc for Alloc {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            if l.align() <= PLAIN_ALIGN {
                // SAFETY: any size is valid for `mi_malloc`.
                unsafe { mi_malloc(l.size()) }
            } else {
                // SAFETY: the caller's contract for `l` is passed on.
                unsafe { MiMalloc.alloc(l) }
            }
        }

        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            if l.align() <= PLAIN_ALIGN {
                // SAFETY: any size is valid for `mi_zalloc`.
                unsafe { mi_zalloc(l.size()) }
            } else {
                // SAFETY: the caller's contract for `l` is passed on.
                unsafe { MiMalloc.alloc_zeroed(l) }
            }
        }

        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` came from this allocator, so from mimalloc.
            unsafe { MiMalloc.dealloc(p, l) }
        }

        unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
            if l.align() <= PLAIN_ALIGN {
                // SAFETY: `p` is a live mimalloc block, made by the plain
                // path since its alignment is at most 8.
                unsafe { mi_realloc(p, new_size) }
            } else {
                // SAFETY: the caller's contract is passed on.
                unsafe { MiMalloc.realloc(p, l, new_size) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::int::Int;
    use crate::value::{V, Value};

    #[test]
    fn a_value_is_one_word() {
        assert_eq!(std::mem::size_of::<Value>(), 8);
    }

    #[test]
    fn immediates_round_trip() {
        for (w, ok) in [
            (Word::NONE, matches!(Word::NONE.kind(), Kind::None)),
            (
                Word::bool(true),
                matches!(Word::bool(true).kind(), Kind::Bool(true)),
            ),
            (
                Word::bool(false),
                matches!(Word::bool(false).kind(), Kind::Bool(false)),
            ),
            (
                Word::inf(false),
                matches!(Word::inf(false).kind(), Kind::Inf(false)),
            ),
        ] {
            assert!(ok, "{:#x}", w.bits());
            assert!(w.is_unboxed());
        }
        for c in ['\0', 'a', 'é', '\u{10FFFF}'] {
            assert!(matches!(Word::char(c).kind(), Kind::Char(d) if d == c));
        }
        let items = PairItems::new(3, -4);
        assert_eq!(items.as_slice().len(), 2);
        assert_eq!(items.as_slice()[1].as_small(), Some(-4));
        for (x, y) in [
            (0, 0),
            (3, -4),
            (-1, 1),
            (PAIR_MIN, PAIR_MAX),
            (PAIR_MAX, PAIR_MIN),
        ] {
            let w = Word::pair(x, y).unwrap();
            assert!(
                matches!(w.kind(), Kind::Pair(a, b) if (a, b) == (x, y)),
                "{x}, {y}"
            );
            assert!(w.is_unboxed() && plausible(w.bits()));
        }
        // `pair_order` orders pairs as tuples, including across signs.
        let edge = [PAIR_MIN, PAIR_MIN + 1, -1, 0, 1, PAIR_MAX - 1, PAIR_MAX];
        for p in edge.iter().flat_map(|&x| edge.iter().map(move |&y| (x, y))) {
            for q in edge.iter().flat_map(|&x| edge.iter().map(move |&y| (x, y))) {
                let order = |(x, y)| Word::pair(x, y).unwrap().pair_order().unwrap();
                assert_eq!(order(p).cmp(&order(q)), p.cmp(&q), "{p:?} {q:?}");
            }
        }
        assert!(Word::fixnum(5).unwrap().pair_order().is_none());
        assert!(Word::char('a').pair_order().is_none());
        assert!(Word::pair(PAIR_MAX + 1, 0).is_none());
        assert!(Word::pair(0, PAIR_MIN - 1).is_none());
        for n in [0, 1, -1, FIX_MIN, FIX_MAX] {
            let w = Word::fixnum(n).unwrap();
            assert_eq!(w.as_fix(), Some(n));
            assert!(matches!(w.kind(), Kind::Fix(m) if m == n));
        }
        assert!(Word::fixnum(FIX_MAX + 1).is_none());
        assert!(Word::fixnum(FIX_MIN - 1).is_none());
    }

    #[test]
    fn heap_words_share_and_release() {
        let before = live::words();
        let a = Value::str("hello");
        let b = a.clone();
        assert_eq!(a.heap_refs().map(|r| r.1), Some(2));
        drop(a);
        assert_eq!(b.heap_refs().map(|r| r.1), Some(1));
        assert!(matches!(b.view(), V::Str(s) if &**s == "hello"));
        drop(b);
        if cfg!(debug_assertions) {
            assert_eq!(live::words(), before);
        }
    }

    #[test]
    fn make_mut_updates_in_place_or_copies() {
        let mut a = Value::list(vec![Value::int(1), Value::int(2)]);
        let first = a.heap_refs().map(|r| r.0);
        let set = |v: &mut Value, i, x| match v.make_mut() {
            Some(Heap::List(l)) => l.set_mut(i, x),
            _ => panic!("not a list"),
        };
        set(&mut a, 0, Value::int(5));
        assert_eq!(a.heap_refs().map(|r| r.0), first, "unique: same object");
        let b = a.clone();
        set(&mut a, 1, Value::int(6));
        assert_ne!(a.heap_refs().map(|r| r.0), first, "shared: copied");
        assert_eq!(a.repr(), "[5, 6]");
        assert_eq!(b.repr(), "[5, 2]");
    }

    #[test]
    fn unpack_moves_or_clones() {
        let a = Value::tuple(vec![Value::int(1), Value::str("x")]);
        let b = a.clone();
        assert_eq!(Value::from(a.unpack()).repr(), "(1, \"x\")");
        assert_eq!(Value::from(b.unpack()).repr(), "(1, \"x\")");
    }

    #[test]
    fn raw_words_round_trip() {
        let before = live::words();
        let a = Value::str("raw");
        let b = a.clone();
        let bits = a.into_raw();
        assert_eq!(
            b.heap_refs().map(|r| r.1),
            Some(2),
            "the raw word keeps its count"
        );
        // SAFETY: `bits` came from `into_raw` just above and is used once.
        let a = unsafe { Value::from_raw(bits) };
        assert_eq!(a, b);
        drop(a);
        assert_eq!(b.heap_refs().map(|r| r.1), Some(1));
        drop(b);
        for v in [Value::int(7), Value::NONE, Value::TRUE, Value::char('x')] {
            let bits = v.raw_bits();
            // SAFETY: unboxed words carry no reference.
            assert_eq!(unsafe { Value::from_raw(bits) }, v);
        }
        for s in [sentinel::ERR, sentinel::DONE, sentinel::MISS] {
            assert!(!plausible(s), "{s:#x} must never be taken for a value");
        }
        if cfg!(debug_assertions) {
            assert_eq!(live::words(), before);
        }
    }

    #[test]
    fn layouts_are_measured() {
        assert!(rc_count_offset().is_some());
        let (p, l) = boxed_slice_layout();
        assert!((p, l) == (0, 8) || (p, l) == (8, 0));
        let o = crate::value::layout::offsets();
        assert!(o.tup >= 1 && o.two > o.tup && o.three > o.tup && o.internal >= 1);
        assert!(o.other_ptr > o.tup && o.other_len > o.tup);
    }

    #[test]
    fn ints_outside_fixnums() {
        for n in [i64::MAX, i64::MIN, FIX_MAX + 1, FIX_MIN - 1] {
            let v = Value::int(n);
            assert_eq!(v.as_small(), Some(n));
            assert!(v == Value::from_int(Int::Small(n)));
            assert!(v != Value::int(0));
        }
        assert!(Value::int(FIX_MAX) < Value::int(FIX_MAX + 1));
    }
}
