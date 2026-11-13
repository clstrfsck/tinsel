//! Standard library (SPEC 9), grouped as in the spec.

pub mod collections;
pub mod grids;
pub mod intervals;
pub mod iteration;
pub mod numbers;
pub mod search;
pub mod strings;

use crate::ast::Name;
use crate::error::{Error, Result, Span};
use crate::eval::Interp;
use crate::value::{Typed, Value};
use std::sync::LazyLock;

pub type BuiltinFn = fn(&mut Interp<'_>, Args) -> Result<Value>;

#[derive(Clone, Copy)]
pub struct Builtin {
    pub name: &'static str,
    pub f: BuiltinFn,
    /// A constant such as `input`, evaluated on reference rather than called.
    pub constant: bool,
    /// Set when the function always takes exactly this many (two or more)
    /// positional arguments, so a single tuple argument is spread (SPEC 5.2).
    pub arity: Option<usize>,
}

pub const fn func(name: &'static str, f: BuiltinFn) -> Builtin {
    Builtin {
        name,
        f,
        constant: false,
        arity: None,
    }
}

/// A function that always takes exactly `n` positional arguments.
pub const fn fixed(name: &'static str, n: usize, f: BuiltinFn) -> Builtin {
    Builtin {
        name,
        f,
        constant: false,
        arity: Some(n),
    }
}

pub const fn constant(name: &'static str, f: BuiltinFn) -> Builtin {
    Builtin {
        name,
        f,
        constant: true,
        arity: None,
    }
}

pub static BUILTINS: LazyLock<Vec<Builtin>> = LazyLock::new(|| {
    let mut all = Vec::new();
    all.extend_from_slice(numbers::BUILTINS);
    all.extend_from_slice(strings::BUILTINS);
    all.extend_from_slice(collections::BUILTINS);
    all.extend_from_slice(iteration::BUILTINS);
    all.extend_from_slice(grids::BUILTINS);
    all.extend_from_slice(search::BUILTINS);
    all.extend_from_slice(intervals::BUILTINS);
    all.push(constant("input", input));
    all.push(func("dbg", dbg));
    all
});

pub fn lookup(name: &str) -> Option<usize> {
    BUILTINS.iter().position(|b| b.name == name)
}

/// Arguments to a builtin call.
pub struct Args {
    pub pos: Vec<Value>,
    pub named: Vec<(Name, Value)>,
    pub span: Span,
}

impl Args {
    /// Checks the number of positional arguments and that there are no
    /// unexpected named ones.
    pub fn check(&self, name: &str, min: usize, max: usize, allowed_named: &[&str]) -> Result<()> {
        let n = self.pos.len();
        if n < min || n > max {
            let expected = if min == max {
                format!("{min}")
            } else if max == usize::MAX {
                format!("at least {min}")
            } else {
                format!("{min} to {max}")
            };
            return Err(Error::new(
                format!("`{name}` takes {expected} arguments, but was given {n}"),
                self.span,
            ));
        }
        for (k, _) in &self.named {
            if !allowed_named.contains(&&**k) {
                return Err(Error::new(
                    format!("`{name}` has no argument named `{k}`"),
                    self.span,
                ));
            }
        }
        Ok(())
    }

    pub fn named(&self, key: &str) -> Option<&Value> {
        self.named.iter().find(|(k, _)| &**k == key).map(|(_, v)| v)
    }

    pub fn err(&self, msg: impl Into<String>) -> Error {
        Error::new(msg, self.span)
    }
}

/// `"expected X, found Y"` error for a builtin argument.
pub fn type_err(args: &Args, fname: &str, expected: &str, found: impl Typed) -> Error {
    args.err(format!(
        "`{fname}` expects {expected}, found {}",
        found.type_name()
    ))
}

fn input(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    match &it.input {
        Ok(s) => Ok(Value::from_str_rc(s.clone())),
        Err(why) => {
            it.input_missing = true;
            Err(args.err(why.clone()))
        }
    }
}

/// `dbg(x)` and `dbg(label, x)` (SPEC 9.6): prints where it was called,
/// then the label or the source text of `x`, then `x` cut short.
fn dbg(it: &mut Interp<'_>, args: Args) -> Result<Value> {
    args.check("dbg", 1, 2, &[])?;
    // A part run again to record arguments for a stack trace (SPEC 8.3)
    // has already printed this.
    if it.capture.is_some() {
        return Ok(args.pos.into_iter().last().unwrap_or(Value::NONE));
    }
    let prog = it.prog;
    let at = crate::runner::location(&prog.path, &prog.src, &prog.files, args.span.start);
    let call = prog.src.get(args.span.start..args.span.end).unwrap_or("");
    let mut pos = args.pos;
    let v = pos.pop().unwrap_or(Value::NONE);
    match pos.pop() {
        Some(label) => eprintln!("{at}: {}: {}", label.display(), v.repr_brief()),
        None => eprintln!("{at}: {} = {}", dbg_text(call), v.repr_brief()),
    }
    Ok(v)
}

/// The text of `dbg`'s argument, from the text of the call: `dbg(x)` or
/// `x.dbg`. Runs of whitespace (as in a call split over lines) become one
/// space.
fn dbg_text(call: &str) -> String {
    let call = call.trim();
    let arg = call
        .strip_prefix("dbg(")
        .and_then(|c| c.strip_suffix(')'))
        .or_else(|| call.strip_suffix(".dbg"))
        .unwrap_or(call);
    arg.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::Interp;
    use crate::runner::compile;

    /// A function marked with a fixed arity must reject any other number of
    /// arguments with its own check, so the marking can't drift from the
    /// function (SPEC 5.2: only such functions spread tuples).
    #[test]
    fn fixed_arities_match_the_functions() {
        let prog = compile(String::new()).expect("compiles");
        let mut it = Interp::new(&prog, Err("no input".into()), &[]);
        for b in BUILTINS.iter() {
            let Some(n) = b.arity else { continue };
            let args = Args {
                pos: vec![Value::NONE; n + 1],
                named: Vec::new(),
                span: Span::default(),
            };
            let msg = match (b.f)(&mut it, args) {
                Err(e) => e.into_msg(),
                Ok(_) => String::from("no error"),
            };
            let expected = format!("`{}` takes {n} arguments, but was given {}", b.name, n + 1);
            assert_eq!(msg, expected, "arity marking of `{}`", b.name);
        }
    }
}
