//! Loading programs and running their checks: `test`, `test_error`,
//! `example` and `answers` (SPEC 2).

use crate::ast::{Check, CmpOp, ExprKind, Item, PatKind, RawDef};
use crate::error::{Error, LineIndex, SourceFile, Span, file_at};
use crate::eval::{Frame, Interp, short};
use crate::resolve::{Program, build_program};
use crate::value::Value;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Parses and resolves a program from source.
pub fn compile(src: String) -> Result<Program, Error> {
    let items = crate::parser::parse_program(&src)?;
    build_program(src, items)
}

/// Why a file failed to load, with whatever source was read, so that the
/// error can be shown in context.
#[derive(Debug)]
pub struct LoadError {
    pub src: Option<String>,
    pub files: Vec<SourceFile>,
    pub err: Error,
}

/// Reads and compiles a `.tin` file, along with the files it includes
/// (SPEC 2.8).
pub fn load(path: &Path) -> Result<Program, LoadError> {
    load_with(path, None)
}

/// The name of the constant that `tinsel eval` adds for its expression.
/// It isn't an identifier, so it can't clash with anything a program
/// defines, and the expression can't refer to it.
pub const EVAL_NAME: &str = "<expression>";

/// What the expression is parsed as the definition of, before it is
/// renamed to [`EVAL_NAME`].
const EVAL_PARSED_AS: &str = "__eval";

/// The file name shown for `tinsel eval`'s expression in messages.
pub const EVAL_FILE: &str = "<expression>";

/// [`load`], plus `expr` as the constant [`EVAL_NAME`], compiled in the
/// program's scope (SPEC 2.7, `tinsel eval`). It is kept as one more file,
/// named [`EVAL_FILE`] in messages.
pub fn load_with_expr(path: &Path, expr: &str) -> Result<Program, LoadError> {
    load_with(path, Some(expr))
}

fn load_with(path: &Path, expr: Option<&str>) -> Result<Program, LoadError> {
    let mut loader = Loader {
        src: String::new(),
        files: Vec::new(),
        stack: Vec::new(),
        seen: Vec::new(),
    };
    let items = loader
        .file(path, PathBuf::new(), None)
        .and_then(|mut items| {
            if let Some(expr) = expr {
                items.extend(loader.expr(expr)?);
            }
            Ok(items)
        });
    let fail = |loader: Loader, err| LoadError {
        src: (!loader.src.is_empty()).then_some(loader.src),
        files: loader.files,
        err,
    };
    let items = match items {
        Ok(items) => items,
        Err(e) => return Err(fail(loader, e)),
    };
    match build_program(loader.src.clone(), items) {
        Ok(mut prog) => {
            prog.files = loader.files;
            prog.path = path.to_path_buf();
            Ok(prog)
        }
        Err(e) => Err(fail(loader, e)),
    }
}

struct Loader {
    src: String,
    files: Vec<SourceFile>,
    /// Files being loaded, outermost first, to catch include cycles.
    stack: Vec<PathBuf>,
    /// Every file loaded so far, so that each is included only once.
    seen: Vec<PathBuf>,
}

impl Loader {
    /// Reads `path` (shown as `rel`, relative to the main file's directory)
    /// and returns its items, with its includes expanded. `included_at` is
    /// the span of the `include` that named it, or none for the main file.
    fn file(
        &mut self,
        path: &Path,
        rel: PathBuf,
        included_at: Option<Span>,
    ) -> Result<Vec<Item>, Error> {
        let cant_read = |e: std::io::Error| {
            let msg = format!("cannot read {}: {e}", path.display());
            match included_at {
                Some(span) => Error::new(msg, span),
                None => Error::msg(msg),
            }
        };
        let canonical = path.canonicalize().map_err(cant_read)?;
        let text = std::fs::read_to_string(path).map_err(cant_read)?;
        if !self.src.is_empty() && !self.src.ends_with('\n') {
            self.src.push('\n');
        }
        let base = self.src.len();
        self.src.push_str(&text);
        self.files.push(SourceFile {
            start: base,
            path: rel.clone(),
        });
        self.stack.push(canonical.clone());
        self.seen.push(canonical);

        let mut out = Vec::new();
        for item in crate::parser::parse_file(&self.src, base)? {
            match item {
                Item::Include { path: inc, span } => {
                    let dir = path.parent().unwrap_or(Path::new(""));
                    let target = dir.join(&inc);
                    let target_rel = normalize(&rel.parent().unwrap_or(Path::new("")).join(&inc));
                    let canon = target.canonicalize().map_err(|e| {
                        Error::new(format!("cannot read {}: {e}", target.display()), span)
                    })?;
                    if let Some(i) = self.stack.iter().position(|p| *p == canon) {
                        let names: Vec<String> = self.stack[i..]
                            .iter()
                            .chain([&canon])
                            .map(|p| {
                                p.file_name()
                                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                            })
                            .collect();
                        return Err(Error::new(
                            format!("include cycle: {}", names.join(" -> ")),
                            span,
                        ));
                    }
                    if self.seen.contains(&canon) {
                        continue;
                    }
                    out.extend(self.file(&target, target_rel, Some(span))?);
                }
                Item::Day { span, .. } if included_at.is_some() => {
                    return Err(Error::new(
                        "an included file can't have a `day` header",
                        span,
                    ));
                }
                Item::Example(e) if included_at.is_some() => {
                    return Err(Error::new(
                        "an included file can't have `example` blocks",
                        e.span,
                    ));
                }
                Item::Answers { span, .. } if included_at.is_some() => {
                    return Err(Error::new(
                        "an included file can't have an `answers` block",
                        span,
                    ));
                }
                // An included file's tests run only when it is tested itself.
                Item::Test { .. } if included_at.is_some() => {}
                item => out.push(item),
            }
        }
        self.stack.pop();
        Ok(out)
    }

    /// `expr` as the definition of [`EVAL_NAME`], kept as a file of its own.
    /// It is parsed as an ordinary constant, then renamed, so a program's
    /// own definitions can't clash with it.
    fn expr(&mut self, expr: &str) -> Result<Vec<Item>, Error> {
        if !self.src.ends_with('\n') {
            self.src.push('\n');
        }
        // `__eval = ` goes at the end of the file before, so that the file
        // shown in messages holds just the expression.
        let base = self.src.len();
        self.src.push_str(&format!("{EVAL_PARSED_AS} = "));
        let start = self.src.len();
        self.src.push_str(&eval_source(expr));
        self.src.push('\n');
        self.files.push(SourceFile {
            start,
            path: PathBuf::from(EVAL_FILE),
        });
        let mut items = crate::parser::parse_file(&self.src, base)?;
        let renamed = match items.as_mut_slice() {
            [Item::Def(RawDef::Const(c))] => match &mut c.pat.kind {
                PatKind::Bind(b) if &*b.name == EVAL_PARSED_AS => {
                    b.name = EVAL_NAME.into();
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !renamed {
            return Err(Error::new(
                "expected a single expression",
                Span::new(start, self.src.len() - 1),
            ));
        }
        Ok(items)
    }
}

/// `expr` laid out to follow `__eval = ` at the start of a line (SPEC 3.1).
/// Its line breaks are kept, since match arms and `where` blocks depend on
/// them. Every line after the first must be indented, or it would start a
/// new top-level item; if any isn't, all of them move one column right,
/// which keeps their indentation relative to each other. Otherwise the
/// text is unchanged, so error columns match what was typed.
fn eval_source(expr: &str) -> String {
    let mut lines = expr.trim_end().lines();
    let first = lines.next().unwrap_or("").to_string();
    let rest: Vec<&str> = lines.collect();
    let flush = rest
        .iter()
        .any(|l| !l.trim().is_empty() && !l.starts_with([' ', '\t']));
    let mut out = first;
    for line in rest {
        out.push('\n');
        if flush && !line.trim().is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// `s` as a JSON string, quoted.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// An error as a JSON object, for tools (SPEC 2.7): its message, and the
/// file and 1-based line and column where its span starts and ends, or
/// just the file when it has no span.
pub fn error_json(path: &Path, src: Option<&str>, files: &[SourceFile], err: &Error) -> String {
    let message = json_string(&err.msg);
    match (src, err.span) {
        (Some(src), Some(span)) => {
            let (i, start, text) = file_at(src, files, span.start);
            let lines = LineIndex::new(text);
            let at = |offset: usize| {
                let offset = offset.clamp(start, start + text.len()) - start;
                lines.line_col(text, offset)
            };
            let (line, column) = at(span.start);
            let (end_line, end_column) = at(span.end.max(span.start));
            let file = file_name(path, files, i);
            format!(
                "{{\"file\":{},\"line\":{line},\"column\":{column},\"end_line\":{end_line},\"end_column\":{end_column},\"message\":{message}}}",
                json_string(&file.display().to_string())
            )
        }
        _ => format!(
            "{{\"file\":{},\"message\":{message}}}",
            json_string(&path.display().to_string())
        ),
    }
}

/// Removes `.` and `name/..` from a relative path, for display.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.last(), Some(Component::Normal(_))) => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out.iter().collect()
}

/// Where `offset` is, as `file:line:col`. `path` is the main file; other
/// files are shown relative to its directory.
pub fn location(path: &Path, src: &str, files: &[SourceFile], offset: usize) -> String {
    let (i, start, text) = file_at(src, files, offset);
    let (l, c) = LineIndex::new(text).line_col(text, offset.saturating_sub(start));
    format!("{}:{l}:{c}", file_name(path, files, i).display())
}

/// The name to show for file `i` of a program whose main file is `path`.
pub fn file_name(path: &Path, files: &[SourceFile], i: usize) -> PathBuf {
    match files.get(i) {
        Some(f) if f.path.as_os_str() == EVAL_FILE => f.path.clone(),
        Some(f) if !f.path.as_os_str().is_empty() => {
            normalize(&path.parent().unwrap_or(Path::new("")).join(&f.path))
        }
        _ => path.to_path_buf(),
    }
}

/// Formats an error as `file:line:col: message`, followed by the stack trace.
pub fn format_error(path: &Path, src: Option<&str>, files: &[SourceFile], err: &Error) -> String {
    let loc = |offset: usize| match src {
        Some(src) => location(path, src, files, offset),
        None => path.display().to_string(),
    };
    let mut out = match err.span {
        Some(span) => format!("{}: {}", loc(span.start), err.msg),
        None => format!("{}: {}", path.display(), err.msg),
    };
    for entry in &err.trace {
        let at = format!("at {}", loc(entry.span.start));
        out.push_str(&format!("\n    {}", entry.describe(&at)));
    }
    out
}

/// Looks for `input/YYYY/yYYdDD.txt` in the program's directory and then in
/// each parent directory (SPEC 2.5).
pub fn find_input(program_path: &Path, year: u32, day: u32) -> Option<PathBuf> {
    let rel = PathBuf::from("input")
        .join(year.to_string())
        .join(format!("y{:02}d{day:02}.txt", year % 100));
    let start = program_path.canonicalize().ok()?;
    start
        .parent()?
        .ancestors()
        .map(|dir| dir.join(&rel))
        .find(|p| p.is_file())
}

/// The real input for a program: its contents, or why there isn't one.
pub fn real_input(
    prog: &Program,
    path: &Path,
    input_override: Option<&Path>,
) -> Result<Rc<str>, String> {
    let file = match (input_override, prog.day) {
        (Some(p), _) => p.to_path_buf(),
        (None, Some((year, day))) => find_input(path, year, day).ok_or_else(|| {
            format!(
                "no input file input/{year}/y{:02}d{day:02}.txt found",
                year % 100
            )
        })?,
        (None, None) => return Err("`input` needs a `day` header or --input".into()),
    };
    std::fs::read_to_string(&file)
        .map(Rc::from)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub kind: &'static str,
    pub name: String,
    pub line: usize,
    pub outcome: Outcome,
}

/// Which interpreter runs a program's checks.
#[derive(Clone)]
pub enum Engine {
    /// The tree-walking evaluator.
    Tree,
    /// The IR interpreter, for checking the native compiler's lowering
    /// (`crate::ir`).
    Ir(Rc<crate::ir::Module>),
    /// The IR compiled to native code (`crate::native`).
    Native(Rc<crate::ir::Module>, Rc<crate::native::Native>),
}

impl Engine {
    /// The IR engine for `prog`.
    pub fn ir(prog: &Program) -> Result<Engine, Error> {
        Ok(Engine::Ir(Rc::new(crate::ir::lower::lower(prog)?)))
    }

    /// The native engine for `prog`: its IR, compiled.
    pub fn native(prog: &Program) -> Result<Engine, Error> {
        let m = Rc::new(crate::ir::lower::lower(prog)?);
        let n = crate::native::compile(m.clone())
            .map_err(|e| Error::msg(format!("internal: native code generation: {e}")))?;
        Ok(Engine::Native(m, Rc::new(n)))
    }

    /// An interpreter for one run: the real input, or `example`.
    pub fn interp<'p>(
        &self,
        prog: &'p Program,
        input: Result<Rc<str>, String>,
        example: Option<usize>,
    ) -> Interp<'p> {
        let withs = example.map_or(&[][..], |e| &prog.examples[e].withs[..]);
        let mut it = Interp::new(prog, input, withs);
        match self {
            Engine::Tree => {}
            Engine::Ir(m) => crate::ir::interp::install(&mut it, m.clone(), None, example),
            Engine::Native(m, n) => {
                crate::ir::interp::install(&mut it, m.clone(), Some(n.clone()), example)
            }
        }
        it
    }
}

/// How long [`with_arguments`] may spend running a part again.
pub const ARGUMENTS_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// `err`, from evaluating the global `name`, with its stack trace showing
/// each call's arguments (SPEC 8.3). Programs are deterministic, so
/// running the same global again on the tree-walker, recording arguments,
/// fails the same way. If it doesn't fail the same way, or takes longer
/// than `budget`, `err` is given back as it was.
pub fn with_arguments(
    prog: &Program,
    input: Result<Rc<str>, String>,
    example: Option<usize>,
    name: &str,
    err: Error,
    budget: std::time::Duration,
) -> Error {
    if err.trace.is_empty() {
        return err;
    }
    let mut it = Engine::Tree.interp(prog, input, example);
    it.capture = Some(crate::eval::Capture::new(budget));
    match it.global_by_name(name, Span::default()) {
        Some(Err(again)) if again.msg == err.msg && again.span == err.span => again,
        _ => err,
    }
}

fn eval_check(it: &mut Interp<'_>, check: &Check) -> Result<Value, Error> {
    if it.ir.is_some() {
        return crate::ir::interp::run_check(it, check.span.start, check.span);
    }
    let frame = Frame::new(check.frame_size, None);
    it.eval(&check.expr, &frame)
}

/// Evaluates `name` and compares it with the check's expected value.
fn expect(it: &mut Interp<'_>, check: &Check) -> Result<(), String> {
    let actual = match it.global_by_name(&check.name, check.span) {
        None => return Err(format!("`{}` is not defined", check.name)),
        Some(r) => r.map_err(|e| format!("{}: {}", check.name, e.msg))?,
    };
    let expected = eval_check(it, check)
        .map_err(|e| format!("{}: expected value failed: {}", check.name, e.msg))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{}: expected {}, got {}",
            check.name,
            short(&expected),
            short(&actual)
        ))
    }
}

fn run_test(
    it: &mut Interp<'_>,
    check: &Check,
    expect_error: bool,
    path: &Path,
    prog: &Program,
) -> Outcome {
    it.input_missing = false;
    let result = eval_check(it, check);
    // Like `answers`, a test that needs the real input is skipped without it.
    if result.is_err()
        && it.input_missing
        && let Err(why) = &it.input
    {
        return Outcome::Skip(why.clone());
    }
    match (expect_error, result) {
        (true, Err(_)) => Outcome::Pass,
        (true, Ok(v)) => Outcome::Fail(format!("expected an error, got {}", short(&v))),
        (false, Ok(v)) if v.as_bool() == Some(true) => Outcome::Pass,
        (false, Ok(v)) if v.as_bool() == Some(false) => {
            // For a top-level `==`, show both sides (SPEC 2.6).
            if it.ir.is_none()
                && let ExprKind::Compare(lhs, rest) = &check.expr.kind
                && let [(CmpOp::Eq, rhs)] = rest.as_slice()
            {
                let frame = Frame::new(check.frame_size, None);
                if let (Ok(l), Ok(r)) = (it.eval(lhs, &frame), it.eval(rhs, &frame)) {
                    return Outcome::Fail(format!(
                        "left:  {}\n      right: {}",
                        short(&l),
                        short(&r)
                    ));
                }
            }
            Outcome::Fail("evaluated to false".into())
        }
        (false, Ok(v)) => Outcome::Fail(format!(
            "a test must evaluate to a bool, got {}",
            v.type_name()
        )),
        (false, Err(e)) => Outcome::Fail(format_error(path, Some(&prog.src), &prog.files, &e)),
    }
}

/// Runs every check in the program.
/// Checks the `example` blocks only, which never read the real input.
pub fn run_examples(prog: &Program) -> Vec<CheckResult> {
    run_examples_with(prog, &Engine::Tree)
}

pub fn run_examples_with(prog: &Program, engine: &Engine) -> Vec<CheckResult> {
    let lines = LineIndex::new(&prog.src);
    let mut results = Vec::new();
    for (i, ex) in prog.examples.iter().enumerate() {
        let mut it = engine.interp(prog, Ok(ex.text.clone()), Some(i));
        let failures: Vec<String> = ex
            .expects
            .iter()
            .filter_map(|c| expect(&mut it, c).err())
            .collect();
        results.push(CheckResult {
            kind: "example",
            name: format!("example {}", i + 1),
            line: lines.line(ex.span.start),
            outcome: if failures.is_empty() {
                Outcome::Pass
            } else {
                Outcome::Fail(failures.join("\n      "))
            },
        });
    }
    results
}

pub fn run_checks(prog: &Program, path: &Path) -> Vec<CheckResult> {
    run_checks_with(prog, path, &Engine::Tree)
}

pub fn run_checks_with(prog: &Program, path: &Path, engine: &Engine) -> Vec<CheckResult> {
    let lines = LineIndex::new(&prog.src);
    let line = |offset: usize| lines.line(offset);
    let mut results = Vec::new();

    let input = real_input(prog, path, None);
    let mut it = engine.interp(prog, input.clone(), None);
    for (check, expect_error) in &prog.tests {
        results.push(CheckResult {
            kind: if *expect_error { "test_error" } else { "test" },
            name: check.name.to_string(),
            line: line(check.span.start),
            outcome: run_test(&mut it, check, *expect_error, path, prog),
        });
    }

    results.extend(run_examples_with(prog, engine));

    if let Some((expects, span)) = &prog.answers {
        let outcome = match &input {
            Err(why) => Outcome::Skip(why.clone()),
            Ok(_) => {
                let mut it = engine.interp(prog, input.clone(), None);
                let failures: Vec<String> = expects
                    .iter()
                    .filter_map(|c| expect(&mut it, c).err())
                    .collect();
                if failures.is_empty() {
                    Outcome::Pass
                } else {
                    Outcome::Fail(failures.join("\n      "))
                }
            }
        };
        results.push(CheckResult {
            kind: "answers",
            name: "answers".into(),
            line: line(span.start),
            outcome,
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string(r"C:\x\a.tin"), r#""C:\\x\\a.tin""#);
        assert_eq!(json_string("say \"hi\"\n\ttab"), r#""say \"hi\"\n\ttab""#);
        assert_eq!(json_string("\u{1}é"), "\"\\u0001é\"");
    }

    /// The error from `part1` on `engine`, run again with arguments.
    fn traced(src: &str, engine: &Engine, budget: std::time::Duration) -> Vec<String> {
        let prog = compile(src.to_string()).expect("compiles");
        let mut it = engine.interp(&prog, Err("no input".into()), None);
        let err = match it.global_by_name("part1", Span::default()) {
            Some(Err(e)) => e,
            other => panic!(
                "expected an error, got {:?}",
                other.map(|r| r.map(|v| v.repr()))
            ),
        };
        let err = with_arguments(&prog, Err("no input".into()), None, "part1", err, budget);
        err.trace.iter().map(|t| t.describe("here")).collect()
    }

    #[test]
    fn traces_show_arguments_on_every_engine() {
        let src = "down(0) = [1][9]\ndown(n) = 1 + down(n - 1)\n\
                   walk(p, 0) = down(p.0)\nwalk(p, n) = walk(p + (1, 0), n - 1)\n\
                   part1 = [(0, 0)].map(p => walk(p, 2)).len\n";
        let prog = compile(src.to_string()).expect("compiles");
        let engines = [
            Engine::Tree,
            Engine::ir(&prog).expect("lowers"),
            Engine::native(&prog).expect("compiles natively"),
        ];
        for engine in &engines {
            assert_eq!(
                traced(src, engine, ARGUMENTS_BUDGET),
                // `walk` and its call of `down` are tail calls, so the
                // lambda's call starts a chain of 4 of them, ending in
                // `down(2)`.
                [
                    "`down(0)` called here (2 times, the innermost shown)",
                    "`down(2)` called here",
                    "`<lambda>((0, 0))` called here, then 4 tail calls",
                ]
            );
        }
    }

    #[test]
    fn long_arguments_are_cut_short() {
        let src = "f(xs, s) = xs[99]\npart1 = f(list(1..30), \"ab\".repeat(50).join)\n";
        let trace = traced(src, &Engine::Tree, ARGUMENTS_BUDGET);
        let list = "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17,…";
        let string = format!("\"{}…", "ab".repeat(29));
        assert_eq!(trace, [format!("`f({list}, {string})` called here")]);
        assert_eq!(list.chars().count(), 60);
    }

    #[test]
    fn running_again_stops_at_the_budget() {
        // Thousands of calls before the error: with no time to spare, the
        // trace comes back without arguments.
        let src = "count(0) = [1][9]\ncount(n) = 0 + count(n - 1)\npart1 = count(5000)\n";
        let trace = traced(src, &Engine::Tree, std::time::Duration::ZERO);
        assert_eq!(
            trace,
            ["`count` called here (5000 times)", "`count` called here"]
        );
        let trace = traced(src, &Engine::Tree, ARGUMENTS_BUDGET);
        assert_eq!(
            trace[0],
            "`count(0)` called here (5000 times, the innermost shown)"
        );
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string(r"C:\x\a.tin"), r#""C:\\x\\a.tin""#);
        assert_eq!(json_string("say \"hi\"\n\ttab"), r#""say \"hi\"\n\ttab""#);
        assert_eq!(json_string("\u{1}é"), "\"\\u0001é\"");
    }

    /// The trace of the error from evaluating `part1`, as (name, line,
    /// repeats, tail calls). Every engine must give the same one.
    fn trace(src: &str) -> Vec<(String, usize, usize, usize)> {
        let prog = compile(src.to_string()).expect("compiles");
        let lines = LineIndex::new(src);
        let engines = [
            ("tree", Engine::Tree),
            ("ir", Engine::ir(&prog).expect("lowers")),
            ("native", Engine::native(&prog).expect("compiles natively")),
        ];
        let mut traces = engines.iter().map(|(name, engine)| {
            let mut it = engine.interp(&prog, Err("no input".into()), None);
            let err = match it.global_by_name("part1", Span::default()) {
                Some(Err(e)) => e,
                _ => panic!("{name}: expected an error"),
            };
            let t: Vec<_> = err
                .trace
                .iter()
                .map(|t| {
                    (
                        t.name.clone(),
                        lines.line(t.span.start),
                        t.repeats,
                        t.tail_calls,
                    )
                })
                .collect();
            (*name, t)
        });
        let (_, tree) = traces.next().expect("the tree-walker");
        for (name, t) in traces {
            assert_eq!(t, tree, "{name} differs from the tree-walker");
        }
        tree
    }

    #[test]
    fn checks_needing_missing_input_are_skipped() {
        let src = "day 2099.1\n\
                   test \"uses input\" = input.len > 0\n\
                   test_error \"would pass for the wrong reason\" = input[0]\n\
                   test \"doesn't use input\" = 1 + 1 == 2\n\
                   answers\n  part1 = 1\n\
                   part1 = input.len\n";
        let prog = compile(src.to_string()).expect("compiles");
        let outcomes: Vec<Outcome> = run_checks(&prog, Path::new("/nonexistent/day.tin"))
            .into_iter()
            .map(|r| r.outcome)
            .collect();
        let skip = Outcome::Skip("no input file input/2099/y99d01.txt found".into());
        assert_eq!(
            outcomes,
            vec![skip.clone(), skip.clone(), Outcome::Pass, skip]
        );
    }

    #[test]
    fn tail_calls_keep_the_call_that_started_them() {
        let src = "step(0, xs) = xs[5]\nstep(n, xs) = step(n - 1, xs)\nmain(xs) = 1 + step(3, xs)\npart1 = main([1])\n";
        assert_eq!(
            trace(src),
            vec![
                ("step".into(), 2, 1, 0),
                ("step".into(), 3, 1, 3),
                ("main".into(), 4, 1, 0),
            ]
        );
    }

    #[test]
    fn tail_calls_between_functions_are_counted() {
        let src = "even(n) = if n == 0 then [][0] else odd(n - 1)\n\
                   odd(n) = if n == 0 then [][1] else even(n - 1)\n\
                   main(n) = 1 + even(n)\n\
                   part1 = main(4)\n";
        assert_eq!(
            trace(src),
            vec![
                ("even".into(), 2, 1, 0),
                ("even".into(), 3, 1, 4),
                ("main".into(), 4, 1, 0),
            ]
        );
    }

    #[test]
    fn calls_inside_a_chain_of_tail_calls_start_their_own() {
        let src = "inner(n) = if n == 0 then 0 else inner(n - 1)\n\
                   step(n) = if n == 0 then [][0] else step(n - 1 + inner(2))\n\
                   part1 = 1 + step(3)\n";
        assert_eq!(
            trace(src),
            vec![("step".into(), 2, 1, 0), ("step".into(), 3, 1, 3)]
        );
    }

    #[test]
    fn a_constant_doesnt_start_a_chain() {
        let src = "step(n) = if n == 0 then [][0] else step(n - 1)\nx = step(2)\npart1 = x + 1\n";
        assert_eq!(
            trace(src),
            vec![("step".into(), 1, 1, 0), ("step".into(), 2, 1, 2)]
        );
    }

    #[test]
    fn tail_calls_of_function_values_are_counted() {
        let src = "apply(f, n) = f(n)\n\
                   step(n) = if n == 0 then [][0] else apply(step, n - 1)\n\
                   part1 = 1 + step(2)\n";
        assert_eq!(
            trace(src),
            vec![("step".into(), 1, 1, 0), ("step".into(), 3, 1, 4)]
        );
    }

    #[test]
    fn recursion_is_counted_not_listed() {
        let src = "down(0) = [][0]\ndown(n) = 1 + down(n - 1)\npart1 = down(30)\n";
        assert_eq!(
            trace(src),
            vec![("down".into(), 2, 30, 0), ("down".into(), 3, 1, 0)]
        );
    }

    #[test]
    fn a_lambda_appears_even_when_it_tail_calls() {
        let src = "f(x) = x / 0\npart1 = [1].map(y => f(y))\n";
        assert_eq!(
            trace(src),
            vec![("f".into(), 2, 1, 0), ("<lambda>".into(), 2, 1, 1)]
        );
    }

    /// Writes `files` (name, text) into a fresh directory and loads the
    /// first one.
    fn load_files(test: &str, files: &[(&str, &str)]) -> Result<Program, LoadError> {
        let dir =
            std::env::temp_dir().join(format!("tinsel-include-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        }
        let result = load(&dir.join(files[0].0));
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// The error from loading `files`, as `file:line:col: message` with the
    /// main file shown as `main.tin`.
    fn load_error(test: &str, files: &[(&str, &str)]) -> String {
        match load_files(test, files) {
            Ok(_) => panic!("expected a compile error"),
            Err(e) => format_error(Path::new("main.tin"), e.src.as_deref(), &e.files, &e.err),
        }
    }

    #[test]
    fn include_merges_definitions_and_drops_tests() {
        let prog = load_files(
            "merge",
            &[
                (
                    "main.tin",
                    "include \"lib/a.tin\"\npart1 = f(1)\ntest \"main\" = true\n",
                ),
                (
                    "lib/a.tin",
                    "include \"b.tin\"\nf(x) = g(x) + 1\ntest \"a\" = true\n",
                ),
                ("lib/b.tin", "g(x) = x * 10"),
            ],
        )
        .expect("loads");
        assert_eq!(prog.tests.len(), 1);
        let names: Vec<_> = prog.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            names,
            [
                PathBuf::new(),
                PathBuf::from("lib/a.tin"),
                PathBuf::from("lib/b.tin")
            ]
        );
        let mut it = Interp::new(&prog, Err("no input".into()), &[]);
        let v = it.global_by_name("part1", Span::default());
        assert!(matches!(&v, Some(Ok(x)) if x.as_int().is_some()));
        assert_eq!(
            v.and_then(Result::ok).map(|v| v.display()),
            Some("11".into())
        );
    }

    #[test]
    fn include_reports_errors_in_the_included_file() {
        let err = load_error(
            "located",
            &[
                ("main.tin", "include \"lib/a.tin\"\n"),
                ("lib/a.tin", "x = 1\n\ny = )\n"),
            ],
        );
        assert!(err.starts_with("lib/a.tin:3:5: expected"), "{err}");
    }

    #[test]
    fn include_runtime_errors_name_the_included_file() {
        let prog = load_files(
            "runtime",
            &[
                ("main.tin", "include \"a.tin\"\npart1 = f(0)\n"),
                ("a.tin", "# lib\nf(x) = 1 / x\n"),
            ],
        )
        .expect("loads");
        let mut it = Interp::new(&prog, Err("no input".into()), &[]);
        let Some(Err(e)) = it.global_by_name("part1", Span::default()) else {
            panic!("expected an error");
        };
        let msg = format_error(Path::new("main.tin"), Some(&prog.src), &prog.files, &e);
        assert!(msg.starts_with("a.tin:2:8: "), "{msg}");
        assert!(msg.contains("called at main.tin:2:9"), "{msg}");
    }

    #[test]
    fn include_rejects_cycles_and_missing_files() {
        let err = load_error(
            "cycle",
            &[
                ("main.tin", "include \"a.tin\"\n"),
                ("a.tin", "include \"main.tin\"\n"),
            ],
        );
        assert!(
            err.contains("include cycle: main.tin -> a.tin -> main.tin"),
            "{err}"
        );
        let err = load_error("missing", &[("main.tin", "x = 1\ninclude \"nope.tin\"\n")]);
        assert!(err.starts_with("main.tin:2:1: cannot read"), "{err}");
    }

    #[test]
    fn include_rejects_day_example_answers_and_duplicates() {
        for (test, lib, msg) in [
            ("day", "day 2019.1\n", "can't have a `day` header"),
            (
                "example",
                "example \"x\"\n  part1 = 1\n",
                "can't have `example` blocks",
            ),
            (
                "answers",
                "answers\n  part1 = 1\n",
                "can't have an `answers` block",
            ),
            ("dup", "x = 2\n", "`x` is defined more than once"),
        ] {
            let err = load_error(
                test,
                &[("main.tin", "include \"a.tin\"\nx = 1\n"), ("a.tin", lib)],
            );
            assert!(err.contains(msg), "{test}: {err}");
        }
    }

    #[test]
    fn include_needs_a_file() {
        let Err(err) = compile("include \"a.tin\"\n".to_string()) else {
            panic!("expected a compile error");
        };
        assert!(err.msg.contains("loaded from a file"), "{}", err.msg);
    }
}
