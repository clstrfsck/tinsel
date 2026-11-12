//! Directions and search (SPEC 9.7).

use super::{Args, Builtin, constant, fixed, func, type_err};
use crate::error::Result;
use crate::eval::{Interp, short};
use crate::fxhash::{FxHashMap, FxHashSet};
use crate::int::Int;
use crate::value::{self, V, Value};
use crate::value::{ValueMap, ValueSet};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

pub const BUILTINS: &[Builtin] = &[
    constant("N", |_, _| Ok(step(0, -1))),
    constant("E", |_, _| Ok(step(1, 0))),
    constant("S", |_, _| Ok(step(0, 1))),
    constant("W", |_, _| Ok(step(-1, 0))),
    constant("DIRS4", |_, _| Ok(steps(&DIRS4))),
    constant("DIRS8", |_, _| Ok(steps(&DIRS8))),
    func("cw", cw),
    func("ccw", ccw),
    fixed("manhattan", 2, manhattan),
    fixed("bfs", 2, bfs),
    fixed("dijkstra", 2, dijkstra),
    fixed("bfs_from", 2, bfs_from),
    fixed("dijkstra_from", 2, dijkstra_from),
    fixed("components", 2, components),
];

// The same orders as `nbrs4` and `nbrs8` (SPEC 9.3).
const DIRS4: [(i64, i64); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
const DIRS8: [(i64, i64); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

fn step(x: i64, y: i64) -> Value {
    Value::pair(Value::int(x), Value::int(y))
}

fn steps(dirs: &[(i64, i64)]) -> Value {
    Value::list(dirs.iter().map(|&(x, y)| step(x, y)).collect::<Vec<_>>())
}

// --- Directions ---------------------------------------------------------------

fn xy(args: &Args, fname: &str) -> Result<(Int, Int)> {
    match args.pos[0].view() {
        V::Tuple(t) if t.len() == 2 => match (t[0].view(), t[1].view()) {
            (V::Int(x), V::Int(y)) => Ok((x, y)),
            _ => Err(args.err(format!(
                "`{fname}` expects an (x, y) of ints, found {}",
                short(&args.pos[0])
            ))),
        },
        other => Err(type_err(args, fname, "an (x, y) tuple", other)),
    }
}

/// Clockwise on screen, where y increases downwards: (x, y) -> (-y, x).
fn cw(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("cw", 1, 1, &[])?;
    let (x, y) = xy(&args, "cw")?;
    Ok(Value::tuple(vec![
        Value::from_int(y.neg()),
        Value::from_int(x.clone()),
    ]))
}

/// (x, y) -> (y, -x).
fn ccw(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("ccw", 1, 1, &[])?;
    let (x, y) = xy(&args, "ccw")?;
    Ok(Value::tuple(vec![
        Value::from_int(y.clone()),
        Value::from_int(x.neg()),
    ]))
}

fn manhattan(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("manhattan", 2, 2, &[])?;
    let (V::Tuple(a), V::Tuple(b)) = (args.pos[0].view(), args.pos[1].view()) else {
        return Err(args.err(format!(
            "`manhattan` expects two tuples, found {} and {}",
            args.pos[0].type_name(),
            args.pos[1].type_name()
        )));
    };
    if a.len() != b.len() {
        return Err(args.err(format!(
            "`manhattan` needs tuples of the same length, found {} and {}",
            a.len(),
            b.len()
        )));
    }
    let mut total = Int::Small(0);
    for (p, q) in a.iter().zip(b.iter()) {
        let (V::Int(p), V::Int(q)) = (p.view(), q.view()) else {
            return Err(args.err(format!(
                "`manhattan` expects ints, found {} and {}",
                p.type_name(),
                q.type_name()
            )));
        };
        let d = p.sub(&q).map_err(|m| args.err(m))?;
        total = total.add(&d.abs()).map_err(|m| args.err(m))?;
    }
    Ok(Value::from_int(total))
}

// --- Search -------------------------------------------------------------------

/// The states the search has found, numbered in the order they were found.
/// `parent` links give paths; distances are kept by the caller.
struct Found {
    states: Vec<Value>,
    index: FxHashMap<Value, usize>,
    parent: Vec<Option<usize>>,
}

impl Found {
    /// The start states, numbered in order; repeats are dropped.
    fn new(starts: Vec<Value>) -> Found {
        let mut found = Found {
            states: Vec::new(),
            index: FxHashMap::default(),
            parent: Vec::new(),
        };
        for s in starts {
            if !found.index.contains_key(&s) {
                found.index.insert(s.clone(), found.states.len());
                found.states.push(s);
                found.parent.push(None);
            }
        }
        found
    }

    /// The number of a state, adding it if it is new: (number, is new).
    fn add(&mut self, state: Value, parent: usize) -> (usize, bool) {
        if let Some(&i) = self.index.get(&state) {
            return (i, false);
        }
        let i = self.states.len();
        self.index.insert(state.clone(), i);
        self.states.push(state);
        self.parent.push(Some(parent));
        (i, true)
    }

    /// The states from the start to `i`, inclusive.
    fn path(&self, mut i: usize) -> Value {
        let mut path = vec![self.states[i].clone()];
        while let Some(p) = self.parent[i] {
            path.push(self.states[p].clone());
            i = p;
        }
        path.reverse();
        Value::list(path.into_iter().collect::<crate::value::List>())
    }

    /// A map from each state to its distance.
    fn distances(&self, dist: &[Int], args: &Args) -> Result<Value> {
        let mut out = ValueMap::new();
        for (s, d) in self.states.iter().zip(dist) {
            value::map_insert(&mut out, s.clone(), Value::from_int(d.clone()))
                .map_err(|m| args.err(m))?;
        }
        Ok(Value::map(out))
    }
}

/// The `goal:` argument: a predicate, or a value to compare with.
fn is_goal(
    it: &mut Interp<'_>,
    goal: &Value,
    state: &Value,
    args: &Args,
    fname: &str,
) -> Result<bool> {
    if let V::Func(_) = goal.view() {
        match (it.call(goal, vec![state.clone()], args.span)?).view() {
            V::Bool(b) => Ok(b),
            other => Err(args.err(format!(
                "the `goal` predicate passed to `{fname}` must return a bool, but returned {}",
                other.type_name()
            ))),
        }
    } else {
        Ok(state == goal)
    }
}

/// Calls `next(state)` and collects the states (or pairs) it gives.
fn neighbours(
    it: &mut Interp<'_>,
    next: &Value,
    state: &Value,
    args: &Args,
    fname: &str,
) -> Result<Vec<Value>> {
    let result = it.call(next, vec![state.clone()], args.span)?;
    it.to_vec(&result).map_err(|e| {
        args.err(format!(
            "`next` passed to `{fname}` must return a collection of states: {}",
            e.into_msg()
        ))
    })
}

/// The `goal:` and `all_paths:` arguments.
fn options(args: &Args, fname: &str) -> Result<(Option<Value>, bool)> {
    let goal = args.named("goal").cloned();
    let all = match (args.named("all_paths")).map(Value::view) {
        None | Some(V::Bool(false)) => false,
        Some(V::Bool(true)) => true,
        Some(other) => return Err(type_err(args, fname, "a bool for `all_paths`", other)),
    };
    if all && goal.is_none() {
        return Err(args.err(format!("`{fname}` needs `goal:` to use `all_paths:`")));
    }
    Ok((goal, all))
}

/// For `all_paths: true`: every predecessor of each state at its best
/// distance, and the goals found at the shortest goal distance. Does nothing
/// otherwise, so ordinary searches don't pay for it.
struct Best {
    all: bool,
    preds: Vec<Vec<usize>>,
    dist: Option<Int>,
    goals: Vec<usize>,
}

impl Best {
    fn new(all: bool, starts: usize) -> Best {
        Best {
            all,
            preds: if all {
                vec![Vec::new(); starts]
            } else {
                Vec::new()
            },
            dist: None,
            goals: Vec::new(),
        }
    }

    /// A new state, found from `i`.
    fn first_pred(&mut self, i: usize) {
        if self.all {
            self.preds.push(vec![i]);
        }
    }

    /// A better way into `j`, from `i`.
    fn only_pred(&mut self, j: usize, i: usize) {
        if self.all {
            self.preds[j] = vec![i];
        }
    }

    /// Another way into `j` at its best distance, from `i`.
    fn another_pred(&mut self, j: usize, i: usize) {
        if self.all && !self.preds[j].contains(&i) {
            self.preds[j].push(i);
        }
    }

    fn goal(&mut self, i: usize, d: &Int) {
        self.dist.get_or_insert_with(|| d.clone());
        self.goals.push(i);
    }

    /// Once a goal is found, states beyond its distance can't matter.
    fn finished(&self, d: &Int) -> bool {
        self.dist.as_ref().is_some_and(|best| d > best)
    }

    /// Every state on a shortest path to one of the goals.
    fn states(&self, found: &Found, args: &Args) -> Result<Value> {
        let mut seen = vec![false; found.states.len()];
        let mut todo = self.goals.clone();
        let mut out = ValueSet::new();
        while let Some(i) = todo.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            value::set_insert(&mut out, found.states[i].clone()).map_err(|m| args.err(m))?;
            todo.extend(&self.preds[i]);
        }
        Ok(Value::set(out))
    }
}

/// The result once the search has run out of states to try.
fn finish(found: &Found, dist: &[Int], has_goal: bool, best: &Best, args: &Args) -> Result<Value> {
    match (&best.dist, has_goal) {
        (Some(d), _) => Ok(Value::tuple(vec![
            Value::from_int(d.clone()),
            best.states(found, args)?,
        ])),
        (None, true) => Ok(Value::NONE),
        (None, false) => found.distances(dist, args),
    }
}

fn bfs(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("bfs", 2, 2, &["goal", "all_paths"])?;
    let start = vec![args.pos[0].clone()];
    breadth_first(it, &args, "bfs", start)
}

fn bfs_from(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("bfs_from", 2, 2, &["goal", "all_paths"])?;
    let starts = it.to_vec(&args.pos[0])?;
    breadth_first(it, &args, "bfs_from", starts)
}

fn breadth_first(
    it: &mut Interp<'_>,
    args: &Args,
    fname: &str,
    starts: Vec<Value>,
) -> Result<Value> {
    let next = args.pos[1].clone();
    let (goal, all) = options(args, fname)?;
    let mut found = Found::new(starts);
    let mut dist = vec![Int::Small(0); found.states.len()];
    let mut best = Best::new(all, found.states.len());
    let mut queue: VecDeque<usize> = (0..found.states.len()).collect();
    while let Some(i) = queue.pop_front() {
        if best.finished(&dist[i]) {
            break;
        }
        let state = found.states[i].clone();
        if let Some(g) = &goal
            && is_goal(it, g, &state, args, fname)?
        {
            if !all {
                return Ok(Value::tuple(vec![
                    Value::from_int(dist[i].clone()),
                    found.path(i),
                ]));
            }
            best.goal(i, &dist[i]);
        }
        let d = dist[i].add(&Int::Small(1)).map_err(|m| args.err(m))?;
        for n in neighbours(it, &next, &state, args, fname)? {
            let (j, new) = found.add(n, i);
            if new {
                dist.push(d.clone());
                best.first_pred(i);
                queue.push_back(j);
            } else if dist[j] == d {
                best.another_pred(j, i);
            }
        }
    }
    finish(&found, &dist, goal.is_some(), &best, args)
}

fn dijkstra(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("dijkstra", 2, 2, &["goal", "all_paths"])?;
    let start = vec![args.pos[0].clone()];
    cheapest_first(it, &args, "dijkstra", start)
}

fn dijkstra_from(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("dijkstra_from", 2, 2, &["goal", "all_paths"])?;
    let starts = it.to_vec(&args.pos[0])?;
    cheapest_first(it, &args, "dijkstra_from", starts)
}

fn cheapest_first(
    it: &mut Interp<'_>,
    args: &Args,
    fname: &str,
    starts: Vec<Value>,
) -> Result<Value> {
    let next = args.pos[1].clone();
    let (goal, all) = options(args, fname)?;
    let mut found = Found::new(starts);
    let n = found.states.len();
    let mut dist = vec![Int::Small(0); n];
    let mut done = vec![false; n];
    let mut best = Best::new(all, n);
    // (distance, order pushed, state): ties go to whichever was found first,
    // and the starts are pushed in order.
    let mut heap: BinaryHeap<_> = (0..n)
        .map(|i| Reverse((Int::Small(0), i as u64, i)))
        .collect();
    let mut pushed = n as u64;
    while let Some(Reverse((d, _, i))) = heap.pop() {
        if done[i] {
            continue;
        }
        if best.finished(&d) {
            break;
        }
        done[i] = true;
        let state = found.states[i].clone();
        if let Some(g) = &goal
            && is_goal(it, g, &state, args, fname)?
        {
            if !all {
                return Ok(Value::pair(Value::from_int(d), found.path(i)));
            }
            best.goal(i, &d);
        }
        for pair in neighbours(it, &next, &state, args, fname)? {
            let (n, cost) = match pair.view() {
                V::Tuple(t) if t.len() == 2 => match t[1].view() {
                    V::Int(c) if c.signum() >= 0 => (t[0].clone(), c.clone()),
                    V::Int(c) => {
                        return Err(
                            args.err(format!("`{fname}` costs must be 0 or more, found {c}"))
                        );
                    }
                    other => return Err(type_err(args, fname, "int costs", other)),
                },
                _ => {
                    return Err(args.err(format!(
                        "`next` passed to `{fname}` must give (state, cost) pairs, found {}",
                        short(&pair)
                    )));
                }
            };
            let nd = d.add(&cost).map_err(|m| args.err(m))?;
            let (j, new) = found.add(n, i);
            if new {
                dist.push(nd.clone());
                done.push(false);
                best.first_pred(i);
            } else if nd == dist[j] {
                // Another way in at the same cost; with zero costs, even to
                // a state that's already done.
                best.another_pred(j, i);
                continue;
            } else if done[j] || nd > dist[j] {
                continue;
            } else {
                dist[j] = nd.clone();
                found.parent[j] = Some(i);
                best.only_pred(j, i);
            }
            pushed += 1;
            heap.push(Reverse((nd, pushed, j)));
        }
    }
    finish(&found, &dist, goal.is_some(), &best, args)
}

fn components(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("components", 2, 2, &[])?;
    let next = args.pos[1].clone();
    // Sorted, so each component is found from its smallest member.
    let mut nodes = ValueSet::new();
    for n in it.to_vec(&args.pos[0])? {
        value::set_insert(&mut nodes, n).map_err(|m| args.err(m))?;
    }
    let mut assigned: FxHashSet<Value> = FxHashSet::default();
    let mut out = Vec::new();
    for start in nodes.iter() {
        if assigned.contains(start) {
            continue;
        }
        let mut component = ValueSet::new();
        assigned.insert(start.clone());
        let mut queue = VecDeque::from([start.clone()]);
        while let Some(s) = queue.pop_front() {
            for n in neighbours(it, &next, &s, &args, "components")? {
                if nodes.contains(&n) && assigned.insert(n.clone()) {
                    queue.push_back(n);
                }
            }
            component.insert(s);
        }
        out.push(Value::set(component));
    }
    Ok(Value::list(out))
}
