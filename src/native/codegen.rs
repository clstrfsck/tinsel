//! Lowers the IR to Cranelift (`reference/native-plan.md`, phase 3).
//!
//! **Functions.** IR function `i` becomes `f{i}`, with Cranelift's `tail`
//! calling convention and the parameters `(cx, clo, site, chain, args...)`:
//! the runtime context, the closure being run (`none` for a direct call of
//! a function without captures), the call site and chain (for traces; see
//! [`rt::rt_trace`]) and the arguments. Everything is a word, and the result is a word or `ERR`. A
//! memoized function's body is `b{i}`, and `f{i}` wraps it with the memo
//! table lookup. Rust calls in through `shim{n}`, one per argument count,
//! with the platform's C convention.
//!
//! **Ownership** follows the IR's liveness (`ir::live`): a function owns
//! its closure and arguments, and every variable from its definition to
//! its last use. A last use hands the word on; any other use takes a new
//! reference (`rt_dup`). Words dropped on a control-flow edge, or never
//! used, are released (`rt_drop`). Each runtime entry point consumes or
//! borrows its words as its comment in `rt.rs` says.
//!
//! **Errors.** A call that returns `ERR` branches to an error exit, which
//! releases what the function still owns at that point, adds the function
//! to the error's trace, and returns `ERR`.
//!
//! **Operations** run in the runtime (`rt_op`, the same code as the IR
//! interpreter) except for a few fast paths inlined here: fixnum
//! arithmetic and comparisons, bool checks, and equality with an unboxed
//! constant.

use super::rt::{self, ENTRY};
use super::{Native, Site};
use crate::ast::{BinOp, CmpOp};
use crate::error::Span;
use crate::ir::live::{self, Bits};
use crate::ir::{self, Callee, FnKind, Op, Term, Use, Var};
use crate::value::Value;
use crate::value::word::sentinel::{DONE, ERR, MISS};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I64;
use cranelift_codegen::ir::{
    AbiParam, Block, BlockArg, FuncRef, InstBuilder, MemFlagsData, Signature, StackSlot,
    StackSlotData, StackSlotKind, UserFuncName, Value as Cv,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId as ClifId, Linkage, Module as _, default_libcall_names};
use std::collections::HashMap;
use std::rc::Rc;

// Indices of the runtime entry points, in the order of `rt::symbols`.
const DUP: usize = 0;
const DROP: usize = 1;
const OP: usize = 2;
const NEXT: usize = 3;
const FAIL: usize = 4;
const TRACE: usize = 5;
const STACK: usize = 6;
const CLOSURE: usize = 7;
const TAIL_TARGET: usize = 8;
const CALL_VALUE: usize = 9;
const MEMO_GET: usize = 10;
const MEMO_PUT: usize = 11;
const CAP: usize = 12;
const RECVAL: usize = 13;
const RECPARENT: usize = 14;
const FORCE: usize = 15;
const THUNK_CODE: usize = 16;
const FORCED: usize = 17;
const ITEM: usize = 18;
const IS_TUPLE: usize = 19;
const IS_LIST: usize = 20;
const IS_TYPE: usize = 21;
const INDEX: usize = 22;
const INDEX_INT: usize = 23;
const CMP: usize = 24;
const TUPLE: usize = 25;
const GLOBAL: usize = 26;
const RECORD: usize = 27;
const BIN: usize = 28;
const BUILTIN: usize = 29;
const LEN: usize = 30;
const GET: usize = 31;
const SET: usize = 32;
const LIST: usize = 33;
const ACC: usize = 34;
const PUSH: usize = 35;
const DONE_ACC: usize = 36;
const ITER: usize = 37;
const LIVE: usize = 38;
const LOOPABLE: usize = 39;
const MIDDLE: usize = 40;
const SLICE: usize = 41;
const ITER_INIT: usize = 42;
const SEQ_GET: usize = 43;

/// A site's `inst` for a terminator.
const TERM: u32 = u32::MAX;

/// The words of a few unboxed values.
struct Words {
    none: u64,
    t: u64,
    f: u64,
}

fn words() -> Words {
    Words {
        none: Value::NONE.raw_bits(),
        t: Value::TRUE.raw_bits(),
        f: Value::FALSE.raw_bits(),
    }
}

/// The signature of compiled functions taking `n` arguments.
fn tail_sig(n: u32) -> Signature {
    let mut s = Signature::new(CallConv::Tail);
    s.params
        .extend((0..4 + n as usize).map(|_| AbiParam::new(I64)));
    s.returns.push(AbiParam::new(I64));
    s
}

pub fn compile(module: Rc<ir::Module>) -> Result<Native, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let verify = if cfg!(debug_assertions) {
        "true"
    } else {
        "false"
    };
    let mut jb = JITBuilder::with_flags(
        &[("opt_level", "speed"), ("enable_verifier", verify)],
        default_libcall_names(),
    )
    .map_err(|e| err(&e))?;
    let syms = rt::symbols();
    for (name, addr, _, _) in &syms {
        jb.symbol(*name, *addr);
    }
    let mut jit = JITModule::new(jb);
    let mut rt_ids = Vec::new();
    for (name, _, params, ret) in &syms {
        let mut sig = jit.make_signature();
        sig.params.extend((0..*params).map(|_| AbiParam::new(I64)));
        if *ret {
            sig.returns.push(AbiParam::new(I64));
        }
        rt_ids.push(
            jit.declare_function(name, Linkage::Import, &sig)
                .map_err(|e| err(&e))?,
        );
    }

    // Declare every function first, so that bodies can call each other.
    let mut entries = Vec::new();
    let mut bodies = Vec::new();
    for (i, f) in module.funcs.iter().enumerate() {
        let sig = tail_sig(f.params);
        entries.push(
            jit.declare_function(&format!("f{i}"), Linkage::Local, &sig)
                .map_err(|e| err(&e))?,
        );
        bodies.push(match f.memo {
            Some(_) => Some(
                jit.declare_function(&format!("b{i}"), Linkage::Local, &sig)
                    .map_err(|e| err(&e))?,
            ),
            None => None,
        });
    }
    let max_params = module.funcs.iter().map(|f| f.params).max().unwrap_or(0);
    let mut shim_sig = jit.make_signature();
    shim_sig.params.extend((0..5).map(|_| AbiParam::new(I64)));
    shim_sig.returns.push(AbiParam::new(I64));
    let mut shims = Vec::new();
    for n in 0..=max_params {
        shims.push(
            jit.declare_function(&format!("shim{n}"), Linkage::Local, &shim_sig)
                .map_err(|e| err(&e))?,
        );
    }

    let tc = jit.target_config();
    let off = crate::value::layout::offsets();
    let rc = crate::value::word::rc_count_offset();
    let mut ctx = jit.make_context();
    let mut fctx = FunctionBuilderContext::new();
    let mut sites = Vec::new();
    let mut pool = Vec::new();
    let w = words();
    for (i, f) in module.funcs.iter().enumerate() {
        let body = bodies[i].unwrap_or(entries[i]);
        ctx.func.signature = tail_sig(f.params);
        ctx.func.name = UserFuncName::user(0, body.as_u32());
        {
            let b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            let mut g = Gen {
                b,
                jit: &mut jit,
                rt_ids: &rt_ids,
                rt: vec![None; rt_ids.len()],
                targets: &entries,
                m: &module,
                frefs: HashMap::new(),
                f,
                fid: i as u32,
                sites: &mut sites,
                pool: &mut pool,
                vars: Vec::new(),
                cx: None,
                clo: None,
                site: None,
                chain: None,
                tails: None,
                argv: None,
                blocks: Vec::new(),
                live_in: Vec::new(),
                errs: HashMap::new(),
                off,
                rc,
                iters: HashMap::new(),
                iter_slot: None,
                edges: Vec::new(),
                w: &w,
            };
            g.function();
            g.b.seal_all_blocks();
            g.b.finalize(tc);
        }
        jit.define_function(body, &mut ctx)
            .map_err(|e| format!("`{}`: {e:?}", f.name))?;
        jit.clear_context(&mut ctx);

        if let Some(body) = bodies[i] {
            ctx.func.signature = tail_sig(f.params);
            ctx.func.name = UserFuncName::user(0, entries[i].as_u32());
            {
                let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
                memo_wrapper(&mut b, &mut jit, &rt_ids, body, i as u32);
                b.seal_all_blocks();
                b.finalize(tc);
            }
            jit.define_function(entries[i], &mut ctx)
                .map_err(|e| format!("`{}` (memo): {e:?}", f.name))?;
            jit.clear_context(&mut ctx);
        }
    }
    for (n, &id) in shims.iter().enumerate() {
        ctx.func.signature = shim_sig.clone();
        ctx.func.name = UserFuncName::user(1, id.as_u32());
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            shim(&mut b, n as u32);
            b.seal_all_blocks();
            b.finalize(tc);
        }
        jit.define_function(id, &mut ctx)
            .map_err(|e| format!("shim{n}: {e:?}"))?;
        jit.clear_context(&mut ctx);
    }
    jit.finalize_definitions().map_err(|e| err(&e))?;
    let code = entries
        .iter()
        .map(|&id| jit.get_finalized_function(id))
        .collect();
    let shims = shims
        .iter()
        .map(|&id| jit.get_finalized_function(id))
        .collect();
    Ok(Native {
        module,
        _jit: jit,
        code,
        shims,
        sites,
        _pool: pool,
    })
}

/// `shim{n}(cx, code, clo, site, argv)`: calls `code`, a compiled function
/// of `n` arguments, with the arguments in `argv`.
fn shim(b: &mut FunctionBuilder<'_>, n: u32) {
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let p = b.block_params(entry).to_vec();
    let (cx, code, clo, site, argv) = (p[0], p[1], p[2], p[3], p[4]);
    let chain = b.ins().iconst(I64, 0);
    let mut args = vec![cx, clo, site, chain];
    for i in 0..n {
        args.push(
            b.ins()
                .load(I64, MemFlagsData::trusted(), argv, (i * 8) as i32),
        );
    }
    let sig = b.import_signature(tail_sig(n));
    let call = b.ins().call_indirect(sig, code, &args);
    let r = b.inst_results(call)[0];
    b.ins().return_(&[r]);
}

/// Calls the runtime entry point `k` of `rt_ids` from `b`.
fn call_rt(
    b: &mut FunctionBuilder<'_>,
    jit: &mut JITModule,
    rt_ids: &[ClifId],
    k: usize,
    args: &[Cv],
) -> Option<Cv> {
    let f = jit.declare_func_in_func(rt_ids[k], b.func);
    let call = b.ins().call(f, args);
    b.inst_results(call).first().copied()
}

/// Calls runtime entry point `k` on `v` if it is a pointer word.
fn if_ptr(b: &mut FunctionBuilder<'_>, f: FuncRef, v: Cv) {
    let tag = b.ins().band_imm_u(v, 3);
    let is_ptr = b.ins().icmp_imm_s(IntCC::Equal, tag, 1);
    let yes = b.create_block();
    let done = b.create_block();
    b.ins().brif(is_ptr, yes, &[], done, &[]);
    b.switch_to_block(yes);
    b.ins().call(f, &[v]);
    b.ins().jump(done, &[]);
    b.switch_to_block(done);
}

/// Stores words in a new stack slot and gives its address.
fn spill(b: &mut FunctionBuilder<'_>, vals: &[Cv]) -> Cv {
    if vals.is_empty() {
        return b.ins().iconst(I64, 0);
    }
    let ss = b.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        (vals.len() * 8) as u32,
        3,
    ));
    for (i, v) in vals.iter().enumerate() {
        b.ins().stack_store(I64, *v, ss, (i * 8) as i32);
    }
    b.ins().stack_addr(I64, ss, 0)
}

/// `f{i}` for a memoized function: looks the arguments up in its table,
/// and on a miss calls the body `b{i}` and stores the result.
fn memo_wrapper(
    b: &mut FunctionBuilder<'_>,
    jit: &mut JITModule,
    rt_ids: &[ClifId],
    body: ClifId,
    fid: u32,
) {
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let p = b.block_params(entry).to_vec();
    let (cx, clo, site, chain) = (p[0], p[1], p[2], p[3]);
    let args = p[4..].to_vec();
    let dup = jit.declare_func_in_func(rt_ids[DUP], b.func);
    let drop = jit.declare_func_in_func(rt_ids[DROP], b.func);
    let fid = b.ins().iconst(I64, i64::from(fid));

    // Look up, borrowing the arguments.
    let addr = spill(b, &args);
    let r = call_rt(b, jit, rt_ids, MEMO_GET, &[cx, clo, fid, addr]).unwrap_or(cx);
    let hit = b.ins().icmp_imm_s(IntCC::NotEqual, r, MISS as i64);
    let (hit_b, miss_b) = (b.create_block(), b.create_block());
    b.ins().brif(hit, hit_b, &[], miss_b, &[]);

    b.switch_to_block(hit_b);
    for &a in &args {
        if_ptr(b, drop, a);
    }
    if_ptr(b, drop, clo);
    b.ins().return_(&[r]);

    // Miss: the body gets its own references; these ones become the key.
    b.switch_to_block(miss_b);
    for &a in &args {
        if_ptr(b, dup, a);
    }
    if_ptr(b, dup, clo);
    let target = jit.declare_func_in_func(body, b.func);
    // The body continues any chain of tail calls this call is part of.
    let mut call_args = vec![cx, clo, site, chain];
    call_args.extend(&args);
    let call = b.ins().call(target, &call_args);
    let r = b.inst_results(call)[0];
    let failed = b.ins().icmp_imm_s(IntCC::Equal, r, ERR as i64);
    let (err_b, ok_b) = (b.create_block(), b.create_block());
    b.ins().brif(failed, err_b, &[], ok_b, &[]);

    b.switch_to_block(err_b);
    b.set_cold_block(err_b);
    for &a in &args {
        if_ptr(b, drop, a);
    }
    if_ptr(b, drop, clo);
    let e = b.ins().iconst(I64, ERR as i64);
    b.ins().return_(&[e]);

    b.switch_to_block(ok_b);
    let addr = spill(b, &args);
    call_rt(b, jit, rt_ids, MEMO_PUT, &[cx, clo, fid, addr, r]);
    if_ptr(b, drop, clo);
    b.ins().return_(&[r]);
}

/// The code generator for one function's body.
struct Gen<'a> {
    b: FunctionBuilder<'a>,
    jit: &'a mut JITModule,
    rt_ids: &'a [ClifId],
    rt: Vec<Option<FuncRef>>,
    targets: &'a [ClifId],
    m: &'a ir::Module,
    frefs: HashMap<u32, FuncRef>,
    f: &'a ir::Function,
    fid: u32,
    sites: &'a mut Vec<Site>,
    pool: &'a mut Vec<Value>,
    vars: Vec<Variable>,
    cx: Option<Cv>,
    clo: Option<Cv>,
    site: Option<Cv>,
    chain: Option<Cv>,
    /// For a function with self tail calls (`Op::SelfTail`): how many it
    /// has made, and the site of the last.
    tails: Option<(Variable, Variable)>,
    argv: Option<StackSlot>,
    blocks: Vec<Block>,
    live_in: Vec<Bits>,
    /// Error exits, by the variables they release.
    errs: HashMap<Vec<Var>, Block>,
    /// Where things are inside heap objects.
    off: crate::value::layout::Offsets,
    /// Where an object's reference count is, from its address, if known.
    rc: Option<i32>,
    /// For each iterator variable only read by `Next`: its mode, position
    /// and end (see `iter_init`).
    iters: HashMap<Var, (Variable, Variable, Variable)>,
    /// Three words for `rt_iter_init`'s results.
    iter_slot: Option<StackSlot>,
    /// Edge blocks to fill in: (block, variables to release, target).
    edges: Vec<(Block, Vec<Var>, Block)>,
    w: &'a Words,
}

impl Gen<'_> {
    fn cx(&self) -> Cv {
        self.cx.expect("set in the prologue")
    }

    fn clo(&self) -> Cv {
        self.clo.expect("set in the prologue")
    }

    fn site_param(&self) -> Cv {
        self.site.expect("set in the prologue")
    }

    fn chain_param(&self) -> Cv {
        self.chain.expect("set in the prologue")
    }

    /// The self tail calls made so far, and the site of the last.
    fn self_tails(&mut self) -> (Cv, Cv) {
        match self.tails {
            Some((n, last)) => (self.b.use_var(n), self.b.use_var(last)),
            None => (self.iconst(0), self.iconst(0)),
        }
    }

    /// The `site` and `chain` words for a tail call made at site `t`: one
    /// more tail call in this chain, which starts here if this function
    /// wasn't itself reached by tail calls. A function that isn't traced
    /// doesn't start a chain, as in the IR interpreter.
    fn tail_words(&mut self, t: Cv) -> (Cv, Cv) {
        if !self.traced() {
            let z = self.iconst(0);
            return (t, z);
        }
        let incoming = self.site_param();
        let before = self.b.ins().ushr_imm_u(incoming, 32);
        let (n, _) = self.self_tails();
        let total = self.b.ins().iadd(before, n);
        let total = self.b.ins().iadd_imm_u(total, 1);
        let most = self.iconst(u64::from(u32::MAX));
        let total = self.b.ins().umin(total, most);
        let high = self.b.ins().ishl_imm_u(total, 32);
        let site = self.b.ins().bor(high, t);
        // Starting a chain: where this function was called (the whole
        // incoming word, as it has no count), and the function.
        let fid = self.iconst(u64::from(self.fid) << 32);
        let fresh = self.b.ins().bor(incoming, fid);
        let chain_in = self.chain_param();
        let chain = self.b.ins().select(before, chain_in, fresh);
        (site, chain)
    }

    fn iconst(&mut self, n: u64) -> Cv {
        self.b.ins().iconst(I64, n as i64)
    }

    fn rt_ref(&mut self, k: usize) -> FuncRef {
        if let Some(f) = self.rt[k] {
            return f;
        }
        let f = self.jit.declare_func_in_func(self.rt_ids[k], self.b.func);
        self.rt[k] = Some(f);
        f
    }

    fn call_rt(&mut self, k: usize, args: &[Cv]) -> Option<Cv> {
        let f = self.rt_ref(k);
        let call = self.b.ins().call(f, args);
        self.b.inst_results(call).first().copied()
    }

    /// Calls a runtime entry point that returns a word.
    fn call_rt_word(&mut self, k: usize, args: &[Cv]) -> Cv {
        match self.call_rt(k, args) {
            Some(v) => v,
            None => self.iconst(ERR),
        }
    }

    fn target(&mut self, func: u32) -> FuncRef {
        if let Some(f) = self.frefs.get(&func) {
            return *f;
        }
        let f = self
            .jit
            .declare_func_in_func(self.targets[func as usize], self.b.func);
        self.frefs.insert(func, f);
        f
    }

    /// Takes a new reference to `v`: for a pointer word, increments its
    /// object's count inline (when its place is known).
    fn dup(&mut self, v: Cv) {
        let Some(off) = self.rc else {
            let f = self.rt_ref(DUP);
            if_ptr(&mut self.b, f, v);
            return;
        };
        let tag = self.b.ins().band_imm_u(v, 3);
        let is_ptr = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 1);
        let (yes, done) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is_ptr, yes, &[], done, &[]);
        self.b.switch_to_block(yes);
        let p = self.b.ins().band_imm_s(v, !7);
        let c = self.b.ins().load(I64, MemFlagsData::trusted(), p, off);
        let c1 = self.b.ins().iadd_imm_s(c, 1);
        self.b.ins().store(MemFlagsData::trusted(), c1, p, off);
        self.count_live(1);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// Releases `v`: for a pointer word, decrements its object's count
    /// inline, calling the runtime only to free it, or when it is watched
    /// for cycles (bit 2; see `ir::interp::release_cycle`).
    fn release(&mut self, v: Cv) {
        let f = self.rt_ref(DROP);
        let Some(off) = self.rc else {
            if_ptr(&mut self.b, f, v);
            return;
        };
        let low = self.b.ins().band_imm_u(v, 7);
        let plain = self.b.ins().icmp_imm_s(IntCC::Equal, low, 1);
        let (fast, other, call, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().brif(plain, fast, &[], other, &[]);
        // A watched pointer (`…101`) goes to the runtime; anything else
        // holds no reference.
        self.b.switch_to_block(other);
        let tag = self.b.ins().band_imm_u(v, 3);
        let is_ptr = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 1);
        self.b.ins().brif(is_ptr, call, &[], done, &[]);
        self.b.switch_to_block(fast);
        let p = self.b.ins().band_imm_s(v, !7);
        let c = self.b.ins().load(I64, MemFlagsData::trusted(), p, off);
        let last = self.b.ins().icmp_imm_s(IntCC::Equal, c, 1);
        let dec = self.b.create_block();
        self.b.ins().brif(last, call, &[], dec, &[]);
        self.b.switch_to_block(dec);
        let c1 = self.b.ins().iadd_imm_s(c, -1);
        self.b.ins().store(MemFlagsData::trusted(), c1, p, off);
        self.count_live(-1);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(call);
        self.b.set_cold_block(call);
        self.b.ins().call(f, &[v]);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// Debug builds: keeps the live word count (`word::live`) right for
    /// references counted inline.
    fn count_live(&mut self, d: i64) {
        if cfg!(debug_assertions) {
            let d = self.b.ins().iconst(I64, d);
            self.call_rt(LIVE, &[d]);
        }
    }

    fn var(&self, v: Var) -> Variable {
        self.vars[v as usize]
    }

    fn get(&mut self, u: Use) -> Cv {
        let v = self.var(u.var);
        self.b.use_var(v)
    }

    /// An operand to hand on: moved on its last use, otherwise a new
    /// reference.
    fn owned(&mut self, u: Use) -> Cv {
        let v = self.get(u);
        if !u.last {
            self.dup(v);
        }
        v
    }

    fn release_var(&mut self, v: Var) {
        let x = self.b.use_var(self.var(v));
        self.release(x);
    }

    fn new_site(&mut self, block: usize, inst: u32, n: usize, span: Span) -> Cv {
        self.sites.push(Site {
            func: self.fid,
            block: block as u32,
            inst,
            n: n as u32,
            span,
        });
        let s = (self.sites.len() - 1) as u64;
        self.iconst(s)
    }

    /// Stores words in the function's argument array and gives its address.
    fn argv(&mut self, vals: &[Cv]) -> Cv {
        let Some(ss) = self.argv else {
            return self.iconst(0);
        };
        for (i, v) in vals.iter().enumerate() {
            self.b.ins().stack_store(I64, *v, ss, (i * 8) as i32);
        }
        self.b.ins().stack_addr(I64, ss, 0)
    }

    /// The error exit releasing `held`.
    fn err_block(&mut self, mut held: Vec<Var>) -> Block {
        held.sort_unstable();
        if let Some(b) = self.errs.get(&held) {
            return *b;
        }
        let b = self.b.create_block();
        self.errs.insert(held, b);
        b
    }

    /// Continues if `r` isn't `ERR`, and otherwise leaves through the error
    /// exit releasing `held`.
    fn check(&mut self, r: Cv, held: Vec<Var>) {
        let bad = self.b.ins().icmp_imm_s(IntCC::Equal, r, ERR as i64);
        let eb = self.err_block(held);
        let ok = self.b.create_block();
        self.b.ins().brif(bad, eb, &[], ok, &[]);
        self.b.switch_to_block(ok);
    }

    /// Whether errors leaving this function name it in the trace (as the IR
    /// interpreter does).
    fn traced(&self) -> bool {
        matches!(self.f.kind, FnKind::Named | FnKind::Lambda)
    }

    fn function(&mut self) {
        let f = self.f;
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let p = self.b.block_params(entry).to_vec();
        self.cx = Some(p[0]);
        self.clo = Some(p[1]);
        self.site = Some(p[2]);
        self.chain = Some(p[3]);
        let args = p[4..].to_vec();
        self.vars = (0..f.nvars).map(|_| self.b.declare_var(I64)).collect();
        if f.blocks
            .iter()
            .any(|b| b.insts.iter().any(|i| matches!(i.op, Op::SelfTail)))
        {
            let (n, last) = (self.b.declare_var(I64), self.b.declare_var(I64));
            let z = self.iconst(0);
            self.b.def_var(n, z);
            self.b.def_var(last, z);
            self.tails = Some((n, last));
        }
        self.blocks = f.blocks.iter().map(|_| self.b.create_block()).collect();
        self.live_in = live::live_in(f);
        // Iterators read only by `Next` (comprehensions and library
        // loops) can keep their state in machine variables.
        let mut only_next: HashMap<Var, bool> = HashMap::new();
        for block in &f.blocks {
            for inst in &block.insts {
                if let (Op::Iter { .. }, Some(d)) = (&inst.op, inst.dst) {
                    only_next.entry(d).or_insert(true);
                }
            }
        }
        for block in &f.blocks {
            for inst in &block.insts {
                inst.op.uses(&mut |u| {
                    if let Some(ok) = only_next.get_mut(&u.var) {
                        *ok = false;
                    }
                });
            }
            if !matches!(block.term, Term::Next { .. }) {
                block.term.uses(&mut |u| {
                    if let Some(ok) = only_next.get_mut(&u.var) {
                        *ok = false;
                    }
                });
            }
            if block.params.iter().any(|p| only_next.contains_key(p)) {
                for p in &block.params {
                    only_next.remove(p);
                }
            }
        }
        for (v, ok) in only_next {
            if ok {
                let vars = (
                    self.b.declare_var(I64),
                    self.b.declare_var(I64),
                    self.b.declare_var(I64),
                );
                self.iters.insert(v, vars);
            }
        }
        let mut most = 0;
        for block in &f.blocks {
            for inst in &block.insts {
                let mut n = 0;
                inst.op.uses(&mut |_| n += 1);
                most = most.max(n);
            }
            let mut n = 0;
            block.term.uses(&mut |_| n += 1);
            most = most.max(n);
        }
        if most > 0 {
            self.argv = Some(self.b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                most * 8,
                3,
            )));
        }

        // The stack check.
        let sp = self.b.ins().get_stack_pointer(I64);
        let cx = self.cx();
        let limit = self.b.ins().load(I64, MemFlagsData::trusted(), cx, 0);
        let low = self.b.ins().icmp(IntCC::UnsignedLessThan, sp, limit);
        let over = self.b.create_block();
        let start = self.b.create_block();
        self.b.ins().brif(low, over, &[], start, &[]);
        self.b.switch_to_block(over);
        self.b.set_cold_block(over);
        let (cx, site) = (self.cx(), self.site_param());
        let r = self.call_rt_word(STACK, &[cx, site]);
        for &a in &args {
            self.release(a);
        }
        let clo = self.clo();
        self.release(clo);
        self.trace();
        self.b.ins().return_(&[r]);

        self.b.switch_to_block(start);
        for (&p, &a) in f.blocks[0].params.iter().zip(&args) {
            self.b.def_var(self.var(p), a);
        }
        self.b.ins().jump(self.blocks[0], &[]);

        for bi in 0..f.blocks.len() {
            self.block(bi);
        }

        for (e, drops, target) in std::mem::take(&mut self.edges) {
            self.b.switch_to_block(e);
            for v in drops {
                self.release_var(v);
            }
            self.b.ins().jump(target, &[]);
        }
        let mut errs: Vec<(Vec<Var>, Block)> = self.errs.drain().collect();
        errs.sort_by_key(|(_, b)| *b);
        for (held, b) in errs {
            self.b.switch_to_block(b);
            self.b.set_cold_block(b);
            for v in held {
                self.release_var(v);
            }
            let clo = self.clo();
            self.release(clo);
            self.trace();
            let e = self.iconst(ERR);
            self.b.ins().return_(&[e]);
        }
    }

    /// Adds this function to the pending error's trace, if it is traced.
    fn trace(&mut self) {
        if self.traced() {
            let (cx, site, chain) = (self.cx(), self.site_param(), self.chain_param());
            let fid = self.iconst(u64::from(self.fid));
            let (n, last) = self.self_tails();
            self.call_rt(TRACE, &[cx, fid, site, chain, n, last]);
        }
    }

    fn block(&mut self, bi: usize) {
        let f = self.f;
        let block = &f.blocks[bi];
        self.b.switch_to_block(self.blocks[bi]);

        // Liveness inside the block, from its end back.
        let mut live = Bits::new(f.nvars);
        for s in block.term.successors() {
            live.union(&self.live_in[s as usize]);
        }
        block.term.uses(&mut |u| live.set(u.var));
        let before_term = live.clone();
        let mut after = Vec::with_capacity(block.insts.len());
        for inst in block.insts.iter().rev() {
            after.push(live.clone());
            if let Some(d) = inst.dst {
                live.clear(d);
            }
            inst.op.uses(&mut |u| live.set(u.var));
        }
        after.reverse();

        // Parameters nothing reads.
        for &p in &block.params {
            if !live.has(p) {
                self.release_var(p);
            }
        }
        for (ii, inst) in block.insts.iter().enumerate() {
            if let (Op::SelfTail, Some((n, last))) = (&inst.op, self.tails) {
                let s = self.new_site(bi, ii as u32, 0, inst.span);
                let c = self.b.use_var(n);
                let c = self.b.ins().iadd_imm_u(c, 1);
                self.b.def_var(n, c);
                self.b.def_var(last, s);
                continue;
            }
            let mut held = after[ii].clone();
            if let Some(d) = inst.dst {
                held.clear(d);
            }
            let held: Vec<Var> = held.iter().collect();
            let r = self.inst(bi, ii, inst, held);
            match inst.dst {
                Some(d) => self.b.def_var(self.var(d), r),
                None => self.release(r),
            }
        }
        self.term(bi, before_term);
    }

    /// The generic path: the runtime runs the instruction.
    fn generic(&mut self, bi: usize, ii: usize, inst: &ir::Inst, held: Vec<Var>) -> Cv {
        let mut uses = Vec::new();
        inst.op.uses(&mut |u| uses.push(u));
        let vals: Vec<Cv> = uses.iter().map(|&u| self.owned(u)).collect();
        let addr = self.argv(&vals);
        let site = self.new_site(bi, ii as u32, uses.len(), inst.span);
        let (cx, clo) = (self.cx(), self.clo());
        let r = self.call_rt_word(OP, &[cx, clo, site, addr]);
        self.check(r, held);
        r
    }

    /// A comparison in the runtime (borrowing its operands), as the slow
    /// side of a fast path: jumps to `merge` with the result.
    #[allow(clippy::too_many_arguments)]
    fn cmp_slow(
        &mut self,
        op: CmpOp,
        ua: Use,
        ub: Use,
        bi: usize,
        ii: usize,
        inst: &ir::Inst,
        held: Vec<Var>,
        merge: Block,
    ) {
        let o = rt::CMPS.iter().position(|x| *x == op).unwrap_or(0);
        let pre = [
            self.cx(),
            self.site_for(bi, ii, inst),
            self.iconst(o as u64),
        ];
        let r = self.borrowing(CMP, &pre, &[ua, ub], &[], held, true);
        self.b.ins().jump(merge, &[BlockArg::Value(r)]);
    }

    /// The generic path as the slow side of a fast path: jumps to `merge`
    /// with the result.
    fn slow(&mut self, bi: usize, ii: usize, inst: &ir::Inst, held: Vec<Var>, merge: Block) {
        let r = self.generic(bi, ii, inst, held);
        self.b.ins().jump(merge, &[BlockArg::Value(r)]);
    }

    fn bool_word(&mut self, c: Cv) -> Cv {
        let t = self.iconst(self.w.t);
        let f = self.iconst(self.w.f);
        self.b.ins().select(c, t, f)
    }

    /// Whether both words are fixnums.
    fn both_fix(&mut self, a: Cv, b: Cv) -> Cv {
        let both = self.b.ins().bor(a, b);
        let tag = self.b.ins().band_imm_u(both, 1);
        self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0)
    }

    /// Jumps to `merge` with `w` if it is a fixnum word in range, and to
    /// `slow` otherwise.
    fn fix_result(&mut self, w: Cv, merge: Block, slow: Block) {
        let up = self.b.ins().ishl_imm_u(w, 1);
        let back = self.b.ins().sshr_imm_s(up, 1);
        let ok = self.b.ins().icmp(IntCC::Equal, back, w);
        self.b
            .ins()
            .brif(ok, merge, &[BlockArg::Value(w)], slow, &[]);
    }

    /// Calls runtime entry point `k` with `pre`, then the operands `uses`
    /// (borrowed), then `post`; releases operands at their last use; and
    /// if `fallible`, leaves through the error exit on `ERR`.
    fn borrowing(
        &mut self,
        k: usize,
        pre: &[Cv],
        uses: &[Use],
        post: &[Cv],
        held: Vec<Var>,
        fallible: bool,
    ) -> Cv {
        let mut args = pre.to_vec();
        let vals: Vec<Cv> = uses.iter().map(|&u| self.get(u)).collect();
        args.extend(&vals);
        args.extend(post);
        let r = self.call_rt_word(k, &args);
        for (u, v) in uses.iter().zip(vals) {
            if u.last {
                self.release(v);
            }
        }
        if fallible {
            self.check(r, held);
        }
        r
    }

    /// The address of the heap object of pointer word `v`, after checking
    /// it is one with tag `tag` (otherwise branching to `slow`).
    fn heap_ptr(&mut self, v: Cv, tag: u8, slow: Block) -> Cv {
        let low = self.b.ins().band_imm_u(v, 3);
        let is_ptr = self.b.ins().icmp_imm_s(IntCC::Equal, low, 1);
        let ok = self.b.create_block();
        self.b.ins().brif(is_ptr, ok, &[], slow, &[]);
        self.b.switch_to_block(ok);
        let p = self.b.ins().band_imm_s(v, !7);
        let t = self.b.ins().uload8(I64, MemFlagsData::trusted(), p, 0);
        let right = self.b.ins().icmp_imm_s(IntCC::Equal, t, i64::from(tag));
        let ok = self.b.create_block();
        self.b.ins().brif(right, ok, &[], slow, &[]);
        self.b.switch_to_block(ok);
        p
    }

    /// Item `i` of `v` if it is a tuple of two or three items, read inline
    /// (a new reference); otherwise branches to `slow`.
    fn tuple_item(&mut self, v: Cv, i: usize, slow: Block) -> Cv {
        use crate::value::layout::{TUP_THREE, TUP_TWO, TUPLE};
        use crate::value::word::{PAIR_LOW, PAIR_X_SHIFT, PAIR_Y_UP};
        let got = self.b.create_block();
        let w = self.b.append_block_param(got, I64);
        // A packed pair: its ints are in the word itself.
        let heap = self.b.create_block();
        if i < 2 {
            let low = self.b.ins().band_imm_u(v, 0xff);
            let is_pair = self.b.ins().icmp_imm_s(IntCC::Equal, low, PAIR_LOW as i64);
            let unpack = self.b.create_block();
            self.b.ins().brif(is_pair, unpack, &[], heap, &[]);
            self.b.switch_to_block(unpack);
            let n = if i == 0 {
                self.b.ins().sshr_imm_s(v, i64::from(PAIR_X_SHIFT))
            } else {
                let up = self.b.ins().ishl_imm_u(v, i64::from(PAIR_Y_UP));
                self.b.ins().sshr_imm_s(up, i64::from(PAIR_X_SHIFT))
            };
            let x = self.b.ins().ishl_imm_u(n, 1);
            self.b.ins().jump(got, &[BlockArg::Value(x)]);
        } else {
            self.b.ins().jump(heap, &[]);
        }
        self.b.switch_to_block(heap);
        let p = self.heap_ptr(v, TUPLE, slow);
        let tt = self
            .b
            .ins()
            .uload8(I64, MemFlagsData::trusted(), p, self.off.tup);
        let three = self.b.create_block();
        if i < 2 {
            let two = self.b.create_block();
            let is_two = self
                .b
                .ins()
                .icmp_imm_s(IntCC::Equal, tt, i64::from(TUP_TWO));
            self.b.ins().brif(is_two, two, &[], three, &[]);
            self.b.switch_to_block(two);
            let x = self
                .b
                .ins()
                .load(I64, MemFlagsData::trusted(), p, self.off.two + 8 * i as i32);
            self.b.ins().jump(got, &[BlockArg::Value(x)]);
        } else {
            self.b.ins().jump(three, &[]);
        }
        self.b.switch_to_block(three);
        let is_three = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, tt, i64::from(TUP_THREE));
        let load3 = self.b.create_block();
        let other = self.b.create_block();
        if i < 3 {
            self.b.ins().brif(is_three, load3, &[], other, &[]);
        } else {
            self.b.ins().jump(other, &[]);
        }
        // A longer tuple: its items are behind a boxed slice.
        self.b.switch_to_block(other);
        let is_other =
            self.b
                .ins()
                .icmp_imm_s(IntCC::Equal, tt, i64::from(crate::value::layout::TUP_OTHER));
        let check_len = self.b.create_block();
        self.b.ins().brif(is_other, check_len, &[], slow, &[]);
        self.b.switch_to_block(check_len);
        let len = self
            .b
            .ins()
            .load(I64, MemFlagsData::trusted(), p, self.off.other_len);
        let inside = self
            .b
            .ins()
            .icmp_imm_u(IntCC::UnsignedGreaterThan, len, i as i64);
        let load_other = self.b.create_block();
        self.b.ins().brif(inside, load_other, &[], slow, &[]);
        self.b.switch_to_block(load_other);
        let data = self
            .b
            .ins()
            .load(I64, MemFlagsData::trusted(), p, self.off.other_ptr);
        let x = self
            .b
            .ins()
            .load(I64, MemFlagsData::trusted(), data, 8 * i as i32);
        self.b.ins().jump(got, &[BlockArg::Value(x)]);
        if i >= 3 {
            self.b.switch_to_block(load3);
            self.b.ins().jump(slow, &[]);
            self.b.switch_to_block(got);
            self.dup(w);
            return w;
        }
        self.b.switch_to_block(load3);
        let x = self.b.ins().load(
            I64,
            MemFlagsData::trusted(),
            p,
            self.off.three + 8 * i as i32,
        );
        self.b.ins().jump(got, &[BlockArg::Value(x)]);
        self.b.switch_to_block(got);
        self.dup(w);
        w
    }

    /// An item read inline from a small tuple, or by the runtime entry
    /// `k` (`cx, site, v, i`) otherwise.
    #[allow(clippy::too_many_arguments)]
    fn item_inline(
        &mut self,
        k: usize,
        u: Use,
        i: usize,
        bi: usize,
        ii: usize,
        inst: &ir::Inst,
        held: Vec<Var>,
    ) -> Cv {
        let v = self.get(u);
        let merge = self.b.create_block();
        let r = self.b.append_block_param(merge, I64);
        let slow = self.b.create_block();
        let w = self.tuple_item(v, i, slow);
        self.b.ins().jump(merge, &[BlockArg::Value(w)]);
        self.b.switch_to_block(slow);
        let (cx, site) = (self.cx(), self.site_for(bi, ii, inst));
        let iv = self.iconst(i as u64);
        let x = self.call_rt_word(k, &[cx, site, v, iv]);
        self.b.ins().jump(merge, &[BlockArg::Value(x)]);
        self.b.switch_to_block(merge);
        if u.last {
            self.release(v);
        }
        self.check(r, held);
        r
    }

    /// Library function `pre[2]` on `args` (handed over), in the runtime.
    fn builtin_call(&mut self, pre: &[Cv], args: &[Use], held: Vec<Var>) -> Cv {
        let vals: Vec<Cv> = args.iter().map(|&u| self.owned(u)).collect();
        let addr = self.argv(&vals);
        let n = self.iconst(args.len() as u64);
        let r = self.call_rt_word(BUILTIN, &[pre[0], pre[1], pre[2], addr, n]);
        self.check(r, held);
        r
    }

    fn site_for(&mut self, bi: usize, ii: usize, inst: &ir::Inst) -> Cv {
        self.new_site(bi, ii as u32, 0, inst.span)
    }

    /// `force rec.k`: the cell's value, or its thunk run here.
    fn force(
        &mut self,
        bi: usize,
        ii: usize,
        inst: &ir::Inst,
        u: Use,
        k: u32,
        held: Vec<Var>,
    ) -> Cv {
        let site = self.site_for(bi, ii, inst);
        let rec = self.get(u);
        let cx = self.cx();
        let kv = self.iconst(u64::from(k));
        let r = self.call_rt_word(FORCE, &[cx, site, rec, kv]);
        let merge = self.b.create_block();
        let out = self.b.append_block_param(merge, I64);
        let miss = self.b.create_block();
        let is_miss = self.b.ins().icmp_imm_s(IntCC::Equal, r, MISS as i64);
        self.b
            .ins()
            .brif(is_miss, miss, &[], merge, &[BlockArg::Value(r)]);
        self.b.switch_to_block(miss);
        self.b.set_cold_block(miss);
        let code = self.call_rt_word(THUNK_CODE, &[cx, rec, kv]);
        self.dup(rec);
        let sig = self.b.import_signature(tail_sig(0));
        let z = self.iconst(0);
        let call = self.b.ins().call_indirect(sig, code, &[cx, rec, site, z]);
        let v = self.b.inst_results(call)[0];
        let stored = self.call_rt_word(FORCED, &[cx, rec, kv, v]);
        self.b.ins().jump(merge, &[BlockArg::Value(stored)]);
        self.b.switch_to_block(merge);
        if u.last {
            self.release(rec);
        }
        self.check(out, held);
        out
    }

    fn inst(&mut self, bi: usize, ii: usize, inst: &ir::Inst, held: Vec<Var>) -> Cv {
        match &inst.op {
            Op::Cap(0) if super::record_clo(self.f) => {
                // Given the record itself, or a closure holding it.
                use crate::value::layout::{INTERNAL, INTERNAL_REC};
                let clo = self.clo();
                let merge = self.b.create_block();
                let r = self.b.append_block_param(merge, I64);
                let slow = self.b.create_block();
                let p = self.heap_ptr(clo, INTERNAL, slow);
                let t = self
                    .b
                    .ins()
                    .uload8(I64, MemFlagsData::trusted(), p, self.off.internal);
                let is_rec = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, t, i64::from(INTERNAL_REC));
                let yes = self.b.create_block();
                self.b.ins().brif(is_rec, yes, &[], slow, &[]);
                self.b.switch_to_block(yes);
                self.dup(clo);
                self.b.ins().jump(merge, &[BlockArg::Value(clo)]);
                self.b.switch_to_block(slow);
                let zero = self.iconst(0);
                let x = self.call_rt_word(CAP, &[clo, zero]);
                self.b.ins().jump(merge, &[BlockArg::Value(x)]);
                self.b.switch_to_block(merge);
                r
            }
            Op::Cap(i) => {
                let clo = self.clo();
                let i = self.iconst(u64::from(*i));
                self.call_rt_word(CAP, &[clo, i])
            }
            Op::RecVal(u, i) => {
                let i = self.iconst(u64::from(*i));
                self.borrowing(RECVAL, &[], &[*u], &[i], held, false)
            }
            Op::RecParent(u) => self.borrowing(RECPARENT, &[], &[*u], &[], held, false),
            Op::Force(u, k) => self.force(bi, ii, inst, *u, *k, held),
            Op::Item(u, i) if *i < 64 => self.item_inline(ITEM, *u, *i, bi, ii, inst, held),
            Op::IndexInt(u, i @ 0..64) => {
                self.item_inline(INDEX_INT, *u, *i as usize, bi, ii, inst, held)
            }
            Op::Item(u, i) => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let i = self.iconst(*i as u64);
                self.borrowing(ITEM, &pre, &[*u], &[i], held, true)
            }
            Op::IsTuple(u, n @ (2 | 3)) => {
                // Tuples of two or three items are always stored inline
                // (`Tup`), so the inline tag says it all.
                use crate::value::layout::{TUP_THREE, TUP_TWO, TUPLE};
                let v = self.get(*u);
                let merge = self.b.create_block();
                let r = self.b.append_block_param(merge, I64);
                let no = self.b.create_block();
                if *n == 2 {
                    // A packed pair (see `value::word`) is a 2-tuple.
                    let low = self.b.ins().band_imm_u(v, 0xff);
                    let is_pair = self.b.ins().icmp_imm_s(
                        IntCC::Equal,
                        low,
                        crate::value::word::PAIR_LOW as i64,
                    );
                    let heap = self.b.create_block();
                    let t = self.iconst(self.w.t);
                    self.b
                        .ins()
                        .brif(is_pair, merge, &[BlockArg::Value(t)], heap, &[]);
                    self.b.switch_to_block(heap);
                }
                let p = self.heap_ptr(v, TUPLE, no);
                let tt = self
                    .b
                    .ins()
                    .uload8(I64, MemFlagsData::trusted(), p, self.off.tup);
                let want = if *n == 2 { TUP_TWO } else { TUP_THREE };
                let yes = self.b.ins().icmp_imm_s(IntCC::Equal, tt, i64::from(want));
                let w = self.bool_word(yes);
                self.b.ins().jump(merge, &[BlockArg::Value(w)]);
                self.b.switch_to_block(no);
                let f = self.iconst(self.w.f);
                self.b.ins().jump(merge, &[BlockArg::Value(f)]);
                self.b.switch_to_block(merge);
                if u.last {
                    self.release(v);
                }
                r
            }
            Op::IsTuple(u, n) if *n > 3 => {
                // A longer tuple keeps its items behind a boxed slice; its
                // length says it all.
                use crate::value::layout::{TUP_OTHER, TUPLE};
                let v = self.get(*u);
                let merge = self.b.create_block();
                let r = self.b.append_block_param(merge, I64);
                let no = self.b.create_block();
                let p = self.heap_ptr(v, TUPLE, no);
                let tt = self
                    .b
                    .ins()
                    .uload8(I64, MemFlagsData::trusted(), p, self.off.tup);
                let is_other = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, tt, i64::from(TUP_OTHER));
                let len_b = self.b.create_block();
                self.b.ins().brif(is_other, len_b, &[], no, &[]);
                self.b.switch_to_block(len_b);
                let len = self
                    .b
                    .ins()
                    .load(I64, MemFlagsData::trusted(), p, self.off.other_len);
                let yes = self.b.ins().icmp_imm_s(IntCC::Equal, len, *n as i64);
                let w = self.bool_word(yes);
                self.b.ins().jump(merge, &[BlockArg::Value(w)]);
                self.b.switch_to_block(no);
                let f = self.iconst(self.w.f);
                self.b.ins().jump(merge, &[BlockArg::Value(f)]);
                self.b.switch_to_block(merge);
                if u.last {
                    self.release(v);
                }
                r
            }
            Op::IsTuple(u, n) => {
                let n = self.iconst(*n as u64);
                self.borrowing(IS_TUPLE, &[], &[*u], &[n], held, false)
            }
            Op::IsList { v, n, at_least } => {
                let n = self.iconst(*n as u64);
                let a = self.iconst(u64::from(*at_least));
                self.borrowing(IS_LIST, &[], &[*v], &[n, a], held, false)
            }
            Op::IsType(u, ty) => {
                let t = rt::TYPES.iter().position(|x| x == ty).unwrap_or(0);
                let t = self.iconst(t as u64);
                self.borrowing(IS_TYPE, &[], &[*u], &[t], held, false)
            }
            Op::Index(a, i) => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                self.borrowing(INDEX, &pre, &[*a, *i], &[], held, true)
            }
            Op::IndexInt(a, i) => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let i = self.iconst(*i as u64);
                self.borrowing(INDEX_INT, &pre, &[*a], &[i], held, true)
            }
            Op::Cmp(op @ (CmpOp::In | CmpOp::NotIn), a, b) => {
                let o = rt::CMPS.iter().position(|x| x == op).unwrap_or(0);
                let pre = [
                    self.cx(),
                    self.site_for(bi, ii, inst),
                    self.iconst(o as u64),
                ];
                self.borrowing(CMP, &pre, &[*a, *b], &[], held, true)
            }
            Op::Tuple(xs) => {
                let vals: Vec<Cv> = xs.iter().map(|&u| self.owned(u)).collect();
                let addr = self.argv(&vals);
                let n = self.iconst(xs.len() as u64);
                self.call_rt_word(TUPLE, &[addr, n])
            }
            Op::Global(i) => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let i = self.iconst(*i as u64);
                let r = self.call_rt_word(GLOBAL, &[pre[0], pre[1], i]);
                self.check(r, held);
                r
            }
            Op::NewRecord {
                layout,
                parent,
                vals,
            } => {
                let mut ws = Vec::new();
                if let Some(p) = parent {
                    ws.push(self.owned(*p));
                }
                for &u in vals {
                    ws.push(self.owned(u));
                }
                let addr = self.argv(&ws);
                let cx = self.cx();
                let l = self.iconst(*layout as u64);
                let p = self.iconst(u64::from(parent.is_some()));
                let n = self.iconst(vals.len() as u64);
                self.call_rt_word(RECORD, &[cx, l, p, addr, n])
            }
            Op::Call { f, args, named } if named.is_empty() => {
                let fv = self.owned(*f);
                let vals: Vec<Cv> = args.iter().map(|&u| self.owned(u)).collect();
                let site = self.new_site(bi, ii as u32, 1 + vals.len(), inst.span);
                let cx = self.cx();
                let n = self.iconst(vals.len() as u64);
                let ptr = self.call_rt_word(TAIL_TARGET, &[cx, fv, n]);
                let merge = self.b.create_block();
                let r = self.b.append_block_param(merge, I64);
                let (fast, slow) = (self.b.create_block(), self.b.create_block());
                self.b.ins().brif(ptr, fast, &[], slow, &[]);
                self.b.switch_to_block(fast);
                let sig = self.b.import_signature(tail_sig(vals.len() as u32));
                let z = self.iconst(0);
                let mut call = vec![cx, fv, site, z];
                call.extend(&vals);
                let c = self.b.ins().call_indirect(sig, ptr, &call);
                let v = self.b.inst_results(c)[0];
                self.b.ins().jump(merge, &[BlockArg::Value(v)]);
                self.b.switch_to_block(slow);
                let mut all = vec![fv];
                all.extend(&vals);
                let addr = self.argv(&all);
                let v = self.call_rt_word(CALL_VALUE, &[cx, site, addr]);
                self.b.ins().jump(merge, &[BlockArg::Value(v)]);
                self.b.switch_to_block(merge);
                self.check(r, held);
                r
            }
            Op::Loopable { coll, f } => {
                self.borrowing(LOOPABLE, &[], &[*coll, *f], &[], held, false)
            }
            Op::Middle { v, before, after } => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let post = [self.iconst(*before as u64), self.iconst(*after as u64)];
                self.borrowing(MIDDLE, &pre, &[*v], &post, held, true)
            }
            Op::List(xs) => {
                let vals: Vec<Cv> = xs.iter().map(|&u| self.owned(u)).collect();
                let addr = self.argv(&vals);
                let n = self.iconst(xs.len() as u64);
                self.call_rt_word(LIST, &[addr, n])
            }
            Op::NewAcc(k, hint) => {
                let k = self.iconst(match k {
                    ir::AccKind::List => 0,
                    ir::AccKind::Set => 1,
                    ir::AccKind::Map => 2,
                });
                match hint {
                    Some(u) => self.borrowing(ACC, &[k], &[*u], &[], held, false),
                    None => {
                        let none = self.iconst(self.w.none);
                        self.call_rt_word(ACC, &[k, none])
                    }
                }
            }
            Op::AccPush(a, x) => {
                let (a, x) = (self.owned(*a), self.owned(*x));
                let (cx, site) = (self.cx(), self.site_for(bi, ii, inst));
                let r = self.call_rt_word(PUSH, &[cx, site, a, x]);
                self.check(r, held);
                r
            }
            Op::AccDone(a) => {
                let a = self.owned(*a);
                self.call_rt_word(DONE_ACC, &[a])
            }
            Op::Iter { coll, finite } if inst.dst.is_some_and(|d| self.iters.contains_key(&d)) => {
                let (mode_v, cur_v, end_v) = self.iters[&inst.dst.unwrap_or_default()];
                let ss = *self.iter_slot.get_or_insert_with(|| {
                    self.b.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        24,
                        3,
                    ))
                });
                let addr = self.b.ins().stack_addr(I64, ss, 0);
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let f = self.iconst(u64::from(*finite));
                let mode = self.borrowing(ITER_INIT, &pre, &[*coll], &[f, addr], held, true);
                let w = self.b.ins().stack_load(I64, I64, ss, 0);
                let cur = self.b.ins().stack_load(I64, I64, ss, 8);
                let end = self.b.ins().stack_load(I64, I64, ss, 16);
                self.b.def_var(mode_v, mode);
                self.b.def_var(cur_v, cur);
                self.b.def_var(end_v, end);
                w
            }
            Op::Iter { coll, finite } => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let f = self.iconst(u64::from(*finite));
                self.borrowing(ITER, &pre, &[*coll], &[f], held, true)
            }
            Op::CallBuiltin { id, args, named } if named.is_empty() => {
                let name = crate::builtins::BUILTINS[*id].name;
                let pre = [
                    self.cx(),
                    self.site_for(bi, ii, inst),
                    self.iconst(*id as u64),
                ];
                match (name, args.len()) {
                    ("len", 1) => self.borrowing(LEN, &pre, args, &[], held, true),
                    ("get", 2) => {
                        let none = self.iconst(self.w.none);
                        self.borrowing(GET, &pre, args, &[none], held, true)
                    }
                    ("get", 3) => self.borrowing(GET, &pre, args, &[], held, true),
                    ("set", 3) => {
                        let vals: Vec<Cv> = args.iter().map(|&u| self.owned(u)).collect();
                        let mut all = pre.to_vec();
                        all.extend(vals);
                        let r = self.call_rt_word(SET, &all);
                        self.check(r, held);
                        r
                    }
                    ("min" | "max", 2) => {
                        // Two small ints inline: `a` unless `b` is strictly
                        // better, as the library function does.
                        let (a, b) = (self.get(args[0]), self.get(args[1]));
                        let merge = self.b.create_block();
                        let r = self.b.append_block_param(merge, I64);
                        let (fast, slow) = (self.b.create_block(), self.b.create_block());
                        let both = self.both_fix(a, b);
                        self.b.ins().brif(both, fast, &[], slow, &[]);
                        self.b.switch_to_block(fast);
                        let cc = if name == "min" {
                            IntCC::SignedLessThan
                        } else {
                            IntCC::SignedGreaterThan
                        };
                        let better = self.b.ins().icmp(cc, b, a);
                        let w = self.b.ins().select(better, b, a);
                        self.b.ins().jump(merge, &[BlockArg::Value(w)]);
                        self.b.switch_to_block(slow);
                        let v = self.builtin_call(&pre, args, held);
                        self.b.ins().jump(merge, &[BlockArg::Value(v)]);
                        self.b.switch_to_block(merge);
                        r
                    }
                    ("abs", 1) => {
                        let a = self.get(args[0]);
                        let merge = self.b.create_block();
                        let r = self.b.append_block_param(merge, I64);
                        let (fast, slow) = (self.b.create_block(), self.b.create_block());
                        let tag = self.b.ins().band_imm_u(a, 1);
                        let fix = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0);
                        self.b.ins().brif(fix, fast, &[], slow, &[]);
                        self.b.switch_to_block(fast);
                        let neg = self.b.ins().ineg(a);
                        let below = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, a, 0);
                        let w = self.b.ins().select(below, neg, a);
                        // `abs` of the least fixnum leaves the fixnums.
                        self.fix_result(w, merge, slow);
                        self.b.switch_to_block(slow);
                        let v = self.builtin_call(&pre, args, held);
                        self.b.ins().jump(merge, &[BlockArg::Value(v)]);
                        self.b.switch_to_block(merge);
                        r
                    }
                    _ => self.builtin_call(&pre, args, held),
                }
            }
            Op::Slice {
                coll,
                lo,
                hi,
                inclusive,
            } => {
                let pre = [self.cx(), self.site_for(bi, ii, inst)];
                let mut uses = vec![*coll];
                uses.extend(lo.iter().chain(hi.iter()).copied());
                let flags = u64::from(lo.is_some())
                    | u64::from(hi.is_some()) << 1
                    | u64::from(*inclusive) << 2;
                let none = self.iconst(self.w.none);
                // The bounds that are absent are `none` (never read).
                let vals: Vec<Cv> = uses.iter().map(|&u| self.get(u)).collect();
                let lo_v = if lo.is_some() { vals[1] } else { none };
                let hi_v = if hi.is_some() {
                    vals[vals.len() - 1]
                } else {
                    none
                };
                let f = self.iconst(flags);
                let r = self.call_rt_word(SLICE, &[pre[0], pre[1], vals[0], lo_v, hi_v, f]);
                for (u, v) in uses.iter().zip(vals) {
                    if u.last {
                        self.release(v);
                    }
                }
                self.check(r, held);
                r
            }
            Op::MakeClosure { func, caps } => {
                let vals: Vec<Cv> = caps.iter().map(|&u| self.owned(u)).collect();
                let addr = self.argv(&vals);
                let cx = self.cx();
                let fid = self.iconst(u64::from(*func));
                let n = self.iconst(caps.len() as u64);
                self.call_rt_word(CLOSURE, &[cx, fid, n, addr])
            }
            Op::Const(v) => {
                let bits = v.raw_bits();
                let c = self.iconst(bits);
                if bits & 3 == 1 {
                    // The pool keeps the value alive for as long as the code.
                    self.pool.push(v.clone());
                    self.dup(c);
                }
                c
            }
            Op::CallKnown { func, caps, args } => {
                let clo = self.closure(*func, caps);
                let mut call = vec![self.cx(), clo];
                call.push(self.new_site(bi, ii as u32, 0, inst.span));
                call.push(self.iconst(0));
                for &a in args {
                    let v = self.owned(a);
                    call.push(v);
                }
                let target = self.target(*func);
                let c = self.b.ins().call(target, &call);
                let r = self.b.inst_results(c)[0];
                self.check(r, held);
                r
            }
            Op::Bin(op, ua, ub) => match op {
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::Xor
                | BinOp::Shl
                | BinOp::Shr => self.arith(*op, *ua, *ub, bi, ii, inst, held),
                BinOp::Dot(_) => self.generic(bi, ii, inst, held),
                _ => self.bin_slow(*op, *ua, *ub, bi, ii, inst, held),
            },
            Op::Cmp(op, ua, ub) => match op {
                CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                    let (a, b) = (self.get(*ua), self.get(*ub));
                    let merge = self.b.create_block();
                    let r = self.b.append_block_param(merge, I64);
                    let (fast, slow) = (self.b.create_block(), self.b.create_block());
                    let both = self.both_fix(a, b);
                    self.b.ins().brif(both, fast, &[], slow, &[]);
                    self.b.switch_to_block(fast);
                    let cc = match op {
                        CmpOp::Lt => IntCC::SignedLessThan,
                        CmpOp::Le => IntCC::SignedLessThanOrEqual,
                        CmpOp::Gt => IntCC::SignedGreaterThan,
                        _ => IntCC::SignedGreaterThanOrEqual,
                    };
                    let c = self.b.ins().icmp(cc, a, b);
                    let w = self.bool_word(c);
                    self.b.ins().jump(merge, &[BlockArg::Value(w)]);
                    self.b.switch_to_block(slow);
                    self.cmp_slow(*op, *ua, *ub, bi, ii, inst, held, merge);
                    self.b.switch_to_block(merge);
                    r
                }
                CmpOp::Eq | CmpOp::Ne => {
                    let (a, b) = (self.get(*ua), self.get(*ub));
                    let merge = self.b.create_block();
                    let r = self.b.append_block_param(merge, I64);
                    let (same, differ, decided, slow) = (
                        self.b.create_block(),
                        self.b.create_block(),
                        self.b.create_block(),
                        self.b.create_block(),
                    );
                    let decided_r = self.b.append_block_param(decided, I64);
                    let eq = self.b.ins().icmp(IntCC::Equal, a, b);
                    self.b.ins().brif(eq, same, &[], differ, &[]);
                    // Equal words are equal values.
                    self.b.switch_to_block(same);
                    let yes = self.iconst(if *op == CmpOp::Eq { self.w.t } else { self.w.f });
                    self.b.ins().jump(decided, &[BlockArg::Value(yes)]);
                    // Different words differ unless both are heap objects.
                    self.b.switch_to_block(differ);
                    let both = self.b.ins().band(a, b);
                    let tag = self.b.ins().band_imm_u(both, 3);
                    let ptrs = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 1);
                    let no = self.iconst(if *op == CmpOp::Eq { self.w.f } else { self.w.t });
                    self.b
                        .ins()
                        .brif(ptrs, slow, &[], decided, &[BlockArg::Value(no)]);
                    self.b.switch_to_block(decided);
                    if ua.last {
                        self.release(a);
                    }
                    if ub.last {
                        self.release(b);
                    }
                    self.b.ins().jump(merge, &[BlockArg::Value(decided_r)]);
                    self.b.switch_to_block(slow);
                    self.cmp_slow(*op, *ua, *ub, bi, ii, inst, held, merge);
                    self.b.switch_to_block(merge);
                    r
                }
                _ => self.generic(bi, ii, inst, held),
            },
            Op::CheckBool(u, _) | Op::CheckPred(u, _) | Op::CheckElem(u, _) | Op::Not(u) => {
                let v = self.get(*u);
                let merge = self.b.create_block();
                let r = self.b.append_block_param(merge, I64);
                let (fast, slow) = (self.b.create_block(), self.b.create_block());
                let is_t = self.b.ins().icmp_imm_s(IntCC::Equal, v, self.w.t as i64);
                let is_f = self.b.ins().icmp_imm_s(IntCC::Equal, v, self.w.f as i64);
                let is_bool = self.b.ins().bor(is_t, is_f);
                self.b.ins().brif(is_bool, fast, &[], slow, &[]);
                self.b.switch_to_block(fast);
                let out = if matches!(inst.op, Op::Not(_)) {
                    self.b.ins().bxor_imm_u(v, (self.w.t ^ self.w.f) as i64)
                } else {
                    v
                };
                self.b.ins().jump(merge, &[BlockArg::Value(out)]);
                self.b.switch_to_block(slow);
                self.slow(bi, ii, inst, held, merge);
                self.b.switch_to_block(merge);
                r
            }
            Op::EqConst(u, c) if c.raw_bits() & 3 != 1 => {
                let v = self.get(*u);
                let e = self
                    .b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, v, c.raw_bits() as i64);
                let r = self.bool_word(e);
                if u.last {
                    self.release(v);
                }
                r
            }
            _ => self.generic(bi, ii, inst, held),
        }
    }

    /// Fixnum arithmetic inline, with the runtime for anything else
    /// (including overflow out of the fixnums).
    #[allow(clippy::too_many_arguments)]
    fn arith(
        &mut self,
        op: BinOp,
        ua: Use,
        ub: Use,
        bi: usize,
        ii: usize,
        inst: &ir::Inst,
        held: Vec<Var>,
    ) -> Cv {
        let (a, b) = (self.get(ua), self.get(ub));
        let merge = self.b.create_block();
        let r = self.b.append_block_param(merge, I64);
        let (fast, slow) = (self.b.create_block(), self.b.create_block());
        let both = self.both_fix(a, b);
        self.b.ins().brif(both, fast, &[], slow, &[]);
        self.b.switch_to_block(fast);
        match op {
            // Fixnum words are `n << 1`, and in 62 bits, so sums can't
            // overflow 64 bits; the range check catches leaving 62.
            BinOp::Add => {
                let w = self.b.ins().iadd(a, b);
                self.fix_result(w, merge, slow);
            }
            BinOp::Sub => {
                let w = self.b.ins().isub(a, b);
                self.fix_result(w, merge, slow);
            }
            BinOp::Mul => {
                let x = self.b.ins().sshr_imm_s(a, 1);
                let (w, of) = self.b.ins().smul_overflow(x, b);
                let ok = self.b.create_block();
                self.b.ins().brif(of, slow, &[], ok, &[]);
                self.b.switch_to_block(ok);
                self.fix_result(w, merge, slow);
            }
            BinOp::Div | BinOp::Mod => {
                let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, b, 0);
                let go = self.b.create_block();
                self.b.ins().brif(nz, go, &[], slow, &[]);
                self.b.switch_to_block(go);
                let x = self.b.ins().sshr_imm_s(a, 1);
                let y = self.b.ins().sshr_imm_s(b, 1);
                // `x` is at least -2^61, so `x / y` can't overflow.
                let q = self.b.ins().sdiv(x, y);
                let m = self.b.ins().srem(x, y);
                // Floor division (SPEC 5.1): adjust when the remainder is
                // nonzero and its sign differs from the divisor's.
                let m_nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, m, 0);
                let sx = self.b.ins().bxor(m, y);
                let diff = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, sx, 0);
                let adj = self.b.ins().band(m_nz, diff);
                if op == BinOp::Div {
                    let one = self.b.ins().uextend(I64, adj);
                    let qa = self.b.ins().isub(q, one);
                    let w = self.b.ins().ishl_imm_u(qa, 1);
                    self.fix_result(w, merge, slow);
                } else {
                    let my = self.b.ins().iadd(m, y);
                    let ma = self.b.ins().select(adj, my, m);
                    let w = self.b.ins().ishl_imm_u(ma, 1);
                    self.b.ins().jump(merge, &[BlockArg::Value(w)]);
                }
            }
            BinOp::Shl | BinOp::Shr => {
                // Shift counts 0..62 inline (the word of 62 is 124).
                let small = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, b, 124);
                let go = self.b.create_block();
                self.b.ins().brif(small, go, &[], slow, &[]);
                self.b.switch_to_block(go);
                let n = self.b.ins().sshr_imm_s(b, 1);
                if op == BinOp::Shl {
                    let w = self.b.ins().ishl(a, n);
                    let back = self.b.ins().sshr(w, n);
                    let same = self.b.ins().icmp(IntCC::Equal, back, a);
                    let ok = self.b.create_block();
                    self.b.ins().brif(same, ok, &[], slow, &[]);
                    self.b.switch_to_block(ok);
                    self.fix_result(w, merge, slow);
                } else {
                    let w = self.b.ins().sshr(a, n);
                    let w = self.b.ins().band_imm_s(w, -2);
                    self.b.ins().jump(merge, &[BlockArg::Value(w)]);
                }
            }
            BinOp::BitAnd => {
                let w = self.b.ins().band(a, b);
                self.b.ins().jump(merge, &[BlockArg::Value(w)]);
            }
            BinOp::BitOr => {
                let w = self.b.ins().bor(a, b);
                self.b.ins().jump(merge, &[BlockArg::Value(w)]);
            }
            _ => {
                let w = self.b.ins().bxor(a, b);
                self.b.ins().jump(merge, &[BlockArg::Value(w)]);
            }
        }
        self.b.switch_to_block(slow);
        let v = self.bin_slow(op, ua, ub, bi, ii, inst, held);
        self.b.ins().jump(merge, &[BlockArg::Value(v)]);
        self.b.switch_to_block(merge);
        r
    }

    /// `a op b` in the runtime.
    #[allow(clippy::too_many_arguments)]
    fn bin_slow(
        &mut self,
        op: BinOp,
        ua: Use,
        ub: Use,
        bi: usize,
        ii: usize,
        inst: &ir::Inst,
        held: Vec<Var>,
    ) -> Cv {
        let o = rt::BINS.iter().position(|x| *x == op).unwrap_or(0);
        let (a, b) = (self.owned(ua), self.owned(ub));
        let (cx, site) = (self.cx(), self.site_for(bi, ii, inst));
        let o = self.iconst(o as u64);
        let r = self.call_rt_word(BIN, &[cx, site, o, a, b]);
        self.check(r, held);
        r
    }

    /// The closure word for a call of `func` with these captures: `none`
    /// when there are none.
    fn closure(&mut self, func: u32, caps: &[Use]) -> Cv {
        if caps.is_empty() {
            return self.iconst(self.w.none);
        }
        if caps.len() == 1 && super::record_clo(&self.m.funcs[func as usize]) {
            return self.owned(caps[0]);
        }
        let vals: Vec<Cv> = caps.iter().map(|&u| self.owned(u)).collect();
        let addr = self.argv(&vals);
        let cx = self.cx();
        let fid = self.iconst(u64::from(func));
        let n = self.iconst(caps.len() as u64);
        self.call_rt_word(CLOSURE, &[cx, fid, n, addr])
    }

    /// Releases what's held at the end of a block that leaves the function.
    fn release_all(&mut self, held: &Bits) {
        for v in held.iter() {
            self.release_var(v);
        }
        let clo = self.clo();
        self.release(clo);
    }

    /// The block to branch to for the edge into `s`: one releasing what
    /// `held` has that `s` doesn't need, or `s` itself.
    fn edge(&mut self, held: &Bits, s: u32) -> Block {
        let drops: Vec<Var> = held
            .iter()
            .filter(|&v| !self.live_in[s as usize].has(v))
            .collect();
        if drops.is_empty() {
            return self.blocks[s as usize];
        }
        let e = self.b.create_block();
        self.edges.push((e, drops, self.blocks[s as usize]));
        e
    }

    fn term(&mut self, bi: usize, before_term: Bits) {
        let f = self.f;
        let term = &f.blocks[bi].term;
        let mut held = before_term;
        term.uses(&mut |u| {
            if u.last {
                held.clear(u.var);
            }
        });
        match term {
            Term::Jump(t, args) => {
                let vals: Vec<Cv> = args.iter().map(|&u| self.owned(u)).collect();
                for v in held.iter() {
                    if !self.live_in[*t as usize].has(v) {
                        self.release_var(v);
                    }
                }
                for (&p, &v) in f.blocks[*t as usize].params.iter().zip(&vals) {
                    self.b.def_var(self.var(p), v);
                }
                self.b.ins().jump(self.blocks[*t as usize], &[]);
            }
            Term::If { cond, then, els } => {
                let c = self.get(*cond);
                let is_t = self.b.ins().icmp_imm_s(IntCC::Equal, c, self.w.t as i64);
                let t = self.edge(&held, *then);
                let e = self.edge(&held, *els);
                self.b.ins().brif(is_t, t, &[], e, &[]);
            }
            Term::Return(u) => {
                let v = self.owned(*u);
                self.release_all(&held);
                self.b.ins().return_(&[v]);
            }
            Term::TailCall { callee, args, span } => match callee {
                Callee::Known { func, caps } => {
                    let clo = self.closure(*func, caps);
                    let vals: Vec<Cv> = args.iter().map(|&u| self.owned(u)).collect();
                    let t = self.new_site(bi, TERM, 0, *span);
                    let (site, chain) = self.tail_words(t);
                    self.release_all(&held);
                    let mut call = vec![self.cx(), clo, site, chain];
                    call.extend(vals);
                    let target = self.target(*func);
                    self.b.ins().return_call(target, &call);
                }
                Callee::Value(u) => {
                    let fv = self.owned(*u);
                    let vals: Vec<Cv> = args.iter().map(|&u| self.owned(u)).collect();
                    let site = self.new_site(bi, TERM, 1 + vals.len(), *span);
                    let cx = self.cx();
                    let n = self.iconst(vals.len() as u64);
                    let ptr = self.call_rt_word(TAIL_TARGET, &[cx, fv, n]);
                    let (fast, slow) = (self.b.create_block(), self.b.create_block());
                    self.b.ins().brif(ptr, fast, &[], slow, &[]);
                    self.b.switch_to_block(fast);
                    let (tsite, chain) = self.tail_words(site);
                    self.release_all(&held);
                    let sig = self.b.import_signature(tail_sig(vals.len() as u32));
                    let mut call = vec![cx, fv, tsite, chain];
                    call.extend(&vals);
                    self.b.ins().return_call_indirect(sig, ptr, &call);
                    self.b.switch_to_block(slow);
                    let mut all = vec![fv];
                    all.extend(&vals);
                    let addr = self.argv(&all);
                    let r = self.call_rt_word(CALL_VALUE, &[cx, site, addr]);
                    self.check(r, held.iter().collect());
                    self.release_all(&held);
                    self.b.ins().return_(&[r]);
                }
            },
            Term::Next {
                iter,
                some,
                none,
                span,
            } if self.iters.contains_key(&iter.var) => {
                let (mode_v, cur_v, end_v) = self.iters[&iter.var];
                let site = self.new_site(bi, TERM, 0, *span);
                let it = self.get(*iter);
                let mode = self.b.use_var(mode_v);
                let got = self.b.create_block();
                let item = self.b.append_block_param(got, I64);
                let empty = self.b.create_block();
                let (range_b, other, seq_b, gen_b) = (
                    self.b.create_block(),
                    self.b.create_block(),
                    self.b.create_block(),
                    self.b.create_block(),
                );
                let is_range = self.b.ins().icmp_imm_s(IntCC::Equal, mode, 1);
                self.b.ins().brif(is_range, range_b, &[], other, &[]);
                self.b.switch_to_block(other);
                let is_seq = self.b.ins().icmp_imm_s(IntCC::Equal, mode, 2);
                self.b.ins().brif(is_seq, seq_b, &[], gen_b, &[]);
                // A range of small ints: a counter, `cur` up to `end`.
                self.b.switch_to_block(range_b);
                let c = self.b.use_var(cur_v);
                let e = self.b.use_var(end_v);
                let past = self.b.ins().icmp(IntCC::SignedGreaterThan, c, e);
                let step = self.b.create_block();
                self.b.ins().brif(past, empty, &[], step, &[]);
                self.b.switch_to_block(step);
                let next = self.b.ins().iadd_imm_s(c, 1);
                self.b.def_var(cur_v, next);
                let x = self.b.ins().ishl_imm_u(c, 1);
                self.b.ins().jump(got, &[BlockArg::Value(x)]);
                // A list or tuple: items by index.
                self.b.switch_to_block(seq_b);
                let i = self.b.use_var(cur_v);
                let n = self.b.use_var(end_v);
                let past = self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, i, n);
                let step = self.b.create_block();
                self.b.ins().brif(past, empty, &[], step, &[]);
                self.b.switch_to_block(step);
                let next = self.b.ins().iadd_imm_s(i, 1);
                self.b.def_var(cur_v, next);
                let x = self.call_rt_word(SEQ_GET, &[it, i]);
                self.b.ins().jump(got, &[BlockArg::Value(x)]);
                // Anything else: the runtime's iterator.
                self.b.switch_to_block(gen_b);
                let cx = self.cx();
                let r = self.call_rt_word(NEXT, &[cx, site, it]);
                let mut held_err: Vec<Var> = held.iter().collect();
                if iter.last {
                    held_err.push(iter.var);
                }
                self.check(r, held_err);
                let done = self.b.ins().icmp_imm_s(IntCC::Equal, r, DONE as i64);
                self.b
                    .ins()
                    .brif(done, empty, &[], got, &[BlockArg::Value(r)]);
                // No more items.
                self.b.switch_to_block(empty);
                if iter.last {
                    self.release(it);
                }
                let none_b = self.edge(&held, *none);
                self.b.ins().jump(none_b, &[]);
                // The next item.
                self.b.switch_to_block(got);
                if iter.last {
                    self.release(it);
                }
                for v in held.iter() {
                    if !self.live_in[*some as usize].has(v) {
                        self.release_var(v);
                    }
                }
                let p = f.blocks[*some as usize].params[0];
                self.b.def_var(self.var(p), item);
                self.b.ins().jump(self.blocks[*some as usize], &[]);
            }
            Term::Next {
                iter,
                some,
                none,
                span,
            } => {
                let site = self.new_site(bi, TERM, 0, *span);
                let it = self.get(*iter);
                let cx = self.cx();
                let r = self.call_rt_word(NEXT, &[cx, site, it]);
                if iter.last {
                    self.release(it);
                }
                self.check(r, held.iter().collect());
                let done = self.b.ins().icmp_imm_s(IntCC::Equal, r, DONE as i64);
                let none_b = self.edge(&held, *none);
                let some_b = self.b.create_block();
                self.b.ins().brif(done, none_b, &[], some_b, &[]);
                self.b.switch_to_block(some_b);
                for v in held.iter() {
                    if !self.live_in[*some as usize].has(v) {
                        self.release_var(v);
                    }
                }
                let p = f.blocks[*some as usize].params[0];
                self.b.def_var(self.var(p), r);
                self.b.ins().jump(self.blocks[*some as usize], &[]);
            }
            Term::Fail { vals, span, .. } => {
                let vs: Vec<Cv> = vals.iter().map(|&u| self.owned(u)).collect();
                let site = self.new_site(bi, TERM, vs.len(), *span);
                let addr = self.argv(&vs);
                let cx = self.cx();
                self.call_rt_word(FAIL, &[cx, site, addr]);
                let eb = self.err_block(held.iter().collect());
                self.b.ins().jump(eb, &[]);
            }
            Term::Unset => {
                let site = self.new_site(bi, TERM, 0, f.span);
                let addr = self.iconst(0);
                let cx = self.cx();
                self.call_rt_word(FAIL, &[cx, site, addr]);
                let eb = self.err_block(held.iter().collect());
                self.b.ins().jump(eb, &[]);
            }
        }
        let _ = ENTRY;
    }
}
