# Native compilation plan for Tinsel

Status: phase 0 done 2026-11-10, result **go** (section 8). Phase 1
done 2026-11-10 (section 9). Phase 2's value representation done
2026-11-10 (section 10); its runtime entry points moved to phase 3.
Phase 3 done 2026-11-11 (section 11): the whole corpus passes natively.
Phase 4 done 2026-11-11 (section 12): native code is faster than the
tree-walker and Racket on all 15 solutions, with a median 2.99× over
today's tree-walker (3.7× over the plan's), just short of the 3× goal;
what's left is in `next-steps.md`. Native code became the default for
`tinsel run` and `tinsel test` on 2026-11-11 (`--tree` for the
tree-walker); whole-corpus timings are in `TIMINGS.md`. The
tree-walking interpreter on `main` stays the reference implementation
throughout.

## 1. Goal and success criteria

Compile Tinsel programs to native code so that they are **uniformly**
faster than both the tree-walker and the Racket implementation (branch
`racket-based`). The Racket route reached a median 1.99× over the
tree-walker on the 15 slowest solutions, but it was slower on three of
them:

| Solution | Rust tree-walker | Racket | Racket speed-up |
|---|---|---|---|
| 2019 day 25 | 2.51 s | 0.76 s | 3.31× |
| 2024 day 6 | 1.67 s | 0.53 s | 3.17× |
| 2018 day 24 | 5.72 s | 1.91 s | 2.99× |
| 2020 day 23 | 10.18 s | 3.46 s | 2.94× |
| 2020 day 15 | 8.53 s | 3.65 s | 2.34× |
| 2023 day 23 | 3.32 s | 1.57 s | 2.12× |
| 2018 day 9 | 3.51 s | 1.69 s | 2.08× |
| 2021 day 23 | 3.51 s | 1.84 s | 1.91× |
| 2018 day 11 | 3.44 s | 1.93 s | 1.78× |
| 2019 day 18 | 2.86 s | 1.76 s | 1.63× |
| 2022 day 23 | 5.33 s | 3.60 s | 1.48× |
| 2024 day 22 | 2.96 s | 2.31 s | 1.28× |
| 2018 day 14 | 8.79 s | 9.67 s | 0.91× |
| 2018 day 15 | 10.24 s | 11.70 s | 0.87× |
| 2022 day 20 | 3.03 s | 4.74 s | 0.64× |

Success means, on these 15 solutions (remeasure the tree-walker first,
since it has changed since this table):

- **Every solution is faster than both the tree-walker and Racket.**
- The median speed-up over the tree-walker is **at least 3×**.
- All of `tests/` passes on the native backend, with the same results and
  the same error behaviour (messages, spans, Tinsel stack traces).

The three solutions where Racket was slower spend most of their time in the
runtime: list growth by `++` (2018 day 14), OrdMap/OrdSet comparisons
(2018 day 15) and list rotation (2022 day 20). The native backend reuses
the Rust runtime, so it shouldn't regress on them. Making them clearly
faster needs runtime work as well as code generation; see phase 4.

## 2. Key decisions

### 2.1 No MLIR, and no direct AST → machine IR either

LLVM IR and Cranelift IR only know machine words, pointers and calls. What
makes Tinsel fast or slow has to be decided before that level: which
values are small ints, which calls are self tail calls (loops), where a
local's last use allows an in-place update, how pattern clauses share
tests, and which `where` constants are never forced. So there's a
**Tinsel-specific mid-level IR, written in Rust**, between the resolved AST
and the backend.

MLIR is the general framework for such IRs, but it doesn't suit Tinsel. Its
API is C++ and TableGen, and defining a dialect from Rust (through
`melior` and the C API) is immature. Its existing dialects target
statically typed numeric code, and none of them models tagged dynamic
values, refcounts or persistent collections. The languages closest to
Tinsel (Lean 4, Koka, Roc) all use their own IR and then emit C or LLVM IR.

### 2.2 Cranelift first, with an in-process JIT

The backend is **Cranelift**, used as a JIT inside the `tinsel` binary:

- It's pure Rust, with no system LLVM, so `cargo build` stays simple.
- It compiles quickly, so `tinsel run` can compile on every run, with no
  object files, linker or cache.
- It has guaranteed tail calls (the `tail` calling convention and
  `return_call`).
- The runtime is the same binary: compiled code calls Rust functions
  registered as JIT symbols. There's no separate static library or C
  header to keep in sync.

Cranelift's code is somewhat slower than LLVM's at `-O2`, but most time in
these programs is spent in the runtime (collections, BigInt, hashing),
which rustc compiles with LLVM anyway. The mid-level IR stays independent
of the backend. If profiles later show that the time is in generated code,
an LLVM backend (`inkwell`) can be added behind the same IR.

### 2.3 One-word tagged values, refcounted

Profiling of the tree-walker (closure compilation reached only 1.07×)
showed that the cost is the value, frame and calling model, not
dispatch: 32-byte `Value` clone and drop, frame slots, argument vectors,
and `Result` plumbing. Compiling to calls on today's `Value` would remove
only dispatch, for an estimated 1.2–1.5×. So the backend needs a new value
representation:

- A **64-bit word**. One tag pattern holds a 63-bit fixnum (an int that
  fits); others hold `bool`, `char`, `none`, `inf` and `-inf` as
  immediates; and one marks a pointer to a refcounted heap object (BigInt,
  str, tuple, list, set, map, range, function, rangeset, generator, grid).
  The exact bit layout is settled in phase 0. The aim is that fixnum add
  and compare need no untagging, only an overflow check.
- **Refcounting stays**, not a tracing GC. In-place updates (`set`, `add`,
  `|`, `++` at a local's last use; SPEC 4.1) depend on knowing a value is
  unique. The compiler inserts the increments and decrements and removes
  the ones it can prove unnecessary, as in Lean 4 and Koka's Perceus.
  `moves.rs` already does a light version of this.
- The **interpreter moves to the same representation** (phase 2), so there
  is one runtime and one library. The existing test corpus then checks the
  new representation before any code generation exists.

### 2.4 Calls, errors and the stack

- **Calling convention.** Every compiled Tinsel function takes
  `(env, a1, …, an)` as words and returns a word, using Cranelift's `tail`
  convention. Tail calls use `return_call`, so loops run in constant stack
  space (SPEC 8.1). Self tail calls become plain loops in the mid-level IR.
  Rust can't call the `tail` convention directly, so each function that Rust
  can call (an entry point, or a closure passed to `map`, `fold`, `search`
  and so on) gets a small generated shim with the platform's C convention.
- **Errors.** A runtime call that fails records a pending error in the
  context and returns a sentinel word. Compiled code checks for the sentinel
  after each call that can fail and returns at once. Nothing unwinds through
  generated code.
- **Spans and stack traces.** Each call site and each operation that can
  fail gets an ID in a static table of spans. Calls push their call-site ID
  on a cheap **shadow stack** and pop it on return. A tail call replaces the
  top entry and increments a counter, so the SPEC 8.3 trace (most recent
  first, repeats collapsed, tail-call counts) comes out exactly as now.
- **Recursion depth.** The program runs on a thread with a large reserved
  stack, so a depth of 1,000,000 needs no `stacker` (SPEC 8.2). Function
  prologues compare the stack pointer against a limit, so running out of
  stack becomes a Tinsel error with a trace rather than a crash. Phase 0
  checks that a very large stack reservation works on macOS and Linux.

### 2.5 Program structure

- **Constants.** Top-level and `where` constants are thunk cells with three
  states (unevaluated, evaluating, done). "Evaluating" gives the cycle
  error (SPEC 2.1).
- **Runs.** Top-level constants live in a per-run context. Each `example`
  run, and the real-input run, gets a fresh context with its own `input` and
  its `with` overrides (SPEC 2.3).
- **Memoization.** Memoization is an explicit wrapper in the mid-level IR,
  calling the runtime's FxHash memo tables. A `where` function's cache
  lives in the enclosing call's environment (SPEC 8.1). The memo-key size
  warning stays in the runtime.
- **Checks.** `test`, `test_error`, `example` and `answers` are compiled
  like any other definition, and the runner drives them as it does now.

## 3. The mid-level IR

It's built from the resolved AST, after `resolve.rs` and the call graph
(so memoization decisions are already known):

- **Shape.** A-normal form: every intermediate value is named, and control
  flow is explicit blocks with parameters (close to SSA, and easy to lower
  to Cranelift).
- **Closures.** Closure conversion makes environments explicit. `where`
  blocks become an environment record holding the thunk cells and the
  local functions' memo tables.
- **Patterns.** All pattern kinds, including string patterns (SPEC 6.1),
  compile to decision trees (Maranget-style), so clauses share tests and
  each value is examined once.
- **Comprehensions and generators.** These lower to loops over an iterator
  protocol, with native counted loops for ranges and index loops for lists
  and tuples. Generators stay runtime objects, which keeps the
  single-pass rule (SPEC 4) in one place.
- **Annotations.** Tail calls, last uses (from `moves.rs`) and memo
  wrappers are explicit in the IR.
- **Types.** Phase 4 adds a simple local type inference over the IR (int,
  bool, "known tuple of n", unknown), so int-heavy code can be specialised.

A small **IR interpreter** comes with it, used only in tests. Running the
whole corpus through it checks the lowering before any machine code exists.
It can be dropped once the backend covers everything, if it becomes a
burden.

## 4. Use of `unsafe`

The user allowed limited `unsafe` for this work (2026-11-10), with extra
care that each use really is safe. The rules:

- **Where.** `unsafe` is confined to three modules: the tagged word
  (`src/value/word.rs` or similar: pointer tagging, `Rc::into_raw` and
  `Rc::from_raw`); the JIT boundary (turning finished code into function
  pointers and calling them); and the runtime entry points that compiled
  code calls. The crate root has `#![deny(unsafe_code)]`, and only those
  modules carry `#[allow(unsafe_code)]`. `word.rs` also holds the global
  allocator (added 2026-11-11 with the user's agreement): mimalloc, called
  as plain `mi_malloc` for alignments up to 8, since every mimalloc block
  has that alignment. It is left out under Miri, which can't run
  mimalloc's C code.
- **Documentation.** Every `unsafe` block has a `// SAFETY:` comment
  stating the invariant it relies on. Clippy enforces this with
  `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block`.
- **Interfaces.** The word type's safe API (clone, drop, inspect, build)
  can't be used to create an invalid word or a dangling pointer. Code
  outside the module never sees raw pointers.
- **Ownership.** Each runtime entry point documents its convention: which
  arguments it borrows, which it consumes, and whether its result is
  owned. The code generator follows a single table of these conventions,
  so it's written down once.
- **No unwinding across the boundary.** Runtime entry points return errors
  as values and never panic. A panic out of an `extern "C"` function aborts
  under edition 2024, which is a safe failure but still a bug to fix.
- **Checking.**
  - Miri runs the unit tests of the word module and the refcounting
    helpers. Miri can't run JIT code, so the boundary needs the other
    checks.
  - Debug builds check the tag of every word passed to the runtime, and
    count live heap objects so the test harness can report leaks and
    double frees.
  - Cranelift's IR verifier is on in debug builds and tests.
  - The whole corpus runs on both the interpreter and the native backend
    (differential testing).
- **Review.** Each change to an `unsafe` module gets its own review
  against these rules before it's committed.

CLAUDE.md's "No `unsafe`" rule and crate list need updating to match when
phase 0 starts.

## 5. Dependencies

Only the Cranelift family, from one project (Bytecode Alliance):
`cranelift-codegen`, `cranelift-frontend`, `cranelift-module`,
`cranelift-jit` and `cranelift-native`. Nothing else is planned. An LLVM
backend would add `inkwell` and need a system LLVM, which is one reason it
is deferred.

## 6. Phases

Each phase ends with `cargo test`, `cargo clippy -- -D warnings` and
`cargo fmt` clean, and timings recorded against the table in section 1.

### Phase 0: spike and go/no-go (about 1 week)

On a branch, outside the main pipeline:

- Compile a tiny subset (int literals and arithmetic, comparisons, `if`,
  `match` on ints, named functions, tail calls, one collection call into
  the runtime) to Cranelift by hand.
- Try both a boxed version (today's `Value`, behind a pointer) and the
  tagged-word version on a few micro loops and on a cut-down int-heavy
  solution (2020 day 15 or day 23), so it's clear how much each part
  contributes.
- Check the big-stack thread and the stack-limit check, and the shim
  between the C and `tail` conventions.
- **Go/no-go:** at least 3× over the tree-walker on the int-heavy code
  with tagged words. If not, stop, and compare with continuing on Racket.

### Phase 1: mid-level IR (2–3 weeks)

The lowering from the resolved AST (section 3) and the test-only IR
interpreter. The whole corpus passes on the IR interpreter. No new value
representation yet, so this can overlap with phase 2.

### Phase 2: runtime and value representation (3–4 weeks; the largest phase)

- The tagged word replaces `Value` in the interpreter and the whole
  library (`src/builtins/`, `list.rs`, `int.rs`, collections). This also
  shrinks every collection element from 32 bytes to 8.
- Runtime entry points for compiled code, with their ownership table.
- The pending-error mechanism and the shadow stack, shared with the
  interpreter where that's practical.
- **Gate:** the corpus passes on the interpreter, and its timings are no
  worse than before (the smaller values may well make them better).

### Phase 3: code generation (2–3 weeks)

- Lower the IR to Cranelift. Follow milestones A to E in order, as the
  interpreter did (CLAUDE.md), checking each feature file on both
  backends.
- Add `tinsel run --native` and `tinsel test --native`. The test harness
  runs every `.tin` file on both backends.
- Everything boxed and generic at first, apart from the fixnum fast paths
  that come for free from the tagged word.
- **Gate:** all of `tests/` passes natively; timings against section 1.

### Phase 4: optimisation (open-ended)

In rough order of expected value, each measured and kept only if it helps:

1. Local type inference and specialisation: unboxed int loops with guards
   that fall back to the generic path.
2. Inline the small builtins (`len`, indexing, tuple access, comparisons,
   `abs`, `min`/`max` of two values) as Cranelift code instead of calls.
3. Refcount elision (borrowed parameters, Perceus-style reuse).
4. Runtime work for the solutions that are mostly runtime: 2018 day 14
   (`++` growth), 2018 day 15 and 2022 day 23 (OrdMap/OrdSet comparisons;
   the deferred hash map/set item), and 2022 day 20 (list rotation).
5. Make `--native` the default for `tinsel run` once it's complete and
   faster everywhere; keep the interpreter as the reference and for
   `--interp`.

## 7. Risks

- **Phase 2 is a large refactor** of the interpreter and library (about
  6,000 lines touch `Value`). It can be done type by type behind the
  existing tests, but it's where the most time will go.
- **Refcounting bugs** in generated code show up as leaks or crashes, not
  test failures. Debug-build object counting and tag checks (section 4)
  are there to catch them early.
- **Runtime-bound solutions** may gain little from code generation, which
  threatens the "uniformly faster" goal. Phase 4 item 4 addresses them
  directly, and the tagged word's smaller collections should help them
  too.
- **Cranelift code quality** may limit the int-heavy cases. If so, add an
  LLVM backend behind the same IR rather than changing the design.

## 8. Phase 0 results (2026-11-10)

The spike was `spike/native0/`, a separate package that used `tinsel` as
a library for parsing and resolution, so the main crate didn't depend on
Cranelift yet. It was removed after phase 1; commit `46147c3` still has
it, with a README saying how to run it.

### What it compiles

- **Subset.** Named functions with name, `_` and int-literal parameters;
  `part1`/`part2`; ints, bools, literals, locals, `+ - * / %`, unary `-`
  and `not`, single comparisons, `and`/`or`, `if`, and calls of named and
  library functions (also as methods). Also indexing and list literals.
  Memoized functions are rejected.
- **Values.** Words as in 2.3: fixnums tagged `…0`, `Rc<Value>` pointers
  `…01`, and immediates `…11` (`false`, `true`, `none`, `ERR`).
  `--boxed` makes every int a pointer too.
- **Ownership.** Perceus-style: parameters are owned, and a local's last
  use on each path consumes it. Each branch first drops the locals it never
  uses. Runtime functions either borrow or consume their arguments, per a
  table in `rt.rs`.
- **Inline code.** Fixnum `+ - * / %` (floor division) and comparisons are
  inline, with an overflow or non-fixnum fallback to the runtime.
- **Calls.** Calls use Cranelift's `tail` convention, and tail calls use
  `return_call`. Rust calls in through generated C-convention shims.
- **Runtime checks.**
  - Every prologue compares the stack pointer with a limit.
  - `--trace` keeps a shadow stack of call sites.
  - Errors return `ERR`, with the message in a thread-local.
- **Library calls.** These go through the tree-walker's library
  (`rt_builtin`, with a `Vec` of `Value`s), except `set`, which has its own
  entry point that updates a uniquely held list inside its `Rc` (switched
  off with `--generic`).

### Correctness

- All five benchmarks and an edge-case file give the tree-walker's results.
  The edge cases cover floor division and modulo of every sign, fixnum
  overflow into BigInt, BigInt arithmetic, and boolean operators. `inf` is
  outside the subset.
- Debug builds count live heap objects. Every successful run ends with 0.
  Runs that stop with an error leak what the failing function owned, as
  designed for the spike.
- Type errors, non-bool conditions, division by zero and stack overflow
  are reported as errors, not crashes. A 100,000,000-deep non-tail recursion
  runs on a 64 GB stack reservation in 0.63 s (about 32 bytes per frame,
  3.2 GB resident). The tree-walker gets this depth through `stacker`.
- `cargo clippy -- -D warnings` is clean with `undocumented_unsafe_blocks`
  and `multiple_unsafe_ops_per_block`. Every `unsafe` block is in
  `word.rs`, `rt.rs` or `compile.rs`, and has a `SAFETY` comment.
- Miri (`cargo +nightly miri test` in `spike/native0`) passed the three
  `word.rs` tests: tagging, `from_raw`/`into_raw`, in-place update, and
  the refcount drop. Words cross into generated code as plain `u64`s, so
  pointers must use exposed provenance; strict provenance isn't possible.
  `word.rs` says so explicitly, with `expose_provenance` and
  `with_exposed_provenance` in one place each. The package's
  `.cargo/config.toml` sets `MIRIFLAGS=-Zmiri-permissive-provenance`, which
  silences Miri's warning about this. Phase 2's `word.rs` should do the
  same. Miri can't run JIT-compiled code, so generated code stays covered
  by the other checks in section 4.

### Timings

Best of three, wall clock (including start-up and compilation), release
builds, Apple M3 Max. The benchmarks were in `spike/native0/bench/`:
`day15` and `day23` are 2020 days 15 and 23 restated in the subset, on
their examples' inputs (30 million turns; a million cups and ten million
moves).

| Benchmark | Tree-walker | Racket (solve only) | Boxed | Tagged, generic calls | Tagged, direct `set` |
|---|---|---|---|---|---|
| day15 | 8.51 s | 3.16 s | 3.09 s (2.8×) | 2.52 s (3.4×) | 1.64 s (5.2×) |
| day23 | 9.96 s | 2.04 s | 2.55 s (3.9×) | 2.26 s (4.4×) | 1.35 s (7.4×) |
| deep (recursion 10M deep) | 2.59 s | 1.61 s | 0.52 s (5.0×) | 0.078 s (33×) | 0.078 s (33×) |
| fib(32), no memo | 0.55 s | 0.064 s | 0.26 s (2.1×) | 0.019 s (29×) | 0.019 s (29×) |
| loop (100M tail calls) | 9.46 s | 0.98 s | 7.70 s (1.2×) | 0.65 s (15×) | 0.65 s (15×) |

The boxed column also uses the direct `set`. Racket times are its own
"solved in" figure, which leaves out about 1 s of start-up and
compilation; the native times include theirs (5–30 ms).

### What the numbers say

- **Go.** Tagged words are 3.4–33× faster than the tree-walker, against
  the 3× threshold, and faster than Racket on every benchmark.
- **Tagged words matter most.** Boxed ints with compiled control flow give
  only 1.2–5×, which is the "compile against today's `Value`" case. The
  tree-walker's int loop gains 1.2× from compilation alone and 15× with
  fixnums.
- **The runtime boundary is the next cost.** On day 15, generated code
  was about 20% of samples. Most of the rest was the generic library call:
  the argument `Vec`, `Args::check`, and moving values out of their `Rc`
  and back. A direct entry point for `set` alone gave 1.5–1.7×. Phase 2's
  direct entry points for common operations (and words stored in
  collections, so no `Rc<Value>` boxing per element) are therefore worth as
  much as code generation itself on collection-heavy solutions.
- **Safety checks are nearly free.** The stack-limit check costs nothing
  measurable. The shadow stack costs nothing measurable, except for the
  page faults of a 10-million-deep trace (0.03 s).
- **Fast enough to compile on every run.** Compile time is 5–30 ms for
  these programs.

### Carried into the later phases

- **Self tail calls.** These still go through `return_call`. Turning them
  into loops (phase 1) is the next gain for loop-shaped code. *Done in the
  IR (section 9).*
- **Error reports.** Errors carry only a message. Spans and traces from the
  shadow stack come with phase 2's error mechanism.
- **Re-entrancy.** `rt_builtin` borrows the interpreter, so a library
  function that calls back into compiled code (`map`, `fold`) would
  re-enter it. Phase 2's runtime must allow re-entrant calls.
- **Freeing on error.** The error path leaks what it owns. That's harmless
  for `tinsel run`, but `tinsel test` runs many checks in one process, so
  phase 3 must free what it owns on error.

## 9. Phase 1 results (2026-11-10)

The IR is in `src/ir/`: the types and a printer (`mod.rs`), the lowering
from the resolved AST (`lower.rs`), liveness (`live.rs`) and the IR
interpreter (`interp.rs`). No `unsafe`, and no new dependencies.

**The whole corpus passes on the IR interpreter**: all 205 files, 1085
checks, including the real-input answers. Run it with
`TIN_ENGINE=ir cargo test --release --test run_tin` (about 5 minutes, against
2m45s on the tree-walker). A plain `cargo test` also runs
`tests/features/` on the IR, so the lowering can't quietly break. To read
the IR of a program: `cargo run --example ir -- FILE.tin`.

### How it's built

- **Functions.** Every top-level and `where` function, lambda, constant
  body and check becomes an IR function: blocks with parameters, in
  A-normal form. Calls to a known function with the right number of
  arguments are direct (`CallKnown`); library calls named directly are
  `CallBuiltin`; the rest go through the value.
- **Closures.** A lambda captures, by value, the variables it uses from
  enclosing functions, found as it is lowered. A closure without captures
  is made once per run, which keeps the "closed lambda is the same value"
  rule.
- **`where` blocks** become a record made once per call of the clause (as
  in section 2.5): a lazy cell per constant, memo tables for its memoized
  functions, the enclosing record, and copies of the clause variables its
  definitions use. Every `where` definition captures only its record, so
  mutually recursive `where` functions need no fixpoint over their free
  variables. A constant's own `where` block joins its enclosing record, as
  the evaluator's `init_wheres` does.
- **Self tail calls are jumps** to the entry block, for top-level and
  `where` functions, unless the function is memoized (each call's result
  must still reach the table).
- **Last uses** come from liveness over the IR, not from `moves.rs`: every
  operand says whether it moves its value, and each block lists what it
  releases on entry. The IR interpreter moves on last use, so in-place
  updates (`set`, accumulating sets) work as in the evaluator.
- **Generator comprehensions** are nested `FlatMap` generators, one per
  `for`, each calling a closure for the rest of the clauses. The outermost
  starts from a one-item list, so nothing runs until the generator is read.
  This added one generator kind to `eval.rs`.
- **The IR interpreter** runs inside the tree-walker's `Interp`, as a new
  `Func::Ir` value, so the whole library works unchanged and calls back
  into IR closures.

### Simpler than section 3, for now

- **Patterns** are tested clause by clause and arm by arm, not as
  Maranget decision trees. That's correct and easy to generate code for;
  decision trees move to phase 4, where they're an optimisation.
- **Comprehensions** loop over one generic iterator (`Iter`/`Next`). Native
  counted loops for ranges and index loops for lists are phase 4.
- **Drops are per block** (the union over its predecessors). The IR
  interpreter doesn't mind releasing an empty register, but code
  generation needs them per edge, so phase 3 splits edges or computes them
  per edge.
- **Reference cycles through records.** A `where` constant holding a
  closure that captures its own record is a cycle and leaks in the IR
  interpreter. The evaluator breaks these with `release_frame`; phase 2/3
  needs an equivalent (most likely the same check at clause exit). *Done
  in phase 3, checked at each release rather than at clause exit (section
  11).*
- Error messages match the evaluator's, but traces don't yet count tail
  calls, and a failing `test` comparing with `==` doesn't show both sides.

### Bugs the corpus found

Only one beyond the feature tests: a memo hit inside a chain of tail calls
held the memo table borrowed while the chain's results were stored in that
same table (2024 day 11).

## 10. Phase 2 results (2026-11-10)

**`Value` is now one tagged word**, in the interpreter and the whole
library. The corpus passes unchanged (all 205 files, on the tree-walker
and on the IR interpreter), and the 15 slowest solutions are all faster.

### How it's built

- **`src/value/word.rs`** is the only module with `unsafe`. It holds the
  tagged word: fixnums `…0` (62-bit, as in the spike), `Rc<Heap>` pointers
  `…01`, and immediates `…11` (`none`, bools, chars, `inf`, `-inf`). Its
  safe API can't make an invalid word: clone, drop, `kind` (a borrowed
  view), `into_rc`, `make_mut` and `ref_count`. The crate root has
  `#![deny(unsafe_code)]`, the module opts in with
  `#[allow(unsafe_code)]`, and clippy checks every `unsafe` block has a
  `SAFETY` comment and one operation. Debug builds count live heap words
  (`word::live::words`).
- **Miri** passes the word module's tests and the list tests
  (`cargo +nightly miri test --lib value::word`, likewise `list::`), with
  `-Zmiri-permissive-provenance` set in `.cargo/config.toml` as the spike
  did.
- **`Heap`** holds what used to be the enum's boxed variants, plus `Wide`
  for an `i64` outside the fixnum range (so each int has one form, and
  equality of unboxed words is equality of bits). Tuples of two or three
  items are stored inline in the heap object, so a position costs one
  allocation and one indirection.
- **Reading values.** `Value::view()` gives a `V` enum of borrowed
  payloads (ints by value), which every `match` in the interpreter and
  library now uses. `Value::unpack()` gives an owned `Own`, moving the
  payload out when the value was the only reference.
- **In-place updates** (`set`, `add`, `remove`, `|`, `-` and `++` at a last
  use; SPEC 4.1) go through `Value::make_mut()`, which updates the heap
  object where it is when nothing else refers to it, so they no longer
  allocate a new value at all.
- **The evaluator's cycle check** (`release_frame`) now counts references
  to the value's heap object rather than to the closure inside it.

### Timings

Best of two, wall clock, release builds, Apple M3 Max, real inputs with
`tinsel run --no-examples`; before is commit 46147c3 (phase 1).

| Solution | Before | After | Speed-up |
|---|---|---|---|
| 2019 day 25 | 2.54 s | 2.04 s | 1.25× |
| 2024 day 6 | 1.74 s | 1.48 s | 1.18× |
| 2018 day 24 | 5.78 s | 4.68 s | 1.24× |
| 2020 day 23 | 10.16 s | 7.58 s | 1.34× |
| 2020 day 15 | 8.63 s | 6.66 s | 1.30× |
| 2023 day 23 | 3.27 s | 2.98 s | 1.10× |
| 2018 day 9 | 3.51 s | 2.88 s | 1.22× |
| 2021 day 23 | 3.49 s | 3.08 s | 1.13× |
| 2018 day 11 | 3.53 s | 3.09 s | 1.14× |
| 2019 day 18 | 2.96 s | 2.56 s | 1.16× |
| 2022 day 23 | 5.29 s | 4.41 s | 1.20× |
| 2024 day 22 | 2.97 s | 2.36 s | 1.26× |
| 2018 day 14 | 9.05 s | 7.79 s | 1.16× |
| 2018 day 15 | 10.00 s | 8.98 s | 1.11× |
| 2022 day 20 | 3.03 s | 1.60 s | 1.89× |
| **Total** | **75.95 s** | **62.97 s** | **1.21×** |

The gate was "no worse than before". The gains come from 8-byte values
(collections take a quarter of the memory, and copying a value copies one word),
unboxed small ints in every collection, and in-place updates that no
longer re-wrap the result. 2022 day 20 gains most: it repeatedly
searches, slices and joins a list of 5,000 ints, and each of those now
moves a quarter as much memory.

### Moved to phase 3

- **Runtime entry points and their ownership table**, and **the
  pending-error mechanism and shadow stack**. Their shape depends on
  what the code generator calls, and they can't be exercised without it,
  so they're written alongside it rather than ahead of it. The
  interpreter keeps `Result` errors and its own traces.
- **Leak reporting in the test harness** from the debug live-word count.
  The IR interpreter's known record cycles (section 9) would show up
  as leaks, so this comes with their fix.
- **Further layout work.** Strings and lists are still a heap object
  pointing at a shared buffer (two indirections). Inlining them, like
  small tuples, is a phase 4 candidate if profiles point there.

## 11. Phase 3 results (2026-11-11)

`tinsel run --native` and `tinsel test --native` compile the program's IR
with Cranelift and run it. **The whole corpus passes natively**: all 205
files, 1085 checks, including the real-input answers
(`TIN_ENGINE=native cargo test --release --test run_tin`, about 3
minutes). A plain `cargo test` runs `tests/features/` on all three
engines.

### How it's built

- **One runtime.** Compiled code runs inside the IR interpreter's
  runtime: globals, `where` records, closures and memo tables are the
  same objects, and `Func::Ir` closures are the function values. Every IR
  instruction that isn't inlined calls `rt_op`, which runs the
  interpreter's own code for it (`ir::interp::exec`, which both engines
  now share). So the engines can differ only in control flow and
  ownership, and library functions that call closures (`map`, `fold`)
  re-enter compiled code through the ordinary `Interp::call`.
- **Registers are words.** `where` records, iterators and comprehension
  accumulators became `Internal` heap values (never visible to Tinsel
  code), so every IR variable is one word in both engines.
- **Calls.** `f{i}(cx, clo, site, chain, args…)` in Cranelift's `tail`
  convention. `site` and `chain` are for stack traces: a tail call puts
  the count of tail calls so far in the high half of `site`, and where
  the chain started, and the function called there, in `chain`
  (`rt::rt_trace`). Self tail calls (jumps) are counted in two machine
  variables. Known calls are direct; tail calls are `return_call`, or
  `return_call_indirect` when the callee is a compiled closure of the right
  arity, so loops through lambdas run in constant stack too. Other value
  calls go through the runtime. A memoized function's `f{i}` is a wrapper
  that looks the table up and calls the body `b{i}` on a miss. Rust enters
  through one C-convention shim per arity.
- **Ownership** follows the IR's liveness exactly: last uses move, other
  uses take a reference, and words dropped on a control-flow edge are
  released on that edge (the per-edge drops that section 9 called for).
  Each runtime entry point says which words it consumes and which it
  borrows (`native/rt.rs`).
- **Errors.** A failing entry point stores the error in the context and
  returns a sentinel word; compiled code branches to an error exit that
  releases what it owns, adds the function to the trace, and returns. The
  trace is built on the way out, as the IR interpreter does, so no shadow
  stack was needed: each function gets its call site as a parameter.
- **Stack.** Each entry from Rust runs on a stack with at least 64 MB
  left, moving to a fresh 4 GB segment (address space only) otherwise.
  Prologues compare the stack pointer with a limit 1 MB above the
  segment's end, so running out is a Tinsel error.
- **Inline fast paths** (the only ones so far): fixnum `+ - * / %` and
  bitwise operators, fixnum comparisons, `==`/`!=` by word where that
  decides it, bool checks and `not`, and matching an unboxed constant.

### Safety

- `unsafe` is in two modules: `value/word.rs` (raw words: `into_raw`,
  `from_raw`, and the sentinels, which no constructor can make) and
  `native/rt.rs` (the entry points, the context, argument arrays, and the
  call through a shim). The JIT boundary and the entry points share one
  module rather than the three of section 4, since they use the same
  conventions. Every block has a `SAFETY` comment, and clippy checks this.
- Debug builds check every word received from compiled code
  (`Value::from_raw`) and count live heap words. **A file run on the IR
  interpreter or natively fails the harness if it leaks** (or frees
  twice). The whole corpus is leak-free on both, error paths included.
- **Record cycles are freed** (section 9's open item). A `where` record
  whose cell holds a closure capturing the record is a cycle. Whenever a
  reference to a record, or to a closure capturing one, is dropped while
  the object is still shared, `ir::interp::release_cycle` checks whether
  every remaining reference to the record comes from closures held only in
  its own cells, and if so clears the cells. It is the IR engines' version
  of the evaluator's `release_frame`, but runs at every such release
  rather than once at clause exit, so it also catches a lambda that
  escaped and was dropped later. Anything it can't account for counts as
  an outside reference, so its worst case is a leak; and clearing a cell
  that was still reachable could only make it be computed again.
- **Its cost.** Only words with the new *watch* bit (bit 2 of a pointer
  word, set when a record or a `where` closure capturing one is made) call
  out of `drop`, so other values pay one bit test. The check proper runs
  only once a record's cell has held a closure capturing it. Even so, the
  extra call on each drop of a record word costs natively up to 7% (2020
  day 23, whose `look` makes a record per move) and 2–4% on three other
  solutions; the tree-walker is unaffected. Phase 4's borrowed operands
  should remove most of those drops.
- **The tree-walker still leaks** in that last case (a lambda that
  escapes a frame whose `where` constants refer back to it): about two
  values per call. Catching it there would put a frame scan on every
  lambda call, so it is left as a known exception, shown by
  `TIN_LEAKS=1`. `release_frame` did get one fix: a slot value that
  doesn't refer to the frame no longer blocks freeing it.
- Cranelift's verifier runs in debug builds.
- Dependencies: `cranelift-codegen`, `-frontend`, `-jit` and `-module`.
  `cranelift-native` isn't needed directly (`cranelift-jit` uses it).

### Timings

Best of two, wall clock, release builds, Apple M3 Max, real inputs with
`--no-examples`. The tree-walker is this commit's (phase 2's tagged
words).

| Solution | Tree-walker | Native | Speed-up |
|---|---|---|---|
| 2019 day 25 | 2.04 s | 1.52 s | 1.34× |
| 2024 day 6 | 1.44 s | 1.19 s | 1.21× |
| 2018 day 24 | 4.65 s | 5.90 s | 0.79× |
| 2020 day 23 | 7.34 s | 6.65 s | 1.10× |
| 2020 day 15 | 6.60 s | 2.03 s | 3.25× |
| 2023 day 23 | 2.98 s | 2.74 s | 1.09× |
| 2018 day 9 | 2.89 s | 1.43 s | 2.02× |
| 2021 day 23 | 3.09 s | 2.60 s | 1.19× |
| 2018 day 11 | 3.04 s | 1.77 s | 1.72× |
| 2019 day 18 | 2.47 s | 2.13 s | 1.16× |
| 2022 day 23 | 4.39 s | 4.77 s | 0.92× |
| 2024 day 22 | 2.41 s | 2.56 s | 0.94× |
| 2018 day 14 | 7.55 s | 3.64 s | 2.07× |
| 2018 day 15 | 9.28 s | 9.85 s | 0.94× |
| 2022 day 20 | 1.59 s | 1.60 s | 0.99× |

The gate was that everything passes natively, with timings recorded.
The median is 1.16×, far from the 3× goal, and four solutions are slower.
That's expected from "everything boxed and generic": all but the inlined
operations pay for an argument array, a call, a dispatch on the IR
instruction, and moving each operand into and out of a `Value`. It's the
phase 0 lesson again: the runtime boundary is the cost, and the spike's
5× on 2020 day 15 needed a direct entry point for `set`.

### Next (phase 4)

In order of what these numbers suggest:

1. **Direct entry points for the hot library calls** (`set`, `get`,
   indexing, `len`, `add`, tuple items, map and set lookups), called with
   borrowed words and no argument array, and inline code for tuple and
   list items, as phase 4 item 2 planned.
2. **Calls through values to compiled closures** without the runtime
   (`map`, `fold` and lambdas passed around), and cheaper captures.
3. Then local type inference and refcount elision (phase 4 items 1 and 3)
   and the runtime-bound solutions (item 4).

## 12. Phase 4 results (2026-11-11)

Phase 4 ran as a series of measured steps: profile one of the slower
solutions, fix what it showed, check the whole corpus on every engine,
time it. This section summarises what came of it; `next-steps.md` has
what's left. Every change keeps results and errors exactly as they were
(the same messages, in the same order), and the corpus passes natively
and on the IR interpreter with the leak check on in debug builds.

### The runtime boundary

Phase 3's cost was the generic boundary between compiled code and the
runtime (`rt_op`: look up the instruction, move values in and out). It
was taken down step by step:

- **Direct entry points** that borrow their operands, for captures,
  record values, pattern tests and items, indexing, `in`, tuples, lists,
  records, globals, comprehension steps, operators, comparisons, slices
  and library calls without named arguments (`rt_builtin`, with fast
  paths for `len`, `get` with or without a default, and `set`, each
  falling back to the library function itself so errors are
  unchanged). `TIN_PROF=1` counts what still takes `rt_op`.
- **Records as closures.** A `where` function or constant captures just
  its record, so calls pass the record itself as the closure word
  (`native::record_clo`), and no closure is allocated per call.
- **Compiled `where` constants** are forced by calling the compiled
  thunk from compiled code.
- **Calls through values** to compiled closures are direct
  `call_indirect`s, and library functions calling closures (`map`,
  `fold`) enter compiled code without the tree-walker's trampoline.

### Inline code and the value layout

- `Heap`, `Tup` and `Internal` are `#[repr(u8)]`, and their field offsets
  are measured at start-up in safe Rust (`value::layout`); only a boxed
  slice's (pointer, length) order is read in `word.rs`, which Miri
  checks. A 2- or 3-item tuple is always stored inline (`Tup::new` is
  the only constructor), which compiled code relies on.
- **Reference counting is inline**: the offset of an `Rc`'s count is
  measured at start-up (`word::rc_count_offset`, checked by Miri), and
  the runtime is called only to free an object or for a watched word.
  Debug builds still count live words for the leak check.
- **Packed pairs.** A pair of ints within ±2^27 is an immediate word
  (`x` in bits 36–63, `y` in bits 8–35; `value::word`), never a heap
  tuple, so positions cost no allocation and equal pairs have equal
  words. `V::Tuple` carries `Items`, a slice or the pair's two ints,
  which reads as `&[Value]`.
- Fixnum arithmetic, shifts, comparisons, tuple items and size tests,
  `min`/`max` of two small ints and `abs` are inline.

### Passes over the IR

In order, on every function: lowering, `cse`, `cse::gather_items`,
`simplify`, then `live`.

- **Library calls as loops** (`lower::builtin_loop` and friends):
  `filter`, `map`, `count`, `any`, `all`, `first`, `min_by`, `max_by`,
  `fold` and `scan` with a function argument become IR loops, guarded by
  `Op::Loopable` (an ordinary collection and a function value, so lazy
  and odd arguments still take the library call). Calling a closure from
  Rust costs about 33 ns, against about 7 ns per element in a loop.
- **Generator comprehensions passed to library functions**
  (`lower::fused_generator`): `sum` and `product` fold as they go, `any`,
  `all` and `first` stop at the deciding item (`Op::CheckElem` gives the
  library's message), and `min`, `max`, `count`, `list`, `tuple`, `set`,
  `sort`, `reverse` and `join`, which read the whole generator first, are
  given a list built by the loop. Each item is computed and consumed in
  turn, so errors keep their order, and infinite sources are allowed as
  in a generator.
- **Allocation-free iteration**: an iterator only read by `Next` keeps a
  mode, position and end in machine variables (`rt_iter_init`): a range
  of small ints is a plain counter, a list or tuple is read by index.
- **Size hints**: accumulators are made at the length of a list, tuple
  or finite range they are built from (`Op::NewAcc`, up to 1024 items).
- **Common-subexpression elimination** (`ir::cse`): a repeat of an
  instruction on the same operands, where the earlier one dominates it,
  reads the earlier result. Only instructions whose value depends just on
  their operands take part (not calls, which may return a fresh
  generator, nor closures, iterators or accumulators); because the
  earlier one dominates, errors happen where they did.
- **`where` patterns unpacked at once** (`cse::gather_items`): the items
  of one forced `where` cell, within a block, are taken together, so the
  tuple is released straight away. Otherwise `store(m, …) = (mem.set(…),
  …) where (mem, ip, rb) = m` kept `mem` shared through the `set`, and
  the map was copied on every write.
- **Tuples only taken apart, and unused results** (`ir::simplify`): a
  tuple whose every use is a size test or an in-range item (as in
  `match (a, d)`) isn't built, and instructions that can't fail and whose
  results nothing reads (the items `_` takes) are dropped.

### The runtime, shared with the tree-walker

These help both engines.

- Pair arithmetic and comparison on small ints, `Value::cmp` with only
  the two-small-ints case inline, list indexing that views the list
  once, one-allocation list literals and `chars`, and an immediate return
  from `make_mut` when the word holds the only reference.
- **The global allocator** is set in `value/word.rs` (section 4) and
  calls plain `mi_malloc` for alignments up to 8. The `mimalloc` crate
  passes everything to `mi_malloc_aligned`, which in mimalloc 3, on a
  missed fast path and a size that isn't a power of two, over-allocates
  from the next size class: objects spread over two classes and pages
  churn (2023 day 22 made 222,600 page allocations in a 0.13 s run, 45%
  of it in `madvise`). mimalloc 2 also fixed it, but was 4% slower over
  the corpus.

### Tried and set aside

- **Hashed maps and sets with sorted views** (branch `hash-proto`):
  about 4% over the corpus, with some programs up to 2× slower.
- **Compiling `bfs` and the other searches**: their own bookkeeping is a
  few percent; the time is in the user's `next` function.
- **A priority-queue type**: the corpus barely uses sets that way.

### Lessons

- Profile the slowest remaining solution before choosing the next step.
  The plan's guesses (type inference first) were less useful than what
  profiles found: the runtime boundary, a missing in-place update, a
  tuple built only to be matched, an allocator interaction.
- Most of what was left was allocation and the shared runtime (compiled
  code itself is 1–4% of mid-range solutions), so many gains came to the
  tree-walker too, and the speed-up over it is a moving target.
- Keeping errors exactly where they were ruled some things out, such as
  hoisting loop-invariant work, which can raise an error from a loop
  that runs no times.

### Timings

Best of two, wall clock, release builds, Apple M3 Max, real inputs with
`--no-examples`, measured together on 2026-11-11 at the end of phase 4.
"Plan baseline" is the tree-walker when this plan was written (section
1); Racket is from the same table.

| Solution | Plan baseline | Tree-walker | Native | Speed-up | Racket |
|---|---|---|---|---|---|
| 2019 day 25 | 2.51 s | 1.91 s | 0.54 s | 3.5× | 0.76 s |
| 2024 day 6 | 1.67 s | 1.17 s | 0.39 s | 3.0× | 0.53 s |
| 2018 day 24 | 5.72 s | 4.57 s | 1.53 s | 3.0× | 1.91 s |
| 2020 day 23 | 10.18 s | 6.97 s | 2.30 s | 3.0× | 3.46 s |
| 2020 day 15 | 8.53 s | 6.28 s | 1.02 s | 6.2× | 3.65 s |
| 2023 day 23 | 3.32 s | 2.69 s | 1.17 s | 2.3× | 1.57 s |
| 2018 day 9 | 3.51 s | 2.63 s | 0.46 s | 5.7× | 1.69 s |
| 2021 day 23 | 3.51 s | 2.97 s | 1.06 s | 2.8× | 1.84 s |
| 2018 day 11 | 3.44 s | 3.18 s | 0.58 s | 5.5× | 1.93 s |
| 2019 day 18 | 2.86 s | 2.53 s | 1.44 s | 1.8× | 1.76 s |
| 2022 day 23 | 5.33 s | 4.11 s | 2.43 s | 1.7× | 3.60 s |
| 2024 day 22 | 2.96 s | 2.14 s | 1.35 s | 1.6× | 2.31 s |
| 2018 day 14 | 8.79 s | 6.95 s | 1.75 s | 4.0× | 9.67 s |
| 2018 day 15 | 10.24 s | 7.24 s | 5.56 s | 1.3× | 11.70 s |
| 2022 day 20 | 3.03 s | 2.00 s | 1.96 s | 1.0× | 4.74 s |

Against section 1's criteria:

- **Faster than the tree-walker on every solution:** yes, on all 187
  apart from 10–20 ms of compiling on the quickest; 2022 day 20 only
  just (its time is list slicing in the shared runtime).
- **Faster than Racket on every solution:** yes, on all 15.
- **Median speed-up of at least 3×:** just short. Section 1 measures
  against the tree-walker as it is now, and that gives 2.99×; against the
  tree-walker when the plan was written it is 3.7×. The difference is
  the runtime work both engines share.
- **All of `tests/` passing natively, with the same results and
  errors:** yes, and native code is the default for `tinsel run` and
  `tinsel test` (SPEC 2.7; `--tree` and `--ir` choose the others).

Over the whole corpus, native code takes 44.52 s against 92.50 s on
the tree-walker (`TIMINGS.md`).
