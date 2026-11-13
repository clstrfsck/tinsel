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

#[test]
fn doc_prints_a_builtins_entries() {
    let (ok, out, err) = tinsel(&["doc", "split"]);
    assert!(ok, "{err}");
    assert!(out.starts_with("s.split(sep) → list    (Strings)\n\nSplits on"));
    assert!(out.contains("\n    \"abc\".split(\"\")  # error\n"));
    // The method form works too, and a name can have several entries.
    let (ok, out, _) = tinsel(&["doc", ".set"]);
    assert!(ok);
    assert!(out.contains("set(xs), set() → set"));
    assert!(out.contains("g.set(p, v) → grid    (Grids and positions)"));
}

#[test]
fn doc_reports_a_name_without_an_entry() {
    let (ok, out, err) = tinsel(&["doc", "nonexistent"]);
    assert!(!ok);
    assert_eq!(out, "");
    assert_eq!(
        err,
        "Error: no entry for `nonexistent` in the library reference\n"
    );
}

#[test]
fn doc_json_gives_one_object_per_entry() {
    let (ok, out, err) = tinsel(&["doc", "--json", "chr"]);
    assert!(ok, "{err}");
    assert_eq!(
        out,
        "{\"section\":\"Strings\",\"signature\":\"chr(n) → char\",\"names\":[\"chr\"],\
         \"description\":\"The char for a Unicode code point. An invalid code point is an error.\",\
         \"examples\":[\"chr(97) == 'a'\",\"chr(ord('a') + 2) == 'c'\",\"chr(-1) # error\"]}\n"
    );
    let (ok, all, _) = tinsel(&["doc", "--json"]);
    assert!(ok);
    assert_eq!(all.lines().count(), tinsel::docs::ENTRIES.len());
    assert!(all.lines().all(|l| l.starts_with("{\"section\":")));
}

#[test]
fn test_names_keep_their_braces() {
    let path = save("brace_names.tin", "test \"{{x}} and {\" = true\n");
    let (ok, out, err) = tinsel(&["test", "--json", &path]);
    assert!(ok, "{err}");
    assert!(
        out.contains("\"name\":\"{{x}} and {\""),
        "the name is as written: {out}"
    );
}

#[test]
fn a_where_under_one_match_arm_is_explained() {
    let path = save(
        "where_arm.tin",
        "f(x) = match x\n  0 => a\n    where\n      a = 1\n  _ => 2\n",
    );
    let (ok, _, err) = tinsel(&["check", &path]);
    assert!(!ok);
    assert!(
        err.starts_with(
            "Error: a `where` block can't belong to one `match` arm: put it after the last \
             arm, lined up with the arms, where it serves every arm, or move this arm's work \
             into a function\n"
        ),
        "{err}"
    );
}

#[test]
fn an_include_path_keeps_its_braces() {
    // Not interpolated (SPEC 3), so a lone `{` in the path is plain text.
    save("lone{brace.tin", "helper = 41\n");
    let main = save(
        "includes_brace.tin",
        "include \"lone{brace.tin\"\n\npart1 = helper + 1\n",
    );
    let (ok, out, err) = tinsel(&["run", "--input", "/dev/null", &main]);
    assert!(ok, "{err}");
    assert_eq!(out, "part1: 42\n");
    // A function named `include` is still an ordinary definition.
    let f = save("include_fn.tin", "include(x) = x + 1\npart1 = include(1)\n");
    let (ok, out, err) = tinsel(&["run", "--input", "/dev/null", &f]);
    assert!(ok, "{err}");
    assert_eq!(out, "part1: 2\n");
}

#[test]
fn a_capture_in_a_triple_quoted_pattern_is_located() {
    // The newline after the opening quotes is dropped from the text, but
    // the capture's position still counts it.
    let path = save(
        "triple_capture.tin",
        "f(\"\"\"\nName: {name}\nAge: {age:float}\"\"\") = 1\n",
    );
    let (ok, out, _) = tinsel(&["check", "--json", &path]);
    assert!(!ok);
    assert!(out.contains("\"line\":3,\"column\":6"), "{out}");
}

#[test]
fn symbols_link_each_use_to_its_definition() {
    save("sym_lib.tin", "twice(n) = n * 2\n");
    let main = save(
        "sym_main.tin",
        "include \"sym_lib.tin\"\n\
         example \"x\"\n  with x = 2\n  part1 = 8\n\n\
         x = 5\n\
         f(0) = 0\n\
         f(x) = g(x)\n  where\n    g(y) = twice(y) + k\n    k = 1\n\n\
         part1 = f(x).map(_ + 1)\n",
    );
    let (ok, out, err) = tinsel(&["symbols", "--json", &main]);
    assert!(ok, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    let field = |l: &str, name: &str| -> String {
        let rest = l.split(&format!("\"{name}\":")).nth(1).unwrap_or("");
        rest.split([',', '}'])
            .next()
            .unwrap_or("")
            .trim_matches('"')
            .to_string()
    };
    let lib = PathBuf::from(&main).with_file_name("sym_lib.tin");
    // The id of the definition of `name`, checking what it is, where, and
    // what it is inside.
    let def = |name: &str, what: &str, file: &str, at: (u32, u32), container: Option<&str>| {
        let l = lines
            .iter()
            .find(|l| l.starts_with("{\"kind\":\"def\"") && field(l, "name") == name)
            .unwrap_or_else(|| panic!("no definition of {name} in\n{out}"));
        assert_eq!(field(l, "what"), what, "{l}");
        assert_eq!(field(l, "file"), file, "{l}");
        assert_eq!(
            (field(l, "line"), field(l, "column")),
            (at.0.to_string(), at.1.to_string()),
            "{l}"
        );
        assert_eq!(
            field(l, "end_column"),
            (at.1 as usize + name.len()).to_string(),
            "{l}"
        );
        assert_eq!(field(l, "container"), container.unwrap_or("null"), "{l}");
        field(l, "id")
    };
    let f = def("f", "function", &main, (7, 1), None);
    let twice = def("twice", "function", lib.to_str().unwrap(), (1, 1), None);
    let x = def("x", "constant", &main, (6, 1), None);
    def("g", "where function", &main, (10, 5), Some(&f));
    def("k", "where constant", &main, (11, 5), Some(&f));
    // What the use at (line, column) refers to.
    let ref_at = |line: u32, column: u32| {
        lines
            .iter()
            .find(|l| {
                l.starts_with("{\"kind\":\"ref\"")
                    && field(l, "line") == line.to_string()
                    && field(l, "column") == column.to_string()
            })
            .map(|l| field(l, "def"))
            .unwrap_or_else(|| panic!("no use at {line}:{column} in\n{out}"))
    };
    // The second clause's parameter `x` hides the constant: the use in
    // `g(x)` refers to the parameter.
    let param = lines
        .iter()
        .find(|l| {
            field(l, "what") == "parameter" && field(l, "line") == "8" && field(l, "column") == "3"
        })
        .map(|l| field(l, "id"))
        .expect("the parameter x");
    assert_eq!(ref_at(8, 10), param);
    // The constant is used by `part1`, and named by the example's `with`.
    assert_eq!(ref_at(13, 11), x);
    assert_eq!(ref_at(3, 8), x);
    // The second clause's head names `f` again; `twice` is used from the
    // included file.
    assert_eq!(ref_at(8, 1), f);
    assert_eq!(ref_at(10, 12), twice);
    // `_` makes a lambda, but isn't a name in the program.
    assert!(!out.contains("\"name\":\"_\""), "{out}");
}

#[test]
fn symbols_can_read_unsaved_text() {
    let main = save("sym_stdin.tin", "old = 1\n");
    let out = Command::new(env!("CARGO_BIN_EXE_tinsel"))
        .args(["symbols", "--stdin", &main])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(b"new = 2\nused = new + 1\n")?;
            child.wait_with_output()
        })
        .expect("run tinsel");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(text.contains(":1:1: constant new (1 use)"), "{text}");
    assert!(!text.contains("old"), "{text}");
}
