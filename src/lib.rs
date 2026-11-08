//! Tinsel: a small, pure, dynamically typed language for Advent of Code.
//! See `SPEC.md` for the language definition.
//!
//! Pipeline: [`lexer`] → [`layout`] → [`parser`] → [`resolve`] (names, call
//! graph, [`memo`] decisions) → [`eval`] (tree-walking evaluator).
//!
//! `unsafe` is denied everywhere except the modules that
//! `reference/native-plan.md` (section 4) allows, which opt in one by one.

#![deny(unsafe_code)]
#![warn(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]

/// The most memory one value built from a number in the program may take:
/// a `str` width, a `repeat` count, or the result of `<<` or `**`. Past this
/// the request is an error rather than an allocation that the system grants
/// lazily and then cannot back (SPEC 5.1).
pub const MAX_VALUE_BYTES: u64 = 1 << 32;

pub mod ast;
pub mod builtins;
pub mod error;
pub mod eval;
pub mod fxhash;
pub mod int;
pub mod ir;
pub mod layout;
pub mod lexer;
pub mod list;
pub mod memo;
pub mod moves;
pub mod native;
pub mod parser;
pub mod resolve;
pub mod runner;
pub mod value;
