//! Name resolution. Rewrites every `ExprKind::Name` into a frame slot,
//! global or builtin, assigns slots to pattern bindings, and computes frame
//! sizes. Along the way it records the static call graph between named
//! functions, which decides memoization (SPEC 8.1). Also groups top-level
//! items into a [`Program`].

use crate::ast::*;
use crate::builtins;
use crate::error::{Error, Result, SourceFile, Span};
use crate::parser::group_defs;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

pub enum GlobalDef {
    /// A constant; `slot` is the binding's slot in the constant's own frame.
    Const {
        def: Rc<ConstDef>,
        slot: usize,
    },
    Func(Rc<FnDef>),
}

pub struct Program {
    /// The source of every file in the program, one after another.
    pub src: String,
    /// Where each file starts in `src` (SPEC 2.8). Empty when the program
    /// wasn't loaded from a file.
    pub files: Vec<SourceFile>,
    pub day: Option<(u32, u32)>,
    pub globals: Vec<GlobalDef>,
    pub global_names: Vec<Name>,
    pub global_index: HashMap<Name, usize>,
    pub tests: Vec<(Check, bool)>,
    pub examples: Vec<Example>,
    pub answers: Option<(Vec<Check>, Span)>,
    /// Whether each function (by `FnDef::id`) is memoized.
    pub memo: Vec<bool>,
    /// How many lambdas are closed (`Lambda::closed`).
    pub closed_lambdas: usize,
    /// The main file, for messages. Empty when the program wasn't loaded
    /// from a file.
    pub path: PathBuf,
    /// Functions (by `FnDef::id`) already warned about for large memo keys
    /// (SPEC 8.1), so each warning is given once per run.
    pub memo_warned: RefCell<HashSet<usize>>,
    /// The `where` constants that hide a parameter of their function, as
    /// in `f(pos) = ... where pos = pos + 1`, by where each definition
    /// starts in the source, for a hint when one turns out to depend on
    /// itself. By definition rather than by name, so that a cycle in some
    /// other constant of the same name gets no hint.
    pub hidden_params: HashSet<usize>,
    /// Every definition and every use of one, for editors (`tinsel
    /// symbols`, SPEC 2.7).
    pub symbols: Symbols,
}

/// A definition, for `tinsel symbols` (SPEC 2.7).
#[derive(Debug, Clone)]
pub struct SymDef {
    pub name: Name,
    /// `function`, `constant`, `where function`, `where constant`,
    /// `parameter` or `variable` (bound by a lambda, a `match` arm or a
    /// comprehension).
    pub what: &'static str,
    /// The name, where it is defined.
    pub span: Span,
    /// The definition this one is inside, as an index into `defs`.
    pub container: Option<usize>,
}

/// The program's definitions, and each use of one: the name's span and the
/// index of the definition it refers to.
#[derive(Debug, Clone, Default)]
pub struct Symbols {
    pub defs: Vec<SymDef>,
    pub refs: Vec<(Span, usize)>,
}

/// The symbols without the parameters of the lambdas that `_`, `_1`, ...
/// make (SPEC 5.4), which aren't names in the program. What was inside one
/// moves to its container, and uses of one go.
fn without_placeholders(defs: Vec<SymDef>, refs: Vec<(Span, usize)>) -> Symbols {
    let hidden = |d: &SymDef| crate::parser::is_placeholder(&d.name);
    // Each definition's new index, if it stays.
    let mut index = Vec::with_capacity(defs.len());
    let mut kept = 0;
    for d in &defs {
        index.push((!hidden(d)).then(|| {
            kept += 1;
            kept - 1
        }));
    }
    // The nearest container that stays.
    let lift = |mut c: Option<usize>| {
        while let Some(i) = c {
            match index.get(i).copied().flatten() {
                Some(new) => return Some(new),
                None => c = defs.get(i).and_then(|d| d.container),
            }
        }
        None
    };
    let new_defs = defs
        .iter()
        .filter(|d| !hidden(d))
        .map(|d| SymDef {
            container: lift(d.container),
            ..d.clone()
        })
        .collect();
    let new_refs = refs
        .into_iter()
        .filter_map(|(span, d)| index.get(d).copied().flatten().map(|n| (span, n)))
        .collect();
    Symbols {
        defs: new_defs,
        refs: new_refs,
    }
}

/// Where `name` is written in the definition at `span` (which may start
/// with an annotation such as `@memo`): its first occurrence there as a
/// whole word.
fn name_span(src: &str, span: Span, name: &str) -> Span {
    let text = src.get(span.start..span.end.min(src.len())).unwrap_or("");
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut from = 0;
    while let Some(i) = text[from..].find(name) {
        let at = from + i;
        if !word(text[..at].chars().next_back()) && !word(text[at + name.len()..].chars().next()) {
            return Span::new(span.start + at, span.start + at + name.len());
        }
        from = at + name.len();
    }
    Span::new(span.start, span.start + name.len())
}

impl Program {
    pub fn global(&self, name: &str) -> Option<usize> {
        self.global_index.get(name).copied()
    }

    /// The error for a constant that depends on itself through `chain`
    /// (which ends with `name`), the same on every engine. `def` is where
    /// the definition of the constant in the cycle starts, if known.
    pub fn cycle_message(&self, name: &str, chain: &[&str], def: Option<usize>) -> String {
        let mut msg = format!(
            "constant `{name}` depends on itself: {}",
            chain.join(" -> ")
        );
        if chain.len() == 2 && def.is_some_and(|d| self.hidden_params.contains(&d)) {
            msg.push_str(&format!(
                " (this `where` constant hides the parameter `{name}`, so `{name}` in \
                 its own definition means the constant; give it another name)"
            ));
        }
        msg
    }
}

/// A name may be bound only once in a pattern, or across a function's or
/// lambda's parameters (SPEC 6).
fn unique_bindings(pats: &[Pattern]) -> Result<()> {
    let mut seen: Vec<Name> = Vec::new();
    for p in pats {
        let mut bindings = Vec::new();
        p.clone()
            .bindings_mut(&mut |b| bindings.push((b.name.clone(), b.span)));
        for (name, span) in bindings {
            if seen.contains(&name) {
                return Err(Error::new(
                    format!("`{name}` is bound twice in this pattern"),
                    span,
                ));
            }
            seen.push(name);
        }
    }
    Ok(())
}

fn rc_mut<T>(rc: &mut Rc<T>) -> &mut T {
    Rc::get_mut(rc).expect("AST nodes are uniquely owned during resolution")
}

pub fn build_program(src: String, items: Vec<Item>) -> Result<Program> {
    let mut day = None;
    let mut raw_defs = Vec::new();
    let mut tests = Vec::new();
    let mut examples = Vec::new();
    let mut answers = None;
    for item in items {
        if !matches!(item, Item::Def(_)) {
            // Anything else between two clauses separates them.
            raw_defs.push(None);
        }
        match item {
            Item::Day { year, day: d, span } => {
                if day.is_some() {
                    return Err(Error::new("duplicate `day` header", span));
                }
                day = Some((year, d));
            }
            Item::Example(e) => examples.push(e),
            Item::Answers { expects, span } => {
                if answers.is_some() {
                    return Err(Error::new("duplicate `answers` block", span));
                }
                answers = Some((expects, span));
            }
            Item::Test {
                check,
                expect_error,
            } => tests.push((check, expect_error)),
            Item::Def(d) => raw_defs.push(Some(d)),
            Item::Include { span, .. } => {
                return Err(Error::new(
                    "`include` works only in a program loaded from a file",
                    span,
                ));
            }
        }
    }
    let (mut fns, consts) = group_defs(raw_defs)?;

    // Global table.
    let mut global_index: HashMap<Name, usize> = HashMap::new();
    let mut global_names = Vec::new();
    let mut declare = |name: &Name, span: Span| -> Result<usize> {
        if global_index.contains_key(name) {
            return Err(Error::new(
                format!("`{name}` is defined more than once"),
                span,
            ));
        }
        let i = global_names.len();
        global_index.insert(name.clone(), i);
        global_names.push(name.clone());
        Ok(i)
    };
    let mut const_globals = Vec::new();
    // Each constant binding's global and name span, to define below.
    let mut const_binds: Vec<(Name, usize, Span)> = Vec::new();
    for c in &consts {
        unique_bindings(std::slice::from_ref(&c.pat))?;
        let mut names = Vec::new();
        let mut copy = c.pat.clone();
        copy.bindings_mut(&mut |b| names.push((b.name.clone(), b.span)));
        let mut idxs = Vec::new();
        for (n, span) in names {
            let gi = declare(&n, span)?;
            const_binds.push((n, gi, span));
            idxs.push(gi);
        }
        const_globals.push(idxs);
    }

    let mut r = Resolver {
        frames: Vec::new(),
        globals: HashMap::new(),
        annotations: Vec::new(),
        edges: Vec::new(),
        current_fn: Vec::new(),
        lambda_reach: Vec::new(),
        closed_lambdas: 0,
        hidden_params: HashSet::new(),
        src: src.clone(),
        defs: Vec::new(),
        refs: Vec::new(),
        within: Vec::new(),
        fn_defs: HashMap::new(),
        def_at: HashMap::new(),
    };
    let mut fn_globals = Vec::new();
    for f in &mut fns {
        let gi = declare(&f.name, f.span)?;
        f.id = r.new_fn(f.annotation);
        let def = r.define(&f.name, "function", name_span(&src, f.span, &f.name));
        r.fn_defs.insert(f.id, def);
        r.globals.insert(f.name.clone(), (gi, Some(f.id), def));
        fn_globals.push(gi);
    }
    for (name, gi, span) in const_binds {
        let def = r.define(&name, "constant", span);
        r.globals.entry(name).or_insert((gi, None, def));
    }

    let mut globals: Vec<Option<GlobalDef>> = (0..global_names.len()).map(|_| None).collect();
    for (mut c, idxs) in consts.into_iter().zip(const_globals) {
        let slots = r.top_const(&mut c)?;
        crate::moves::constant(&mut c);
        let def = Rc::new(c);
        for (gi, slot) in idxs.into_iter().zip(slots) {
            globals[gi] = Some(GlobalDef::Const {
                def: def.clone(),
                slot,
            });
        }
    }
    for (mut f, gi) in fns.into_iter().zip(fn_globals) {
        r.fn_def(&mut f)?;
        crate::moves::function(&mut f);
        globals[gi] = Some(GlobalDef::Func(Rc::new(f)));
    }
    let globals = globals
        .into_iter()
        .map(|g| g.ok_or_else(|| Error::msg("internal: unresolved global")))
        .collect::<Result<Vec<_>>>()?;

    for (check, _) in &mut tests {
        r.check(check)?;
    }
    for ex in &mut examples {
        for (name, def, span) in &mut ex.withs {
            match global_index.get(name).map(|&i| &globals[i]) {
                Some(GlobalDef::Const { .. }) => {}
                _ => {
                    return Err(Error::new(
                        format!("`with` must name a top-level constant, not `{name}`"),
                        *span,
                    ));
                }
            }
            // The name in `with name = ...` refers to that constant.
            if let Some(&(_, _, d)) = r.globals.get(name) {
                r.refs.push((name_span(&src, *span, name), d));
            }
            r.top_const(rc_mut(def))?;
            crate::moves::constant(rc_mut(def));
        }
        for check in &mut ex.expects {
            r.check(check)?;
        }
    }
    if let Some((expects, _)) = &mut answers {
        for check in expects {
            r.check(check)?;
        }
    }

    let memo = crate::memo::decide(&r.annotations, &r.edges);
    Ok(Program {
        src,
        files: Vec::new(),
        day,
        globals,
        global_names,
        global_index,
        tests,
        examples,
        answers,
        memo,
        closed_lambdas: r.closed_lambdas,
        path: PathBuf::new(),
        memo_warned: RefCell::new(HashSet::new()),
        hidden_params: r.hidden_params,
        symbols: without_placeholders(r.defs, r.refs),
    })
}

/// A name in scope: its slot, the function id if it names a `where`
/// function, and the index of its definition in `Resolver::defs`.
type Local = (usize, Option<usize>, usize);

struct FrameScope {
    scopes: Vec<HashMap<Name, Local>>,
    size: usize,
}

struct Resolver {
    frames: Vec<FrameScope>,
    /// name -> (global index, function id if it names a function, index of
    /// its definition in `defs`)
    globals: HashMap<Name, (usize, Option<usize>, usize)>,
    annotations: Vec<Option<Annotation>>,
    /// Call graph: (caller, callee, is the reference a tail call).
    edges: Vec<(usize, usize, bool)>,
    current_fn: Vec<usize>,
    /// For each lambda being resolved: the index of its frame, and the
    /// outermost frame index any name inside it has resolved to.
    lambda_reach: Vec<(usize, usize)>,
    closed_lambdas: usize,
    /// See `Program::hidden_params`.
    hidden_params: HashSet<usize>,
    /// The program's source, to find names in definitions.
    src: String,
    /// See `Program::symbols`.
    defs: Vec<SymDef>,
    refs: Vec<(Span, usize)>,
    /// The definitions being resolved, innermost last (indexes into `defs`).
    within: Vec<usize>,
    /// The definition of each function, by function id.
    fn_defs: HashMap<usize, usize>,
    /// The definition at each binding's span start, for constants.
    def_at: HashMap<usize, usize>,
}

impl Resolver {
    /// Records a definition of `name` at `span`, inside the innermost one
    /// being resolved; gives its index.
    fn define(&mut self, name: &Name, what: &'static str, span: Span) -> usize {
        self.defs.push(SymDef {
            name: name.clone(),
            what,
            span,
            container: self.within.last().copied(),
        });
        self.def_at.insert(span.start, self.defs.len() - 1);
        self.defs.len() - 1
    }

    fn new_fn(&mut self, annotation: Option<Annotation>) -> usize {
        self.annotations.push(annotation);
        self.annotations.len() - 1
    }

    fn push_frame(&mut self) {
        self.frames.push(FrameScope {
            scopes: vec![HashMap::new()],
            size: 0,
        });
    }

    fn pop_frame(&mut self) -> usize {
        self.frames.pop().map_or(0, |f| f.size)
    }

    fn frame(&mut self) -> &mut FrameScope {
        self.frames
            .last_mut()
            .expect("resolver always has a frame here")
    }

    fn push_scope(&mut self) {
        self.frame().scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.frame().scopes.pop();
    }

    fn declare(&mut self, name: &Name, fn_id: Option<usize>, def: usize) -> usize {
        let f = self.frame();
        let slot = f.size;
        f.size += 1;
        if let Some(scope) = f.scopes.last_mut() {
            scope.insert(name.clone(), (slot, fn_id, def));
        }
        slot
    }

    /// Declares a pattern's bindings, each a definition of kind `what`,
    /// giving each its slot in the same pass.
    fn declare_pattern(&mut self, p: &mut Pattern, what: &'static str) {
        p.bindings_mut(&mut |b| {
            let def = self.define(&b.name, what, b.span);
            b.slot = self.declare(&b.name, None, def);
        });
    }

    /// Finds a local or global binding: its resolved form, the function id
    /// if it is a function definition, and its definition's index.
    fn find(&self, name: &Name) -> Option<(ExprKind, Option<usize>, usize)> {
        for (depth, frame) in self.frames.iter().rev().enumerate() {
            for scope in frame.scopes.iter().rev() {
                if let Some(&(slot, fn_id, def)) = scope.get(name) {
                    let local = ExprKind::Local {
                        depth,
                        slot,
                        name: name.clone(),
                        moved: false,
                    };
                    return Some((local, fn_id, def));
                }
            }
        }
        self.globals
            .get(name)
            .map(|&(g, fn_id, def)| (ExprKind::Global(g), fn_id, def))
    }

    /// Resolves a name, recording a call-graph edge if it names a function.
    fn lookup(&mut self, name: &Name, span: Span, tail_call: bool) -> Result<ExprKind> {
        match self.find(name) {
            Some((kind, fn_id, def)) => {
                self.refs.push((span, def));
                if let ExprKind::Local { depth, .. } = &kind {
                    let reached = self.frames.len() - 1 - depth;
                    for (_, outermost) in &mut self.lambda_reach {
                        *outermost = (*outermost).min(reached);
                    }
                }
                if let (Some(callee), Some(&caller)) = (fn_id, self.current_fn.last()) {
                    self.edges.push((caller, callee, tail_call));
                }
                Ok(kind)
            }
            None => match builtins::lookup(name) {
                Some(b) => Ok(ExprKind::Builtin(b)),
                None if crate::parser::is_placeholder(name) => Err(Error::new(
                    format!(
                        "`{name}` here isn't inside a call argument or a definition's right-hand side, so it can't become a lambda; write `x => ...`"
                    ),
                    span,
                )),
                None => Err(Error::new(format!("unknown name `{name}`"), span)),
            },
        }
    }

    fn check(&mut self, c: &mut Check) -> Result<()> {
        self.push_frame();
        self.expr(&mut c.expr, false)?;
        c.frame_size = self.pop_frame();
        crate::moves::lambdas(&mut c.expr);
        Ok(())
    }

    /// Resolves a top-level constant in its own frame. Returns the slots of
    /// its pattern bindings, in source order.
    fn top_const(&mut self, c: &mut ConstDef) -> Result<Vec<usize>> {
        self.push_frame();
        // The bindings get slots, but aren't in scope: inside the body the
        // names refer to the globals, so self-reference is a detected cycle.
        let mut slots = Vec::new();
        c.pat.bindings_mut(&mut |b| {
            b.slot = slots.len();
            slots.push(b.slot);
        });
        self.frame().size = slots.len();
        let container = self.const_def(&c.pat);
        self.within.extend(container);
        self.declare_wheres(&mut c.wheres)?;
        self.expr(&mut c.body, false)?;
        self.where_bodies(&mut c.wheres)?;
        if container.is_some() {
            self.within.pop();
        }
        c.frame_size = self.pop_frame();
        Ok(slots)
    }

    /// The definition of a constant: of its first binding.
    fn const_def(&self, pat: &Pattern) -> Option<usize> {
        let mut first = None;
        pat.clone().bindings_mut(&mut |b| {
            first.get_or_insert(b.span.start);
        });
        first.and_then(|s| self.def_at.get(&s).copied())
    }

    fn fn_def(&mut self, f: &mut FnDef) -> Result<()> {
        self.current_fn.push(f.id);
        let def = self.fn_defs.get(&f.id).copied();
        self.within.extend(def);
        for (i, clause) in f.clauses.iter_mut().enumerate() {
            // Each later clause's head names the function again.
            if let (Some(def), true) = (def, i > 0) {
                let span = name_span(&self.src, clause.span, &f.name);
                self.refs.push((span, def));
            }
            unique_bindings(&clause.params)?;
            let mut params = Vec::new();
            for p in &clause.params {
                p.clone().bindings_mut(&mut |b| params.push(b.name.clone()));
            }
            for c in &clause.wheres.consts {
                let mut hides = false;
                c.pat
                    .clone()
                    .bindings_mut(&mut |b| hides |= params.contains(&b.name));
                if hides {
                    self.hidden_params.insert(c.span.start);
                }
            }
            self.push_frame();
            for p in &mut clause.params {
                self.declare_pattern(p, "parameter");
            }
            self.push_scope();
            self.declare_wheres(&mut clause.wheres)?;
            if let Some(g) = &mut clause.guard {
                self.expr(g, false)?;
            }
            self.expr(&mut clause.body, true)?;
            self.where_bodies(&mut clause.wheres)?;
            clause.frame_size = self.pop_frame();
        }
        if def.is_some() {
            self.within.pop();
        }
        self.current_fn.pop();
        Ok(())
    }

    fn declare_wheres(&mut self, w: &mut Wheres) -> Result<()> {
        // As at top level, each name is defined once per `where` block.
        let mut defined: Vec<(Name, Span)> = Vec::new();
        for c in &w.consts {
            unique_bindings(std::slice::from_ref(&c.pat))?;
            c.pat
                .clone()
                .bindings_mut(&mut |b| defined.push((b.name.clone(), b.span)));
        }
        defined.extend(w.fns.iter().map(|(f, _)| (f.name.clone(), f.span)));
        defined.sort_by_key(|(_, span)| span.start);
        for (i, (name, span)) in defined.iter().enumerate() {
            if defined[..i].iter().any(|(n, _)| n == name) {
                return Err(Error::new(
                    format!("`{name}` is defined more than once"),
                    *span,
                ));
            }
        }
        for c in &mut w.consts {
            self.declare_pattern(&mut rc_mut(c).pat, "where constant");
        }
        for (f, slot) in &mut w.fns {
            let f = rc_mut(f);
            f.id = self.new_fn(f.annotation);
            let def = self.define(
                &f.name,
                "where function",
                name_span(&self.src, f.span, &f.name),
            );
            self.fn_defs.insert(f.id, def);
            *slot = self.declare(&f.name, Some(f.id), def);
        }
        Ok(())
    }

    fn where_bodies(&mut self, w: &mut Wheres) -> Result<()> {
        for c in &mut w.consts {
            let c = rc_mut(c);
            let container = self.const_def(&c.pat);
            self.within.extend(container);
            self.push_scope();
            self.declare_wheres(&mut c.wheres)?;
            self.expr(&mut c.body, false)?;
            self.where_bodies(&mut c.wheres)?;
            self.pop_scope();
            if container.is_some() {
                self.within.pop();
            }
        }
        for (f, _) in &mut w.fns {
            self.fn_def(rc_mut(f))?;
        }
        Ok(())
    }

    /// `tail` is true when `e` is in tail position (SPEC 8.1).
    fn expr(&mut self, e: &mut Expr, tail: bool) -> Result<()> {
        match &mut e.kind {
            ExprKind::Name(n) => {
                let n = n.clone();
                e.kind = self.lookup(&n, e.span, false)?;
            }
            ExprKind::Lit(_)
            | ExprKind::Local { .. }
            | ExprKind::Global(_)
            | ExprKind::Builtin(_) => {}
            ExprKind::Interp(parts) => {
                for p in parts {
                    if let InterpPart::Expr(e) = p {
                        self.expr(e, false)?;
                    }
                }
            }
            ExprKind::Tuple(xs) | ExprKind::List(xs) | ExprKind::Set(xs) => {
                for x in xs {
                    self.expr(x, false)?;
                }
            }
            ExprKind::Map(entries) => {
                for (k, v) in entries {
                    self.expr(k, false)?;
                    self.expr(v, false)?;
                }
            }
            ExprKind::Comp(comp) => {
                let comp = rc_mut(comp);
                let mut pushed = Vec::new();
                for (i, clause) in comp.clauses.iter_mut().enumerate() {
                    match clause {
                        CompClause::For { pat, iter, .. } => {
                            self.expr(iter, false)?;
                            unique_bindings(std::slice::from_ref(pat))?;
                            self.push_frame();
                            self.declare_pattern(pat, "variable");
                            pushed.push(i);
                        }
                        CompClause::If(c) => self.expr(c, false)?,
                    }
                }
                self.expr(&mut comp.body, false)?;
                if let Some(v) = &mut comp.value {
                    self.expr(v, false)?;
                }
                for i in pushed.into_iter().rev() {
                    let size = self.pop_frame();
                    if let CompClause::For { frame_size, .. } = &mut comp.clauses[i] {
                        *frame_size = size;
                    }
                }
            }
            ExprKind::Call { func, args, method } => {
                if let ExprKind::Name(n) = &func.kind {
                    let n = n.clone();
                    // In `x.f`, a constant or variable doesn't hide a library
                    // function of the same name (SPEC 5.2).
                    let names_fn = matches!(self.find(&n), Some((_, Some(_), _)));
                    func.kind = match builtins::lookup(&n) {
                        Some(b) if *method && !names_fn => ExprKind::Builtin(b),
                        _ => self.lookup(&n, func.span, tail)?,
                    };
                } else {
                    self.expr(func, false)?;
                }
                for a in args {
                    self.expr(&mut a.value, false)?;
                }
            }
            ExprKind::TupleIndex(x, _) | ExprKind::Unary(_, x) => self.expr(x, false)?,
            ExprKind::Index(a, b)
            | ExprKind::Binary(_, a, b)
            | ExprKind::And(a, b)
            | ExprKind::Or(a, b)
            | ExprKind::Coalesce(a, b) => {
                self.expr(a, false)?;
                self.expr(b, false)?;
            }
            ExprKind::Compare(first, rest) => {
                self.expr(first, false)?;
                for (_, x) in rest {
                    self.expr(x, false)?;
                }
            }
            ExprKind::If(c, a, b) => {
                self.expr(c, false)?;
                self.expr(a, tail)?;
                self.expr(b, tail)?;
            }
            ExprKind::Lambda(lam) => {
                // A lambda body is a tail position for the trampoline, but
                // calls inside it aren't tail calls of the enclosing function.
                let lam = rc_mut(lam);
                unique_bindings(&lam.params)?;
                self.push_frame();
                let own = self.frames.len() - 1;
                self.lambda_reach.push((own, own));
                for p in &mut lam.params {
                    self.declare_pattern(p, "variable");
                }
                self.expr(&mut lam.body, false)?;
                lam.frame_size = self.pop_frame();
                if let Some((own, outermost)) = self.lambda_reach.pop()
                    && outermost >= own
                {
                    lam.closed = Some(self.closed_lambdas);
                    self.closed_lambdas += 1;
                }
            }
            ExprKind::Match(scrutinee, arms) => {
                self.expr(scrutinee, false)?;
                for arm in arms {
                    unique_bindings(std::slice::from_ref(&arm.pat))?;
                    self.push_scope();
                    self.declare_pattern(&mut arm.pat, "variable");
                    if let Some(g) = &mut arm.guard {
                        self.expr(g, false)?;
                    }
                    self.expr(&mut arm.body, tail)?;
                    self.pop_scope();
                }
            }
            ExprKind::Range { lo, hi, .. } => {
                if let Some(lo) = lo {
                    self.expr(lo, false)?;
                }
                if let Some(hi) = hi {
                    self.expr(hi, false)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::runner::compile;

    fn error(src: &str) -> String {
        match compile(src.to_string()) {
            Ok(_) => "compiled".into(),
            Err(e) => e.into_msg(),
        }
    }

    #[test]
    fn where_among_match_arms_needs_the_match_as_the_body() {
        let src = "f(x) = 1 + match x\n  0 => 1\n  where\n    a = 1\n";
        assert!(
            error(src).contains("whole body is that `match`"),
            "{}",
            error(src)
        );
        let src = "f(x) = match x\n  where\n    a = 1\n  0 => 1\n";
        assert!(
            error(src).contains("after its `where` block"),
            "{}",
            error(src)
        );
        let src = "f(x) = match x\n  0 => a\n  where\n    a = 1\n  where\n    b = 2\n";
        assert!(error(src).contains("one `where` block"), "{}", error(src));
    }

    #[test]
    fn numbered_placeholder_errors() {
        assert!(error("f = [1].map(_ + _1)").contains("can't be mixed"));
        assert!(error("f(_1) = 1").contains("is a placeholder"));
        assert!(error("f(x) when _1 > 0 = 1").contains("`_1` here isn't inside"));
    }

    // Compile errors can't be tested from a .tin file, since they stop the
    // whole file, so the duplicate-binding rule (SPEC 6) is tested here.
    #[test]
    fn names_bound_twice_are_rejected() {
        let twice = "`x` is bound twice in this pattern";
        assert_eq!(error("f(x, x) = 1"), twice);
        assert_eq!(error("f((x, [x])) = 1"), twice);
        assert_eq!(error("g = (x, x) => 1"), twice);
        assert_eq!(error("g = match 1\n  (x, x) => 1"), twice);
        assert_eq!(error("g = [1 for (x, x) in []]"), twice);
        assert_eq!(error("(x, x) = (1, 2)"), twice);
        assert_eq!(error("f(s) = y\n  where\n    (x, x) = s\n    y = 1"), twice);
        assert_eq!(error("f(\"{x} {x}\") = 1"), twice);
    }

    #[test]
    fn clauses_separated_by_another_item_are_rejected() {
        let apart = "clauses of `f` must be next to each other";
        assert_eq!(error("f(0) = 1\ntest \"t\" = true\nf(n) = n"), apart);
        assert_eq!(error("f(0) = 1\ntest_error \"t\" = 1 / 0\nf(n) = n"), apart);
        assert_eq!(error("f(0) = 1\nanswers\n  part1 = 1\nf(n) = n"), apart);
    }

    #[test]
    fn only_the_listed_escapes_are_accepted() {
        assert_eq!(error("x = \"\\r\""), "unknown escape `\\r`");
        assert_eq!(error("x = \"\\0\""), "unknown escape `\\0`");
        assert_eq!(error("x = \"\\'\""), "unknown escape `\\'`");
        assert_eq!(error("x = '\\r'"), "unknown escape in character literal");
        assert_eq!(error("x = '\\0'"), "unknown escape in character literal");
    }

    #[test]
    fn where_names_are_defined_once() {
        let twice = "`a` is defined more than once";
        assert_eq!(error("f(x) = a\n  where\n    a = 1\n    a = 2"), twice);
        assert_eq!(
            error("f(x) = a\n  where\n    a = 1\n    (b, a) = (2, 3)"),
            twice
        );
        assert_eq!(error("f(x) = a\n  where\n    a = 1\n    a(y) = y"), twice);
        // Separate where blocks, and a where name shadowing a parameter, are fine.
        assert_eq!(
            error("f(a) = a\n  where\n    b = 1\ng(x) = b\n  where\n    b = 2"),
            "compiled"
        );
    }

    #[test]
    fn the_same_name_in_separate_patterns_is_fine() {
        assert_eq!(
            error("f(x) = 1\ng = match 1\n  x => x\n  (x, y) => y"),
            "compiled"
        );
    }
}
