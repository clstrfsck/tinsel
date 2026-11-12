//! `dbg` (SPEC 9.6) writes to stderr and `tinsel eval` (SPEC 2.7) is a
//! command of its own, so `.tin` tests can't check them. These tests run
//! the interpreter on small programs, on every engine, and check what it
//! prints.

use std::path::{Path, PathBuf};
use std::process::Command;

const ENGINES: [&str; 3] = ["--tree", "--ir", "--native"];

/// Saves `src` as `name` and gives its path. Each test uses its own name,
/// since tests run in parallel.
fn save(name: &str, src: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("debugging");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("temporary program");
    path
}

/// Runs `tinsel` with `args`, giving (success, stdout, stderr).
fn tinsel(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tinsel"))
        .args(args)
        .output()
        .expect("run tinsel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Runs `tinsel run` on `path` with each engine; checks they agree and
/// gives what the native engine printed to stderr.
fn run_all(path: &Path) -> String {
    let path = path.to_str().expect("utf-8 path");
    let results: Vec<_> = ENGINES.iter().map(|e| tinsel(&["run", e, path])).collect();
    for (e, r) in ENGINES.iter().zip(&results) {
        assert!(r.0, "{e} failed: {r:?}");
        assert_eq!(r.1, results[0].1, "{e}: stdout differs");
        assert_eq!(r.2, results[0].2, "{e}: stderr differs");
    }
    results[2].2.clone()
}

#[test]
fn dbg_shows_where_and_what() {
    let src = r#"xs = list(1..5)
part1 = dbg(xs.len) + xs.map(_ * 2).dbg.len + dbg("total", xs.sum)
part2 = dbg(
  xs
    .len)
"#;
    let path = save("where.tin", src);
    let err = run_all(&path);
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(
        lines,
        [
            "where.tin:2:9: xs.len = 5",
            "where.tin:2:23: xs.map(_ * 2) = [2, 4, 6, 8, 10]",
            "where.tin:2:47: total: 15",
            "where.tin:3:9: xs .len = 5",
        ]
        .map(|l| format!(
            "{}{}{}",
            path.parent().unwrap().display(),
            std::path::MAIN_SEPARATOR,
            l
        )),
        "stderr: {err}"
    );
}

#[test]
fn dbg_cuts_large_values_short() {
    let src =
        "part1 = (list(1..25), \"ab\".repeat(150).join, {k: k for k in 1..30}, [[0]]).dbg.0.len\n";
    let path = save("short.tin", src);
    let err = run_all(&path);
    let shown = err.split_once(" = ").map(|(_, v)| v.trim()).unwrap_or("");
    let expected = format!(
        "([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, … 5 more], \
         \"{}\"… (300 chars), \
         {{1: 1, 2: 2, 3: 3, 4: 4, 5: 5, 6: 6, 7: 7, 8: 8, 9: 9, 10: 10, 11: 11, 12: 12, 13: 13, \
         14: 14, 15: 15, 16: 16, 17: 17, 18: 18, 19: 19, 20: 20, … 10 more}}, [[0]])",
        "ab".repeat(100)
    );
    assert_eq!(shown, expected);
}

#[test]
fn dbg_returns_its_value() {
    let path = save("value.tin", "part1 = (dbg(5), [1, 2].dbg, dbg(\"l\", 3))\n");
    for e in ENGINES {
        let (ok, out, _) = tinsel(&["run", e, path.to_str().unwrap()]);
        assert!(ok);
        assert_eq!(out.trim(), "part1: (5, [1, 2], 3)", "{e}");
    }
}

const PROGRAM: &str = r#"day 2099.1

example """
1 2 3
"""
  part1 = 6

example """
10 20
"""
  part1 = 30

nums = input.ints
double(n) = 2 * n
part1 = nums.sum
"#;

#[test]
fn eval_uses_the_program_and_an_example() {
    let path = save("eval.tin", PROGRAM);
    let p = path.to_str().unwrap();
    for e in ENGINES {
        let r = tinsel(&["eval", p, "--example", "1", "nums", e]);
        assert_eq!((r.0, r.1.trim()), (true, "[1, 2, 3]"), "{e}: {r:?}");
        let r = tinsel(&["eval", p, "--example", "2", "nums.map(double)", e]);
        assert_eq!((r.0, r.1.trim()), (true, "[20, 40]"), "{e}: {r:?}");
        // An expression that doesn't read the input needs none.
        let r = tinsel(&["eval", p, "double(21) + \"ab\".len", e]);
        assert_eq!((r.0, r.1.trim()), (true, "44"), "{e}: {r:?}");
        // Strings are shown quoted, as in `dbg`.
        let r = tinsel(&["eval", p, "str(5)", e]);
        assert_eq!((r.0, r.1.trim()), (true, "\"5\""), "{e}: {r:?}");
    }
}

#[test]
fn eval_takes_an_input_file() {
    let path = save("eval_input.tin", PROGRAM);
    let input = save("eval_input.txt", "4 5 6\n");
    let r = tinsel(&[
        "eval",
        path.to_str().unwrap(),
        "--input",
        input.to_str().unwrap(),
        "part1",
    ]);
    assert_eq!((r.0, r.1.trim()), (true, "15"), "{r:?}");
}

#[test]
fn eval_reports_errors() {
    let path = save("eval_errors.tin", PROGRAM);
    let p = path.to_str().unwrap();
    // An error in the expression points into it.
    let r = tinsel(&["eval", p, "nope + 1"]);
    assert!(!r.0);
    assert!(r.2.contains("unknown name `nope`"), "{r:?}");
    assert!(r.2.contains("<expression>:1:1"), "{r:?}");
    // A runtime error.
    let r = tinsel(&["eval", p, "--example", "1", "nums[7]"]);
    assert!(!r.0);
    assert!(r.2.contains("<expression>:1:1"), "{r:?}");
    // Reading a missing real input.
    let r = tinsel(&["eval", p, "nums"]);
    assert!(!r.0);
    assert!(
        r.2.contains("no input file input/2099/y99d01.txt found"),
        "{r:?}"
    );
    // No such example.
    let r = tinsel(&["eval", p, "--example", "3", "nums"]);
    assert!(!r.0);
    assert!(
        r.2.contains("there is no example 3; the program has 2"),
        "{r:?}"
    );
    // Usage errors.
    for args in [
        &["eval", p][..],
        &["eval", p, "nums", "--example", "0"],
        &["eval", p, "nums", "--example", "1", "--input", "x.txt"],
    ] {
        let (ok, _, err) = tinsel(args);
        assert!(!ok);
        assert!(err.contains("usage:"), "{args:?}: {err}");
    }
}

#[test]
fn eval_works_with_any_program_names() {
    // The expression is compiled as a constant whose name no program can
    // write, so a program may define `__eval` (or anything else) itself,
    // and the expression can use it.
    let path = save(
        "own_names.tin",
        "__eval = 41\nf(x) = x + __eval\npart1 = f(1)\n",
    );
    let p = path.to_str().unwrap();
    for e in ENGINES {
        let r = tinsel(&["eval", p, "f(1)", e]);
        assert_eq!((r.0, r.1.trim()), (true, "42"), "{e}: {r:?}");
        let r = tinsel(&["eval", p, "__eval * 2", e]);
        assert_eq!((r.0, r.1.trim()), (true, "82"), "{e}: {r:?}");
        let r = tinsel(&["run", e, p]);
        assert_eq!((r.0, r.1.trim()), (true, "part1: 42"), "{e}: {r:?}");
    }
}
