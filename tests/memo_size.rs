//! A function memoized automatically caches only calls whose arguments hold
//! at most 256 values (SPEC 8.1); `@memo` caches every call. A cached call
//! doesn't run the function's body again, so a `dbg` in the body shows
//! whether the second of two equal calls was cached. These tests run each
//! engine on small programs and count what `dbg` prints.

use std::path::PathBuf;
use std::process::Command;

const ENGINES: [&str; 3] = ["--tree", "--ir", "--native"];

/// Runs `tinsel run` on `src`, saved as `name`, with `engine`, and gives
/// (stdout, stderr).
fn run(name: &str, src: &str, engine: &str) -> (String, String) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("memo_size");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("temporary program");
    let out = Command::new(env!("CARGO_BIN_EXE_tinsel"))
        .args(["run", engine])
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
/// call, called twice with a list of `n` elements: with the list itself
/// and the int, its arguments hold `n + 2` values. Gives how many times
/// its body ran, on each engine.
fn runs(name: &str, annotation: &str, n: usize) -> Vec<usize> {
    let src = format!(
        "{annotation}\nf(xs, 0) = dbg(\"ran\", xs.len)\nf(xs, k) = f(xs, k - 1) + 0\n\n\
         part1 = [f(list(1..{n}), 1), f(list(1..{n}), 1)]\n"
    );
    ENGINES
        .iter()
        .map(|engine| {
            let (out, err) = run(name, &src, engine);
            assert_eq!(out.trim(), format!("part1: [{n}, {n}]"), "{engine}");
            err.matches("ran").count()
        })
        .collect()
}

#[test]
fn keys_of_up_to_256_values_are_cached() {
    assert_eq!(runs("small.tin", "", 254), [1, 1, 1]);
}

#[test]
fn larger_keys_are_not_cached() {
    assert_eq!(runs("large.tin", "", 255), [2, 2, 2]);
}

/// The same with a string of `n` characters in place of the list: with
/// the string itself and the int, its arguments hold `n + 2` values.
fn runs_str(name: &str, n: usize) -> Vec<usize> {
    let src = format!(
        "f(s, 0) = dbg(\"ran\", s.len)\nf(s, k) = f(s, k - 1) + 0\n\n\
         part1 = [f(\"é\".repeat({n}), 1), f(\"é\".repeat({n}), 1)]\n"
    );
    ENGINES
        .iter()
        .map(|engine| {
            let (out, err) = run(name, &src, engine);
            assert_eq!(out.trim(), format!("part1: [{n}, {n}]"), "{engine}");
            err.matches("ran").count()
        })
        .collect()
}

#[test]
fn strings_count_by_character() {
    // Characters, not bytes: each `é` is two bytes in UTF-8.
    assert_eq!(runs_str("short_str.tin", 254), [1, 1, 1]);
    assert_eq!(runs_str("long_str.tin", 255), [2, 2, 2]);
}

#[test]
fn at_memo_caches_large_keys() {
    assert_eq!(runs("forced.tin", "@memo", 2000), [1, 1, 1]);
}

#[test]
fn at_nomemo_caches_nothing() {
    assert_eq!(runs("plain.tin", "@nomemo", 10), [2, 2, 2]);
}

#[test]
fn large_keys_give_no_warning() {
    // The warning that the size limit replaced is gone.
    let src = "grow(xs, 0) = xs.len\ngrow(xs, n) = grow(xs ++ [n], n - 1) + 0\n\n\
               part1 = grow(list(1..1000), 50)\n";
    for engine in ENGINES {
        let (out, err) = run("growing.tin", src, engine);
        assert_eq!(out.trim(), "part1: 1050", "{engine}");
        assert!(!err.contains("warning"), "{engine}: {err}");
    }
}
