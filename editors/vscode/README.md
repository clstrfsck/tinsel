# Tinsel for VS Code

Support for Tinsel (`.tin` files), a small language for Advent of Code.

## Features

- **Highlighting:** comments, strings with `{...}` interpolation and
  string-pattern captures such as `{n:int}`, triple-quoted strings, chars,
  numbers, keywords, top-level items (`day`, `example`, `answers`, `test`,
  `test_error`, `with`, `include`), function definitions, `@memo`/`@nomemo`,
  typed patterns (`n: int`) and operators.
- **Editing:** `#` comments, bracket matching, auto-closing quotes,
  indentation after `where`, `match` and `=`, and folding by indentation.
- **Snippets:** type `day` for a new solution, or `example`,
  `examplewith`, `answers`, `test`, `test_error`, `where`, `match`,
  `clauses`, `grid`, `bfs`, `dijkstra`, `iterate` or `dbg`.
- **Errors on save:** when a file is opened or saved, `tinsel check`
  reads it (without running it) and its first error, such as a syntax
  error or an unknown name, is underlined and listed in the Problems panel.
  An error in an included file is shown in that file.
- **Test Explorer:** the Testing view lists every `.tin` file in the
  workspace, by folder, with its `test`, `test_error`, `example` and
  `answers` blocks. Run all of them, a folder, a file or one block from the
  view or from the ▶ in the editor's margin; each gets a ✓ or ✗, and a
  failure shows its message (such as `part1: expected 6, got 5`) at the
  block. This runs `tinsel test --json`. A file that doesn't load is marked
  with its error.
- **Run and test lenses:** "▶ Run" and "✓ Test" above the `day` line run
  `tinsel run` and `tinsel test` on the file in a terminal. Failed tests,
  failed examples and errors appear in the Problems panel and as squiggles.
  "▶ Run example N" above each `example` shows its `part1` and `part2` in
  the Tinsel output panel.
- **Evaluate a selection:** select an expression, such as `graph["you"]`
  or `ways("svr", 0)`, and press Cmd+Alt+E (Ctrl+Alt+E elsewhere). Its
  value appears after the selection, in full when you hover over it, and in
  the Tinsel output panel. With nothing selected, the word under the cursor
  is evaluated. Cmd+Alt+Shift+E (Ctrl+Alt+Shift+E) evaluates on one of the
  program's examples instead of the real input. Both are also in the
  editor's right-click menu. This runs `tinsel eval` (SPEC 2.7), so only
  what the expression needs is computed.
- **Tasks:** "Tasks: Run Task" lists `run` and `test` for the current file,
  and a `tasks.json` can name its own:

  ```json
  { "type": "tinsel", "command": "test", "file": "tests/aoc2024" }
  ```

  The problem matchers `$tinsel`, `$tinsel-checks` and `$tinsel-examples`
  can be used in any task that runs the CLI.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `tinsel.path` | empty | The `tinsel` binary. If empty: `target/release/tinsel` in the workspace, then `target/debug/tinsel`, then `tinsel` on the `PATH`. |
| `tinsel.engine` | `native` | Which engine runs programs: `native`, `tree` or `ir`. |
| `tinsel.checkOnSave` | `true` | Check a file with `tinsel check` when it is opened or saved. |

Build the interpreter first (`cargo build --release` in this repository),
since the lenses and evaluation run it. The file is saved before it is run.

## Installing

The same extension works in VS Code and in Cursor. For both, see
"VS Code and Cursor" in the [top-level README](../../README.md). In short,
from the repository root:

```bash
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/local.tinsel   # VS Code
ln -s "$PWD/editors/vscode" ~/.cursor/extensions/local.tinsel   # Cursor
```

Then run "Developer: Reload Window". A link made earlier under another
name, such as `local.tinsel-0.2.0`, points at the same folder and keeps
working.

Or package a `.vsix` with
`npx @vscode/vsce package --allow-missing-repository --skip-license`, and
install it with `code --install-extension` or `cursor --install-extension`.

## Changing the extension

- **The grammar** is `syntaxes/tinsel.tmLanguage.json`. TextMate rules are
  tried in order, and a wrong rule doesn't fail: it just highlights
  wrongly, often for every line after it (an unclosed string, for example).
  After a change, check it against real `.tin` files, for example with the
  `vscode-textmate` and `vscode-oniguruma` npm packages, which are the
  tokenizer VS Code itself uses.
- **The code** is plain JavaScript, so there is no build step:
  `extension.js` holds what needs VS Code, and `lib.js` what doesn't
  (finding blocks and checks, choosing the binary, reading errors and JSON
  output). The tests cover `lib.js`, check the problem matchers against the
  CLI's real output, and check that every result `tinsel test --json`
  reports lines up with a block that `lib.js` finds.
  From the repository root, after `cargo build --release`:

  ```bash
  node --test editors/vscode/test
  ```
