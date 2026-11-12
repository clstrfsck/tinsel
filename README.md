# Tinsel

Tinsel is a small, pure, dynamically typed language for solving
[Advent of Code](https://adventofcode.com) puzzles. The interpreter is
written in Rust.

```tinsel
day 2024.1

example """
3   4
4   3
2   5
1   3
3   9
3   3
"""
  part1 = 11
  part2 = 31

[left, right] = input.lines.map(.ints).transpose

part1 = zip(left.sort, right.sort).map((a, b) => abs(a - b)).sum
part2 = left.map(x => x * right.count(x)).sum
```

A program names its puzzle, gives the worked examples from the puzzle text
with their expected answers, and defines `part1` and `part2`. The language
has a lot of built-in support for Advent of Code:
- grids and `(x, y)` vectors;
- string patterns for parsing;
- automatic memoization;
- `bfs` and `dijkstra`;
- cycle detection;
- persistent sets and maps.

[`SPEC.md`](SPEC.md) is the full language definition.
See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the architecture, development
workflow and testing requirements.

## Building

You need a stable Rust toolchain that supports edition 2024 (Rust 1.88 or
newer). Install it from [rustup.rs](https://rustup.rs) if you don't have it.

```sh
cargo build --release
```

This builds the interpreter at `target/release/tinsel`. Use the release
build for real puzzle inputs: some solutions take several seconds even
with optimisation.

## Running a solution

```sh
./target/release/tinsel run tests/aoc2024/day01.tin
```

```
part1: 2769675
part2: 24643097
```

`run` checks every `example` block first. If an example fails, it prints
the expected and actual values and doesn't run the real input. Options:

| Option | Effect |
|---|---|
| `--input PATH` | Read the puzzle input from `PATH` instead of searching for it |
| `--no-examples` | Skip the example checks |
| `--tree` | Run on the tree-walking interpreter (the reference implementation) instead of compiling to machine code |
| `--ir` | Run on the IR interpreter, which checks the native compiler |

`tinsel test` takes `--tree` and `--ir` too.

### Where the input comes from

For a program that starts with `day 2024.1`, the interpreter looks for
`input/2024/y24d01.txt` in the program's directory, then in each parent
directory, and uses the first one it finds. So with this layout, every
solution under `tests/` finds its input automatically:

```
input/2024/y24d01.txt
input/2024/y24d02.txt
...
tests/aoc2024/day01.tin
```

The `input/` directory is gitignored, because Advent of Code asks that
puzzle inputs not be published. Download your own inputs from the site
while logged in, and save each one under the name above.

### Timing solutions

The interpreter doesn't time itself; use the shell's `time` with the
release build. Add `--no-examples` to time only the real input:

```sh
time ./target/release/tinsel run tests/aoc2024/day22.tin                 # examples, then the real input
time ./target/release/tinsel run --no-examples tests/aoc2024/day22.tin   # the real input only
```

To time every solution, one line each:

```sh
cargo build --release
for f in tests/aoc*/day*.tin; do
  /usr/bin/time -p ./target/release/tinsel run --no-examples "$f" 2>&1 >/dev/null | awk -v f="$f" '/^real/ {print f, $2}'
done
```

Leave out `--no-examples` to include the examples. Narrow the glob to time
one year, as in `tests/aoc2018/day*.tin`.

[`TIMINGS.md`](TIMINGS.md) uses the real input only, because some
examples take as long as the real input and others take no time at all.
Each time there is the faster of two runs. A solution whose input file is
missing stops with an error, so its time means nothing.

## Writing a solution

A solution file usually has four parts.

```tinsel
day 2023.6                       # which puzzle; this decides the input file

example """
Time:      7  15   30
Distance:  9  40  200
"""
  part1 = 288                    # expected answers for the example
  part2 = 71503

answers                          # your accepted answers, as a regression check
  part1 = 160816
  part2 = 46561107

[times, records] = input.lines.map(.ints)

ways(T, D) = T + 1 - 2 * (0..T / 2).bisect(t => t * (T - t) > D)

part1 = zip(times, records).map(ways).product
part2 = ways(int(times.join), int(records.join))
```

- Inside an `example` check, `input` is that example's text. Everywhere
  else, it is the real input.
- An example can override a constant with a `with` line, for puzzles where
  the example uses a different parameter (for example, a 7×7 grid instead
  of 71×71):

  ```tinsel
  example """
  ...
  """
    with size = 6
    part1 = 22

  size = 70
  ```

- `answers` is optional. Add it once the site has accepted your answers,
  so later changes to the interpreter can't silently break the solution.
- Code shared between days goes in its own file, without a `day` header,
  and each day includes it, with a path relative to the including file:

  ```tinsel
  day 2019.9
  include "intcode.tin"
  ```

  The included file's definitions join the program. Its own `test`s run
  only when it is tested by itself (SPEC 2.8).

### Debugging

`dbg(x)` prints a value to stderr, with where it was called and the
expression's text, and returns it unchanged, so it can go in the middle of
a chain. Large values are cut short.

```tinsel
part1 = reachable(start).dbg.len    # prints day15.tin:42:9: reachable(start) = {(1, 2), (1, 3), (2, 3)}
```

`tinsel eval` shows the value of any expression in a program, without
editing it, on the real input or on one of the examples:

```sh
./target/release/tinsel eval tests/aoc2025/day11.tin 'graph["you"]'
./target/release/tinsel eval tests/aoc2025/day11.tin --example 1 'ways("svr", 0)'
```

Only what the expression needs is computed, so this is a quick way to
check what parsing gives, or what one function returns on one case.

For complete programs, see the solutions in `tests/aoc2018` to
`tests/aoc2025`. All of 2018 to 2025 is there.
[`TIMINGS.md`](TIMINGS.md) lists every solution with how long it takes
on its real input (see "Timing solutions" above).

## Running the tests

### The whole suite

```sh
cargo test
```

This runs:
- the Rust unit tests;
- every `.tin` file under `tests/`. Each `test`, `test_error`, `example`
  and `answers` block counts as a separate check:

```
75 files: 608 passed, 0 failed, 0 skipped
```

A failure shows the file, the line and both values:

```
FAIL tests/aoc2024/day18.tin:41 answers "answers"
      part2: expected "64,11", got "12,3"
```

An `answers` check is **skipped**, with a note, when the input file for
that day isn't present. So the suite still passes on a machine without
your inputs; only the examples and feature tests run there.

### Part of the suite

To run only the `.tin` files whose path contains some text, pass that text
as a filter:

```sh
cargo test --test run_tin -- aoc2024        # one year
cargo test --test run_tin -- basics memo    # several feature files
TIN_VERBOSE=1 cargo test --test run_tin -- functions   # list passing checks too
```

You can also use the interpreter directly. This is faster, because it uses
the release build:

```sh
./target/release/tinsel test tests/aoc2024/day18.tin   # one file
./target/release/tinsel test tests/aoc2023             # a directory
./target/release/tinsel test                            # everything under .
```

### Before committing interpreter changes

```sh
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

All three must be clean.

## VS Code and Cursor

`editors/vscode/` is an extension for `.tin` files. It gives:
- highlighting, folding, auto-indentation and snippets (type `day` for a
  new solution);
- "▶ Run" and "✓ Test" above the `day` line, and "▶ Run example N" above
  each example, with failed checks and errors in the Problems panel;
- evaluating a selected expression with Cmd+Alt+E (Ctrl+Alt+E elsewhere),
  its value shown next to it, on the real input or on an example.

The lenses and evaluation run the interpreter, so build it first with
`cargo build --release`. [`editors/vscode/README.md`](editors/vscode/README.md)
describes every feature and setting.

Cursor is built on VS Code, so the same extension works in both. Only the
extensions folder and the command-line tool are different:

| | VS Code | Cursor |
|---|---|---|
| Extensions folder (macOS, Linux) | `~/.vscode/extensions` | `~/.cursor/extensions` |
| Extensions folder (Windows) | `%USERPROFILE%\.vscode\extensions` | `%USERPROFILE%\.cursor\extensions` |
| Command-line tool | `code` | `cursor` |

The command-line tools are optional. If `code` isn't on your `PATH`, run
"Shell Command: Install 'code' command in PATH" from the Command Palette
(Cmd+Shift+P on macOS, Ctrl+Shift+P elsewhere). Cursor has the same
command for `cursor`.

### Option 1: link the folder (best if you'll change the extension)

From the repository root, link the extension into the editor's extensions
folder:

```sh
# VS Code
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/local.tinsel-0.2.0

# Cursor
ln -s "$PWD/editors/vscode" ~/.cursor/extensions/local.tinsel-0.2.0
```

Then restart the editor, or run "Developer: Reload Window" from the Command
Palette. After that, a change to the extension takes effect on the next
reload. A link made for an earlier version, such as
`local.tinsel-0.1.0`, points at the same folder, so reloading is enough to
pick up a new version.

On Windows, copy the folder instead, or make a directory link from a
Command Prompt opened in the repository root:

```bat
mklink /D "%USERPROFILE%\.vscode\extensions\local.tinsel-0.2.0" "%CD%\editors\vscode"
mklink /D "%USERPROFILE%\.cursor\extensions\local.tinsel-0.2.0" "%CD%\editors\vscode"
```

To uninstall, delete the link and reload the window.

### Option 2: package and install a `.vsix`

Package the extension with Microsoft's `vsce` tool. This needs Node.js;
`npx` downloads `vsce` the first time. The two flags stop `vsce` from
asking about the missing repository URL and licence file, which this local
extension doesn't need:

```sh
cd editors/vscode
npx @vscode/vsce package --allow-missing-repository --skip-license
```

This writes `tinsel-0.2.0.vsix`. Install it from the command line:

```sh
code --install-extension tinsel-0.2.0.vsix      # VS Code
cursor --install-extension tinsel-0.2.0.vsix    # Cursor
```

Or open the Extensions view, click the `...` menu at the top of the view,
choose "Install from VSIX...", and pick the file. This works in both
editors.

The editor keeps its own copy, so after changing the extension you need to
package and install it again; add `--force` to `--install-extension` to
replace the installed copy. To uninstall, use the Extensions view, or run
`code --uninstall-extension local.tinsel` (`cursor` for Cursor).

### Checking that it works

Open any `.tin` file, such as `tests/aoc2024/day01.tin`. The language
indicator at the right of the status bar should say **Tinsel**, and
keywords, strings and comments should be coloured. "▶ Run | ✓ Test"
should appear above the `day` line; if clicking one says it can't find the
binary, run `cargo build --release` or set `tinsel.path`.

If it says "Plain Text":
- check that the extension appears under "Installed" in the Extensions view;
- run "Developer: Reload Window";
- with Option 1, check that the link points at the real folder:
  `ls ~/.vscode/extensions/local.tinsel-0.2.0/` (or `~/.cursor/...`)
  should list `package.json`.

## Repository layout

```
SPEC.md               the language specification
src/                  the interpreter: lexer, layout, parser, resolver, evaluator
src/builtins/         the standard library, grouped as in SPEC section 9
tests/features/       feature tests (test / test_error declarations)
tests/aocYYYY/        complete solutions, with examples and accepted answers
reference/            reference solutions in other languages
editors/vscode/       syntax highlighting for VS Code
input/YYYY/           puzzle inputs (gitignored; not included)
```
