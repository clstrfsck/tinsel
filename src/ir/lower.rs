//! Lowering from the resolved AST to the IR (see the module docs in
//! `ir/mod.rs` for the shape of the result).
//!
//! The lowering mirrors the resolver's frames: a `Local { depth, slot }`
//! names slot `slot` of the frame `depth` levels up. Each slot is bound to
//! a [`Bind`], owned by one of the functions being built. A function that
//! reads a slot it doesn't own captures it (lambdas and generator levels),
//! or, for `where` definitions, reads it through its record.

use super::*;
use crate::ast::*;
use crate::builtins::BUILTINS;
use crate::error::{Error, Result};
use crate::resolve::{GlobalDef as AstGlobal, Program};
use std::collections::HashMap;

/// Named arguments of a call.
type NamedUses = Vec<(Name, Use)>;

/// What a frame slot holds.
#[derive(Clone, Copy)]
enum Bind {
    /// A variable of function `owner` (an index into `Lowerer::fns`).
    Var { owner: usize, var: Var },
    /// A `where` constant: binding `index` of cell `cell` of the scope's
    /// record.
    Cell {
        scope: usize,
        cell: u32,
        index: Option<usize>,
    },
    /// A `where` function.
    WhereFn {
        scope: usize,
        func: FuncId,
        arity: usize,
    },
}

/// Something a function can capture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Key {
    /// A variable bound to frame `.0`, slot `.1`.
    Slot(usize, usize),
    /// The record of a `where` scope.
    Rec(usize),
}

/// A `where` block being lowered.
struct Scope {
    /// The function that makes the record (an index into `fns`).
    owner: usize,
    rec: Option<Var>,
    parent: Option<usize>,
    /// The owner's variables copied into the record.
    vals: Vec<Key>,
}

struct Builder {
    id: FuncId,
    name: Name,
    kind: FnKind,
    params: u32,
    memo: Option<MemoAt>,
    memo_annotated: bool,
    def_id: Option<usize>,
    span: Span,
    blocks: Vec<Block>,
    cur: BlockId,
    nvars: u32,
    caps: Vec<Key>,
    /// For a `where` definition: the scope whose record is capture 0. Its
    /// captures are then fixed, and other outer variables are read through
    /// the record.
    record: Option<usize>,
    /// Tail calls to this function become jumps to the entry block.
    self_call: bool,
}

struct Lowerer<'p> {
    prog: &'p Program,
    funcs: Vec<Option<Function>>,
    layouts: Vec<Layout>,
    /// Function ids of the globals that are functions.
    global_fns: Vec<Option<(FuncId, usize)>>,
    memos: usize,
    fns: Vec<Builder>,
    frames: Vec<HashMap<usize, Bind>>,
    scopes: Vec<Scope>,
}

pub fn lower(prog: &Program) -> Result<Module> {
    let mut l = Lowerer {
        prog,
        funcs: Vec::new(),
        layouts: Vec::new(),
        global_fns: vec![None; prog.globals.len()],
        memos: 0,
        fns: Vec::new(),
        frames: Vec::new(),
        scopes: Vec::new(),
    };
    // Ids first, so calls can name functions defined later.
    let mut globals = Vec::new();
    let mut consts: Vec<Cell> = Vec::new();
    let mut const_cells: Vec<(*const ConstDef, usize)> = Vec::new();
    let mut fn_defs = Vec::new();
    let mut const_defs = Vec::new();
    for (i, g) in prog.globals.iter().enumerate() {
        match g {
            AstGlobal::Func(def) => {
                let id = l.alloc();
                l.global_fns[i] = Some((id, def.arity));
                globals.push(GlobalDef::Func(id));
                fn_defs.push((def.clone(), id));
            }
            AstGlobal::Const { def, slot } => {
                let ptr = Rc::as_ptr(def);
                let cell = match const_cells.iter().find(|(p, _)| *p == ptr) {
                    Some(&(_, c)) => c,
                    None => {
                        let thunk = l.alloc();
                        consts.push(Cell {
                            thunk,
                            describe: def.describe().into(),
                        });
                        const_defs.push((def.clone(), thunk));
                        const_cells.push((ptr, consts.len() - 1));
                        consts.len() - 1
                    }
                };
                let n = def.pat.binding_names().len();
                let index = (n != 1).then_some(*slot);
                globals.push(GlobalDef::Const { cell, index });
            }
        }
    }
    for (def, id) in fn_defs {
        let memo = l.prog.memo[def.id].then(|| {
            l.memos += 1;
            MemoAt::Global(l.memos - 1)
        });
        l.fndef(&def, id, None, memo)?;
    }
    for (def, id) in const_defs {
        l.top_const(&def, id)?;
    }
    let mut checks = HashMap::new();
    let mut all_checks: Vec<&Check> = prog.tests.iter().map(|(c, _)| c).collect();
    for ex in &prog.examples {
        all_checks.extend(&ex.expects);
    }
    if let Some((expects, _)) = &prog.answers {
        all_checks.extend(expects);
    }
    for c in all_checks {
        let id = l.alloc();
        l.check(c, id)?;
        checks.insert(c.span.start, id);
    }
    let mut withs = Vec::new();
    for ex in &prog.examples {
        let mut list = Vec::new();
        for (name, def, _) in &ex.withs {
            let Some(g) = prog.global(name) else { continue };
            let thunk = l.alloc();
            l.top_const(def, thunk)?;
            list.push((
                g,
                Cell {
                    thunk,
                    describe: def.describe().into(),
                },
            ));
        }
        withs.push(list);
    }
    let funcs = l
        .funcs
        .into_iter()
        .map(|f| f.ok_or_else(|| Error::msg("internal: function not lowered")))
        .collect::<Result<Vec<_>>>()?;
    Ok(Module {
        funcs,
        globals,
        consts,
        memos: l.memos,
        layouts: l.layouts,
        checks,
        withs,
    })
}

/// Removes the blocks that can't be reached from the entry, such as the
/// failure branch after an irrefutable pattern.
fn prune(blocks: Vec<Block>) -> Vec<Block> {
    let mut seen = vec![false; blocks.len()];
    let mut work = vec![0];
    seen[0] = true;
    while let Some(b) = work.pop() {
        for s in blocks[b].term.successors() {
            if !seen[s as usize] {
                seen[s as usize] = true;
                work.push(s as usize);
            }
        }
    }
    let mut new_id = vec![0; blocks.len()];
    let mut n = 0;
    for (i, &keep) in seen.iter().enumerate() {
        new_id[i] = n;
        n += u32::from(keep);
    }
    let mut out = Vec::new();
    for (mut block, keep) in blocks.into_iter().zip(seen) {
        if !keep {
            continue;
        }
        match &mut block.term {
            Term::Jump(b, _) => *b = new_id[*b as usize],
            Term::If { then, els, .. } => {
                *then = new_id[*then as usize];
                *els = new_id[*els as usize];
            }
            Term::Next { some, none, .. } => {
                *some = new_id[*some as usize];
                *none = new_id[*none as usize];
            }
            Term::Return(_) | Term::TailCall { .. } | Term::Fail { .. } | Term::Unset => {}
        }
        out.push(block);
    }
    out
}

fn internal(msg: &str, span: Span) -> Error {
    Error::new(format!("internal: {msg}"), span)
}

impl<'p> Lowerer<'p> {
    fn alloc(&mut self) -> FuncId {
        self.funcs.push(None);
        (self.funcs.len() - 1) as FuncId
    }

    // --- Function builders ---------------------------------------------------

    fn begin(&mut self, id: FuncId, name: Name, kind: FnKind, params: u32, span: Span) -> Vec<Var> {
        let mut b = Builder {
            id,
            name,
            kind,
            params,
            memo: None,
            memo_annotated: false,
            def_id: None,
            span,
            blocks: Vec::new(),
            cur: 0,
            nvars: 0,
            caps: Vec::new(),
            record: None,
            self_call: false,
        };
        let ps: Vec<Var> = (0..params).collect();
        b.nvars = params;
        b.blocks.push(Block {
            params: ps.clone(),
            insts: Vec::new(),
            term: Term::Unset,
            drops: Vec::new(),
        });
        self.fns.push(b);
        ps
    }

    /// Finishes the innermost function, returning what it captures.
    fn end(&mut self) -> Result<Vec<Key>> {
        let b = self
            .fns
            .pop()
            .ok_or_else(|| Error::msg("internal: no function"))?;
        if b.blocks.iter().any(|bl| matches!(bl.term, Term::Unset)) {
            return Err(internal(
                &format!("unterminated block in `{}`", b.name),
                b.span,
            ));
        }
        let blocks = prune(b.blocks);
        let mut f = Function {
            name: b.name,
            kind: b.kind,
            params: b.params,
            caps: b.caps.len() as u32,
            memo: b.memo,
            memo_annotated: b.memo_annotated,
            def_id: b.def_id,
            span: b.span,
            blocks,
            nvars: b.nvars,
        };
        super::cse::function(&mut f);
        super::cse::gather_items(&mut f);
        super::simplify::function(&mut f);
        live::function(&mut f);
        self.funcs[b.id as usize] = Some(f);
        Ok(b.caps)
    }

    fn top(&self) -> usize {
        self.fns.len() - 1
    }

    fn b(&mut self) -> &mut Builder {
        self.fns.last_mut().expect("a function is being built")
    }

    fn new_var(&mut self) -> Var {
        let b = self.b();
        b.nvars += 1;
        b.nvars - 1
    }

    fn emit_in(&mut self, fi: usize, op: Op, span: Span) -> Var {
        let b = &mut self.fns[fi];
        let v = b.nvars;
        b.nvars += 1;
        let cur = b.cur as usize;
        b.blocks[cur].insts.push(Inst {
            dst: Some(v),
            op,
            span,
        });
        v
    }

    fn emit(&mut self, op: Op, span: Span) -> Var {
        let fi = self.top();
        self.emit_in(fi, op, span)
    }

    fn block(&mut self, nparams: usize) -> (BlockId, Vec<Var>) {
        let params: Vec<Var> = (0..nparams).map(|_| self.new_var()).collect();
        let b = self.b();
        b.blocks.push(Block {
            params: params.clone(),
            insts: Vec::new(),
            term: Term::Unset,
            drops: Vec::new(),
        });
        ((b.blocks.len() - 1) as BlockId, params)
    }

    fn term(&mut self, t: Term) {
        let b = self.b();
        let cur = b.cur as usize;
        b.blocks[cur].term = t;
    }

    fn switch(&mut self, block: BlockId) {
        self.b().cur = block;
    }

    fn jump(&mut self, to: BlockId, args: &[Var]) {
        self.term(Term::Jump(to, args.iter().map(|&v| Use::new(v)).collect()));
    }

    fn branch(&mut self, cond: Var, then: BlockId, els: BlockId) {
        self.term(Term::If {
            cond: Use::new(cond),
            then,
            els,
        });
    }

    fn fail(&mut self, fail: Fail, vals: &[Var], span: Span) {
        self.term(Term::Fail {
            fail,
            vals: vals.iter().map(|&v| Use::new(v)).collect(),
            span,
        });
    }

    fn konst(&mut self, v: Value, span: Span) -> Var {
        self.emit(Op::Const(v), span)
    }

    fn source(&self, span: Span) -> Rc<str> {
        self.prog
            .src
            .get(span.start..span.end)
            .unwrap_or("?")
            .into()
    }

    // --- Names ---------------------------------------------------------------

    fn bind(&mut self, slot: usize, var: Var) {
        let owner = self.top();
        if let Some(f) = self.frames.last_mut() {
            f.insert(slot, Bind::Var { owner, var });
        }
    }

    /// Who owns a key: the function holding it as a variable.
    fn key_owner(&self, key: Key) -> Option<(usize, Var)> {
        match key {
            Key::Slot(f, s) => match self.frames.get(f)?.get(&s)? {
                Bind::Var { owner, var } => Some((*owner, *var)),
                _ => None,
            },
            Key::Rec(sc) => {
                let s = &self.scopes[sc];
                s.rec.map(|r| (s.owner, r))
            }
        }
    }

    /// The value of `key` in function `fi`, emitting whatever reads it.
    fn get(&mut self, fi: usize, key: Key, span: Span) -> Result<Var> {
        let owner = self.key_owner(key);
        if let Some((o, v)) = owner
            && o == fi
        {
            return Ok(v);
        }
        if let Some(mut sc) = self.fns[fi].record {
            // Walk out through the records.
            let mut r = self.emit_in(fi, Op::Cap(0), span);
            loop {
                match key {
                    Key::Rec(k) if k == sc => return Ok(r),
                    Key::Slot(..) if owner.is_some_and(|(o, _)| o == self.scopes[sc].owner) => {
                        let s = &mut self.scopes[sc];
                        if s.rec.is_some() {
                            return Err(internal("record already made", span));
                        }
                        let i = match s.vals.iter().position(|k| *k == key) {
                            Some(i) => i,
                            None => {
                                s.vals.push(key);
                                s.vals.len() - 1
                            }
                        };
                        return Ok(self.emit_in(fi, Op::RecVal(Use::new(r), i as u32), span));
                    }
                    _ => {}
                }
                sc = self.scopes[sc]
                    .parent
                    .ok_or_else(|| internal(&format!("cannot reach {key:?}"), span))?;
                r = self.emit_in(fi, Op::RecParent(Use::new(r)), span);
            }
        }
        let caps = &mut self.fns[fi].caps;
        let i = match caps.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                caps.push(key);
                caps.len() - 1
            }
        };
        Ok(self.emit_in(fi, Op::Cap(i as u32), span))
    }

    /// Makes a closure of the function just finished, in the current one.
    fn closure(&mut self, func: FuncId, caps: Vec<Key>, span: Span) -> Result<Var> {
        let fi = self.top();
        let mut vars = Vec::new();
        for k in caps {
            vars.push(Use::new(self.get(fi, k, span)?));
        }
        Ok(self.emit(Op::MakeClosure { func, caps: vars }, span))
    }

    fn local(&mut self, depth: usize, slot: usize, span: Span) -> Result<Var> {
        let frame = self
            .frames
            .len()
            .checked_sub(depth + 1)
            .ok_or_else(|| internal("bad frame depth", span))?;
        let bind = *self.frames[frame]
            .get(&slot)
            .ok_or_else(|| internal("unbound local", span))?;
        let fi = self.top();
        match bind {
            Bind::Var { .. } => self.get(fi, Key::Slot(frame, slot), span),
            Bind::Cell { scope, cell, index } => {
                let r = self.get(fi, Key::Rec(scope), span)?;
                let v = self.emit(Op::Force(Use::new(r), cell), span);
                Ok(match index {
                    Some(i) => self.emit(Op::Item(Use::new(v), i), span),
                    None => v,
                })
            }
            Bind::WhereFn { scope, func, .. } => {
                let r = self.get(fi, Key::Rec(scope), span)?;
                Ok(self.emit(
                    Op::MakeClosure {
                        func,
                        caps: vec![Use::new(r)],
                    },
                    span,
                ))
            }
        }
    }

    // --- Definitions ---------------------------------------------------------

    fn fndef(
        &mut self,
        def: &FnDef,
        id: FuncId,
        record: Option<usize>,
        memo: Option<MemoAt>,
    ) -> Result<()> {
        let params = self.begin(
            id,
            def.name.clone(),
            FnKind::Named,
            def.arity as u32,
            def.span,
        );
        {
            let b = self.b();
            b.record = record;
            b.memo = memo;
            b.memo_annotated = def.annotation == Some(Annotation::Memo);
            b.def_id = Some(def.id);
            b.self_call = memo.is_none();
            b.caps = record.map(Key::Rec).into_iter().collect();
        }
        let first = self.block(0).0;
        self.jump(first, &[]);
        self.switch(first);
        for clause in &def.clauses {
            self.frames.push(HashMap::new());
            let (next, _) = self.block(0);
            for (p, &v) in clause.params.iter().zip(&params) {
                self.pattern(p, v, next)?;
            }
            self.where_scope(&clause.wheres, clause.span)?;
            if let Some(g) = &clause.guard {
                let c = self.expr(g)?;
                let c = self.emit(Op::CheckBool(Use::new(c), "a `when` guard"), g.span);
                let (body, _) = self.block(0);
                self.branch(c, body, next);
                self.switch(body);
            }
            self.tail(&clause.body)?;
            self.frames.pop();
            self.switch(next);
        }
        self.fail(Fail::NoClause(def.name.clone()), &params, def.span);
        self.end()?;
        Ok(())
    }

    /// A top-level constant's (or `with` override's) thunk.
    fn top_const(&mut self, def: &ConstDef, id: FuncId) -> Result<()> {
        self.begin(id, def.describe().into(), FnKind::Thunk, 0, def.span);
        self.frames.push(HashMap::new());
        self.where_scope(&def.wheres, def.span)?;
        let v = self.expr(&def.body)?;
        self.const_result(def, v)?;
        self.frames.pop();
        self.end()?;
        Ok(())
    }

    /// Matches a constant's value against its pattern, and returns the
    /// value bound (or a tuple of them, for several).
    fn const_result(&mut self, def: &ConstDef, v: Var) -> Result<()> {
        let (fail, _) = self.block(0);
        let mut slots = Vec::new();
        def.pat.bindings(&mut |b| slots.push(b.slot));
        let saved: Vec<Option<Bind>> = slots
            .iter()
            .map(|s| self.frames.last().and_then(|f| f.get(s).copied()))
            .collect();
        self.pattern(&def.pat, v, fail)?;
        let mut vars = Vec::new();
        for &s in &slots {
            match self.frames.last().and_then(|f| f.get(&s)) {
                Some(Bind::Var { var, .. }) => vars.push(Use::new(*var)),
                _ => return Err(internal("constant binding", def.span)),
            }
        }
        let result = if vars.len() == 1 {
            vars[0].var
        } else {
            self.emit(Op::Tuple(vars), def.span)
        };
        self.term(Term::Return(Use::new(result)));
        self.switch(fail);
        self.fail(
            Fail::ConstPattern(self.source(def.pat.span)),
            &[v],
            def.pat.span,
        );
        // Others see the bindings as the cell, not this thunk's variables.
        for (s, old) in slots.into_iter().zip(saved) {
            if let Some(f) = self.frames.last_mut() {
                match old {
                    Some(b) => f.insert(s, b),
                    None => f.remove(&s),
                };
            }
        }
        Ok(())
    }

    fn check(&mut self, c: &Check, id: FuncId) -> Result<()> {
        self.begin(id, c.name.clone(), FnKind::Check, 0, c.span);
        self.frames.push(HashMap::new());
        let v = self.expr(&c.expr)?;
        self.term(Term::Return(Use::new(v)));
        self.frames.pop();
        self.end()?;
        Ok(())
    }

    /// Declares a `where` block's definitions in the current frame, lowers
    /// them, and makes the record.
    fn where_scope(&mut self, w: &Wheres, span: Span) -> Result<()> {
        if w.consts.is_empty() && w.fns.is_empty() {
            return Ok(());
        }
        // A constant's own `where` block shares the frame (as in the
        // evaluator's `init_wheres`), so it joins this record.
        let mut consts: Vec<Rc<ConstDef>> = Vec::new();
        let mut fns: Vec<(Rc<FnDef>, usize)> = Vec::new();
        fn collect(w: &Wheres, consts: &mut Vec<Rc<ConstDef>>, fns: &mut Vec<(Rc<FnDef>, usize)>) {
            for c in &w.consts {
                consts.push(c.clone());
                collect(&c.wheres, consts, fns);
            }
            fns.extend(w.fns.iter().cloned());
        }
        collect(w, &mut consts, &mut fns);

        let owner = self.top();
        let parent = self.fns[owner].record;
        let layout = self.layouts.len();
        self.layouts.push(Layout::default());
        let scope = self.scopes.len();
        self.scopes.push(Scope {
            owner,
            rec: None,
            parent,
            vals: Vec::new(),
        });
        let mut cells = Vec::new();
        for (k, c) in consts.iter().enumerate() {
            let thunk = self.alloc();
            cells.push(Cell {
                thunk,
                describe: c.describe().into(),
            });
            let mut slots = Vec::new();
            c.pat.bindings(&mut |b| slots.push(b.slot));
            let many = slots.len() != 1;
            for (j, s) in slots.into_iter().enumerate() {
                let bind = Bind::Cell {
                    scope,
                    cell: k as u32,
                    index: many.then_some(j),
                };
                if let Some(f) = self.frames.last_mut() {
                    f.insert(s, bind);
                }
            }
        }
        let mut memos = 0;
        let mut fn_ids = Vec::new();
        for (f, slot) in &fns {
            let func = self.alloc();
            let memo = self.prog.memo[f.id].then(|| {
                memos += 1;
                MemoAt::Record(memos - 1)
            });
            fn_ids.push((func, memo));
            let bind = Bind::WhereFn {
                scope,
                func,
                arity: f.arity,
            };
            if let Some(fr) = self.frames.last_mut() {
                fr.insert(*slot, bind);
            }
        }
        for (c, cell) in consts.iter().zip(&cells) {
            self.begin(cell.thunk, cell.describe.clone(), FnKind::Thunk, 0, c.span);
            let b = self.b();
            b.record = Some(scope);
            b.caps = vec![Key::Rec(scope)];
            let v = self.expr(&c.body)?;
            self.const_result(c, v)?;
            self.end()?;
        }
        for ((f, _), (func, memo)) in fns.iter().zip(fn_ids) {
            self.fndef(f, func, Some(scope), memo)?;
        }
        self.layouts[layout] = Layout {
            vals: self.scopes[scope].vals.len(),
            cells,
            memos,
        };
        let parent = match parent {
            Some(p) => Some(Use::new(self.get(owner, Key::Rec(p), span)?)),
            None => None,
        };
        let mut vals = Vec::new();
        for k in self.scopes[scope].vals.clone() {
            vals.push(Use::new(self.get(owner, k, span)?));
        }
        let rec = self.emit(
            Op::NewRecord {
                layout,
                parent,
                vals,
            },
            span,
        );
        self.scopes[scope].rec = Some(rec);
        Ok(())
    }

    // --- Patterns ------------------------------------------------------------

    /// Tests `v` against `p`, jumping to `fail` if it doesn't match, and
    /// binds the pattern's names in the current frame.
    fn pattern(&mut self, p: &Pattern, v: Var, fail: BlockId) -> Result<()> {
        let span = p.span;
        let u = Use::new(v);
        match &p.kind {
            PatKind::Wild => {}
            PatKind::Bind(b) => self.bind(b.slot, v),
            PatKind::Typed(b, ty) => {
                let t = self.emit(Op::IsType(u, *ty), span);
                self.test(t, fail);
                if let Some(b) = b {
                    self.bind(b.slot, v);
                }
            }
            PatKind::Lit(l) => {
                let t = self.emit(Op::EqConst(u, l.clone()), span);
                self.test(t, fail);
            }
            PatKind::Tuple(ps) => {
                let t = self.emit(Op::IsTuple(u, ps.len()), span);
                self.test(t, fail);
                for (i, q) in ps.iter().enumerate() {
                    let x = self.emit(Op::Item(u, i), q.span);
                    self.pattern(q, x, fail)?;
                }
            }
            PatKind::List {
                before,
                rest,
                after,
            } => {
                let t = self.emit(
                    Op::IsList {
                        v: u,
                        n: before.len() + after.len(),
                        at_least: rest.is_some(),
                    },
                    span,
                );
                self.test(t, fail);
                for (i, q) in before.iter().enumerate() {
                    let x = self.emit(Op::Item(u, i), q.span);
                    self.pattern(q, x, fail)?;
                }
                for (j, q) in after.iter().enumerate() {
                    let x = self.emit(Op::ItemBack(u, after.len() - 1 - j), q.span);
                    self.pattern(q, x, fail)?;
                }
                if let Some(Some(b)) = rest {
                    let x = self.emit(
                        Op::Middle {
                            v: u,
                            before: before.len(),
                            after: after.len(),
                        },
                        span,
                    );
                    self.bind(b.slot, x);
                }
            }
            PatKind::Str(segs) => {
                let m = self.emit(Op::MatchStr(u, segs.clone().into()), span);
                let none = self.emit(Op::EqConst(Use::new(m), Value::NONE), span);
                let (ok, _) = self.block(0);
                self.branch(none, fail, ok);
                self.switch(ok);
                let mut k = 0;
                for s in segs {
                    if let StrSeg::Cap { bind: Some(b), .. } = s {
                        let x = self.emit(Op::Item(Use::new(m), k), span);
                        self.bind(b.slot, x);
                        k += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// Continues if `t` is true, otherwise jumps to `fail`.
    fn test(&mut self, t: Var, fail: BlockId) {
        let (ok, _) = self.block(0);
        self.branch(t, ok, fail);
        self.switch(ok);
    }

    // --- Expressions ---------------------------------------------------------

    /// Lowers `e` in tail position: ends the current block.
    fn tail(&mut self, e: &Expr) -> Result<()> {
        match &e.kind {
            ExprKind::Call { func, args, .. } => self.call(func, args, e.span, true).map(|_| ()),
            ExprKind::If(c, a, b) => {
                let (then, els) = self.cond(c, "`if`")?;
                self.switch(then);
                self.tail(a)?;
                self.switch(els);
                self.tail(b)
            }
            ExprKind::Match(s, arms) => self.matcher(s, arms, e.span, None),
            _ => {
                let v = self.expr(e)?;
                self.term(Term::Return(Use::new(v)));
                Ok(())
            }
        }
    }

    /// Evaluates a condition, giving the blocks for true and false.
    fn cond(&mut self, c: &Expr, what: &'static str) -> Result<(BlockId, BlockId)> {
        let v = self.expr(c)?;
        let v = self.emit(Op::CheckBool(Use::new(v), what), c.span);
        let (then, _) = self.block(0);
        let (els, _) = self.block(0);
        self.branch(v, then, els);
        Ok((then, els))
    }

    /// A join point: a block taking one parameter.
    fn join(&mut self) -> (BlockId, Var) {
        let (b, ps) = self.block(1);
        (b, ps[0])
    }

    fn exprs(&mut self, xs: &[Expr]) -> Result<Vec<Use>> {
        let mut out = Vec::new();
        for x in xs {
            out.push(Use::new(self.expr(x)?));
        }
        Ok(out)
    }

    fn expr(&mut self, e: &Expr) -> Result<Var> {
        let span = e.span;
        Ok(match &e.kind {
            ExprKind::Lit(v) => self.konst(v.clone(), span),
            ExprKind::Interp(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    out.push(match p {
                        InterpPart::Lit(s) => FmtPart::Lit(s.clone()),
                        InterpPart::Expr(x) => FmtPart::Val(Use::new(self.expr(x)?)),
                    });
                }
                self.emit(Op::Format(out), span)
            }
            ExprKind::Name(n) => return Err(internal(&format!("unresolved name `{n}`"), span)),
            ExprKind::Local { depth, slot, .. } => self.local(*depth, *slot, span)?,
            ExprKind::Global(i) => self.emit(Op::Global(*i), span),
            ExprKind::Builtin(id) => self.emit(Op::BuiltinValue(*id), span),
            ExprKind::Tuple(xs) => {
                let xs = self.exprs(xs)?;
                self.emit(Op::Tuple(xs), span)
            }
            ExprKind::List(xs) => {
                let xs = self.exprs(xs)?;
                self.emit(Op::List(xs), span)
            }
            ExprKind::Set(xs) => {
                let xs = self.exprs(xs)?;
                self.emit(Op::Set(xs), span)
            }
            ExprKind::Map(kvs) => {
                let mut out = Vec::new();
                for (k, v) in kvs {
                    let k = Use::new(self.expr(k)?);
                    let v = Use::new(self.expr(v)?);
                    out.push((k, v));
                }
                self.emit(Op::Map(out), span)
            }
            ExprKind::Comp(comp) => self.comprehension(comp, span)?,
            ExprKind::Call { func, args, .. } => {
                return self
                    .call(func, args, span, false)
                    .and_then(|v| v.ok_or_else(|| internal("call", span)));
            }
            ExprKind::TupleIndex(x, i) => {
                let x = self.expr(x)?;
                self.emit(Op::IndexInt(Use::new(x), *i as i64), span)
            }
            ExprKind::Index(x, idx) => {
                let x = Use::new(self.expr(x)?);
                if let ExprKind::Range { lo, hi, inclusive } = &idx.kind {
                    let lo = match lo {
                        Some(lo) => Some(Use::new(self.expr(lo)?)),
                        None => None,
                    };
                    let hi = match hi {
                        Some(hi) => Some(Use::new(self.expr(hi)?)),
                        None => None,
                    };
                    self.emit(
                        Op::Slice {
                            coll: x,
                            lo,
                            hi,
                            inclusive: *inclusive,
                        },
                        span,
                    )
                } else {
                    let i = Use::new(self.expr(idx)?);
                    self.emit(Op::Index(x, i), span)
                }
            }
            ExprKind::Unary(op, x) => {
                let x = Use::new(self.expr(x)?);
                let op = match op {
                    UnOp::Neg => Op::Neg(x),
                    UnOp::Not => Op::Not(x),
                    UnOp::BitNot => Op::BitNot(x),
                };
                self.emit(op, span)
            }
            ExprKind::Binary(op, a, b) => {
                let a = Use::new(self.expr(a)?);
                let b = Use::new(self.expr(b)?);
                self.emit(Op::Bin(*op, a, b), span)
            }
            ExprKind::Compare(first, rest) => self.compare(first, rest, span)?,
            ExprKind::And(a, b) | ExprKind::Or(a, b) => {
                let and = matches!(e.kind, ExprKind::And(..));
                let what = if and { "`and`" } else { "`or`" };
                let (then, els) = self.cond(a, what)?;
                let (join, r) = self.join();
                let (rhs, short) = if and { (then, els) } else { (els, then) };
                self.switch(short);
                let k = self.konst(Value::bool(!and), span);
                self.jump(join, &[k]);
                self.switch(rhs);
                let v = self.expr(b)?;
                let v = self.emit(Op::CheckBool(Use::new(v), what), b.span);
                self.jump(join, &[v]);
                self.switch(join);
                r
            }
            ExprKind::Coalesce(a, b) => {
                let va = self.expr(a)?;
                let t = self.emit(Op::EqConst(Use::new(va), Value::NONE), span);
                let (rhs, _) = self.block(0);
                let (keep, _) = self.block(0);
                let (join, r) = self.join();
                self.branch(t, rhs, keep);
                self.switch(keep);
                self.jump(join, &[va]);
                self.switch(rhs);
                let vb = self.expr(b)?;
                self.jump(join, &[vb]);
                self.switch(join);
                r
            }
            ExprKind::If(c, a, b) => {
                let (then, els) = self.cond(c, "`if`")?;
                let (join, r) = self.join();
                self.switch(then);
                let va = self.expr(a)?;
                self.jump(join, &[va]);
                self.switch(els);
                let vb = self.expr(b)?;
                self.jump(join, &[vb]);
                self.switch(join);
                r
            }
            ExprKind::Lambda(lam) => self.lambda(lam)?,
            ExprKind::Match(s, arms) => {
                let (join, r) = self.join();
                self.matcher(s, arms, span, Some(join))?;
                self.switch(join);
                r
            }
            ExprKind::Range { lo, hi, inclusive } => {
                let Some(lo) = lo else {
                    // An error when evaluated, as in the evaluator.
                    let (after, _) = self.block(0);
                    let (bad, _) = self.block(0);
                    let t = self.konst(Value::bool(true), span);
                    self.branch(t, bad, after);
                    self.switch(bad);
                    self.fail(
                        Fail::Msg(
                            "a range needs a start (open-start ranges are only for slicing)".into(),
                        ),
                        &[],
                        span,
                    );
                    self.switch(after);
                    return Ok(self.konst(Value::NONE, span));
                };
                let lo = Use::new(self.expr(lo)?);
                let hi = match hi {
                    Some(h) => Some(Use::new(self.expr(h)?)),
                    None => None,
                };
                self.emit(
                    Op::Range {
                        lo,
                        hi,
                        inclusive: *inclusive,
                    },
                    span,
                )
            }
        })
    }

    fn compare(&mut self, first: &Expr, rest: &[(CmpOp, Expr)], span: Span) -> Result<Var> {
        let mut left = self.expr(first)?;
        if let [(op, x)] = rest {
            let r = self.expr(x)?;
            return Ok(self.emit(Op::Cmp(*op, Use::new(left), Use::new(r)), span));
        }
        let (join, result) = self.join();
        for (i, (op, x)) in rest.iter().enumerate() {
            let r = self.expr(x)?;
            let c = self.emit(Op::Cmp(*op, Use::new(left), Use::new(r)), span);
            if i + 1 == rest.len() {
                self.jump(join, &[c]);
            } else {
                let (next, _) = self.block(0);
                let (no, _) = self.block(0);
                self.branch(c, next, no);
                self.switch(no);
                let f = self.konst(Value::bool(false), span);
                self.jump(join, &[f]);
                self.switch(next);
            }
            left = r;
        }
        self.switch(join);
        Ok(result)
    }

    /// A `match`. Each arm's body jumps to `join`, or with no join is in
    /// tail position.
    fn matcher(&mut self, s: &Expr, arms: &[Arm], span: Span, join: Option<BlockId>) -> Result<()> {
        let v = self.expr(s)?;
        for arm in arms {
            let (next, _) = self.block(0);
            self.pattern(&arm.pat, v, next)?;
            if let Some(g) = &arm.guard {
                let c = self.expr(g)?;
                let c = self.emit(Op::CheckBool(Use::new(c), "a `when` guard"), g.span);
                let (body, _) = self.block(0);
                self.branch(c, body, next);
                self.switch(body);
            }
            match join {
                Some(j) => {
                    let r = self.expr(&arm.body)?;
                    self.jump(j, &[r]);
                }
                None => self.tail(&arm.body)?,
            }
            self.switch(next);
        }
        self.fail(Fail::NoArm, &[v], span);
        Ok(())
    }

    fn lambda(&mut self, lam: &Lambda) -> Result<Var> {
        let id = self.alloc();
        let n = lam.params.len();
        let params = self.begin(id, "<lambda>".into(), FnKind::Lambda, n as u32, lam.span);
        self.frames.push(HashMap::new());
        for (p, &v) in lam.params.iter().zip(&params) {
            if matches!(p.kind, PatKind::Bind(_) | PatKind::Wild) {
                self.pattern(p, v, 0)?;
                continue;
            }
            let (fail, _) = self.block(0);
            self.pattern(p, v, fail)?;
            let cur = self.b().cur;
            self.switch(fail);
            self.fail(Fail::LambdaArg(self.source(p.span)), &[v], p.span);
            self.switch(cur);
        }
        self.tail(&lam.body)?;
        self.frames.pop();
        let caps = self.end()?;
        self.closure(id, caps, lam.span)
    }

    /// A call. In tail position it ends the block and gives `None`.
    fn call(&mut self, func: &Expr, args: &[Arg], span: Span, tail: bool) -> Result<Option<Var>> {
        let has_named = args.iter().any(|a| a.name.is_some());
        let lower_args = |l: &mut Self| -> Result<(Vec<Use>, NamedUses)> {
            let mut pos = Vec::new();
            let mut named = Vec::new();
            for a in args {
                let v = Use::new(l.expr(&a.value)?);
                match &a.name {
                    Some(n) => named.push((n.clone(), v)),
                    None => pos.push(v),
                }
            }
            Ok((pos, named))
        };
        let ret = |l: &mut Self, v: Var| -> Option<Var> {
            if tail {
                l.term(Term::Return(Use::new(v)));
                None
            } else {
                Some(v)
            }
        };
        // A library function named directly.
        if let ExprKind::Builtin(id) = func.kind
            && !BUILTINS[id].constant
        {
            if let [arg] = args
                && arg.name.is_none()
                && let ExprKind::Comp(comp) = &arg.value.kind
                && comp.kind == CompKind::Gen
                && let Some(v) = self.fused_generator(id, comp, arg.value.span, span)?
            {
                return Ok(ret(self, v));
            }
            let (args, named) = lower_args(self)?;
            if named.is_empty()
                && let Some(kind) = Loop::of(BUILTINS[id].name)
                && args.len() == kind.arity()
            {
                let vars: Vec<Var> = args.iter().map(|u| u.var).collect();
                let v = match kind {
                    Loop::MinBy | Loop::MaxBy => self.extreme_loop(kind, id, &vars, span),
                    Loop::Fold | Loop::Scan => self.fold_loop(kind, id, &vars, span),
                    _ => self.builtin_loop(kind, id, vars[0], vars[1], span),
                };
                return Ok(ret(self, v));
            }
            let v = self.emit(Op::CallBuiltin { id, args, named }, span);
            return Ok(ret(self, v));
        }
        // A known function with the right number of arguments.
        let known = if has_named {
            None
        } else {
            match &func.kind {
                ExprKind::Global(i) => match self.global_fns[*i] {
                    Some((f, arity)) if arity == args.len() => Some((f, Vec::new())),
                    _ => None,
                },
                ExprKind::Local { depth, slot, .. } => {
                    let frame = self.frames.len().checked_sub(depth + 1);
                    match frame.and_then(|f| self.frames[f].get(slot)).copied() {
                        Some(Bind::WhereFn { scope, func, arity }) if arity == args.len() => {
                            let fi = self.top();
                            if tail && self.b().self_call && self.b().id == func {
                                Some((func, Vec::new()))
                            } else {
                                let r = self.get(fi, Key::Rec(scope), span)?;
                                Some((func, vec![Use::new(r)]))
                            }
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        if let Some((f, caps)) = known {
            let (args, _) = lower_args(self)?;
            if tail {
                if self.b().self_call && self.b().id == f {
                    let vars: Vec<Var> = args.iter().map(|u| u.var).collect();
                    let fi = self.top();
                    let b = &mut self.fns[fi];
                    let cur = b.cur as usize;
                    b.blocks[cur].insts.push(Inst {
                        dst: None,
                        op: Op::SelfTail,
                        span,
                    });
                    self.jump(0, &vars);
                } else {
                    self.term(Term::TailCall {
                        callee: Callee::Known { func: f, caps },
                        args,
                        span,
                    });
                }
                return Ok(None);
            }
            let v = self.emit(
                Op::CallKnown {
                    func: f,
                    caps,
                    args,
                },
                span,
            );
            return Ok(Some(v));
        }
        let f = Use::new(self.expr(func)?);
        let (args, named) = lower_args(self)?;
        if tail && named.is_empty() {
            self.term(Term::TailCall {
                callee: Callee::Value(f),
                args,
                span,
            });
            return Ok(None);
        }
        let v = self.emit(Op::Call { f, args, named }, span);
        Ok(ret(self, v))
    }

    /// The check that a loop can stand in for library call `id` on `args`
    /// (collection first, function last): branches to the returned `fast`
    /// block, or makes the call itself and jumps to `done`.
    fn loop_or_call(&mut self, id: usize, args: &[Var], span: Span) -> (BlockId, BlockId, Var) {
        let (coll, f) = (args[0], args[args.len() - 1]);
        let ok = self.emit(
            Op::Loopable {
                coll: Use::new(coll),
                f: Use::new(f),
            },
            span,
        );
        let (fast, _) = self.block(0);
        let (slow, _) = self.block(0);
        let (done, dp) = self.block(1);
        self.branch(ok, fast, slow);
        self.switch(slow);
        let r = self.emit(
            Op::CallBuiltin {
                id,
                args: args.iter().map(|&v| Use::new(v)).collect(),
                named: Vec::new(),
            },
            span,
        );
        self.jump(done, &[r]);
        self.switch(fast);
        (fast, done, dp[0])
    }

    /// `min_by(f)` and `max_by(f)`: the first element with the least (or
    /// greatest) key, comparing as `<` does; `none` for no elements.
    fn extreme_loop(&mut self, kind: Loop, id: usize, args: &[Var], span: Span) -> Var {
        let (coll, f) = (args[0], args[1]);
        let (_, done, result) = self.loop_or_call(id, args, span);
        let it = self.emit(
            Op::Iter {
                coll: Use::new(coll),
                finite: true,
            },
            span,
        );
        let (first, fp) = self.block(1);
        let (empty, _) = self.block(0);
        self.term(Term::Next {
            iter: Use::new(it),
            some: first,
            none: empty,
            span,
        });
        self.switch(empty);
        let none = self.konst(Value::NONE, span);
        self.jump(done, &[none]);
        let (head, hp) = self.block(2);
        self.switch(first);
        let k = self.call_one(f, &[fp[0]], span);
        self.jump(head, &[k, fp[0]]);
        let (body, bp) = self.block(1);
        let (exit, _) = self.block(0);
        self.switch(head);
        self.term(Term::Next {
            iter: Use::new(it),
            some: body,
            none: exit,
            span,
        });
        self.switch(exit);
        self.jump(done, &[hp[1]]);
        self.switch(body);
        let k = self.call_one(f, &[bp[0]], span);
        let op = if kind == Loop::MinBy {
            CmpOp::Lt
        } else {
            CmpOp::Gt
        };
        let better = self.emit(Op::Cmp(op, Use::new(k), Use::new(hp[0])), span);
        let (yes, _) = self.block(0);
        let (no, _) = self.block(0);
        self.branch(better, yes, no);
        self.switch(yes);
        self.jump(head, &[k, bp[0]]);
        self.switch(no);
        self.jump(head, &[hp[0], hp[1]]);
        self.switch(done);
        result
    }

    /// `fold(init, f)` and `scan(init, f)`: the accumulator is handed to
    /// `f` each time (moved, so it can be updated in place); `scan` also
    /// lists every value it takes.
    fn fold_loop(&mut self, kind: Loop, id: usize, args: &[Var], span: Span) -> Var {
        let (coll, init, f) = (args[0], args[1], args[2]);
        let (_, done, result) = self.loop_or_call(id, args, span);
        let it = self.emit(
            Op::Iter {
                coll: Use::new(coll),
                finite: true,
            },
            span,
        );
        let scan = kind == Loop::Scan;
        let mut state = vec![init];
        if scan {
            let out = self.emit(Op::NewAcc(AccKind::List, Some(Use::new(coll))), span);
            let out = self.emit(Op::AccPush(Use::new(out), Use::new(init)), span);
            state.push(out);
        }
        let (head, hp) = self.block(state.len());
        self.jump(head, &state);
        let (body, bp) = self.block(1);
        let (exit, _) = self.block(0);
        self.switch(head);
        self.term(Term::Next {
            iter: Use::new(it),
            some: body,
            none: exit,
            span,
        });
        self.switch(exit);
        let r = if scan {
            self.emit(Op::AccDone(Use::new(hp[1])), span)
        } else {
            hp[0]
        };
        self.jump(done, &[r]);
        self.switch(body);
        let acc = self.call_one(f, &[hp[0], bp[0]], span);
        if scan {
            let out = self.emit(Op::AccPush(Use::new(hp[1]), Use::new(acc)), span);
            self.jump(head, &[acc, out]);
        } else {
            self.jump(head, &[acc]);
        }
        self.switch(done);
        result
    }

    /// A call of the function value `f`.
    fn call_one(&mut self, f: Var, args: &[Var], span: Span) -> Var {
        self.emit(
            Op::Call {
                f: Use::new(f),
                args: args.iter().map(|&v| Use::new(v)).collect(),
                named: Vec::new(),
            },
            span,
        )
    }

    /// `coll.filter(f)` and the like as a loop, when `coll` is an ordinary
    /// collection and `f` a function (`Op::Loopable`); otherwise the
    /// library call. The loop does what the library function does, element
    /// by element, with the same checks and messages, so only the speed
    /// differs: calling `f` from here needs no trip through Rust.
    fn builtin_loop(&mut self, kind: Loop, id: usize, coll: Var, f: Var, span: Span) -> Var {
        let (_, done, out) = self.loop_or_call(id, &[coll, f], span);
        let it = self.emit(
            Op::Iter {
                coll: Use::new(coll),
                finite: true,
            },
            span,
        );
        let state = match kind {
            Loop::Map | Loop::Filter => {
                vec![self.emit(Op::NewAcc(AccKind::List, Some(Use::new(coll))), span)]
            }
            Loop::Count => vec![self.konst(Value::int(0), span)],
            // The others take their own loops (`extreme_loop`, `fold_loop`).
            _ => Vec::new(),
        };
        let (head, hp) = self.block(state.len());
        self.jump(head, &state);
        let (body, bp) = self.block(1);
        let (exit, _) = self.block(0);
        self.switch(head);
        self.term(Term::Next {
            iter: Use::new(it),
            some: body,
            none: exit,
            span,
        });

        self.switch(exit);
        let result = match kind {
            Loop::Map | Loop::Filter => self.emit(Op::AccDone(Use::new(hp[0])), span),
            Loop::Count => hp[0],
            Loop::Any => self.konst(Value::FALSE, span),
            Loop::All => self.konst(Value::TRUE, span),
            _ => self.konst(Value::NONE, span),
        };
        self.jump(done, &[result]);

        self.switch(body);
        let x = bp[0];
        let r = self.emit(
            Op::Call {
                f: Use::new(f),
                args: vec![Use::new(x)],
                named: Vec::new(),
            },
            span,
        );
        if kind == Loop::Map {
            let acc = self.emit(Op::AccPush(Use::new(hp[0]), Use::new(r)), span);
            self.jump(head, &[acc]);
        } else {
            let b = self.emit(Op::CheckPred(Use::new(r), kind.name()), span);
            let (yes, _) = self.block(0);
            let (no, _) = self.block(0);
            self.branch(b, yes, no);
            self.switch(yes);
            match kind {
                Loop::Filter => {
                    let acc = self.emit(Op::AccPush(Use::new(hp[0]), Use::new(x)), span);
                    self.jump(head, &[acc]);
                }
                Loop::Count => {
                    let one = self.konst(Value::int(1), span);
                    let n = self.emit(Op::Bin(BinOp::Add, Use::new(hp[0]), Use::new(one)), span);
                    self.jump(head, &[n]);
                }
                Loop::Any => {
                    let t = self.konst(Value::TRUE, span);
                    self.jump(done, &[t]);
                }
                Loop::All => self.jump(head, &[]),
                _ => self.jump(done, &[x]),
            }
            self.switch(no);
            match kind {
                Loop::Filter | Loop::Count => self.jump(head, &[hp[0]]),
                Loop::All => {
                    let f = self.konst(Value::FALSE, span);
                    self.jump(done, &[f]);
                }
                _ => self.jump(head, &[]),
            }
        }
        self.switch(done);
        out
    }

    // --- Comprehensions ------------------------------------------------------

    fn comprehension(&mut self, comp: &Comp, span: Span) -> Result<Var> {
        let kind = match comp.kind {
            CompKind::List => AccKind::List,
            CompKind::Set => AccKind::Set,
            CompKind::Map => AccKind::Map,
            CompKind::Gen => return self.generator(comp, span),
        };
        // The first collection is read first, so the accumulator can be
        // made the right size.
        let first = match comp.clauses.first() {
            Some(CompClause::For { iter, .. }) => Some(self.expr(iter)?),
            _ => None,
        };
        let acc = self.emit(Op::NewAcc(kind, first.map(Use::new)), span);
        let (done, ps) = self.block(1);
        let depth = self.frames.len();
        self.comp_level(comp, 0, acc, done, first, Sink::Acc { finite: true })?;
        self.frames.truncate(depth);
        self.switch(done);
        Ok(self.emit(Op::AccDone(Use::new(ps[0])), span))
    }

    /// Library call `id` on a generator comprehension and nothing else, as
    /// one loop over the comprehension's clauses, without making the
    /// generator; `None` if `id` isn't one of the functions this covers.
    ///
    /// Each item is computed and then consumed before the next, as when the
    /// function reads the generator, so errors come in the same order, and
    /// a `for` over an infinite range is no error (`sum` over one runs
    /// forever, as before). The functions that read the whole generator
    /// into a list before doing anything else are given that list, built by
    /// the loop; `sum` and `product` add up as they go, and `any`, `all` and
    /// `first` stop at the item that decides them.
    fn fused_generator(
        &mut self,
        id: usize,
        comp: &Comp,
        gspan: Span,
        span: Span,
    ) -> Result<Option<Var>> {
        let name = BUILTINS[id].name;
        let (init, sink) = match name {
            "min" | "max" | "count" | "list" | "tuple" | "set" | "sort" | "reverse" | "join" => {
                (Value::NONE, Sink::Acc { finite: false })
            }
            "sum" => (Value::int(0), Sink::Fold(BinOp::Add, span)),
            "product" => (Value::int(1), Sink::Fold(BinOp::Mul, span)),
            "any" | "all" | "first" => (Value::NONE, Sink::Exit(name, span, 0)),
            _ => return Ok(None),
        };
        let init = match sink {
            Sink::Acc { .. } => self.emit(Op::NewAcc(AccKind::List, None), gspan),
            _ => self.konst(init, span),
        };
        let (out, op) = self.block(1);
        let sink = match sink {
            Sink::Exit(name, span, _) => Sink::Exit(name, span, out),
            s => s,
        };
        let (rest, rp) = self.block(1);
        let depth = self.frames.len();
        self.comp_level(comp, 0, init, rest, None, sink)?;
        self.frames.truncate(depth);
        self.switch(rest);
        let r = match (sink, name) {
            (Sink::Acc { .. }, _) => {
                let l = self.emit(Op::AccDone(Use::new(rp[0])), gspan);
                self.emit(
                    Op::CallBuiltin {
                        id,
                        args: vec![Use::new(l)],
                        named: Vec::new(),
                    },
                    span,
                )
            }
            (Sink::Fold(..), _) => rp[0],
            (_, "any") => self.konst(Value::FALSE, span),
            (_, "all") => self.konst(Value::TRUE, span),
            _ => self.konst(Value::NONE, span),
        };
        self.jump(out, &[r]);
        self.switch(out);
        Ok(Some(op[0]))
    }

    /// Clause `i` onwards, adding to `acc`; `cont` takes the accumulator
    /// when this level has nothing more to add.
    /// `first` is clause 0's collection, if `comprehension` read it already.
    fn comp_level(
        &mut self,
        comp: &Comp,
        i: usize,
        acc: Var,
        cont: BlockId,
        first: Option<Var>,
        sink: Sink,
    ) -> Result<()> {
        match comp.clauses.get(i) {
            None => {
                let k = self.expr(&comp.body)?;
                let acc2 = match (&comp.value, sink) {
                    (_, Sink::Fold(op, span)) => {
                        self.emit(Op::Bin(op, Use::new(acc), Use::new(k)), span)
                    }
                    (_, Sink::Exit("first", _, out)) => {
                        self.jump(out, &[k]);
                        return Ok(());
                    }
                    (_, Sink::Exit(name, span, out)) => {
                        // `any` stops at a true element, `all` at a false one.
                        let b = self.emit(Op::CheckElem(Use::new(k), name), span);
                        let (yes, _) = self.block(0);
                        let (no, _) = self.block(0);
                        self.branch(b, yes, no);
                        let (stop, go) = if name == "any" { (yes, no) } else { (no, yes) };
                        self.switch(stop);
                        let r = self.konst(Value::bool(name == "any"), span);
                        self.jump(out, &[r]);
                        self.switch(go);
                        self.jump(cont, &[acc]);
                        return Ok(());
                    }
                    (None, Sink::Acc { .. }) => {
                        self.emit(Op::AccPush(Use::new(acc), Use::new(k)), comp.body.span)
                    }
                    (Some(vx), Sink::Acc { .. }) => {
                        let v = self.expr(vx)?;
                        self.emit(
                            Op::AccInsert(Use::new(acc), Use::new(k), Use::new(v)),
                            comp.body.span,
                        )
                    }
                };
                self.jump(cont, &[acc2]);
            }
            Some(CompClause::If(c)) => {
                let (then, els) = self.cond(c, "a comprehension `if`")?;
                self.switch(els);
                self.jump(cont, &[acc]);
                self.switch(then);
                self.comp_level(comp, i + 1, acc, cont, None, sink)?;
            }
            Some(CompClause::For { pat, iter, .. }) => {
                let coll = match first {
                    Some(c) if i == 0 => c,
                    _ => self.expr(iter)?,
                };
                let it = self.emit(
                    Op::Iter {
                        coll: Use::new(coll),
                        finite: matches!(sink, Sink::Acc { finite: true }),
                    },
                    iter.span,
                );
                let (head, hp) = self.block(1);
                self.jump(head, &[acc]);
                let (body, bp) = self.block(1);
                let (exit, _) = self.block(0);
                self.switch(head);
                self.term(Term::Next {
                    iter: Use::new(it),
                    some: body,
                    none: exit,
                    span: iter.span,
                });
                self.switch(exit);
                self.jump(cont, &[hp[0]]);
                self.switch(body);
                let (skip, _) = self.block(0);
                self.frames.push(HashMap::new());
                self.pattern(pat, bp[0], skip)?;
                self.comp_level(comp, i + 1, hp[0], head, None, sink)?;
                self.frames.pop();
                self.switch(skip);
                self.jump(head, &[hp[0]]);
            }
        }
        Ok(())
    }

    /// A generator comprehension: nested `FlatMap`s, one per `for`, each
    /// calling a closure for the rest of the clauses. The outermost level
    /// starts from a one-item list, so nothing runs until it's read.
    fn generator(&mut self, comp: &Comp, span: Span) -> Result<Var> {
        let id = self.alloc();
        self.begin(id, "<generator>".into(), FnKind::Lambda, 1, span);
        let depth = self.frames.len();
        self.gen_level(comp, 0)?;
        self.frames.truncate(depth);
        let caps = self.end()?;
        let f = self.closure(id, caps, span)?;
        let unit = self.konst(Value::NONE, span);
        let src = self.emit(Op::List(vec![Use::new(unit)]), span);
        Ok(self.emit(
            Op::FlatMap {
                src: Use::new(src),
                f: Use::new(f),
            },
            span,
        ))
    }

    /// Returns (in tail position) an iterable of the items from clause `i`
    /// onwards.
    fn gen_level(&mut self, comp: &Comp, i: usize) -> Result<()> {
        match comp.clauses.get(i) {
            None => {
                let v = self.expr(&comp.body)?;
                let l = self.emit(Op::List(vec![Use::new(v)]), comp.body.span);
                self.term(Term::Return(Use::new(l)));
            }
            Some(CompClause::If(c)) => {
                let (then, els) = self.cond(c, "a comprehension `if`")?;
                self.switch(els);
                let empty = self.emit(Op::List(vec![]), c.span);
                self.term(Term::Return(Use::new(empty)));
                self.switch(then);
                self.gen_level(comp, i + 1)?;
            }
            Some(CompClause::For { pat, iter, .. }) => {
                let coll = self.expr(iter)?;
                let id = self.alloc();
                let ps = self.begin(id, "<generator>".into(), FnKind::Lambda, 1, iter.span);
                self.frames.push(HashMap::new());
                let (skip, _) = self.block(0);
                self.pattern(pat, ps[0], skip)?;
                self.gen_level(comp, i + 1)?;
                self.switch(skip);
                let empty = self.emit(Op::List(vec![]), pat.span);
                self.term(Term::Return(Use::new(empty)));
                self.frames.pop();
                let caps = self.end()?;
                let f = self.closure(id, caps, iter.span)?;
                let g = self.emit(
                    Op::FlatMap {
                        src: Use::new(coll),
                        f: Use::new(f),
                    },
                    iter.span,
                );
                self.term(Term::Return(Use::new(g)));
            }
        }
        Ok(())
    }
}

/// What a comprehension loop does with each item ([`Lowerer::comp_level`]).
#[derive(Debug, Clone, Copy)]
enum Sink {
    /// Adds it to the accumulator; with `finite`, as a list, set or map
    /// comprehension, which can't run over an infinite range.
    Acc { finite: bool },
    /// Combines it with the running total, reporting errors at the call.
    Fold(BinOp, Span),
    /// For library function `any`, `all` or `first` (at the span):
    /// jumps to the block with the result once an item decides it.
    Exit(&'static str, Span, BlockId),
}

/// The library functions [`Lowerer::builtin_loop`] compiles as loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loop {
    Map,
    Filter,
    Count,
    Any,
    All,
    First,
    MinBy,
    MaxBy,
    Fold,
    Scan,
}

impl Loop {
    /// How many arguments the call takes, the collection included.
    fn arity(self) -> usize {
        match self {
            Loop::Fold | Loop::Scan => 3,
            _ => 2,
        }
    }

    fn of(name: &str) -> Option<Loop> {
        Some(match name {
            "map" => Loop::Map,
            "filter" => Loop::Filter,
            "count" => Loop::Count,
            "any" => Loop::Any,
            "all" => Loop::All,
            "first" => Loop::First,
            "min_by" => Loop::MinBy,
            "max_by" => Loop::MaxBy,
            "fold" => Loop::Fold,
            "scan" => Loop::Scan,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Loop::Map => "map",
            Loop::Filter => "filter",
            Loop::Count => "count",
            Loop::Any => "any",
            Loop::All => "all",
            Loop::First => "first",
            Loop::MinBy => "min_by",
            Loop::MaxBy => "max_by",
            Loop::Fold => "fold",
            Loop::Scan => "scan",
        }
    }
}
