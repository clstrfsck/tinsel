//! Grids and positions (SPEC 9.3). Positions are `(x, y)` tuples with `y`
//! increasing downwards.

use super::strings::split_lines;
use super::{Args, Builtin, fixed, func, type_err};
use crate::error::Result;
use crate::eval::Interp;
use crate::int::Int;
use crate::value::{self, Grid, V, Value};
use imbl::OrdSet;
use std::rc::Rc;

pub const BUILTINS: &[Builtin] = &[
    func("grid", grid),
    func("width", width),
    func("height", height),
    func("positions", positions),
    fixed("where", 2, where_),
    fixed("find", 2, find),
    func("nbrs4", nbrs4),
    func("nbrs8", nbrs8),
    func("rows", rows),
    func("cols", cols),
    func("rot", rot),
    func("show", show),
];

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

fn xy(p: &Value) -> Option<(i64, i64)> {
    match p.view() {
        V::Tuple(t) if t.len() == 2 => match (t[0].view(), t[1].view()) {
            (V::Int(Int::Small(x)), V::Int(Int::Small(y))) => Some((x, y)),
            _ => None,
        },
        _ => None,
    }
}

fn pos(x: i64, y: i64) -> Value {
    Value::pair(Value::int(x), Value::int(y))
}

/// Cell index of a position, if it is inside the grid.
pub fn position(g: &Grid, p: &Value) -> Option<usize> {
    let (x, y) = xy(p)?;
    if (0..g.width as i64).contains(&x) && (0..g.height as i64).contains(&y) {
        Some(y as usize * g.width + x as usize)
    } else {
        None
    }
}

/// `g.set(p, v)`, in place. When nothing else holds the grid, only the
/// cell is written, rather than copying every cell (SPEC 4.1).
pub fn set_cell(g: &mut Rc<Grid>, p: &Value, v: Value) -> std::result::Result<(), String> {
    let i = position(g, p).ok_or_else(|| format!("position {} is outside the grid", p.repr()))?;
    if let Some(cell) = Rc::make_mut(g).cells.get_mut(i) {
        *cell = v;
    }
    Ok(())
}

fn grid_arg<'a>(args: &'a Args, fname: &str) -> Result<&'a Grid> {
    match args.pos[0].view() {
        V::Grid(g) => Ok(g),
        other => Err(type_err(args, fname, "a grid", other)),
    }
}

fn grid(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("grid", 1, 2, &[])?;
    let V::Str(s) = args.pos[0].view() else {
        return Err(type_err(&args, "grid", "a str", &args.pos[0]));
    };
    let lines = split_lines(s);
    let width = lines.first().map_or(0, |l| l.chars().count());
    let mut cells = Vec::with_capacity(width * lines.len());
    for (y, line) in lines.iter().enumerate() {
        let n = line.chars().count();
        if n != width {
            return Err(args.err(format!(
                "grid row {y} has length {n}, but row 0 has length {width}"
            )));
        }
        for c in line.chars() {
            cells.push(match args.pos.get(1) {
                Some(f) => it.call(f, vec![Value::char(c)], args.span)?,
                None => Value::char(c),
            });
        }
    }
    Ok(Value::grid_rc(Rc::new(Grid {
        width,
        height: lines.len(),
        cells,
    })))
}

fn width(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("width", 1, 1, &[])?;
    Ok(Value::int(grid_arg(&args, "width")?.width as i64))
}

fn height(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("height", 1, 1, &[])?;
    Ok(Value::int(grid_arg(&args, "height")?.height as i64))
}

fn all_positions(g: &Grid) -> impl Iterator<Item = (usize, Value)> + '_ {
    (0..g.cells.len()).map(|i| (i, pos((i % g.width) as i64, (i / g.width) as i64)))
}

fn positions(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("positions", 1, 1, &[])?;
    let g = grid_arg(&args, "positions")?;
    Ok(Value::list(
        all_positions(g).map(|(_, p)| p).collect::<Vec<_>>(),
    ))
}

/// Does the cell match `v`, which is a value or a predicate?
fn cell_matches(it: &mut Interp<'_>, cell: &Value, v: &Value, args: &Args) -> Result<bool> {
    if let V::Func(_) = v.view() {
        match (it.call(v, vec![cell.clone()], args.span)?).view() {
            V::Bool(b) => Ok(b),
            other => Err(args.err(format!(
                "the predicate must return a bool, but returned {}",
                other.type_name()
            ))),
        }
    } else {
        Ok(cell == v)
    }
}

fn where_(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("where", 2, 2, &[])?;
    let g = grid_arg(&args, "where")?.clone();
    let mut out = OrdSet::new();
    for (i, p) in all_positions(&g) {
        if cell_matches(it, &g.cells[i], &args.pos[1], &args)? {
            value::set_insert(&mut out, p).map_err(|m| args.err(m))?;
        }
    }
    Ok(Value::set(out))
}

fn find(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("find", 2, 2, &[])?;
    let g = grid_arg(&args, "find")?.clone();
    for (i, p) in all_positions(&g) {
        if cell_matches(it, &g.cells[i], &args.pos[1], &args)? {
            return Ok(p);
        }
    }
    Ok(Value::NONE)
}

/// `nbrs(p)` (unbounded) or `g.nbrs(p)` (inside the grid only).
fn nbrs(args: &Args, fname: &str, dirs: &[(i64, i64)]) -> Result<Value> {
    args.check(fname, 1, 2, &[])?;
    let (g, p) = match args.pos.as_slice() {
        [p] => (None, p),
        [g, p] => match g.view() {
            V::Grid(g) => (Some(g), p),
            other => return Err(type_err(args, fname, "a grid", other)),
        },
        _ => return Err(args.err("internal: nbrs")),
    };
    let bad = || {
        args.err(format!(
            "`{fname}` expects an (x, y) position, found {}",
            p.repr()
        ))
    };
    let V::Tuple(t) = p.view() else {
        return Err(bad());
    };
    let (V::Int(x), V::Int(y)) = (
        t.first().map_or(V::None, Value::view),
        t.get(1).map_or(V::None, Value::view),
    ) else {
        return Err(bad());
    };
    if t.len() != 2 {
        return Err(bad());
    }
    // Positions have no bounds, so the sums are ints of any size: an
    // `i64` coordinate at the edge has neighbours beyond it.
    let mut out = Vec::with_capacity(dirs.len());
    for &(dx, dy) in dirs {
        let q = match (&x, &y) {
            (Int::Small(x), Int::Small(y))
                if let (Some(qx), Some(qy)) = (x.checked_add(dx), y.checked_add(dy)) =>
            {
                pos(qx, qy)
            }
            _ => {
                let qx = x.add(&Int::Small(dx)).map_err(|m| args.err(m))?;
                let qy = y.add(&Int::Small(dy)).map_err(|m| args.err(m))?;
                Value::pair(Value::from_int(qx), Value::from_int(qy))
            }
        };
        if g.is_none_or(|g| position(g, &q).is_some()) {
            out.push(q);
        }
    }
    Ok(Value::list(out))
}

fn nbrs4(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    nbrs(&args, "nbrs4", &DIRS4)
}

fn nbrs8(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    nbrs(&args, "nbrs8", &DIRS8)
}

/// A grid's lines of cells, as strings for a char grid and lists otherwise.
fn lines_of(g: &Grid, lines: Vec<Vec<Value>>) -> Value {
    let all_chars = g.cells.iter().all(|c| matches!(c.view(), V::Char(_)));
    let line = |cells: Vec<Value>| {
        if all_chars {
            Value::from_str_rc(cells.iter().map(Value::display).collect::<String>().into())
        } else {
            Value::list(cells.into_iter().collect::<crate::value::List>())
        }
    };
    Value::list(lines.into_iter().map(line).collect::<Vec<_>>())
}

fn cols(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("cols", 1, 1, &[])?;
    let g = grid_arg(&args, "cols")?;
    let cols = (0..g.width)
        .map(|x| {
            (0..g.height)
                .map(|y| g.cells[y * g.width + x].clone())
                .collect()
        })
        .collect();
    Ok(lines_of(g, cols))
}

/// The cell at `(x, y)` moves to `(y, x)`.
pub fn transpose_grid(g: &Grid) -> Grid {
    let (w, h) = (g.width, g.height);
    let mut cells = Vec::with_capacity(g.cells.len());
    for y in 0..w {
        for x in 0..h {
            cells.push(g.cells[x * w + y].clone());
        }
    }
    Grid {
        width: h,
        height: w,
        cells,
    }
}

/// One quarter turn clockwise: the new `(x, y)` is the old `(y, h - 1 - x)`.
fn turn(g: &Grid) -> Grid {
    let (w, h) = (g.width, g.height);
    let mut cells = Vec::with_capacity(g.cells.len());
    for y in 0..w {
        for x in 0..h {
            cells.push(g.cells[(h - 1 - x) * w + y].clone());
        }
    }
    Grid {
        width: h,
        height: w,
        cells,
    }
}

/// `g.rot` or `g.rot(n)`: quarter turns clockwise, anticlockwise if negative.
fn rot(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("rot", 1, 2, &[])?;
    let g = grid_arg(&args, "rot")?;
    let turns = match args.pos.get(1).map(Value::view) {
        None => 1,
        // Taken modulo 4, which is undefined for `inf`.
        Some(V::Int(n)) => n
            .rem(&crate::int::Int::Small(4))
            .map_err(|m| args.err(format!("`rot` needs a finite number of turns: {m}")))?
            .as_i64()
            .unwrap_or(0),
        Some(other) => return Err(type_err(&args, "rot", "an int number of turns", other)),
    };
    let mut out = g.clone();
    for _ in 0..turns {
        out = turn(&out);
    }
    Ok(Value::grid_rc(Rc::new(out)))
}

/// Each row's cells. Counts rows by the height, so a zero-width grid still
/// has its (empty) rows.
fn row_slices(g: &Grid) -> impl Iterator<Item = &[Value]> {
    (0..g.height).map(move |y| {
        g.cells
            .get(y * g.width..(y + 1) * g.width)
            .unwrap_or_default()
    })
}

fn rows(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("rows", 1, 1, &[])?;
    let g = grid_arg(&args, "rows")?;
    Ok(lines_of(g, row_slices(g).map(<[Value]>::to_vec).collect()))
}

fn show(_: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("show", 1, 1, &[])?;
    let g = grid_arg(&args, "show")?;
    let mut s = String::new();
    for row in row_slices(g) {
        for c in row {
            s.push_str(&c.display());
        }
        s.push('\n');
    }
    Ok(Value::from_str_rc(s.into()))
}
