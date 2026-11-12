//! `tinsel check` and the `--json` output of `check` and `test`, for
//! editors and other tools (SPEC 2.7). These run the interpreter and check
//! what it prints.

use std::path::PathBuf;
use std::process::Command;
use tinsel::runner::json_string;

fn save(name: &str, src: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tools");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("temporary program");
    path.to_str().expect("utf-8 path").to_string()
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

#[test]
fn check_reports_the_first_error_without_running() {
    // `check` doesn't run anything, so a runtime error isn't reported.
    let good = save("good.tin", "part1 = [1][5]\n");
    assert_eq!(
        tinsel(&["check", &good]),
        (true, String::new(), String::new())
    );
    assert_eq!(
        tinsel(&["check", "--json", &good]),
        (true, "{\"errors\":[]}\n".into(), String::new())
    );

    let bad = save("bad.tin", "x = 1\npart1 = x + nope\n");
    // Paths are compared as the CLI writes them: quoted as JSON, which
    // escapes a Windows path's backslashes.
    let bad_q = json_string(&bad);
    let (ok, out, _) = tinsel(&["check", "--json", &bad]);
    assert!(!ok);
    assert_eq!(
        out.trim(),
        format!(
            "{{\"errors\":[{{\"file\":{bad_q},\"line\":2,\"column\":13,\"end_line\":2,\"end_column\":17,\"message\":\"unknown name `nope`\"}}]}}"
        )
    );
    // Without --json, the usual report.
    let (ok, out, err) = tinsel(&["check", &bad]);
    assert!(!ok && out.is_empty());
    assert!(err.contains("unknown name `nope`"), "{err}");
}

#[test]
fn check_json_finds_errors_in_included_files() {
    save("lib_bad.tin", "helper(x) = x + \"a\\tb\" + missing\n");
    let main = save(
        "uses_lib.tin",
        "include \"lib_bad.tin\"\npart1 = helper(1)\n",
    );
    let (ok, out, _) = tinsel(&["check", "--json", &main]);
    assert!(!ok);
    // The error is in the included file, at its own line and column.
    let lib = main.replace("uses_lib.tin", "lib_bad.tin");
    let lib_q = json_string(&lib);
    assert!(out.contains(&format!("\"file\":{lib_q}")), "{out}");
    assert!(out.contains("\"line\":1,\"column\":26"), "{out}");
    assert!(out.contains("unknown name `missing`"), "{out}");

    let quoted = save("quoted.tin", "part1 = (1 +\n");
    let (_, out, _) = tinsel(&["check", "--json", &quoted]);
    assert!(out.contains("\"line\":2,\"column\":1"), "{out}");
}

#[test]
fn test_json_gives_a_line_per_check_and_a_summary() {
    let file = save(
        "checks.tin",
        "test \"ok\" = 1 == 1\ntest \"say \\\"no\\\"\" = 1 == 2\ntest_error \"err\" = 1 / 0\n\n\
         example \"\"\"\n1 2\n\"\"\"\n  part1 = 4\n\npart1 = input.ints.sum\n",
    );
    let broken = save("broken.tin", "part1 = nope\n");
    let (file_q, broken_q) = (json_string(&file), json_string(&broken));
    let (ok, out, _) = tinsel(&["test", "--json", &broken, &file]);
    assert!(!ok);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            format!(
                "{{\"file\":{broken_q},\"kind\":\"load\",\"outcome\":\"fail\",\"message\":\"unknown name `nope`\",\"error\":{{\"file\":{broken_q},\"line\":1,\"column\":9,\"end_line\":1,\"end_column\":13,\"message\":\"unknown name `nope`\"}}}}"
            ),
            format!(
                "{{\"file\":{file_q},\"line\":1,\"kind\":\"test\",\"name\":\"ok\",\"outcome\":\"pass\",\"message\":\"\"}}"
            ),
            format!(
                "{{\"file\":{file_q},\"line\":2,\"kind\":\"test\",\"name\":\"say \\\"no\\\"\",\"outcome\":\"fail\",\"message\":\"evaluated to false\"}}"
            ),
            format!(
                "{{\"file\":{file_q},\"line\":3,\"kind\":\"test_error\",\"name\":\"err\",\"outcome\":\"pass\",\"message\":\"\"}}"
            ),
            format!(
                "{{\"file\":{file_q},\"line\":5,\"kind\":\"example\",\"name\":\"example 1\",\"outcome\":\"fail\",\"message\":\"part1: expected 4, got 3\"}}"
            ),
            "{\"passed\":2,\"failed\":3,\"skipped\":0}".to_string(),
        ]
    );
}

#[test]
fn check_usage_errors() {
    for args in [
        &["check"][..],
        &["check", "a.tin", "b.tin"],
        &["check", "--bad"],
    ] {
        let (ok, _, err) = tinsel(args);
        assert!(!ok);
        assert!(err.contains("usage:"), "{args:?}: {err}");
    }
}
