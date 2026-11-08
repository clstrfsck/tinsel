//! Command line (SPEC 2.7):
//!
//! ```text
//! tinsel run FILE.tin [--input PATH] [--no-examples] [--tree | --ir | --native]
//! tinsel test [--tree | --ir | --native] [FILE_OR_DIR ...]
//! ```
//!
//! Programs are compiled to machine code by default
//! (`reference/native-plan.md`). `--tree` runs the tree-walking
//! interpreter instead, the reference implementation; `--ir` runs the IR
//! interpreter, the native compiler's checking engine (`tinsel::ir`).

use ariadne::{Color, Label, Report, ReportKind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tinsel::error::{Error, SourceFile, Span, file_at};
use tinsel::resolve::Program;
use tinsel::runner::{
    Engine, Outcome, file_name, load, real_input, run_checks_with, run_examples_with,
};

const USAGE: &str = "usage:
  tinsel run FILE.tin [--input PATH] [--no-examples] [--tree | --ir | --native]
  tinsel test [--tree | --ir | --native] [FILE_OR_DIR ...]";

/// Which engine runs the program (SPEC 2.7): native code unless an option
/// says otherwise.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Tree,
    Native,
    Ir,
}

/// The engine options given so far: at most one kind.
#[derive(Default)]
struct Choice(Option<Which>);

impl Choice {
    /// The engine an option selects, if it is one of those options.
    fn of(arg: &str) -> Option<Which> {
        match arg {
            "--tree" => Some(Which::Tree),
            "--native" => Some(Which::Native),
            "--ir" => Some(Which::Ir),
            _ => None,
        }
    }

    /// Records `arg`'s choice; an error if a different one was made.
    fn choose(&mut self, arg: &str) -> Result<(), String> {
        let new = Choice::of(arg).ok_or_else(|| format!("unexpected argument `{arg}`"))?;
        match self.0 {
            Some(old) if old != new => Err("use only one of --tree, --ir and --native".into()),
            _ => {
                self.0 = Some(new);
                Ok(())
            }
        }
    }

    fn which(&self) -> Which {
        self.0.unwrap_or(Which::Native)
    }
}

/// The engine for `prog`.
fn engine(prog: &Program, which: Which) -> Result<Engine, Error> {
    match which {
        Which::Tree => Ok(Engine::Tree),
        Which::Native => Engine::native(prog),
        Which::Ir => Engine::ir(prog),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("test") => test(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Prints an error with the source line underlined. A program's files are
/// stored one after another in `src` (SPEC 2.8); each label is shown in the
/// file it falls in.
fn report(path: &Path, src: Option<&str>, files: &[SourceFile], err: &Error) {
    match (src, err.span) {
        (Some(src), Some(span)) => {
            // (file name, range within that file) for a span of `src`.
            let place = |span: Span| {
                let (i, start, _) = file_at(src, files, span.start);
                let name = file_name(path, files, i).display().to_string();
                (
                    name,
                    span.start - start..span.end.saturating_sub(start).max(span.start - start),
                )
            };
            let mut report = Report::build(ReportKind::Error, place(span))
                .with_message(&err.msg)
                .with_label(Label::new(place(span)).with_color(Color::Red));
            for entry in &err.trace {
                report = report.with_label(
                    Label::new(place(entry.span))
                        .with_message(entry.describe("here"))
                        .with_color(Color::Yellow),
                );
            }
            let count = files.len().max(1);
            let sources = (0..count).map(|i| {
                let start = files.get(i).map_or(0, |f| f.start);
                let (_, _, text) = file_at(src, files, start);
                (
                    file_name(path, files, i).display().to_string(),
                    text.to_string(),
                )
            });
            let _ = report.finish().eprint(ariadne::sources(sources));
        }
        _ => eprintln!("error: {}: {}", path.display(), err.msg),
    }
}

fn run(args: &[String]) -> ExitCode {
    let mut file = None;
    let mut input_path = None;
    let mut examples = true;
    let mut choice = Choice::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--input" => {
                i += 1;
                match args.get(i) {
                    Some(p) => input_path = Some(PathBuf::from(p)),
                    None => {
                        eprintln!("--input needs a path\n{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
            "--no-examples" => examples = false,
            a if Choice::of(a).is_some() => {
                if let Err(m) = choice.choose(a) {
                    eprintln!("{m}\n{USAGE}");
                    return ExitCode::from(2);
                }
            }
            a if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            a => {
                eprintln!("unexpected argument `{a}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let prog = match load(&file) {
        Ok(p) => p,
        Err(e) => {
            report(&file, e.src.as_deref(), &e.files, &e.err);
            return ExitCode::FAILURE;
        }
    };
    let engine = match engine(&prog, choice.which()) {
        Ok(e) => e,
        Err(e) => {
            report(&file, Some(&prog.src), &prog.files, &e);
            return ExitCode::FAILURE;
        }
    };

    if examples {
        let mut ok = true;
        for r in run_examples_with(&prog, &engine) {
            if let Outcome::Fail(msg) = r.outcome {
                eprintln!(
                    "{}:{}: {} failed:\n      {msg}",
                    file.display(),
                    r.line,
                    r.name
                );
                ok = false;
            }
        }
        if !ok {
            eprintln!("not running the real input because an example failed");
            return ExitCode::FAILURE;
        }
    }

    if prog.global("part1").is_none() && prog.global("part2").is_none() {
        eprintln!(
            "{}: nothing to run: the program defines neither `part1` nor `part2`",
            file.display()
        );
        return ExitCode::FAILURE;
    }
    let input = real_input(&prog, &file, input_path.as_deref());
    let mut it = engine.interp(&prog, input, None);
    let mut status = ExitCode::SUCCESS;
    for part in ["part1", "part2"] {
        match it.global_by_name(part, Default::default()) {
            None => {}
            Some(Ok(v)) => println!("{part}: {}", v.display()),
            Some(Err(e)) => {
                report(&file, Some(&prog.src), &prog.files, &e);
                status = ExitCode::FAILURE;
            }
        }
    }
    tinsel::native::prof::dump();
    status
}

fn test(args: &[String]) -> ExitCode {
    let mut choice = Choice::default();
    for a in args.iter().filter(|a| Choice::of(a).is_some()) {
        if let Err(m) = choice.choose(a) {
            eprintln!("{m}\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let which = choice.which();
    let args: Vec<&String> = args.iter().filter(|a| Choice::of(a).is_none()).collect();
    if let Some(a) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!("unexpected argument `{a}`\n{USAGE}");
        return ExitCode::from(2);
    }
    let roots: Vec<PathBuf> = if args.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    let mut files = Vec::new();
    for r in &roots {
        collect(r, &mut files);
    }
    files.sort();
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for file in &files {
        let prog = match load(file) {
            Ok(p) => p,
            Err(e) => {
                report(file, e.src.as_deref(), &e.files, &e.err);
                failed += 1;
                continue;
            }
        };
        let engine = match engine(&prog, which) {
            Ok(e) => e,
            Err(e) => {
                report(file, Some(&prog.src), &prog.files, &e);
                failed += 1;
                continue;
            }
        };
        for r in run_checks_with(&prog, file, &engine) {
            let loc = format!("{}:{}", file.display(), r.line);
            match r.outcome {
                Outcome::Pass => passed += 1,
                Outcome::Fail(msg) => {
                    failed += 1;
                    println!("FAIL {loc} {} {:?}\n      {msg}", r.kind, r.name);
                }
                Outcome::Skip(why) => {
                    skipped += 1;
                    println!("skip {loc} {} ({why})", r.kind);
                }
            }
        }
    }
    println!("{passed} passed, {failed} failed, {skipped} skipped");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let hidden = p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if p.is_dir() && !hidden && !p.ends_with("target") {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "tin") {
            out.push(p);
        }
    }
}
