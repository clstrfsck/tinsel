//! Marks the last use of each local in a function or lambda body as a move.
//!
//! Values are immutable, but a map, set or list that nothing else refers
//! to can be updated in place: `imbl` copies a node only when it is shared.
//! Reading a local normally copies the reference out of its frame slot, so
//! the slot keeps the value shared. At a local's last use the evaluator
//! instead takes the value out of the slot, so that in a loop such as
//!
//! ```tinsel
//! speak(t, last, seen) = ... speak(t + 1, next, seen.set(last, t))
//! ```
//!
//! `seen.set(...)` updates the only reference to `seen` in place. This
//! changes how fast a program runs, never what it computes.
//!
//! A frame can also be read by code that runs later or more than once: a
//! lambda, a comprehension, a `where` constant (evaluated lazily) or a
//! `where` function. A local that any of those refers to is never moved.
//! Guards are left alone too. Everything else in a body runs at most once,
//! in an order known from the expression, so a use is the last one when no
//! use can follow it on any path.

use crate::ast::*;
use std::collections::HashSet;

/// Marks moves in every body of `f`, and in the lambdas and `where`
/// functions inside it.
pub fn function(f: &mut FnDef) {
    for clause in &mut f.clauses {
        let mut captured = HashSet::new();
        if let Some(g) = &clause.guard {
            references(g, 0, false, &mut captured);
        }
        where_references(&clause.wheres, 0, &mut captured);
        references(&clause.body, 0, false, &mut captured);
        mark(&mut clause.body, &mut HashSet::new(), &captured);

        if let Some(g) = &mut clause.guard {
            lambdas(g);
        }
        lambdas(&mut clause.body);
        wheres(&mut clause.wheres);
    }
}

/// Marks moves in the lambdas and `where` functions inside a constant.
pub fn constant(c: &mut ConstDef) {
    lambdas(&mut c.body);
    wheres(&mut c.wheres);
}

/// Marks moves in the lambdas inside an expression that is not itself a
/// function body, such as a test.
pub fn lambdas(e: &mut Expr) {
    match &mut e.kind {
        ExprKind::Lambda(lam) => {
            if let Some(lam) = std::rc::Rc::get_mut(lam) {
                let mut captured = HashSet::new();
                references(&lam.body, 0, false, &mut captured);
                mark(&mut lam.body, &mut HashSet::new(), &captured);
                lambdas(&mut lam.body);
            }
        }
        ExprKind::Comp(comp) => {
            if let Some(comp) = std::rc::Rc::get_mut(comp) {
                for clause in &mut comp.clauses {
                    match clause {
                        CompClause::For { iter, .. } => lambdas(iter),
                        CompClause::If(c) => lambdas(c),
                    }
                }
                lambdas(&mut comp.body);
                if let Some(v) = &mut comp.value {
                    lambdas(v);
                }
            }
        }
        _ => children_mut(e, &mut lambdas),
    }
}

fn wheres(w: &mut Wheres) {
    for c in &mut w.consts {
        if let Some(c) = std::rc::Rc::get_mut(c) {
            constant(c);
        }
    }
    for (f, _) in &mut w.fns {
        if let Some(f) = std::rc::Rc::get_mut(f) {
            function(f);
        }
    }
}

/// Adds to `out` the slots of the frame `level` frames out from `e` that
/// `e` refers to from code that may run later (`deferred`).
fn references(e: &Expr, level: usize, deferred: bool, out: &mut HashSet<usize>) {
    match &e.kind {
        ExprKind::Local { depth, slot, .. } => {
            if deferred && *depth == level {
                out.insert(*slot);
            }
        }
        ExprKind::Lambda(lam) => references(&lam.body, level + 1, true, out),
        ExprKind::Comp(comp) => {
            // Each `for` clause adds a frame for the clauses after it.
            let mut inner = level;
            for clause in &comp.clauses {
                match clause {
                    CompClause::For { iter, .. } => {
                        references(iter, inner, true, out);
                        inner += 1;
                    }
                    CompClause::If(c) => references(c, inner, true, out),
                }
            }
            references(&comp.body, inner, true, out);
            if let Some(v) = &comp.value {
                references(v, inner, true, out);
            }
        }
        _ => children(e, &mut |c| references(c, level, deferred, out)),
    }
}

/// `where` constants share the frame they are defined in but run lazily;
/// `where` functions have frames of their own.
fn where_references(w: &Wheres, level: usize, out: &mut HashSet<usize>) {
    for c in &w.consts {
        references(&c.body, level, true, out);
        where_references(&c.wheres, level, out);
    }
    for (f, _) in &w.fns {
        for clause in &f.clauses {
            if let Some(g) = &clause.guard {
                references(g, level + 1, true, out);
            }
            references(&clause.body, level + 1, true, out);
            where_references(&clause.wheres, level + 1, out);
        }
    }
}

/// Works backwards through `e` in evaluation order. On entry `live` holds
/// the locals that may be used after `e`; on exit, those that may be used
/// from the start of `e`. A use of a local that isn't live afterwards is
/// its last, and becomes a move.
fn mark(e: &mut Expr, live: &mut HashSet<usize>, captured: &HashSet<usize>) {
    match &mut e.kind {
        ExprKind::Local {
            depth: 0,
            slot,
            moved,
            ..
        } => {
            if !live.contains(slot) && !captured.contains(slot) {
                *moved = true;
            }
            live.insert(*slot);
        }
        ExprKind::Lit(_)
        | ExprKind::Name(_)
        | ExprKind::Local { .. }
        | ExprKind::Global(_)
        | ExprKind::Builtin(_)
        // Locals used in these are captured, so never moved.
        | ExprKind::Lambda(_)
        | ExprKind::Comp(_) => {}
        ExprKind::Interp(parts) => {
            for p in parts.iter_mut().rev() {
                if let InterpPart::Expr(x) = p {
                    mark(x, live, captured);
                }
            }
        }
        ExprKind::Tuple(xs) | ExprKind::List(xs) | ExprKind::Set(xs) => {
            for x in xs.iter_mut().rev() {
                mark(x, live, captured);
            }
        }
        ExprKind::Map(entries) => {
            for (k, v) in entries.iter_mut().rev() {
                mark(v, live, captured);
                mark(k, live, captured);
            }
        }
        // The function, then the arguments in order.
        ExprKind::Call { func, args, .. } => {
            for a in args.iter_mut().rev() {
                mark(&mut a.value, live, captured);
            }
            mark(func, live, captured);
        }
        ExprKind::TupleIndex(x, _) | ExprKind::Unary(_, x) => mark(x, live, captured),
        ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => {
            mark(b, live, captured);
            mark(a, live, captured);
        }
        // The right side may not run at all.
        ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => {
            let mut after_b = live.clone();
            mark(b, &mut after_b, captured);
            live.extend(after_b);
            mark(a, live, captured);
        }
        // Later operands may be skipped, which only removes uses.
        ExprKind::Compare(first, rest) => {
            for (_, x) in rest.iter_mut().rev() {
                mark(x, live, captured);
            }
            mark(first, live, captured);
        }
        ExprKind::If(c, a, b) => {
            let mut before_a = live.clone();
            mark(a, &mut before_a, captured);
            mark(b, live, captured);
            live.extend(before_a);
            mark(c, live, captured);
        }
        // Arms are tried in order: a failed pattern or guard goes on to the
        // next arm, so each arm's test is followed by its body or by the
        // arms after it.
        ExprKind::Match(scrutinee, arms) => {
            let after = live.clone();
            let mut later_arms = after.clone();
            for arm in arms.iter_mut().rev() {
                let mut before_arm = after.clone();
                mark(&mut arm.body, &mut before_arm, captured);
                before_arm.extend(later_arms);
                if let Some(g) = &arm.guard {
                    uses(g, &mut before_arm);
                }
                later_arms = before_arm;
            }
            *live = later_arms;
            mark(scrutinee, live, captured);
        }
        ExprKind::Range { lo, hi, .. } => {
            if let Some(hi) = hi {
                mark(hi, live, captured);
            }
            if let Some(lo) = lo {
                mark(lo, live, captured);
            }
        }
    }
}

/// Adds the frame's own locals used in `e` to `out`.
fn uses(e: &Expr, out: &mut HashSet<usize>) {
    if let ExprKind::Local { depth: 0, slot, .. } = &e.kind {
        out.insert(*slot);
    }
    children(e, &mut |c| uses(c, out));
}

/// Calls `f` on each direct subexpression of `e` in the same frame (not
/// inside lambdas or comprehensions).
fn children(e: &Expr, f: &mut impl FnMut(&Expr)) {
    match &e.kind {
        ExprKind::Lit(_)
        | ExprKind::Name(_)
        | ExprKind::Local { .. }
        | ExprKind::Global(_)
        | ExprKind::Builtin(_)
        | ExprKind::Lambda(_)
        | ExprKind::Comp(_) => {}
        ExprKind::Interp(parts) => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    f(x);
                }
            }
        }
        ExprKind::Tuple(xs) | ExprKind::List(xs) | ExprKind::Set(xs) => xs.iter().for_each(f),
        ExprKind::Map(entries) => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprKind::Call { func, args, .. } => {
            f(func);
            for a in args {
                f(&a.value);
            }
        }
        ExprKind::TupleIndex(x, _) | ExprKind::Unary(_, x) => f(x),
        ExprKind::Index(a, b)
        | ExprKind::Binary(_, a, b)
        | ExprKind::And(a, b)
        | ExprKind::Or(a, b)
        | ExprKind::Coalesce(a, b) => {
            f(a);
            f(b);
        }
        ExprKind::Compare(first, rest) => {
            f(first);
            for (_, x) in rest {
                f(x);
            }
        }
        ExprKind::If(c, a, b) => {
            f(c);
            f(a);
            f(b);
        }
        ExprKind::Match(scrutinee, arms) => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    f(g);
                }
                f(&arm.body);
            }
        }
        ExprKind::Range { lo, hi, .. } => {
            if let Some(lo) = lo {
                f(lo);
            }
            if let Some(hi) = hi {
                f(hi);
            }
        }
    }
}

/// Like [`children`], but mutable.
fn children_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match &mut e.kind {
        ExprKind::Lit(_)
        | ExprKind::Name(_)
        | ExprKind::Local { .. }
        | ExprKind::Global(_)
        | ExprKind::Builtin(_)
        | ExprKind::Lambda(_)
        | ExprKind::Comp(_) => {}
        ExprKind::Interp(parts) => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    f(x);
                }
            }
        }
        ExprKind::Tuple(xs) | ExprKind::List(xs) | ExprKind::Set(xs) => xs.iter_mut().for_each(f),
        ExprKind::Map(entries) => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprKind::Call { func, args, .. } => {
            f(func);
            for a in args {
                f(&mut a.value);
            }
        }
        ExprKind::TupleIndex(x, _) | ExprKind::Unary(_, x) => f(x),
        ExprKind::Index(a, b)
        | ExprKind::Binary(_, a, b)
        | ExprKind::And(a, b)
        | ExprKind::Or(a, b)
        | ExprKind::Coalesce(a, b) => {
            f(a);
            f(b);
        }
        ExprKind::Compare(first, rest) => {
            f(first);
            for (_, x) in rest {
                f(x);
            }
        }
        ExprKind::If(c, a, b) => {
            f(c);
            f(a);
            f(b);
        }
        ExprKind::Match(scrutinee, arms) => {
            f(scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    f(g);
                }
                f(&mut arm.body);
            }
        }
        ExprKind::Range { lo, hi, .. } => {
            if let Some(lo) = lo {
                f(lo);
            }
            if let Some(hi) = hi {
                f(hi);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::ExprKind;
    use crate::resolve::GlobalDef;
    use crate::runner::compile;

    /// The names of the moved locals in function `name`'s first clause, in
    /// order of appearance.
    fn moved(src: &str, name: &str) -> Vec<String> {
        let prog = compile(src.to_string()).expect("compiles");
        let i = prog.global(name).expect("defined");
        let GlobalDef::Func(f) = &prog.globals[i] else {
            panic!("not a function");
        };
        let mut out = Vec::new();
        collect(&f.clauses[0].body, &mut out);
        out
    }

    fn collect(e: &crate::ast::Expr, out: &mut Vec<String>) {
        if let ExprKind::Local {
            name, moved: true, ..
        } = &e.kind
        {
            out.push(name.to_string());
        }
        super::children(e, &mut |c| collect(c, out));
    }

    #[test]
    fn last_uses_move() {
        // In the else branch, `seen.get(k)` is evaluated after the receiver
        // of `set`, so it is the last use: it takes the frame's reference,
        // which leaves `set` holding the only one. The then branch has its
        // own last use, and `k - 1` is the last use of `k`.
        let src = "f(seen, k) = if k == 0 then seen else f(seen.set(k, seen.get(k)), k - 1)";
        assert_eq!(moved(src, "f"), ["seen", "seen", "k"]);
    }

    #[test]
    fn captured_locals_never_move() {
        let src = "f(m) = (m.set(1, 2), x => m)";
        assert_eq!(moved(src, "f"), Vec::<String>::new());
        let src = "f(m) = m.set(1, old)\n  where\n    old = m.get(1)";
        assert!(!moved(src, "f").contains(&"m".to_string()));
    }

    #[test]
    fn a_use_before_a_later_one_does_not_move() {
        let src = "f(m) = (m.set(1, 2), m)";
        assert_eq!(moved(src, "f"), ["m"]);
    }
}
