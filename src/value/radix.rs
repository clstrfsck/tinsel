//! A persistent ordered map from `u64` keys: a 16-way radix tree with
//! compressed paths (like an adaptive radix tree, but with one node size).
//!
//! An inner node looks at one 4-bit digit of the key, at bit `shift`, and
//! has a slot for each digit value, in order. A slot is empty, holds a leaf
//! (a key and its value) directly, or points to another inner node. All
//! keys below a node share the bits above its digit, kept in `prefix`;
//! levels where every key has the same digit are skipped. So a walk visits
//! keys in increasing unsigned order, and a lookup indexes one array per
//! level: about 4 levels for 20,000 keys, against about 15 for a binary
//! trie.
//!
//! Inner nodes are shared with `Rc`. An update copies the path to the
//! changed slot, except for nodes it holds the only reference to, which it
//! changes in place.

use std::rc::Rc;

const DIGIT: u32 = 4;
const WIDTH: usize = 1 << DIGIT;

#[derive(Clone)]
enum Slot<V> {
    Empty,
    Leaf(u64, V),
    Inner(Rc<Inner<V>>),
}

#[derive(Clone)]
struct Inner<V> {
    /// The key bits above this node's digit; the bits below are zero.
    prefix: u64,
    /// Where this node's digit is in the key.
    shift: u32,
    kids: [Slot<V>; WIDTH],
}

/// The bits above the digit at `shift`.
#[inline]
fn above(shift: u32) -> u64 {
    match shift + DIGIT {
        64 => 0,
        s => !((1u64 << s) - 1),
    }
}

#[inline]
fn digit(k: u64, shift: u32) -> usize {
    ((k >> shift) as usize) & (WIDTH - 1)
}

/// A slot holding an inner node over `a` and `b`, whose keys don't overlap
/// and include `ka` and `kb`. It looks at the highest digit in which `ka`
/// and `kb` differ.
fn join<V>(a: Slot<V>, ka: u64, b: Slot<V>, kb: u64) -> Slot<V> {
    let shift = (63 - (ka ^ kb).leading_zeros()) / DIGIT * DIGIT;
    let mut kids: [Slot<V>; WIDTH] = std::array::from_fn(|_| Slot::Empty);
    kids[digit(ka, shift)] = a;
    kids[digit(kb, shift)] = b;
    Slot::Inner(Rc::new(Inner {
        prefix: ka & above(shift),
        shift,
        kids,
    }))
}

/// A persistent map from `u64` to `V`, in increasing key order.
#[derive(Clone)]
pub struct RadixMap<V> {
    root: Slot<V>,
    len: usize,
}

impl<V: Clone> Default for RadixMap<V> {
    fn default() -> Self {
        RadixMap::new()
    }
}

impl<V: Clone> RadixMap<V> {
    pub fn new() -> RadixMap<V> {
        RadixMap {
            root: Slot::Empty,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn get(&self, k: u64) -> Option<&V> {
        let mut slot = &self.root;
        // Follow the digits down to a leaf, then check the whole key there,
        // since skipped levels weren't checked on the way.
        loop {
            match slot {
                Slot::Empty => return None,
                Slot::Leaf(j, v) => return (*j == k).then_some(v),
                Slot::Inner(n) => slot = &n.kids[digit(k, n.shift)],
            }
        }
    }

    #[inline]
    pub fn contains_key(&self, k: u64) -> bool {
        self.get(k).is_some()
    }

    /// The value for `k`, to change in place. Shared nodes on the way are
    /// copied first.
    pub fn get_mut(&mut self, k: u64) -> Option<&mut V> {
        if !self.contains_key(k) {
            return None;
        }
        let mut slot = &mut self.root;
        loop {
            match slot {
                Slot::Empty => return None,
                Slot::Leaf(_, v) => return Some(v),
                Slot::Inner(n) => {
                    let n = Rc::make_mut(n);
                    slot = &mut n.kids[digit(k, n.shift)];
                }
            }
        }
    }

    /// Sets `k` to `v`, returning the previous value.
    pub fn insert(&mut self, k: u64, v: V) -> Option<V> {
        let old = insert(&mut self.root, k, v);
        if old.is_none() {
            self.len += 1;
        }
        old
    }

    /// Removes `k`, returning its value.
    pub fn remove(&mut self, k: u64) -> Option<V> {
        // Check first, so that a miss doesn't copy a shared path.
        if !self.contains_key(k) {
            return None;
        }
        let v = remove(&mut self.root, k)?;
        self.len -= 1;
        Some(v)
    }

    /// The entries in increasing key order.
    pub fn iter(&self) -> Iter<'_, V> {
        Iter {
            stack: vec![&self.root],
            back: Vec::new(),
        }
    }

    /// The entries with keys greater than `k`, in increasing key order.
    pub fn iter_after(&self, k: u64) -> Iter<'_, V> {
        let mut stack = Vec::new();
        let mut slot = &self.root;
        // Walk down towards `k`. Siblings to the right of the path hold
        // greater keys: push them (largest first, so that the smallest is
        // visited first), then carry on down.
        loop {
            match slot {
                Slot::Empty => break,
                Slot::Leaf(j, _) => {
                    if *j > k {
                        stack.push(slot);
                    }
                    break;
                }
                Slot::Inner(n) => {
                    let high = k & above(n.shift);
                    if n.prefix != high {
                        if n.prefix > high {
                            stack.push(slot);
                        }
                        break;
                    }
                    let d = digit(k, n.shift);
                    stack.extend(n.kids[d + 1..].iter().rev());
                    slot = &n.kids[d];
                }
            }
        }
        Iter {
            stack,
            back: Vec::new(),
        }
    }

    /// The entry with the smallest key.
    pub fn first(&self) -> Option<(u64, &V)> {
        let mut slot = &self.root;
        loop {
            match slot {
                Slot::Empty => return None,
                Slot::Leaf(k, v) => return Some((*k, v)),
                Slot::Inner(n) => slot = n.kids.iter().find(|s| !matches!(s, Slot::Empty))?,
            }
        }
    }

    /// The entry with the largest key.
    pub fn last(&self) -> Option<(u64, &V)> {
        let mut slot = &self.root;
        loop {
            match slot {
                Slot::Empty => return None,
                Slot::Leaf(k, v) => return Some((*k, v)),
                Slot::Inner(n) => {
                    slot = n.kids.iter().rev().find(|s| !matches!(s, Slot::Empty))?;
                }
            }
        }
    }
}

fn insert<V: Clone>(slot: &mut Slot<V>, k: u64, v: V) -> Option<V> {
    let other = match slot {
        Slot::Empty => {
            *slot = Slot::Leaf(k, v);
            return None;
        }
        Slot::Leaf(j, old) if *j == k => return Some(std::mem::replace(old, v)),
        Slot::Leaf(j, _) => *j,
        Slot::Inner(n) if k & above(n.shift) == n.prefix => {
            let n = Rc::make_mut(n);
            return insert(&mut n.kids[digit(k, n.shift)], k, v);
        }
        Slot::Inner(n) => n.prefix,
    };
    // `k` is outside this slot's keys: put both under a new node.
    let old = std::mem::replace(slot, Slot::Empty);
    *slot = join(old, other, Slot::Leaf(k, v), k);
    None
}

/// Removes `k` from under `slot`, which then holds the remaining keys.
fn remove<V: Clone>(slot: &mut Slot<V>, k: u64) -> Option<V> {
    let (v, only) = match slot {
        Slot::Empty => return None,
        Slot::Leaf(j, _) if *j != k => return None,
        Slot::Leaf(..) => match std::mem::replace(slot, Slot::Empty) {
            Slot::Leaf(_, v) => return Some(v),
            _ => return None,
        },
        Slot::Inner(n) => {
            let n = Rc::make_mut(n);
            let v = remove(&mut n.kids[digit(k, n.shift)], k)?;
            // A node left with one child gives way to it: a leaf holds its
            // whole key, and an inner node its own prefix.
            let mut kids = n.kids.iter_mut().filter(|s| !matches!(s, Slot::Empty));
            let only = match (kids.next(), kids.next()) {
                (Some(kid), None) => Some(std::mem::replace(kid, Slot::Empty)),
                _ => None,
            };
            (v, only)
        }
    };
    if let Some(kid) = only {
        *slot = kid;
    }
    Some(v)
}

/// A walk in key order: a stack of slots still to visit, leftmost on top,
/// and for walking backwards, the same from the other end.
pub struct Iter<'a, V> {
    stack: Vec<&'a Slot<V>>,
    back: Vec<&'a Slot<V>>,
}

impl<'a, V> Iterator for Iter<'a, V> {
    type Item = (u64, &'a V);

    fn next(&mut self) -> Option<(u64, &'a V)> {
        loop {
            match self.stack.pop()? {
                Slot::Empty => {}
                Slot::Leaf(k, v) => return Some((*k, v)),
                Slot::Inner(n) => self.stack.extend(n.kids.iter().rev()),
            }
        }
    }
}

impl<V> Iter<'_, V> {
    /// Whether this walk is from the start (so that it can be reversed).
    fn whole(&self) -> bool {
        self.back.is_empty() && self.stack.len() == 1
    }
}

impl<'a, V> Iter<'a, V> {
    /// The entries in decreasing key order. Only a walk from the start can
    /// be reversed; anything else gives nothing.
    pub fn rev_all(mut self) -> impl Iterator<Item = (u64, &'a V)> {
        if self.whole() {
            self.back = std::mem::take(&mut self.stack);
        }
        std::iter::from_fn(move || {
            loop {
                match self.back.pop()? {
                    Slot::Empty => {}
                    Slot::Leaf(k, v) => return Some((*k, v)),
                    Slot::Inner(n) => self.back.extend(n.kids.iter()),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed
    }

    #[test]
    fn matches_btreemap() {
        let mut seed = 7;
        let mut t = RadixMap::new();
        let mut b = BTreeMap::new();
        let mut versions = Vec::new();
        for i in 0..20_000u64 {
            let k = match lcg(&mut seed) % 4 {
                0 => lcg(&mut seed) % 64,
                1 => u64::MAX - lcg(&mut seed) % 64,
                2 => (1 << 63) ^ (lcg(&mut seed) % 1000),
                _ => lcg(&mut seed),
            };
            match lcg(&mut seed) % 4 {
                0 => assert_eq!(t.remove(k), b.remove(&k)),
                1 => {
                    if let Some(v) = t.get_mut(k) {
                        *v += 1;
                    }
                    if let Some(v) = b.get_mut(&k) {
                        *v += 1;
                    }
                }
                _ => assert_eq!(t.insert(k, i), b.insert(k, i)),
            }
            if i.is_multiple_of(1000) {
                versions.push((t.clone(), b.clone()));
            }
            assert_eq!(t.len(), b.len());
        }
        for (t, b) in versions {
            let got: Vec<(u64, u64)> = t.iter().map(|(k, v)| (k, *v)).collect();
            let want: Vec<(u64, u64)> = b.iter().map(|(k, v)| (*k, *v)).collect();
            assert_eq!(got, want);
            let back: Vec<(u64, u64)> = t.iter().rev_all().map(|(k, v)| (k, *v)).collect();
            let want_back: Vec<(u64, u64)> = b.iter().rev().map(|(k, v)| (*k, *v)).collect();
            assert_eq!(back, want_back);
            assert_eq!(t.first(), b.first_key_value().map(|(k, v)| (*k, v)));
            assert_eq!(t.last(), b.last_key_value().map(|(k, v)| (*k, v)));
            for (k, v) in &b {
                assert_eq!(t.get(*k), Some(v));
            }
            // Walks from a key: present keys, and keys in the gaps.
            let probes = b
                .keys()
                .copied()
                .step_by(37)
                .flat_map(|k| [k, k.wrapping_add(1)])
                .chain([0, 5, 1 << 63, u64::MAX]);
            for p in probes {
                let got: Vec<u64> = t.iter_after(p).map(|(k, _)| k).collect();
                let want: Vec<u64> = b
                    .range((std::ops::Bound::Excluded(p), std::ops::Bound::Unbounded))
                    .map(|(k, _)| *k)
                    .collect();
                assert_eq!(got, want, "after {p}");
            }
        }
    }
}
