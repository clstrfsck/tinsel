//! The engine options of `tinsel run` and `tinsel test` (SPEC 2.7):
//! native code by default, `--tree` and `--ir` give the same results, and
//! two different ones can't be combined.

use std::path::PathBuf;
use std::process::{Command, Output};

/// Saves `src` as `name` and runs `tinsel` with `args` then the file.
fn tinsel(name: &str, src: &str, args: &[&str]) -> Output {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("engines");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("temporary program");
    let (cmd, opts) = args.split_first().expect("a subcommand");
    Command::new(env!("CARGO_BIN_EXE_tinsel"))
        .arg(cmd)
        .args(opts)
        .arg(&path)
        .output()
        .expect("run tinsel")
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const PROGRAM: &str = "\
fib(n) = if n < 2 then n else fib(n - 1) + fib(n - 2)
part1 = [fib(n) for n in 0..10].filter(_ % 2 == 0)
part2 = {(x, x * x) for x in 1..3}.map(.1).sum
test \"fib\" = fib(10) == 55
";

#[test]
fn every_engine_gives_the_same_answers() {
    let outs: Vec<String> = [&[][..], &["--tree"], &["--ir"]]
        .iter()
        .map(|opt| {
            let mut args = vec!["run"];
            args.extend_from_slice(opt);
            let out = tinsel("same.tin", PROGRAM, &args);
            assert!(out.status.success(), "{opt:?}: {out:?}");
            text(&out).0
        })
        .collect();
    assert_eq!(outs[0], "part1: [0, 2, 8, 34]\npart2: 14\n");
    assert_eq!(outs[1], outs[0], "--tree");
    assert_eq!(outs[2], outs[0], "--ir");
}

#[test]
fn every_engine_runs_the_tests() {
    for opt in ["--tree", "--native", "--ir"] {
        let out = tinsel("tests.tin", PROGRAM, &["test", opt]);
        assert!(out.status.success(), "{opt}: {out:?}");
        assert!(
            text(&out).0.contains("1 passed, 0 failed"),
            "{opt}: {out:?}"
        );
    }
}

#[test]
fn every_engine_reports_the_same_error() {
    let src = "part1 = [1, 2][5]\n";
    let errs: Vec<String> = [&[][..], &["--tree"], &["--ir"]]
        .iter()
        .map(|opt| {
            let mut args = vec!["run"];
            args.extend_from_slice(opt);
            let out = tinsel("error.tin", src, &args);
            assert!(!out.status.success(), "{opt:?} should fail");
            text(&out).1
        })
        .collect();
    assert!(errs[0].contains("index 5 is out of range"), "{}", errs[0]);
    assert_eq!(errs[1], errs[0], "--tree");
    assert_eq!(errs[2], errs[0], "--ir");
}

#[test]
fn two_engines_are_refused() {
    for cmd in ["run", "test"] {
        for (a, b) in [
            ("--native", "--ir"),
            ("--tree", "--native"),
            ("--ir", "--tree"),
        ] {
            let out = tinsel("both.tin", PROGRAM, &[cmd, a, b]);
            assert_eq!(out.status.code(), Some(2), "{cmd} {a} {b}: {out:?}");
            assert!(
                text(&out).1.contains("use only one of"),
                "{cmd} {a} {b}: {out:?}"
            );
        }
        // Repeating the same one is fine.
        let out = tinsel("same_twice.tin", PROGRAM, &[cmd, "--tree", "--tree"]);
        assert!(out.status.success(), "{cmd}: {out:?}");
    }
}
