//! Command line (SPEC 2.7):
//!
//! ```text
//! tinsel run FILE.tin [--input PATH] [--no-examples] [--tree | --ir | --native]
//! tinsel test [--tree | --ir | --native] [FILE_OR_DIR ...]
//! tinsel eval FILE.tin EXPR [--example N | --input PATH] [--raw] [--tree | --ir | --native]
//! tinsel check FILE.tin [--json]
//! tinsel doc [NAME] [--json]
//! tinsel symbols FILE.tin [--json] [--stdin]
//! ```
//!
//! Programs are compiled to machine code by default
//! (`reference/native-plan.md`). `--tree` runs the tree-walking
//! interpreter instead, the reference implementation; `--ir` runs the IR
//! interpreter, the native compiler's checking engine (`tinsel::ir`).

use ariadne::{Color, Config, Label, Report, ReportKind};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tinsel::error::{Error, SourceFile, Span, file_at};
use tinsel::resolve::Program;
use tinsel::runner::{
    ARGUMENTS_BUDGET, EVAL_NAME, Engine, Outcome, error_json, file_name, json_string, load,
    load_with_expr, real_input, run_checks_with, run_examples_with, span_json, with_arguments,
};

const USAGE: &str = "usage:
  tinsel run FILE.tin [--input PATH] [--no-examples] [--tree | --ir | --native]
  tinsel test [--tree | --ir | --native] [--json] [FILE_OR_DIR ...]
  tinsel eval FILE.tin EXPR [--example N | --input PATH] [--raw] [--tree | --ir | --native]
  tinsel check FILE.tin [--json]
  tinsel doc [NAME] [--json]
  tinsel symbols FILE.tin [--json] [--stdin]";

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
        Some("eval") => eval(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("doc") => doc(&args[1..]),
        Some("symbols") => symbols(&args[1..]),
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
            // Colour only for a terminal, so that tools reading the output
            // (such as an editor's problem matcher) see plain text.
            let config = Config::default().with_color(std::io::stderr().is_terminal());
            let mut report = Report::build(ReportKind::Error, place(span))
                .with_config(config)
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
    let mut it = engine.interp(&prog, input.clone(), None);
    let mut status = ExitCode::SUCCESS;
    for part in ["part1", "part2"] {
        match it.global_by_name(part, Default::default()) {
            None => {}
            Some(Ok(v)) => println!("{part}: {}", v.display()),
            Some(Err(e)) => {
                let e = with_arguments(&prog, input.clone(), None, part, e, ARGUMENTS_BUDGET);
                report(&file, Some(&prog.src), &prog.files, &e);
                status = ExitCode::FAILURE;
            }
        }
    }
    tinsel::native::prof::dump();
    status
}

/// `tinsel eval`: the value of an expression in a program's scope, with
/// its real input or one of its examples' (SPEC 2.7).
fn eval(args: &[String]) -> ExitCode {
    let usage = |msg: &str| {
        eprintln!("{msg}\n{USAGE}");
        ExitCode::from(2)
    };
    let (mut file, mut expr) = (None, None);
    let (mut input_path, mut example) = (None, None);
    let mut raw = false;
    let mut choice = Choice::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--input" | "--example" => {
                let flag = args[i].as_str();
                i += 1;
                let Some(v) = args.get(i) else {
                    return usage(&format!("{flag} needs a value"));
                };
                if flag == "--input" {
                    input_path = Some(PathBuf::from(v));
                } else {
                    match v.parse::<usize>() {
                        Ok(n) if n >= 1 => example = Some(n),
                        _ => {
                            return usage(&format!("--example needs a number from 1, found `{v}`"));
                        }
                    }
                }
            }
            "--raw" => raw = true,
            a if Choice::of(a).is_some() => {
                if let Err(m) = choice.choose(a) {
                    return usage(&m);
                }
            }
            a if file.is_none() && !a.starts_with('-') => file = Some(PathBuf::from(a)),
            a if expr.is_none() && file.is_some() => expr = Some(a.to_string()),
            a => return usage(&format!("unexpected argument `{a}`")),
        }
        i += 1;
    }
    let (Some(file), Some(expr)) = (file, expr) else {
        return usage("eval needs a file and an expression");
    };
    if input_path.is_some() && example.is_some() {
        return usage("use only one of --example and --input");
    }
    let prog = match load_with_expr(&file, &expr) {
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
    // The input, kept for running again to show arguments in a trace.
    let input = match example {
        Some(n) => prog.examples.get(n - 1).map(|ex| Ok(ex.text.clone())),
        None => Some(real_input(&prog, &file, input_path.as_deref())),
    };
    let mut it = match example {
        Some(n) => match prog.examples.get(n - 1) {
            Some(ex) => engine.interp(&prog, Ok(ex.text.clone()), Some(n - 1)),
            None => {
                eprintln!(
                    "{}: there is no example {n}; the program has {}",
                    file.display(),
                    match prog.examples.len() {
                        0 => "none".to_string(),
                        1 => "1".to_string(),
                        k => format!("{k}"),
                    }
                );
                return ExitCode::FAILURE;
            }
        },
        None => engine.interp(&prog, real_input(&prog, &file, input_path.as_deref()), None),
    };
    match it.global_by_name(EVAL_NAME, Default::default()) {
        Some(Ok(v)) => {
            // `--raw` gives the display form, so a str is its own text.
            let shown = if raw { v.display() } else { v.repr() };
            if shown.ends_with('\n') {
                print!("{shown}");
            } else {
                println!("{shown}");
            }
            ExitCode::SUCCESS
        }
        Some(Err(e)) => {
            let input = input.unwrap_or_else(|| Err("no input".into()));
            let example = example.map(|n| n - 1);
            let e = with_arguments(&prog, input, example, EVAL_NAME, e, ARGUMENTS_BUDGET);
            report(&file, Some(&prog.src), &prog.files, &e);
            ExitCode::FAILURE
        }
        None => {
            eprintln!("internal: the expression wasn't compiled");
            ExitCode::FAILURE
        }
    }
}

/// `tinsel check`: loads a program (parsing and resolving it, with its
/// includes) without running anything, and reports the first error
/// (SPEC 2.7).
fn check(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();
    let [file] = rest.as_slice() else {
        eprintln!("check needs one file\n{USAGE}");
        return ExitCode::from(2);
    };
    if file.starts_with('-') {
        eprintln!("unexpected argument `{file}`\n{USAGE}");
        return ExitCode::from(2);
    }
    let file = PathBuf::from(file);
    let error = load(&file).err();
    if json {
        let errors = error
            .iter()
            .map(|e| error_json(&file, e.src.as_deref(), &e.files, &e.err))
            .collect::<Vec<_>>()
            .join(",");
        println!("{{\"errors\":[{errors}]}}");
    } else if let Some(e) = &error {
        report(&file, e.src.as_deref(), &e.files, &e.err);
    }
    if error.is_some() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `tinsel symbols`: every definition in a program and its includes, and
/// with `--json`, every use of one (SPEC 2.7).
fn symbols(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let stdin = args.iter().any(|a| a == "--stdin");
    let rest: Vec<&String> = args
        .iter()
        .filter(|a| *a != "--json" && *a != "--stdin")
        .collect();
    let [file] = rest.as_slice() else {
        eprintln!("symbols needs one file\n{USAGE}");
        return ExitCode::from(2);
    };
    if file.starts_with('-') {
        eprintln!("unexpected argument `{file}`\n{USAGE}");
        return ExitCode::from(2);
    }
    let file = PathBuf::from(file);
    // With --stdin, the program's text comes from standard input, as an
    // editor's unsaved changes; includes are still read from their files.
    let loaded = if stdin {
        let mut text = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
            eprintln!("Error: cannot read standard input: {e}");
            return ExitCode::FAILURE;
        }
        tinsel::runner::load_text(&file, text)
    } else {
        load(&file)
    };
    let prog = match loaded {
        Ok(p) => p,
        Err(e) => {
            report(&file, e.src.as_deref(), &e.files, &e.err);
            return ExitCode::FAILURE;
        }
    };
    let syms = &prog.symbols;
    let at = |span| span_json(&file, &prog.src, &prog.files, span);
    if json {
        for (id, d) in syms.defs.iter().enumerate() {
            let container = d.container.map_or("null".to_string(), |c| c.to_string());
            println!(
                "{{\"kind\":\"def\",\"id\":{id},\"name\":{},\"what\":{},\"container\":{container},{}}}",
                json_string(&d.name),
                json_string(d.what),
                at(d.span)
            );
        }
        for (span, def) in &syms.refs {
            println!("{{\"kind\":\"ref\",\"def\":{def},{}}}", at(*span));
        }
    } else {
        let mut uses = vec![0usize; syms.defs.len()];
        for (_, def) in &syms.refs {
            if let Some(n) = uses.get_mut(*def) {
                *n += 1;
            }
        }
        // A tree: each definition, then what is inside it, in source order.
        let mut order: Vec<usize> = (0..syms.defs.len()).collect();
        order.sort_by_key(|&i| syms.defs[i].span.start);
        let mut stack: Vec<(usize, usize)> = order
            .iter()
            .rev()
            .filter(|&&i| syms.defs[i].container.is_none())
            .map(|&i| (i, 0))
            .collect();
        while let Some((id, depth)) = stack.pop() {
            let d = &syms.defs[id];
            let (i, start, text) = file_at(&prog.src, &prog.files, d.span.start);
            let (line, col) =
                tinsel::error::LineIndex::new(text).line_col(text, d.span.start - start);
            println!(
                "{}:{line}:{col}: {}{} {} ({} {})",
                file_name(&file, &prog.files, i).display(),
                "  ".repeat(depth),
                d.what,
                d.name,
                uses[id],
                if uses[id] == 1 { "use" } else { "uses" }
            );
            stack.extend(
                order
                    .iter()
                    .rev()
                    .filter(|&&c| syms.defs[c].container == Some(id))
                    .map(|&c| (c, depth + 1)),
            );
        }
    }
    ExitCode::SUCCESS
}

/// `tinsel doc`: entries from the library reference (SPEC 2.7).
fn doc(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();
    let entries: Vec<&tinsel::docs::Entry> = match rest.as_slice() {
        [] => tinsel::docs::ENTRIES.iter().collect(),
        [name] if !name.starts_with('-') => {
            // Allow the method form, as in `tinsel doc .lines`.
            let name = name.strip_prefix('.').unwrap_or(name);
            let found = tinsel::docs::lookup(name);
            if found.is_empty() {
                eprintln!("Error: no entry for `{name}` in the library reference");
                return ExitCode::FAILURE;
            }
            found
        }
        [arg] => {
            eprintln!("unexpected argument `{arg}`\n{USAGE}");
            return ExitCode::from(2);
        }
        _ => {
            eprintln!("doc takes at most one name\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if json {
        for e in &entries {
            println!("{}", tinsel::docs::entry_json(e));
        }
    } else {
        // With every entry, each section gets a heading.
        let all = rest.is_empty();
        let mut section = "";
        for e in &entries {
            if all && e.section != section {
                section = &e.section;
                println!("{section}\n{}\n", "=".repeat(section.chars().count()));
            }
            println!("{}", tinsel::docs::entry_text(e));
        }
    }
    ExitCode::SUCCESS
}

/// One line of `tinsel test --json` (SPEC 2.7).
fn result_json(file: &Path, line: usize, kind: &str, name: &str, outcome: &Outcome) -> String {
    let (status, message) = match outcome {
        Outcome::Pass => ("pass", ""),
        Outcome::Fail(m) => ("fail", m.as_str()),
        Outcome::Skip(m) => ("skip", m.as_str()),
    };
    format!(
        "{{\"file\":{},\"line\":{line},\"kind\":{},\"name\":{},\"outcome\":\"{status}\",\"message\":{}}}",
        json_string(&file.display().to_string()),
        json_string(kind),
        json_string(name),
        json_string(message)
    )
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
    let json = args.iter().any(|a| a == "--json");
    let args: Vec<&String> = args
        .iter()
        .filter(|a| Choice::of(a).is_none() && *a != "--json")
        .collect();
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
    // A file that can't be loaded or compiled counts as one failure. With
    // --json it is a line of kind "load" with the error.
    let load_failure = |file: &Path, src: Option<&str>, sf: &[SourceFile], e: &Error| {
        if json {
            println!(
                "{{\"file\":{},\"kind\":\"load\",\"outcome\":\"fail\",\"message\":{},\"error\":{}}}",
                json_string(&file.display().to_string()),
                json_string(&e.msg),
                error_json(file, src, sf, e)
            );
        } else {
            report(file, src, sf, e);
        }
    };
    for file in &files {
        let prog = match load(file) {
            Ok(p) => p,
            Err(e) => {
                load_failure(file, e.src.as_deref(), &e.files, &e.err);
                failed += 1;
                continue;
            }
        };
        let engine = match engine(&prog, which) {
            Ok(e) => e,
            Err(e) => {
                load_failure(file, Some(&prog.src), &prog.files, &e);
                failed += 1;
                continue;
            }
        };
        for r in run_checks_with(&prog, file, &engine) {
            if json {
                println!("{}", result_json(file, r.line, r.kind, &r.name, &r.outcome));
                match r.outcome {
                    Outcome::Pass => passed += 1,
                    Outcome::Fail(_) => failed += 1,
                    Outcome::Skip(_) => skipped += 1,
                }
                continue;
            }
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
    if json {
        println!("{{\"passed\":{passed},\"failed\":{failed},\"skipped\":{skipped}}}");
    } else {
        println!("{passed} passed, {failed} failed, {skipped} skipped");
    }
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
