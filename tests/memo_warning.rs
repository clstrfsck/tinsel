//! The warning for large memo keys (SPEC 8.1) goes to stderr, so `.tin`
//! tests can't see it. These tests run the interpreter on small programs
//! and check what it prints.

use std::path::PathBuf;
use std::process::Command;

/// Runs `tinsel run` on `src`, saved as `name`, and gives (stdout, stderr).
fn run(name: &str, src: &str) -> (String, String) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("memo_warning");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("temporary program");
    let out = Command::new(env!("CARGO_BIN_EXE_tinsel"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run tinsel");
    assert!(out.status.success(), "tinsel failed: {out:?}");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A recursive function, memoized because its recursive call isn't a tail
/// call, that passes a growing list along.
fn program(annotation: &str, start: usize) -> String {
    format!(
        "{annotation}\ngrow(xs, 0) = xs.len\ngrow(xs, n) = grow(xs ++ [n], n - 1) + 0\n\n\
         part1 = grow((1..{start}).list, 50)\n"
    )
}

#[test]
fn warns_once_for_a_large_key() {
    let (out, err) = run("large.tin", &program("", 1000));
    assert_eq!(out.trim(), "part1: 1050");
    assert_eq!(err.matches("warning:").count(), 1, "stderr: {err}");
    assert!(
        err.contains("large.tin:2:1: warning: `grow` is memoized automatically"),
        "stderr: {err}"
    );
    assert!(err.contains("more than 1000 values"), "stderr: {err}");
    assert!(
        err.contains("@nomemo") && err.contains("@memo"),
        "stderr: {err}"
    );
}

#[test]
fn small_keys_get_no_warning() {
    // The list and its 998 elements, plus the int: exactly 1000 values.
    let src = "grow(xs, 0) = xs.len\ngrow(xs, n) = grow(xs, n - 1) + 0\n\npart1 = grow((1..998).list, 5)\n";
    let (out, err) = run("small.tin", src);
    assert_eq!(out.trim(), "part1: 998");
    assert!(!err.contains("warning"), "stderr: {err}");
}

#[test]
fn at_memo_silences_the_warning() {
    let (out, err) = run("forced.tin", &program("@memo", 1000));
    assert_eq!(out.trim(), "part1: 1050");
    assert!(!err.contains("warning"), "stderr: {err}");
}

#[test]
fn at_nomemo_needs_no_warning() {
    let (out, err) = run("plain.tin", &program("@nomemo", 1000));
    assert_eq!(out.trim(), "part1: 1050");
    assert!(!err.contains("warning"), "stderr: {err}");
}
