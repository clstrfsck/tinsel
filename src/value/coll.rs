//! The set and map types behind Tinsel's `set` and `map` values.
//!
//! While every key is a small int, or every key is a packed pair, a set or
//! map keeps its entries in a radix tree ([`RadixMap`]), keyed by numbers
//! that sort as the values do ([`key_of`]). A lookup there indexes a few
//! arrays instead of comparing values down a B-tree. Any other key moves
//! the collection to `imbl`'s ordered B-tree for good. Both forms iterate in
//! sorted order, and equality, ordering and hashing depend only on the
//! entries, never on the form (SPEC 4.1).

use super::Value;
use super::radix::RadixMap;
use imbl::{OrdMap, OrdSet};
use std::hash::{Hash, Hasher};

/// The kinds of key that the radix form can hold. One collection holds
/// one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    Int,
    Pair,
}

const SIGN: u64 = 1 << 63;

/// The radix key for `v`, if it has one: unsigned numbers that order as the
/// values do. Flipping the sign bit turns two's complement order into
/// unsigned order.
#[inline]
fn key_of(v: &Value) -> Option<(KeyKind, u64)> {
    if let Some(n) = v.0.as_fix() {
        return Some((KeyKind::Int, n as u64 ^ SIGN));
    }
    v.0.pair_order().map(|n| (KeyKind::Pair, n as u64 ^ SIGN))
}

/// The radix key for `v` in a collection of `kind`, if it can be in it.
#[inline]
fn key_in(v: &Value, kind: KeyKind) -> Option<u64> {
    match key_of(v) {
        Some((k, key)) if k == kind => Some(key),
        _ => None,
    }
}

/// One of two iterators with the same items.
enum Either<A, B> {
    A(A),
    B(B),
}

impl<T, A: Iterator<Item = T>, B: Iterator<Item = T>> Iterator for Either<A, B> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        match self {
            Either::A(a) => a.next(),
            Either::B(b) => b.next(),
        }
    }
}

// Sets

#[derive(Clone)]
pub struct ValueSet(SetRepr);

#[derive(Clone)]
enum SetRepr {
    /// Each element is stored with its key, so that walks can lend it.
    Radix(KeyKind, RadixMap<Value>),
    Ord(OrdSet<Value>),
}

impl Default for ValueSet {
    fn default() -> Self {
        ValueSet::new()
    }
}

impl ValueSet {
    pub fn new() -> ValueSet {
        ValueSet(SetRepr::Radix(KeyKind::Int, RadixMap::new()))
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            SetRepr::Radix(_, m) => m.len(),
            SetRepr::Ord(s) => s.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn contains(&self, v: &Value) -> bool {
        match &self.0 {
            SetRepr::Radix(kind, m) => key_in(v, *kind).is_some_and(|k| m.contains_key(k)),
            SetRepr::Ord(s) => s.contains(v),
        }
    }

    /// Adds `v`, returning the equal element it replaced, if any.
    pub fn insert(&mut self, v: Value) -> Option<Value> {
        if let SetRepr::Radix(kind, m) = &mut self.0 {
            match key_of(&v) {
                Some((k, key)) if k == *kind || m.is_empty() => {
                    *kind = k;
                    return m.insert(key, v);
                }
                _ => {
                    let s = m.iter().map(|(_, v)| v.clone()).collect();
                    self.0 = SetRepr::Ord(s);
                }
            }
        }
        match &mut self.0 {
            SetRepr::Ord(s) => s.insert(v),
            SetRepr::Radix(..) => None,
        }
    }

    pub fn remove(&mut self, v: &Value) -> Option<Value> {
        match &mut self.0 {
            SetRepr::Radix(kind, m) => m.remove(key_in(v, *kind)?),
            SetRepr::Ord(s) => s.remove(v),
        }
    }

    /// The elements in increasing order.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        match &self.0 {
            SetRepr::Radix(_, m) => Either::A(m.iter().map(|(_, v)| v)),
            SetRepr::Ord(s) => Either::B(s.iter()),
        }
    }

    /// The elements in decreasing order.
    pub fn iter_rev(&self) -> impl Iterator<Item = &Value> {
        match &self.0 {
            SetRepr::Radix(_, m) => Either::A(m.iter().rev_all().map(|(_, v)| v)),
            SetRepr::Ord(s) => Either::B(s.iter().rev()),
        }
    }

    /// The elements greater than `after`, in increasing order.
    pub fn iter_after<'a>(&'a self, after: &'a Value) -> impl Iterator<Item = &'a Value> {
        use std::ops::Bound::{Excluded, Unbounded};
        match &self.0 {
            SetRepr::Radix(kind, m) => match key_in(after, *kind) {
                Some(k) => Either::A(Either::A(m.iter_after(k).map(|(_, v)| v))),
                // Not a possible element: compare values instead.
                None => Either::A(Either::B(
                    m.iter().map(|(_, v)| v).filter(move |v| *v > after),
                )),
            },
            SetRepr::Ord(s) => Either::B(s.range::<_, Value>((Excluded(after), Unbounded))),
        }
    }

    pub fn get_min(&self) -> Option<&Value> {
        match &self.0 {
            SetRepr::Radix(_, m) => m.first().map(|(_, v)| v),
            SetRepr::Ord(s) => s.get_min(),
        }
    }

    pub fn get_max(&self) -> Option<&Value> {
        match &self.0 {
            SetRepr::Radix(_, m) => m.last().map(|(_, v)| v),
            SetRepr::Ord(s) => s.get_max(),
        }
    }

    pub fn is_subset(&self, other: &ValueSet) -> bool {
        self.len() <= other.len() && self.iter().all(|v| other.contains(v))
    }

    /// The elements in exactly one of `self` and `other`.
    pub fn symmetric_difference(self, other: &ValueSet) -> ValueSet {
        let mut out = self;
        for v in other.iter() {
            if out.remove(v).is_none() {
                out.insert(v.clone());
            }
        }
        out
    }
}

impl FromIterator<Value> for ValueSet {
    fn from_iter<I: IntoIterator<Item = Value>>(items: I) -> ValueSet {
        let mut s = ValueSet::new();
        for v in items {
            s.insert(v);
        }
        s
    }
}

impl PartialEq for ValueSet {
    fn eq(&self, o: &ValueSet) -> bool {
        self.len() == o.len() && self.iter().eq(o.iter())
    }
}

impl Eq for ValueSet {}

impl Hash for ValueSet {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.len().hash(h);
        for v in self.iter() {
            v.hash(h);
        }
    }
}

// Maps

#[derive(Clone)]
pub struct ValueMap(MapRepr);

#[derive(Clone)]
enum MapRepr {
    /// Each entry is stored with its key, so that walks can lend it.
    Radix(KeyKind, RadixMap<(Value, Value)>),
    Ord(OrdMap<Value, Value>),
}

impl Default for ValueMap {
    fn default() -> Self {
        ValueMap::new()
    }
}

fn entry((_, (k, v)): (u64, &(Value, Value))) -> (&Value, &Value) {
    (k, v)
}

impl ValueMap {
    pub fn new() -> ValueMap {
        ValueMap(MapRepr::Radix(KeyKind::Int, RadixMap::new()))
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            MapRepr::Radix(_, m) => m.len(),
            MapRepr::Ord(m) => m.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn get(&self, k: &Value) -> Option<&Value> {
        match &self.0 {
            MapRepr::Radix(kind, m) => m.get(key_in(k, *kind)?).map(|(_, v)| v),
            MapRepr::Ord(m) => m.get(k),
        }
    }

    #[inline]
    pub fn contains_key(&self, k: &Value) -> bool {
        self.get(k).is_some()
    }

    pub fn get_mut(&mut self, k: &Value) -> Option<&mut Value> {
        match &mut self.0 {
            MapRepr::Radix(kind, m) => m.get_mut(key_in(k, *kind)?).map(|(_, v)| v),
            MapRepr::Ord(m) => m.get_mut(k),
        }
    }

    /// Sets `k` to `v`, returning the previous value.
    pub fn insert(&mut self, k: Value, v: Value) -> Option<Value> {
        if let MapRepr::Radix(kind, m) = &mut self.0 {
            match key_of(&k) {
                Some((kk, key)) if kk == *kind || m.is_empty() => {
                    *kind = kk;
                    return m.insert(key, (k, v)).map(|(_, old)| old);
                }
                _ => {
                    let o = m.iter().map(|(_, e)| e.clone()).collect();
                    self.0 = MapRepr::Ord(o);
                }
            }
        }
        match &mut self.0 {
            MapRepr::Ord(m) => m.insert(k, v),
            MapRepr::Radix(..) => None,
        }
    }

    pub fn remove(&mut self, k: &Value) -> Option<Value> {
        match &mut self.0 {
            MapRepr::Radix(kind, m) => m.remove(key_in(k, *kind)?).map(|(_, v)| v),
            MapRepr::Ord(m) => m.remove(k),
        }
    }

    /// The entries in increasing key order.
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        match &self.0 {
            MapRepr::Radix(_, m) => Either::A(m.iter().map(entry)),
            MapRepr::Ord(m) => Either::B(m.iter()),
        }
    }

    /// The entries in decreasing key order.
    pub fn iter_rev(&self) -> impl Iterator<Item = (&Value, &Value)> {
        match &self.0 {
            MapRepr::Radix(_, m) => Either::A(m.iter().rev_all().map(entry)),
            MapRepr::Ord(m) => Either::B(m.iter().rev()),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &Value> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.iter().map(|(_, v)| v)
    }

    /// The entries with keys greater than `after`, in increasing key order.
    pub fn iter_after<'a>(
        &'a self,
        after: &'a Value,
    ) -> impl Iterator<Item = (&'a Value, &'a Value)> {
        use std::ops::Bound::{Excluded, Unbounded};
        match &self.0 {
            MapRepr::Radix(kind, m) => match key_in(after, *kind) {
                Some(k) => Either::A(Either::A(m.iter_after(k).map(entry))),
                // Not a possible key: compare values instead.
                None => Either::A(Either::B(
                    m.iter().map(entry).filter(move |(k, _)| *k > after),
                )),
            },
            MapRepr::Ord(m) => Either::B(m.range::<_, Value>((Excluded(after), Unbounded))),
        }
    }

    pub fn get_min(&self) -> Option<(&Value, &Value)> {
        match &self.0 {
            MapRepr::Radix(_, m) => m.first().map(entry),
            MapRepr::Ord(m) => m.get_min().map(|(k, v)| (k, v)),
        }
    }

    pub fn get_max(&self) -> Option<(&Value, &Value)> {
        match &self.0 {
            MapRepr::Radix(_, m) => m.last().map(entry),
            MapRepr::Ord(m) => m.get_max().map(|(k, v)| (k, v)),
        }
    }
}

impl FromIterator<(Value, Value)> for ValueMap {
    fn from_iter<I: IntoIterator<Item = (Value, Value)>>(items: I) -> ValueMap {
        let mut m = ValueMap::new();
        for (k, v) in items {
            m.insert(k, v);
        }
        m
    }
}

impl PartialEq for ValueMap {
    fn eq(&self, o: &ValueMap) -> bool {
        self.len() == o.len() && self.iter().eq(o.iter())
    }
}

impl Eq for ValueMap {}

impl Hash for ValueMap {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.len().hash(h);
        for (k, v) in self.iter() {
            k.hash(h);
            v.hash(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        println!(
            "Heap {} OrdSet {} OrdMap {} ValueSet {} ValueMap {}",
            std::mem::size_of::<super::super::Heap>(),
            std::mem::size_of::<OrdSet<Value>>(),
            std::mem::size_of::<OrdMap<Value, Value>>(),
            std::mem::size_of::<ValueSet>(),
            std::mem::size_of::<ValueMap>()
        );
    }
}
