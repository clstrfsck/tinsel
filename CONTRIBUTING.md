# Contributing to Tinsel

Tinsel is a small language implementation with three execution engines: a
tree-walking reference interpreter, an interpreter for Tinsel's mid-level IR,
and a Cranelift JIT. Contributions should preserve the same language behaviour
and error reporting in all three.

## Before You Start

Read the parts of [`SPEC.md`](SPEC.md) relevant to your change. The
specification is the source of truth for language behaviour; section 11 records
design decisions that should not be reopened without discussion.

Two rules protect the test corpus:

- Do not invent behaviour that the specification does not define. Propose the
  specification wording before implementing it.
- Do not change an expected answer in a `.tin` test merely to make a test pass.
  Advent of Code answers are checked against the reference solutions. If an
  expected value appears wrong, verify it independently and explain the
  discrepancy.

Puzzle inputs under `input/YYYY/` are private and gitignored. Never commit them
or copy them into tests.

## Set Up the Project

Tinsel uses stable Rust, edition 2024. Rust 1.88 or newer is required.

```sh
cargo build
cargo test
```

Use a release build when running full puzzle inputs or measuring performance:

```sh
cargo build --release
./target/release/tinsel run tests/aoc2024/day01.tin
```

The full suite does not require private puzzle inputs. Checks that need a
missing input are skipped; examples and feature tests still run.

## Understand the Architecture

The main compilation and execution path is:

```text
source
  → lexer
  → layout
  → parser / AST
  → name resolution and call graph
    ├→ tree-walking evaluator
    └→ mid-level IR
         ├→ IR interpreter
         └→ Cranelift JIT
```

The principal modules are:

| Area | Files | Responsibility |
|---|---|---|
| Front end | `lexer.rs`, `layout.rs`, `parser.rs`, `ast.rs` | Tokens, indentation-sensitive layout, Pratt parsing and syntax trees |
| Resolution | `resolve.rs`, `memo.rs`, `moves.rs` | Names, frame slots, call graph, memoization decisions and last-use information |
| Reference engine | `eval.rs` | Tree-walking evaluation, generators, lazy constants and tail-call trampoline |
| Values | `value/`, `int.rs`, `list.rs` | Runtime representation, structural equality and hashing, integers and collections |
| Standard library | `builtins/` | Built-ins grouped like SPEC section 9 |
| Mid-level IR | `ir/` | Lowering, optimizations, liveness and the checking interpreter |
| Native engine | `native/` | Cranelift code generation and the runtime boundary |
| Loading and checks | `runner.rs`, `main.rs` | Includes, inputs, examples, tests, answers and the CLI |

The tree walker defines the reference behaviour. The IR interpreter validates
lowering independently of native code. Native execution shares much of the IR
runtime so differences are primarily in control flow and ownership.

Important invariants include:

- Values are immutable and structurally equal and hashable, except functions
  and generators where the specification says otherwise.
- Integers use an `i64` fast path and promote to `BigInt` on overflow.
- Maps and sets are persistent, sorted collections and iterate
  deterministically. `ValueMap` and `ValueSet` (`src/value/coll.rs`) use the
  radix tree in `src/value/radix.rs` while every key is a small int, or
  every key a packed `(x, y)` pair, and `imbl` otherwise. The two forms must
  agree on order, equality and hashing, so a change to how keys compare
  must be made in both.
- Top-level and `where` constants are lazy, evaluated at most once, and report
  dependency cycles.
- Tail calls use constant stack space. Non-tail recursive calls grow the stack
  through `stacker`.
- Runtime errors carry a source span and a short Tinsel stack trace.

## Find the Right Place to Make a Change

Most language changes cross more than one layer:

- **Syntax:** update the lexer or parser, the AST, resolution if names or
  bindings are involved, and every execution path that evaluates the new AST.
- **A built-in:** add it to the matching `src/builtins/` module and its
  `BUILTINS` table. Use `Args::check`, `type_err` and source-aware errors
  consistently.
- **Value semantics:** start in `src/value/`, `src/int.rs` or `src/list.rs`.
  Re-check equality, hashing, internal ordering, display and ownership
  together; sets, maps and memoization rely on these agreeing.
- **Evaluation or control flow:** implement reference behaviour in `eval.rs`,
  then update IR lowering and interpretation. Update native code generation
  or its runtime entry points when the operation cannot use an existing IR
  path.
- **Diagnostics, files or checks:** look in `error.rs`, `runner.rs` and
  `main.rs`.

Search for an analogous feature before adding a new path. Existing code usually
shows the expected error shape, ownership convention and test location.

## Follow the Specification

Every language feature or behaviour change needs:

1. Specification text in `SPEC.md`.
2. At least one feature test under `tests/features/`.
3. Implementation in every affected engine.

Keep implementation comments focused on invariants and reasons rather than
restating the code. Cite the relevant specification section where it clarifies
a non-obvious rule.

Discuss new dependencies before adding them. The current crates were chosen for
specific architectural roles, including Logos, `num-bigint`, `imbl`, `stacker`,
Ariadne, mimalloc and Cranelift.

## Add Tests

Tinsel's main behavioural tests are programs containing `test`,
`test_error`, `example` and `answers` declarations:

- Put focused language tests in the matching `tests/features/*.tin` file.
- Use `test_error` for invalid programs or runtime failures; check the error
  text in a Rust test when its exact wording or source span matters.
- Add Rust unit tests for internal algorithms and invariants that are awkward
  to express in Tinsel.
- Keep whole-program regressions in `tests/aocYYYY/`.
- Test the command line in the Rust files in `tests/`: `debugging.rs`
  (`dbg`, `eval` and error traces), `tools.rs` (`check` and `--json`),
  `engines.rs` (the engine flags) and `memo_warning.rs` (the warning for
  large memo keys). A change to
  `main.rs`, `runner.rs` or the wording of error output is checked there,
  not by the `.tin` files.
- The VS Code extension in `editors/vscode/` has its own tests, run with
  Node.js after a release build (`node --test editors/vscode/test`).

Feature files run on the tree walker, IR interpreter and native engine by
default. AoC programs run on the tree walker in the ordinary debug suite to
keep it reasonably fast.

Useful focused commands:

```sh
# Match one or more test paths
cargo test --test run_tin -- basics collections

# Show individual passing Tinsel checks
TIN_VERBOSE=1 cargo test --test run_tin -- functions

# Run the selected corpus on one engine
TIN_ENGINE=tree cargo test --test run_tin -- grids
TIN_ENGINE=ir cargo test --test run_tin -- grids
TIN_ENGINE=native cargo test --test run_tin -- grids

# Run one Rust unit test
cargo test resolve::tests::clauses_separated_by_another_item_are_rejected
```

When fixing an edge case, test boundaries on all affected representations:
small integers, `i64` limits, `BigInt`, `inf`/`-inf`, empty collections and
single-element collections where relevant.

## Handle Errors Safely

Anything a Tinsel program can cause must return a runtime or compile error, not
panic. In particular:

- Do not use `unwrap`, `expect`, indexing or unchecked arithmetic on
  user-controlled data unless an earlier invariant makes it impossible to
  fail and the invariant is clear locally.
- Use checked integer operations and promote to `BigInt` where required.
- Preserve the source span when converting lower-level failures into
  `Error`.
- Keep messages concise and state what was expected and what was found.
- Do not add truthiness, cross-type numeric coercions or cross-type ordering;
  these are deliberate language rules.

Panics and `expect` are acceptable in tests and for documented internal
invariants that cannot be reached from malformed user input.

## Work on the IR and Native Backend

Read [`reference/native-plan.md`](reference/native-plan.md) before changing the
IR, tagged value representation, generated calling convention or runtime entry
points. It documents the ownership and safety model behind code that may
otherwise look unusual.

The crate denies `unsafe` by default. Only the native runtime and one-word value
implementation opt in under the rules in the native plan. Keep new code safe
unless it must cross that boundary. For every unsafe operation:

- state the exact validity, lifetime, ownership and alignment assumptions;
- keep the unsafe block as small as possible;
- ensure generated code and runtime comments agree about borrowed and consumed
  words;
- return errors through the runtime context rather than unwinding across
  compiled code.

Debug feature tests check IR and native heap-word accounting. A leak or double
free is a test failure. Use `TIN_LEAKS=1` to display the tree walker's known
exception as well.

Performance changes should be measured with a release build and representative
programs. Do not complicate the runtime based only on intuition; record the
benchmark and preserve reference-engine behaviour.

## Run the Required Checks

Before handing off a change, run:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

After changing `editors/vscode/`, or the CLI output it reads, also run:

```sh
cargo build --release
node --test editors/vscode/test
```

The full suite must pass without changing expected values. If a check cannot be
run, state which one and why.

## Keep Changes Reviewable

- Keep a change focused on one behaviour or coherent refactor.
- Include the specification and tests in the same change as the
  implementation.
- Avoid unrelated formatting or cleanup in functional patches.
- Explain subtle representation or ownership decisions in comments.
- Use short, imperative commit messages.
