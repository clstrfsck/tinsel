//! A fast, non-cryptographic hasher for the interpreter's own hash tables:
//! memo tables, the states found by a search, and cycle detection.
//!
//! The standard library's default, SipHash, resists hash-flooding attacks.
//! Tinsel programs only hash their own values, so that protection buys
//! nothing, and SipHash was 5–7% of the time in search-heavy solutions.
//! This is the multiply-and-rotate hash used inside rustc ("FxHash").
//! Iteration order of these tables is never visible to programs.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

pub type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxHashSet<T> = HashSet<T, BuildHasherDefault<FxHasher>>;

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.add(u64::from_le_bytes(*w));
        }
        if !rest.is_empty() {
            let mut w = [0u8; 8];
            w[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(w));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash};

    fn hash<T: Hash>(v: &T) -> u64 {
        BuildHasherDefault::<FxHasher>::default().hash_one(v)
    }

    #[test]
    fn equal_values_hash_equally() {
        assert_eq!(hash(&(1i64, "abc")), hash(&(1i64, "abc")));
        assert_eq!(hash(&vec![1u32, 2, 3]), hash(&vec![1u32, 2, 3]));
    }

    #[test]
    fn small_differences_change_the_hash() {
        assert_ne!(hash(&(1i64, 2i64)), hash(&(2i64, 1i64)));
        assert_ne!(hash(&"abcdefghi"), hash(&"abcdefghj"));
        assert_ne!(hash(&0u64), hash(&1u64));
    }

    #[test]
    fn a_map_works() {
        let mut m: FxHashMap<(i64, i64), usize> = FxHashMap::default();
        for x in 0..100 {
            for y in 0..100 {
                m.insert((x, y), (x * 100 + y) as usize);
            }
        }
        assert_eq!(m.len(), 10_000);
        assert_eq!(m.get(&(42, 17)), Some(&4217));
    }
}
