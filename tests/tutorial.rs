//! Checks `docs/tutorial.md`, so that what it shows is what Tinsel does.
//!
//! - Each ```` ```tinsel ```` block is a program, compiled and checked (its
//!   `test`s and `example`s) on all three engines. A block after
//!   `<!-- start: NAME -->` begins a program that later blocks after
//!   `<!-- continues: NAME -->` add to; the whole is checked once.
//! - A block after `<!-- file: PATH -->` must be exactly that file.
//! - Each ```` ```console ```` block holds `$ tinsel ...` commands, each
//!   followed by what it prints (stdout, then stderr). The commands are run
//!   from the repository root.

use std::path::Path;
use std::process::Command;
use tinsel::runner::{Engine, Outcome, compile, run_checks_with};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A fenced block: its info string, its marker comment, its first line
/// (from 1) and its text.
struct Block {
    lang: String,
    marker: Option<String>,
    line: usize,
    text: String,
}

fn blocks() -> Vec<Block> {
    let text = std::fs::read_to_string(root().join("docs/tutorial.md")).expect("the tutorial");
    let mut out = Vec::new();
    let mut current: Option<Block> = None;
    let mut marker = None;
    for (i, line) in text.lines().enumerate() {
        if let Some(block) = current.as_mut() {
            if line == "```" {
                out.extend(current.take());
            } else {
                block.text.push_str(line);
                block.text.push('\n');
            }
        } else if let Some(lang) = line.strip_prefix("```") {
            current = Some(Block {
                lang: lang.to_string(),
                marker: marker.take(),
                line: i + 2,
                text: String::new(),
            });
        } else if let Some(m) = line
            .strip_prefix("<!-- ")
            .and_then(|l| l.strip_suffix(" -->"))
        {
            marker = Some(m.to_string());
        } else if !line.trim().is_empty() {
            marker = None;
        }
    }
    assert!(current.is_none(), "the tutorial ends inside a code block");
    out
}

/// The programs to check: each standalone block, and each `start:` block
/// with its `continues:` blocks. Each is named by its first line.
fn programs() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut chains: Vec<(String, usize)> = Vec::new();
    for b in blocks().into_iter().filter(|b| b.lang == "tinsel") {
        let marker = b.marker.as_deref().unwrap_or("");
        if let Some(name) = marker.strip_prefix("continues: ") {
            let &(_, i) = chains
                .iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("line {}: no program `{name}` to continue", b.line));
            out[i].1.push('\n');
            out[i].1.push_str(&b.text);
        } else {
            if let Some(name) = marker.strip_prefix("start: ") {
                chains.push((name.to_string(), out.len()));
            }
            out.push((format!("docs/tutorial.md:{}", b.line), b.text));
        }
    }
    out
}

#[test]
fn every_program_passes_its_checks_on_every_engine() {
    let mut failures = Vec::new();
    for (name, src) in programs() {
        let prog =
            compile(src.clone()).unwrap_or_else(|e| panic!("{name}: doesn't compile: {}", e.msg));
        for which in ["tree", "ir", "native"] {
            let engine = match which {
                "ir" => Engine::ir(&prog),
                "native" => Engine::native(&prog),
                _ => Ok(Engine::Tree),
            }
            .unwrap_or_else(|e| panic!("[{which}] {name}: doesn't compile: {}", e.msg));
            let results = run_checks_with(&prog, Path::new("tutorial.tin"), &engine);
            assert!(
                !results.is_empty(),
                "{name}: a program with nothing to check"
            );
            for r in results {
                match r.outcome {
                    Outcome::Pass => {}
                    Outcome::Fail(m) | Outcome::Skip(m) => {
                        failures.push(format!("[{which}] {name}, {} {:?}: {m}", r.kind, r.name));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn file_listings_match_their_files() {
    let mut listed = 0;
    for b in blocks() {
        if let Some(path) = b.marker.as_deref().and_then(|m| m.strip_prefix("file: ")) {
            let file = std::fs::read_to_string(root().join(path))
                .unwrap_or_else(|e| panic!("line {}: can't read {path}: {e}", b.line));
            assert_eq!(b.text, file, "line {}: differs from {path}", b.line);
            listed += 1;
        }
    }
    assert_eq!(listed, 2, "first.tin and garden.tin are listed");
}

/// `line` split into words as a shell would, for the simple quoting the
/// tutorial uses: '...' and "...".
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.get_or_insert_with(String::new).push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_with(String::new);
            }
            (None, ' ') => words.extend(word.take()),
            (None, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    // A shell would wait for the closing quote; a command with one missing
    // is a mistake in the tutorial, not something to guess at.
    assert!(quote.is_none(), "an unclosed {quote:?} quote in `{line}`");
    words.extend(word);
    words
}

/// `text` with trailing spaces and blank lines removed, which a reader
/// can't see.
fn tidy(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    lines.join("\n").trim_end().to_string()
}

#[test]
fn every_command_prints_what_the_tutorial_shows() {
    let mut commands = 0;
    for b in blocks().into_iter().filter(|b| b.lang == "console") {
        let mut runs: Vec<(String, String)> = Vec::new();
        for line in b.text.lines() {
            match line.strip_prefix("$ ") {
                Some(cmd) => runs.push((cmd.to_string(), String::new())),
                None => {
                    let (_, expected) = runs
                        .last_mut()
                        .unwrap_or_else(|| panic!("line {}: output before a command", b.line));
                    expected.push_str(line);
                    expected.push('\n');
                }
            }
        }
        for (cmd, expected) in runs {
            let words = shell_words(&cmd);
            assert_eq!(words[0], "tinsel", "line {}: only tinsel commands", b.line);
            let out = Command::new(env!("CARGO_BIN_EXE_tinsel"))
                .args(&words[1..])
                .current_dir(root())
                .output()
                .expect("run tinsel");
            let printed = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                tidy(&printed),
                tidy(&expected),
                "line {}: `{cmd}` printed something else",
                b.line
            );
            commands += 1;
        }
    }
    assert!(commands >= 8, "only {commands} commands checked");
}

#[test]
fn shell_words_handles_quotes() {
    assert_eq!(
        shell_words("tinsel eval f.tin 'steps(start)'"),
        ["tinsel", "eval", "f.tin", "steps(start)"]
    );
    assert_eq!(shell_words("a \"b c\" ''"), ["a", "b c", ""]);
}

#[test]
#[should_panic(expected = "an unclosed Some('\"') quote in `tinsel eval \"foo`")]
fn shell_words_rejects_an_unclosed_quote() {
    shell_words("tinsel eval \"foo");
}
