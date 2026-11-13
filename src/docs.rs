//! The library reference, `docs/library.md`, built into the binary for
//! `tinsel doc` (SPEC 2.7). `tests/docs.rs` checks that every built-in has
//! an entry and that every example holds.
//!
//! The format: `## Section` headings; under them, one `### signature`
//! heading per entry, such as `### s.split(sep) → list`, then its
//! description, then a ```` ```tinsel ```` block of examples, one per line.

use crate::runner::json_string;
use std::sync::LazyLock;

/// The reference, as written.
pub const REFERENCE: &str = include_str!("../docs/library.md");

/// One example line of an entry.
#[derive(Debug, Clone)]
pub struct Example {
    /// Its line in the reference, from 1.
    pub line: usize,
    /// The expression, without any trailing `# error`.
    pub code: String,
    /// Whether it must raise an error rather than be `true`.
    pub error: bool,
}

/// One `###` entry.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The `##` section it is in, such as `Strings`.
    pub section: String,
    /// The heading, such as `int(s), int(s, base) → int`.
    pub signature: String,
    /// The heading's line, from 1.
    pub line: usize,
    /// The built-ins the heading names, such as `int`.
    pub names: Vec<String>,
    /// The text between the heading and the examples, as Markdown.
    pub description: String,
    pub examples: Vec<Example>,
}

/// Every entry in the built-in reference.
pub static ENTRIES: LazyLock<Vec<Entry>> = LazyLock::new(|| parse(REFERENCE));

/// The entries for `name`, in the order they appear.
pub fn lookup(name: &str) -> Vec<&'static Entry> {
    ENTRIES
        .iter()
        .filter(|e| e.names.iter().any(|n| n == name))
        .collect()
}

/// The names in a heading. Each form, separated by `, ` outside brackets,
/// is a call such as `s.split(sep)` or `int(s, base)`, or a bare name such
/// as `N`: the name is the identifier before any argument list, after any
/// receiver.
pub fn heading_names(heading: &str) -> Vec<String> {
    let forms = heading.split(" → ").next().unwrap_or(heading);
    let mut names: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut form = String::new();
    for c in forms.chars().chain(std::iter::once(',')) {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            let call = form.trim().split('(').next().unwrap_or("");
            let name = call.rsplit('.').next().unwrap_or(call);
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            form.clear();
        } else {
            form.push(c);
        }
    }
    names
}

/// The entries of a reference in this format.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut section = String::new();
    let mut in_example = false;
    // Whether the current entry's description is still being read.
    let mut describing = false;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if in_example {
            if line.starts_with("```") {
                in_example = false;
            } else if !line.trim().is_empty()
                && let Some(entry) = entries.last_mut()
            {
                let (code, error) = match line.trim_end().strip_suffix("# error") {
                    Some(code) => (code.trim_end().to_string(), true),
                    None => (line.trim_end().to_string(), false),
                };
                entry.examples.push(Example {
                    line: n,
                    code,
                    error,
                });
            }
        } else if let Some(heading) = line.strip_prefix("### ") {
            entries.push(Entry {
                section: section.clone(),
                signature: heading.trim().to_string(),
                line: n,
                names: heading_names(heading),
                description: String::new(),
                examples: Vec::new(),
            });
            describing = true;
        } else if let Some(heading) = line.strip_prefix("## ") {
            section = heading.trim().to_string();
            describing = false;
        } else if line.starts_with("```tinsel") {
            in_example = true;
            describing = false;
        } else if describing
            && let Some(entry) = entries.last_mut()
            // Blank lines before the description are skipped.
            && (!entry.description.is_empty() || !line.trim().is_empty())
        {
            entry.description.push_str(line);
            entry.description.push('\n');
        }
    }
    for entry in &mut entries {
        let trimmed = entry.description.trim_end().to_string();
        entry.description = trimmed;
    }
    entries
}

/// An entry as text, for the terminal.
pub fn entry_text(entry: &Entry) -> String {
    let mut out = format!("{}    ({})\n\n", entry.signature, entry.section);
    out.push_str(&entry.description);
    out.push_str("\n\n");
    for ex in &entry.examples {
        let mark = if ex.error { "  # error" } else { "" };
        out.push_str(&format!("    {}{mark}\n", ex.code));
    }
    out
}

/// An entry as one JSON object, for `doc --json` (SPEC 2.7).
pub fn entry_json(entry: &Entry) -> String {
    let list = |items: &mut dyn Iterator<Item = String>| {
        items.map(|s| json_string(&s)).collect::<Vec<_>>().join(",")
    };
    let examples = list(&mut entry.examples.iter().map(|ex| {
        if ex.error {
            format!("{} # error", ex.code)
        } else {
            ex.code.clone()
        }
    }));
    format!(
        "{{\"section\":{},\"signature\":{},\"names\":[{}],\"description\":{},\"examples\":[{examples}]}}",
        json_string(&entry.section),
        json_string(&entry.signature),
        list(&mut entry.names.iter().cloned()),
        json_string(&entry.description),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_give_their_names() {
        assert_eq!(heading_names("s.lines → list"), ["lines"]);
        assert_eq!(heading_names("int(s), int(s, base) → int"), ["int"]);
        assert_eq!(heading_names("s.split(sep) → list"), ["split"]);
        assert_eq!(heading_names("ord(c), chr(n) → int"), ["ord", "chr"]);
        assert_eq!(heading_names("N, E, S, W → tuple"), ["N", "E", "S", "W"]);
    }

    #[test]
    fn entries_have_their_parts() {
        let text = "# Title\n\nIntro.\n\n## Strings\n\nAbout strings.\n\n\
                    ### s.trim → str\nNo whitespace.\nAt either end.\n\n\
                    ```tinsel\n\" a \".trim == \"a\"\n1.trim # error\n```\n";
        let entries = parse(text);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.section, "Strings");
        assert_eq!(e.signature, "s.trim → str");
        assert_eq!(e.line, 9);
        assert_eq!(e.description, "No whitespace.\nAt either end.");
        assert_eq!(e.examples.len(), 2);
        assert_eq!(e.examples[0].code, "\" a \".trim == \"a\"");
        assert!(!e.examples[0].error);
        assert_eq!(
            (e.examples[1].code.as_str(), e.examples[1].error),
            ("1.trim", true)
        );
        assert_eq!(
            entry_json(e),
            "{\"section\":\"Strings\",\"signature\":\"s.trim → str\",\"names\":[\"trim\"],\
             \"description\":\"No whitespace.\\nAt either end.\",\
             \"examples\":[\"\\\" a \\\".trim == \\\"a\\\"\",\"1.trim # error\"]}"
        );
    }

    #[test]
    fn the_built_in_reference_has_every_section() {
        let sections: Vec<&str> = ENTRIES.iter().map(|e| e.section.as_str()).collect();
        for s in ["Strings", "Collections", "Grids and positions", "Debugging"] {
            assert!(sections.contains(&s), "no {s} section");
        }
        assert_eq!(lookup("set").len(), 3);
        assert!(lookup("nonexistent").is_empty());
    }
}
