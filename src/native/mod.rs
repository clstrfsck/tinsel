//! Native code: the IR compiled to machine code with Cranelift, in
//! memory, when a program is loaded (`reference/native-plan.md`).
//!
//! Compiled code runs inside the IR interpreter's runtime
//! (`ir::interp::Runtime`): globals, `where` records, closures and memo
//! tables are the same objects, and each IR instruction that isn't
//! inlined runs the interpreter's own code for it. So the two engines can
//! only differ in control flow and ownership, which the corpus checks.
//!
//! - [`codegen`] lowers the IR to Cranelift.
//! - [`rt`] holds the runtime entry points that compiled code calls and
//!   the calls from Rust into compiled code: the only `unsafe` here.

mod codegen;
#[allow(unsafe_code)]
mod rt;

use crate::error::Span;
use crate::ir::{FuncId, Module};
use crate::value::Value;
use std::rc::Rc;

pub use rt::{prof, run};

/// A compiled program.
pub struct Native {
    module: Rc<Module>,
    /// Owns the memory of the compiled code.
    _jit: cranelift_jit::JITModule,
    /// What a call of each IR function runs (its memo wrapper, if it has
    /// one).
    code: Vec<*const u8>,
    /// The entry shim for each number of arguments.
    shims: Vec<*const u8>,
    /// Places in the IR that compiled code refers to by number.
    sites: Vec<Site>,
    /// Heap constants, which compiled code refers to by address.
    _pool: Vec<Value>,
}

/// An instruction or terminator that compiled code passes to the runtime.
pub struct Site {
    func: FuncId,
    block: u32,
    /// The instruction, or `u32::MAX` for the terminator.
    inst: u32,
    /// How many words the runtime call is given.
    n: u32,
    span: Span,
}

/// Whether calls of `f` pass the record it captures as its closure word,
/// rather than a closure holding it: true for `where` functions and
/// constants' thunks, whose only capture is their record (`ir` module
/// docs). The runtime accepts either (`rt::caps`).
fn record_clo(f: &crate::ir::Function) -> bool {
    f.caps == 1 && matches!(f.kind, crate::ir::FnKind::Named | crate::ir::FnKind::Thunk)
}

/// Compiles `module`.
pub fn compile(module: Rc<Module>) -> Result<Native, String> {
    codegen::compile(module)
}
