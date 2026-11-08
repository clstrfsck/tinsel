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
//! tree-walker only.

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

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let verbose = std::env::var_os("TIN_VERBOSE").is_some();
    let engine = std::env::var("TIN_ENGINE").unwrap_or_default();

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
    let mut runs: Vec<(&PathBuf, &str)> = Vec::new();
    for file in &files {
        let feature = file.starts_with(root.join("tests").join("features"));
        let all = engine.is_empty() && feature;
        for e in ["tree", "ir", "native"] {
            if engine == e || all || (engine.is_empty() && e == "tree") {
                runs.push((file, e));
            }
        }
    }

    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for &(file, which) in &runs {
        let rel = file.strip_prefix(root).unwrap_or(file);
        let rel = if which == "tree" {
            rel.to_path_buf()
        } else {
            PathBuf::from(format!("[{which}] {}", rel.display()))
        };
        let rel = rel.as_path();
        let live_before = tinsel::value::word::live::words();
        let program = match load(file) {
            Ok(p) => p,
            Err(e) => {
                println!(
                    "FAIL {}: does not compile\n      {}",
                    rel.display(),
                    format_error(rel, e.src.as_deref(), &e.files, &e.err)
                );
                failed += 1;
                continue;
            }
        };
        let engine = match which {
            "ir" => Engine::ir(&program),
            "native" => Engine::native(&program),
            _ => Ok(Engine::Tree),
        };
        let engine = match engine {
            Ok(e) => e,
            Err(e) => {
                println!("FAIL {}: does not compile: {}", rel.display(), e.msg);
                failed += 1;
                continue;
            }
        };
        let results = match catch_unwind(AssertUnwindSafe(|| {
            run_checks_with(&program, file, &engine)
        })) {
            Ok(r) => r,
            Err(_) => {
                println!("FAIL {}: the interpreter panicked", rel.display());
                failed += 1;
                continue;
            }
        };
        drop(engine);
        drop(program);
        // Debug builds count live heap values. The IR engines must free
        // everything; the tree-walker has a known exception (lambdas that
        // escape a frame whose own `where` constants refer back to it), so
        // its leaks are only shown with `TIN_LEAKS=1`.
        let leaked = tinsel::value::word::live::words() - live_before;
        if leaked != 0 && which != "tree" {
            println!(
                "FAIL {}: {leaked} heap values leaked (or freed twice)",
                rel.display()
            );
            failed += 1;
        } else if leaked != 0 && std::env::var_os("TIN_LEAKS").is_some() {
            println!("leak {}: {leaked} heap values", rel.display());
        }
        for r in results {
            let loc = format!("{}:{}", rel.display(), r.line);
            match r.outcome {
                Outcome::Pass => {
                    passed += 1;
                    if verbose {
                        println!("pass {loc} {} {:?}", r.kind, r.name);
                    }
                }
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
    let on = |e: &str| runs.iter().filter(|r| r.1 == e).count();
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
