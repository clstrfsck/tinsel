//! Checks `docs/library.md`, the library reference: every example line runs
//! as a Tinsel `test` (or `test_error`, for a line ending in `# error`) on
//! all three engines, every entry is described and names a built-in, and
//! every built-in has an entry. The reference is parsed by `tinsel::docs`,
//! as `tinsel doc` reads it.

use std::collections::BTreeSet;
use std::path::Path;
use tinsel::builtins::BUILTINS;
use tinsel::docs::ENTRIES;
use tinsel::runner::{Engine, Outcome, compile, run_checks_with};

/// Built-ins that need no entry: `input` is the program's own input
/// (SPEC 2.2), and has no value outside a program with a `day`.
const UNDOCUMENTED: &[&str] = &["input"];

/// The examples as one program, each a `test` named by its line.
fn program() -> String {
    let mut src = String::new();
    for entry in ENTRIES.iter() {
        for ex in &entry.examples {
            let kind = if ex.error { "test_error" } else { "test" };
            src.push_str(&format!(
                "{kind} \"docs/library.md:{}\" = {}\n",
                ex.line, ex.code
            ));
        }
    }
    src
}

#[test]
fn every_example_holds_on_every_engine() {
    let src = program();
    let path = Path::new("docs/library.md");
    let mut failures = Vec::new();
    for which in ["tree", "ir", "native"] {
        let live_before = tinsel::value::word::live::words();
        let prog = match compile(src.clone()) {
            Ok(p) => p,
            Err(e) => panic!("the examples don't compile: {}", e.msg),
        };
        let engine = match which {
            "ir" => Engine::ir(&prog),
            "native" => Engine::native(&prog),
            _ => Ok(Engine::Tree),
        }
        .unwrap_or_else(|e| panic!("[{which}] the examples don't compile: {}", e.msg));
        let results = run_checks_with(&prog, path, &engine);
        assert_eq!(
            results.len(),
            src.lines().count(),
            "[{which}] every example ran"
        );
        for r in results {
            match r.outcome {
                Outcome::Pass => {}
                Outcome::Fail(msg) | Outcome::Skip(msg) => {
                    failures.push(format!("[{which}] {}: {msg}", r.name));
                }
            }
        }
        drop(engine);
        drop(prog);
        let leaked = tinsel::value::word::live::words() - live_before;
        if which != "tree" {
            assert_eq!(leaked, 0, "[{which}] heap values leaked");
        }
    }
    assert!(
        failures.is_empty(),
        "failing examples:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_entry_names_a_builtin_and_has_examples() {
    let known: BTreeSet<&str> = BUILTINS.iter().map(|b| b.name).collect();
    for entry in ENTRIES.iter() {
        assert!(
            !entry.names.is_empty(),
            "line {}: a heading with no names",
            entry.line
        );
        assert!(
            !entry.description.is_empty(),
            "line {}: `{}` has no description",
            entry.line,
            entry.signature
        );
        for name in &entry.names {
            assert!(
                known.contains(name.as_str()),
                "line {}: `{name}` is not a built-in",
                entry.line
            );
        }
        assert!(
            !entry.examples.is_empty(),
            "line {}: `{}` has no examples",
            entry.line,
            entry.signature
        );
    }
}

#[test]
fn every_builtin_has_an_entry() {
    let documented: BTreeSet<&str> = ENTRIES
        .iter()
        .flat_map(|e| e.names.iter().map(String::as_str))
        .collect();
    let missing: BTreeSet<&str> = BUILTINS
        .iter()
        .map(|b| b.name)
        .filter(|name| !documented.contains(name) && !UNDOCUMENTED.contains(name))
        .collect();
    let missing: Vec<&str> = missing.into_iter().collect();
    assert!(
        missing.is_empty(),
        "not in docs/library.md: {}",
        missing.join(", ")
    );
}
