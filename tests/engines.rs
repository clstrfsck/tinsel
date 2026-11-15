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

/// `tinsel eval` of `expr` on each engine, which must fail: the three
/// error reports (message, span and trace), native first.
fn eval_errors(expr: &str) -> Vec<String> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("engines");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    let path = dir.join("eval.tin");
    std::fs::write(&path, "one = 1\n").expect("temporary program");
    ["--native", "--tree", "--ir"]
        .iter()
        .map(|opt| {
            let out = Command::new(env!("CARGO_BIN_EXE_tinsel"))
                .args(["eval", opt])
                .arg(&path)
                .arg(expr)
                .output()
                .expect("run tinsel");
            assert!(!out.status.success(), "{opt} {expr} should fail");
            text(&out).1
        })
        .collect()
}

/// A key or element that can't join a map or set literal is reported at
/// that key or element, on every engine.
#[test]
fn a_bad_key_in_a_literal_is_reported_at_the_key() {
    for (expr, message, at) in [
        (
            "{[1]: 2, (1,): 3}",
            "cannot mix list and tuple keys",
            "1:10",
        ),
        ("{1, 2, \"a\"}", "cannot mix int and str keys", "1:8"),
        ("{x => x}", "a function cannot be a map key", "1:2"),
        (
            "{1: 2, (x => x): 1}",
            "a function cannot be a map key",
            "1:9",
        ),
    ] {
        let errs = eval_errors(expr);
        assert!(errs[0].contains(message), "{expr}: {}", errs[0]);
        let place = format!("<expression>:{at} ");
        assert!(errs[0].contains(&place), "{expr}: {}", errs[0]);
        assert_eq!(errs[1], errs[0], "{expr}: --tree");
        assert_eq!(errs[2], errs[0], "{expr}: --ir");
    }
}

/// A deeply nested value that is the program's result is shown and
/// released outside the engine, which mustn't overflow the stack either.
#[test]
fn a_deeply_nested_result_is_printed() {
    let src = "nest(n, acc) = if n == 0 then acc else nest(n - 1, [acc])\n\
               part1 = nest(300000, 7)\n";
    let expected = format!("part1: {}7{}\n", "[".repeat(300_000), "]".repeat(300_000));
    for opt in ["--native", "--tree", "--ir"] {
        let out = tinsel("nested.tin", src, &["run", opt]);
        assert!(out.status.success(), "{opt}: {:?}", out.status);
        assert!(text(&out).0 == expected, "{opt}: wrong output");
    }
}

/// These messages say what was found, on every engine.
#[test]
fn messages_name_the_value_that_was_wrong() {
    for (expr, message) in [
        ("~inf", "~ is undefined for inf"),
        ("~-inf", "~ is undefined for inf"),
        ("~\"a\"", "`~` needs an int, found str"),
        (
            "[1, 2, 3].take(-1)",
            "`take` needs a count of 0 or more, found -1",
        ),
        (
            "[1, 2, 3].drop(-2)",
            "`drop` needs a count of 0 or more, found -2",
        ),
        (
            "[1, 2, 3].take(\"a\")",
            "`take` expects a non-negative int, found str",
        ),
    ] {
        let errs = eval_errors(expr);
        assert!(errs[0].contains(message), "{expr}: {}", errs[0]);
        assert_eq!(errs[1], errs[0], "{expr}: --tree");
        assert_eq!(errs[2], errs[0], "{expr}: --ir");
    }
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
