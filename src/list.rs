//! The representation of Tinsel lists.
//!
//! Most lists in Advent of Code programs are short (a parsed line, a pair of
//! coordinates), so lists up to [`SMALL_MAX`] elements are a flat shared
//! slice.
//!
//! Longer lists start out as a view of a plain vector (`Flat`). While only
//! one value refers to the vector, `set` is a single store and `++` is a
//! push, and slicing just narrows the view, so `[x, ..rest]` costs O(1).
//!
//! Updating or extending a flat list that is shared gives an
//! `imbl::Vector` (`Tree`), whose O(log n) updates copy only a path. The
//! vector's tree form is built once, O(n), and kept with the vector
//! ([`Buf::tree`]), so further updates branching from the same shared list
//! cost O(log n), as they would if it had been a tree all along. Only a
//! short right operand of `++` (up to [`SMALL_MAX`]) is pushed in place; a
//! longer one is joined as a tree in O(log n), using its stored tree form
//! if it is a shared flat list, rather than copied. So the common case of one large list updated at its
//! last use (SPEC 4.1) runs at array speed, and other uses keep the
//! complexity of a persistent vector, at the cost of a one-off O(n) per
//! shared flat vector and the memory of its tree form while both live.

use crate::value::Value;
use imbl::Vector;
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// Lists up to this length are stored as a [`Repr::Small`] slice.
const SMALL_MAX: usize = 32;

#[derive(Clone)]
pub struct List(Repr);

#[derive(Clone)]
enum Repr {
    Small(Rc<[Value]>),
    /// The elements `data.items[start..end]`. Once `data` is unique, the
    /// elements outside the view belong to no list. The bounds are `u32`
    /// so that this variant is no bigger than `Small`; longer lists are
    /// stored as `Tree`.
    Flat {
        data: Rc<Buf>,
        start: u32,
        end: u32,
    },
    Tree(Rc<Vector<Value>>),
}

/// The vector behind flat lists.
struct Buf {
    items: Vec<Value>,
    /// All of `items` as a tree, built the first time a shared flat list
    /// needs one. Cleared before `items` is changed in place.
    tree: OnceCell<Vector<Value>>,
}

impl Buf {
    fn new(items: Vec<Value>) -> Rc<Buf> {
        Rc::new(Buf {
            items,
            tree: OnceCell::new(),
        })
    }

    /// The view `start..end` as a tree, sharing the cached tree form.
    fn tree(&self, start: usize, end: usize) -> Vector<Value> {
        let t = self.tree.get_or_init(|| Vector::from(self.items.clone()));
        if start == 0 && end == t.len() {
            t.clone()
        } else {
            t.clone().slice(start..end)
        }
    }

    /// The items, for an update in place.
    fn items_mut(&mut self) -> &mut Vec<Value> {
        self.tree.take();
        &mut self.items
    }
}

pub enum Iter<'a> {
    Slice(std::slice::Iter<'a, Value>),
    Tree(imbl::vector::Iter<'a, Value, imbl::shared_ptr::DefaultSharedPtr>),
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Value;
    fn next(&mut self) -> Option<&'a Value> {
        match self {
            Iter::Slice(i) => i.next(),
            Iter::Tree(i) => i.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Iter::Slice(i) => i.size_hint(),
            Iter::Tree(i) => i.size_hint(),
        }
    }
}

impl List {
    pub fn new() -> List {
        thread_local! {
            static EMPTY: Rc<[Value]> = Rc::new([]);
        }
        List(Repr::Small(EMPTY.with(Rc::clone)))
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Small(s) => s.len(),
            Repr::Flat { start, end, .. } => (end - start) as usize,
            Repr::Tree(t) => t.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<&Value> {
        match &self.0 {
            Repr::Small(s) => s.get(i),
            Repr::Flat { data, start, end } => data.items[*start as usize..*end as usize].get(i),
            Repr::Tree(t) => t.get(i),
        }
    }

    pub fn iter(&self) -> Iter<'_> {
        match &self.0 {
            Repr::Small(s) => Iter::Slice(s.iter()),
            Repr::Flat { data, start, end } => {
                Iter::Slice(data.items[*start as usize..*end as usize].iter())
            }
            Repr::Tree(t) => Iter::Tree(t.iter()),
        }
    }

    pub fn contains(&self, x: &Value) -> bool {
        self.iter().any(|v| v == x)
    }

    /// The elements `start..end`. The caller keeps them in bounds.
    pub fn slice(&self, start: usize, end: usize) -> List {
        match &self.0 {
            Repr::Small(s) => List(Repr::Small(s[start..end].into())),
            _ if end - start <= SMALL_MAX => {
                List(Repr::Small((start..end).map(|i| self[i].clone()).collect()))
            }
            // `end` is within the view, so the new bounds fit in `u32`.
            Repr::Flat { data, start: s, .. } => List(Repr::Flat {
                data: data.clone(),
                start: s + start as u32,
                end: s + end as u32,
            }),
            Repr::Tree(t) => List(Repr::Tree(Rc::new((**t).clone().slice(start..end)))),
        }
    }

    /// The elements as a tree that the caller owns: moved out when nothing
    /// else refers to them, shared with the vector's cached tree otherwise.
    fn into_tree(self) -> Vector<Value> {
        match self.0 {
            Repr::Small(s) => s.iter().cloned().collect(),
            Repr::Flat { data, start, end } => {
                let (start, end) = (start as usize, end as usize);
                match Rc::try_unwrap(data) {
                    Ok(Buf { tree, .. }) if tree.get().is_some() => {
                        let mut t = tree.into_inner().unwrap_or_default();
                        if start == 0 && end == t.len() {
                            t
                        } else {
                            t.slice(start..end)
                        }
                    }
                    Ok(Buf { mut items, .. }) => {
                        items.truncate(end);
                        items.drain(..start);
                        Vector::from(items)
                    }
                    Err(data) => data.tree(start, end),
                }
            }
            Repr::Tree(t) => Rc::try_unwrap(t).unwrap_or_else(|t| (*t).clone()),
        }
    }

    /// The elements of `self` followed by those of `other`. A long list
    /// that nothing else refers to is extended in place.
    pub fn concat(self, other: List) -> List {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        if self.len() + other.len() <= SMALL_MAX {
            return self.iter().chain(other.iter()).cloned().collect();
        }
        match self.0 {
            // Only a short list is pushed in place. A long one is joined as
            // a tree, which shares its structure rather than copying it.
            Repr::Flat {
                mut data,
                start,
                end,
            } if matches!(other.0, Repr::Small(_)) => {
                if let Some(buf) = Rc::get_mut(&mut data)
                    && let Ok(new_end) = u32::try_from(end as usize + other.len())
                {
                    let v = buf.items_mut();
                    v.truncate(end as usize);
                    v.extend(other.iter().cloned());
                    return List(Repr::Flat {
                        data,
                        start,
                        end: new_end,
                    });
                }
                List(Repr::Flat { data, start, end }).concat_tree(other)
            }
            Repr::Small(s) => match other.0 {
                // Two small lists that are too long together.
                Repr::Small(t) => s.iter().chain(t.iter()).cloned().collect(),
                o => List(Repr::Small(s)).concat_tree(List(o)),
            },
            repr => List(repr).concat_tree(other),
        }
    }

    /// `concat` through `imbl::Vector`, for lists that can't be extended in
    /// place.
    fn concat_tree(self, other: List) -> List {
        let t = match (self.0, other.0) {
            // Pushing or prepending a few elements is much cheaper than a
            // tree merge.
            (a, Repr::Small(s)) => {
                let mut t = List(a).into_tree();
                t.extend(s.iter().cloned());
                t
            }
            (Repr::Small(s), b) => {
                let mut t = List(b).into_tree();
                for x in s.iter().rev() {
                    t.push_front(x.clone());
                }
                t
            }
            (a, b) => {
                let mut t = List(a).into_tree();
                t.append(List(b).into_tree());
                t
            }
        };
        List(Repr::Tree(Rc::new(t)))
    }

    /// The list with element `i` replaced by `x`. The caller keeps `i` in
    /// bounds. A list that nothing else refers to is updated in place.
    pub fn set(mut self, i: usize, x: Value) -> List {
        self.set_mut(i, x);
        self
    }

    /// `set` in place: a single store when nothing else shares the items.
    pub fn set_mut(&mut self, i: usize, x: Value) {
        match &mut self.0 {
            Repr::Small(s) => {
                if let Some(items) = Rc::get_mut(s) {
                    items[i] = x;
                    return;
                }
                let mut items: Vec<Value> = s.to_vec();
                items[i] = x;
                *s = items.into();
            }
            Repr::Flat { data, start, end } => {
                if let Some(buf) = Rc::get_mut(data) {
                    buf.items_mut()[*start as usize..*end as usize][i] = x;
                    return;
                }
                let mut t = self.clone().into_tree();
                t.set(i, x);
                self.0 = Repr::Tree(Rc::new(t));
            }
            Repr::Tree(t) => {
                Rc::make_mut(t).set(i, x);
            }
        }
    }
}

impl Default for List {
    fn default() -> List {
        List::new()
    }
}

impl List {
    /// A list of the items of `items`, made as a single allocation when it
    /// is short (no intermediate vector).
    pub fn from_exact(items: impl ExactSizeIterator<Item = Value>) -> List {
        if items.len() <= SMALL_MAX {
            List(Repr::Small(items.collect()))
        } else {
            List::from(items.collect::<Vec<_>>())
        }
    }
}

impl From<Vec<Value>> for List {
    fn from(v: Vec<Value>) -> List {
        if v.len() <= SMALL_MAX {
            List(Repr::Small(v.into()))
        } else if let Ok(end) = u32::try_from(v.len()) {
            List(Repr::Flat {
                data: Buf::new(v),
                start: 0,
                end,
            })
        } else {
            List(Repr::Tree(Rc::new(Vector::from(v))))
        }
    }
}

impl FromIterator<Value> for List {
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> List {
        List::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl std::ops::Index<usize> for List {
    type Output = Value;
    fn index(&self, i: usize) -> &Value {
        match &self.0 {
            Repr::Small(s) => &s[i],
            Repr::Flat { data, start, end } => &data.items[*start as usize..*end as usize][i],
            Repr::Tree(t) => &t[i],
        }
    }
}

impl PartialEq for List {
    fn eq(&self, o: &List) -> bool {
        self.len() == o.len() && self.iter().eq(o.iter())
    }
}

impl Eq for List {}

impl Ord for List {
    fn cmp(&self, o: &List) -> Ordering {
        self.iter().cmp(o.iter())
    }
}

impl PartialOrd for List {
    fn partial_cmp(&self, o: &List) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Hash for List {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.len().hash(h);
        for v in self.iter() {
            v.hash(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(r: std::ops::Range<i64>) -> List {
        r.map(Value::int).collect()
    }

    #[test]
    fn small_and_big_agree() {
        for n in [0, 1, 31, 32, 33, 100, 1000] {
            let l = ints(0..n);
            assert_eq!(l.len(), n as usize);
            assert!(l.iter().cloned().eq((0..n).map(Value::int)));
            let half = l.len() / 2;
            let joined = l.slice(0, half).concat(l.slice(half, l.len()));
            assert!(joined == l);
            assert!(l.slice(1.min(l.len()), l.len()).len() == l.len().saturating_sub(1));
        }
    }

    #[test]
    fn list_is_no_bigger_than_a_small_slice() {
        assert_eq!(std::mem::size_of::<List>(), 24);
    }

    fn is_flat(l: &List) -> bool {
        matches!(l.0, Repr::Flat { .. })
    }

    fn is_tree(l: &List) -> bool {
        matches!(l.0, Repr::Tree(_))
    }

    #[test]
    fn flat_unique_updates_in_place() {
        let l = ints(0..100).set(5, Value::int(-1));
        assert!(is_flat(&l));
        let l = l.concat(ints(0..3)).concat(ints(0..30));
        assert!(is_flat(&l));
        assert_eq!(l.len(), 133);
        assert!(l[5] == Value::int(-1) && l[102] == Value::int(2) && l[132] == Value::int(29));
    }

    #[test]
    fn shared_flat_becomes_tree() {
        let a = ints(0..100);
        let b = a.clone().set(5, Value::int(-1));
        assert!(is_tree(&b) && is_flat(&a));
        assert!(a[5] == Value::int(5) && b[5] == Value::int(-1));
        let c = a.clone().concat(ints(0..1));
        assert!(is_tree(&c));
        assert_eq!((a.len(), c.len()), (100, 101));
        let d = ints(0..3).concat(a.clone());
        assert!(d[0] == Value::int(0) && d[3] == Value::int(0) && d.len() == 103);
        let e = a.clone().concat(a.clone());
        assert!(e.len() == 200 && e[150] == Value::int(50));
    }

    #[test]
    fn flat_views_share_and_stay_separate() {
        let a = ints(0..100);
        let rest = a.slice(1, 100);
        assert!(is_flat(&rest) && rest[0] == Value::int(1));
        // The view shares `a`'s vector, so neither update shows in the other.
        let r2 = rest.clone().set(0, Value::int(-1));
        assert!(a[1] == Value::int(1) && rest[0] == Value::int(1) && r2[0] == Value::int(-1));
        let rest2 = rest.slice(1, 99).set(0, Value::int(-2));
        assert!(rest2[0] == Value::int(-2) && a[2] == Value::int(2));
        // A unique narrowed view drops the elements past its end when it
        // grows.
        let grown = ints(0..100).slice(0, 50).concat(ints(500..501));
        assert!(grown.len() == 51 && grown[50] == Value::int(500));
        let tail = ints(0..100).slice(60, 100).concat(ints(7..8));
        assert!(tail.len() == 41 && tail[0] == Value::int(60) && tail[40] == Value::int(7));
        assert!(
            tail.iter()
                .cloned()
                .eq((60..100).chain(7..8).map(Value::int))
        );
        // A unique view converted to a tree keeps only its own elements.
        let v = ints(0..100).slice(10, 90);
        let w = v.clone().set(0, Value::int(0));
        assert!(w.len() == 80 && w[1] == Value::int(11) && w[79] == Value::int(89));
    }

    #[test]
    fn index_stays_inside_a_view() {
        let a = ints(0..100);
        let v = a.slice(10, 50);
        assert!(v.get(40).is_none() && v.get(39) == Some(&Value::int(49)));
        let past_end = std::panic::AssertUnwindSafe(|| v[40].clone());
        assert!(std::panic::catch_unwind(past_end).is_err());
    }

    #[test]
    fn shared_flat_builds_its_tree_once() {
        let a = ints(0..100);
        let b = a.clone().set(1, Value::int(-1));
        let Repr::Flat { data, .. } = &a.0 else {
            panic!("not flat")
        };
        let first = data.tree.get().map(|t| t.len());
        assert_eq!(first, Some(100));
        let c = a.slice(10, 90).set(0, Value::int(-2));
        assert!(is_tree(&b) && is_tree(&c));
        assert!(b[1] == Value::int(-1) && c[0] == Value::int(-2) && c[1] == Value::int(11));
        assert!(c.len() == 80 && a[10] == Value::int(10));
        // Once unique again, an update in place drops the stale tree.
        drop((b, c));
        let Repr::Flat { data, .. } = &a.0 else {
            panic!("not flat")
        };
        assert!(data.tree.get().is_some());
        let d = a.set(0, Value::int(7));
        let Repr::Flat { data, .. } = &d.0 else {
            panic!("not flat")
        };
        assert!(data.tree.get().is_none() && d[0] == Value::int(7));
        let e = d.clone().set(0, Value::int(8));
        assert!(e[0] == Value::int(8) && d[0] == Value::int(7));
    }

    #[test]
    fn long_flat_on_the_right_is_joined_not_copied() {
        let chunk = ints(0..100);
        let mut acc = ints(0..100);
        for _ in 0..3 {
            acc = acc.concat(chunk.clone());
            assert!(is_tree(&acc));
        }
        assert!(acc.len() == 400 && acc[399] == Value::int(99));
        // The chunk is shared, so its tree form was built once and reused.
        let Repr::Flat { data, .. } = &chunk.0 else {
            panic!("not flat")
        };
        assert!(data.tree.get().is_some());
        // A short right operand is still pushed in place.
        let pushed = ints(0..100).concat(ints(0..5));
        assert!(is_flat(&pushed) && pushed.len() == 105);
    }

    #[test]
    fn tree_on_the_right_is_joined_not_copied() {
        let t = ints(0..100);
        let t = t.clone().set(0, Value::int(0));
        assert!(is_tree(&t));
        let j = ints(0..100).concat(t);
        assert!(is_tree(&j) && j.len() == 200 && j[150] == Value::int(50));
    }

    #[test]
    fn equal_lists_hash_equal() {
        use std::collections::hash_map::DefaultHasher;
        let h = |l: &List| {
            let mut s = DefaultHasher::new();
            l.hash(&mut s);
            s.finish()
        };
        let big = ints(0..40);
        let small = big.slice(0, 10);
        assert!(small == ints(0..10));
        assert_eq!(h(&small), h(&ints(0..10)));
    }
}
