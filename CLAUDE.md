# Tinsel

Tinsel is a small, dynamically typed, pure language for solving Advent of
Code puzzles. The interpreter is written in Rust.

**`SPEC.md` is the source of truth.** Read the sections relevant to a task
before changing the interpreter. Spec section 11 records design decisions
that are already settled, with their reasoning. Don't reopen them without
asking the user.

## Repository layout

```
SPEC.md               language specification
src/                  front end, resolver, tree-walker, CLI (main.rs, runner.rs)
src/value/            values, ValueMap/ValueSet and the radix tree
src/ir/, src/native/  the mid-level IR and its interpreter; the Cranelift JIT
tests/features/*.tin  feature tests (test / test_error declarations)
tests/aocYYYY/*.tin   whole AoC solutions, with example and answers blocks
tests/*.rs            the .tin harness (run_tin.rs) and tests of the CLI
editors/vscode/       the VS Code/Cursor extension (plain JS; node --test)
reference/            the user's own solutions in other languages, plus Python
                      references; these are the ground truth for answers
input/YYYY/           puzzle inputs: gitignored, never commit them or copy them into tests
```

## How to work

- **Every feature needs a spec entry and a `.tin` test.** If the spec doesn't
  cover something, propose the wording to the user. Don't invent behaviour.
- **Never change an expected value in a `.tin` test to make it pass.** The
  answers come from the reference solutions. If a test looks wrong, check it
  independently (for example, with a quick Python script) and tell the user.
- Run `cargo test` after every change. Before saying a change is done,
  run `just check` (fmt, clippy, all tests, the native corpus and the
  extension's tests); it must pass.
- **All three engines must agree:** the tree-walker (`--tree`, the
  reference), the IR interpreter (`--ir`) and native code (the default).
  Same results, same errors, same traces.
- The original milestones A to E (from `basics.tin` to every `tests/aoc*/`
  program) are all done; every `.tin` file must keep passing.
- Real-input `answers` checks are skipped, with a note, when `input/YYYY/` is
  missing. Example checks always run.

## Architecture rules

- **Rust edition 2024, stable toolchain. No `unsafe`**, except in the
  native compiler's few modules that need it, under the rules in
  `reference/native-plan.md` section 4 (agreed 2026-11-10).
- **Crates:**
  - `logos` for the lexer;
  - a hand-written Pratt parser;
  - `num-bigint` for integers;
  - `imbl` (`OrdMap`, `OrdSet`, `Vector`) for persistent collections;
  - `stacker` for deep recursion;
  - `mimalloc` as the global allocator (set in `src/value/word.rs`);
  - `ariadne` for error reports;
  - `md5` for the library's `md5` (SPEC 9.1);
  - the Cranelift crates (`cranelift-codegen`, `-frontend`, `-module`,
    `-jit`, `-native`) for the native compiler (`reference/native-plan.md`).
  
  Ask before adding other dependencies.
- **Pipeline:** layout pass on the tokens, then parse into the AST, then
  resolve names and build the call graph. From there, either the
  tree-walking evaluator, or lowering to the mid-level IR (`src/ir/`),
  which the IR interpreter runs or Cranelift compiles (`src/native/`,
  `reference/native-plan.md`).
- **Values are immutable.** They're shared with `Rc`. Implement `Hash` and
  `Eq` structurally. Ordering is only between values of the same type; an
  ordering across types returns a runtime error, never a panic. Equality
  across types is just `false`. Bools are not numbers, and there is no
  truthiness.
- **Integers:** an `i64` fast path, promoting to `BigInt` on overflow (use
  the `checked_*` operations). `inf`/`-inf` are part of the integer type.
- **Maps and sets** are `value::ValueMap`/`ValueSet` (`src/value/coll.rs`).
  While all keys are small ints, or all are packed pairs, they use the
  radix tree in `src/value/radix.rs`; any other key moves them to
  `imbl::OrdMap`/`OrdSet`. Both forms iterate in sorted order, and
  equality and hashing never depend on the form.
- **Memoization (SPEC 8.1):** compute the strongly connected components of
  the static call graph. Memoize a function when it is in a recursive SCC
  **and** at least one of its recursive calls is not in tail position. A
  function in a `where` block keeps its cache in the environment of the
  enclosing call, not globally.
- **Tail calls must run in constant stack space.** Implement this with a
  trampoline in the evaluator's call loop. Wrap non-tail calls in
  `stacker::maybe_grow`, so a recursion depth of 1,000,000 works.
- **Top-level and `where` constants are lazy thunks**, evaluated at most
  once, with a "currently evaluating" mark so that a constant depending on
  itself is reported as an error naming the cycle.
- **Runtime errors** carry a source span and a short Tinsel stack trace.
  Never `panic!` or `unwrap()` on anything a user's program can cause.
- **The test harness** (`tests/run_tin.rs`) finds every `.tin` file under
  `tests/`. It reports each `test`, `test_error`, `example` and `answers`
  block as a separate pass or fail, with the file and line.

## Conventions

- Keep built-in functions in `src/builtins/`, grouped as in SPEC 9 (strings,
  collections, grids, iteration, numbers).
- Error messages say what was expected and what was found, e.g.
  `cannot order str against int`.
- Commit messages are short and in the imperative mood. Commit only when the
  user asks.
