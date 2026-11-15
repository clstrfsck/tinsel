//! Runs every `.tin` file under `tests/` and reports each `test`,
//! `test_error`, `example` and `answers` block as a separate pass or fail.
//!
//! Usage: `cargo test --test run_tin [-- FILTER...]`. Each FILTER is a
//! substring matched against the file path, e.g. `basics` or `aoc2025`.
//! Set `TIN_VERBOSE=1` to list passing checks too.
//!
//! In debug builds, a file run on the IR interpreter or natively fails if
//! it leaks heap values (`TIN_LEAKS=1` shows the tree-walker's too).
//!
//! By default every file runs on the tree-walker, and `tests/features/` runs
//! on the IR interpreter (`tinsel::ir`) and natively (`tinsel::native`) as
//! well. `TIN_ENGINE=ir` runs every file on the IR interpreter only,
//! `TIN_ENGINE=native` natively only, and `TIN_ENGINE=tree` on the
//! tree-walker only. Runs use up to 12 worker threads, limited by available
//! parallelism; `TIN_JOBS=N` overrides that, and `TIN_JOBS=1` keeps serial
//! output for debugging.

mod support;

use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tinsel::runner::{Engine, Outcome, format_error, load, run_checks_with};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "tin") {
            out.push(path);
        }
    }
}

#[derive(Clone)]
struct Run {
    file: PathBuf,
    which: &'static str,
}

#[derive(Default)]
struct Report {
    output: String,
    passed: usize,
    failed: usize,
    skipped: usize,
}

fn shown_path(root: &Path, run: &Run) -> PathBuf {
    let rel = run.file.strip_prefix(root).unwrap_or(&run.file);
    if run.which == "tree" {
        rel.to_path_buf()
    } else {
        PathBuf::from(format!("[{}] {}", run.which, rel.display()))
    }
}

/// Loads, compiles and executes one file on one engine. Everything that
/// contains `Rc` stays on this worker thread and is dropped before return.
fn run_inner(root: &Path, run: &Run, verbose: bool) -> Report {
    let rel = shown_path(root, run);
    let rel = rel.as_path();
    let mut report = Report::default();
    let program = match load(&run.file) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(
                report.output,
                "FAIL {}: does not compile\n      {}",
                rel.display(),
                format_error(rel, e.src.as_deref(), &e.files, &e.err)
            );
            report.failed += 1;
            return report;
        }
    };
    let engine = match run.which {
        "ir" => Engine::ir(&program),
        "native" => Engine::native(&program),
        _ => Ok(Engine::Tree),
    };
    let engine = match engine {
        Ok(e) => e,
        Err(e) => {
            let _ = writeln!(
                report.output,
                "FAIL {}: does not compile: {}",
                rel.display(),
                e.msg
            );
            report.failed += 1;
            return report;
        }
    };
    let results = run_checks_with(&program, &run.file, &engine);
    for r in results {
        let loc = format!("{}:{}", rel.display(), r.line);
        match r.outcome {
            Outcome::Pass => {
                report.passed += 1;
                if verbose {
                    let _ = writeln!(report.output, "pass {loc} {} {:?}", r.kind, r.name);
                }
            }
            Outcome::Fail(msg) => {
                report.failed += 1;
                let _ = writeln!(
                    report.output,
                    "FAIL {loc} {} {:?}\n      {msg}",
                    r.kind, r.name
                );
            }
            Outcome::Skip(why) => {
                report.skipped += 1;
                if r.name == r.kind {
                    let _ = writeln!(report.output, "skip {loc} {} ({why})", r.kind);
                } else {
                    let _ = writeln!(report.output, "skip {loc} {} {:?} ({why})", r.kind, r.name);
                }
            }
        }
    }
    report
}

/// Contains a whole run's panics and checks its thread-local live word count
/// after unwinding has dropped the program and engine.
fn run_one(root: &Path, run: &Run, verbose: bool, show_tree_leaks: bool) -> Report {
    let live_before = tinsel::value::word::live::words();
    let mut report = match catch_unwind(AssertUnwindSafe(|| run_inner(root, run, verbose))) {
        Ok(report) => report,
        Err(_) => {
            let mut report = Report {
                failed: 1,
                ..Report::default()
            };
            let _ = writeln!(
                report.output,
                "FAIL {}: the interpreter panicked",
                shown_path(root, run).display()
            );
            report
        }
    };
    let leaked = tinsel::value::word::live::words() - live_before;
    if leaked != 0 && run.which != "tree" {
        report.failed += 1;
        report.output.insert_str(
            0,
            &format!(
                "FAIL {}: {leaked} heap values leaked (or freed twice)\n",
                shown_path(root, run).display()
            ),
        );
    } else if leaked != 0 && show_tree_leaks {
        report.output.insert_str(
            0,
            &format!(
                "leak {}: {leaked} heap values\n",
                shown_path(root, run).display()
            ),
        );
    }
    report
}

fn job_count() -> Result<usize, String> {
    match std::env::var("TIN_JOBS") {
        Ok(s) => s
            .parse::<NonZeroUsize>()
            .map(NonZeroUsize::get)
            .map_err(|_| format!("TIN_JOBS must be a positive integer, found `{s}`")),
        Err(std::env::VarError::NotPresent) => Ok(std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(1)
            .min(12)),
        Err(std::env::VarError::NotUnicode(_)) => Err("TIN_JOBS must be a positive integer".into()),
    }
}

fn main() -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let verbose = std::env::var_os("TIN_VERBOSE").is_some();
    let show_tree_leaks = std::env::var_os("TIN_LEAKS").is_some();
    let engine = std::env::var("TIN_ENGINE").unwrap_or_default();
    let jobs = match job_count() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut files = Vec::new();
    collect(&root.join("tests"), &mut files);
    files.sort();
    files.retain(|f| {
        filters.is_empty()
            || filters
                .iter()
                .any(|flt| f.to_string_lossy().contains(flt.as_str()))
    });

    // (file, engine: "tree", "ir" or "native")
    let mut runs = Vec::new();
    for file in &files {
        let feature = file.starts_with(root.join("tests").join("features"));
        let all = engine.is_empty() && feature;
        for e in ["tree", "ir", "native"] {
            if engine == e || all || (engine.is_empty() && e == "tree") {
                runs.push(Run {
                    file: file.clone(),
                    which: e,
                });
            }
        }
    }

    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    let reports = support::parallel::map(runs.len(), jobs, |i| {
        run_one(&root, &runs[i], verbose, show_tree_leaks)
    });
    for (run, report) in runs.iter().zip(reports) {
        let report = match report {
            Ok(report) => report,
            Err(_) => {
                let mut report = Report {
                    failed: 1,
                    ..Report::default()
                };
                let _ = writeln!(
                    report.output,
                    "FAIL {}: the worker panicked",
                    shown_path(&root, run).display()
                );
                report
            }
        };
        print!("{}", report.output);
        passed += report.passed;
        failed += report.failed;
        skipped += report.skipped;
    }
    let on = |e: &str| runs.iter().filter(|r| r.which == e).count();
    println!(
        "\n{} files ({} on the tree-walker, {} on the IR interpreter, {} native): \
         {passed} passed, {failed} failed, {skipped} skipped",
        files.len(),
        on("tree"),
        on("ir"),
        on("native")
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
