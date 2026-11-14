# Next steps for the native backend

Written 2026-11-11 at the end of phase 4 (`native-plan.md`, section 12),
revised the same day after its last round of fixes, on 2026-11-12
after a round of profiling and the radix tree (items 1 and 2), and on
2026-11-13 after `where` constants as locals and faster comparisons of
small values (items 3 and 4), found by converting AoC 2017, and on
2026-11-14 after profiling two slow 2016 solutions (item 5). Native code is
the default for `tinsel run` and `tinsel test`. This note records where
things stand and what could come next, so that another round (if there
is one) can start from it.

## Where things stand

Best of two, wall clock, release builds, Apple M3 Max, real inputs with
`--no-examples`, both engines measured together. "Plan baseline" is the
tree-walker when the plan was written (`native-plan.md`, section 1);
Racket is from the same table.

| Solution | Plan baseline | Tree-walker | Native | vs tree-walker | Racket |
|---|---|---|---|---|---|
| 2019 day 25 | 2.51 s | 1.65 s | 0.36 s | 4.6× | 0.76 s |
| 2024 day 6 | 1.67 s | 1.15 s | 0.35 s | 3.3× | 0.53 s |
| 2018 day 24 | 5.72 s | 4.33 s | 1.42 s | 3.0× | 1.91 s |
| 2020 day 23 | 10.18 s | 7.18 s | 1.08 s | 6.6× | 3.46 s |
| 2020 day 15 | 8.53 s | 6.31 s | 1.16 s | 5.4× | 3.65 s |
| 2023 day 23 | 3.32 s | 2.72 s | 1.07 s | 2.5× | 1.57 s |
| 2018 day 9 | 3.51 s | 2.63 s | 0.45 s | 5.8× | 1.69 s |
| 2021 day 23 | 3.51 s | 3.05 s | 1.05 s | 2.9× | 1.84 s |
| 2018 day 11 | 3.44 s | 3.29 s | 0.55 s | 6.0× | 1.93 s |
| 2019 day 18 | 2.86 s | 2.61 s | 1.47 s | 1.8× | 1.76 s |
| 2022 day 23 | 5.33 s | 3.98 s | 2.14 s | 1.9× | 3.60 s |
| 2024 day 22 | 2.96 s | 1.72 s | 0.71 s | 2.4× | 2.31 s |
| 2018 day 14 | 8.79 s | 6.92 s | 1.29 s | 5.4× | 9.67 s |
| 2018 day 15 | 10.24 s | 5.70 s | 3.97 s | 1.4× | 11.70 s |
| 2022 day 20 | 3.03 s | 1.96 s | 1.41 s | 1.4× | 4.74 s |

The table was measured again on 2026-11-13, with the same method as
`TIMINGS.md`.

Against the goals in `native-plan.md` section 1:

- **Faster than the tree-walker on every solution:** yes, on the 187
  solutions of 2018 to 2025 in `TIMINGS.md` apart from 10–20 ms of
  compiling on the quickest. 2022 day 20, level until 2026-11-13 (its
  time is mostly list slicing in the shared runtime), is now 1.41 s
  natively against 1.96 s. Of the 2017 solutions added that day, day 14
  is level (0.50 s on both), for the same reason: its knot hashes are
  list reversals and joins in the runtime. So is 2016 day 16 (0.29 s
  against 0.27 s on 2026-11-14), whose time is string replacements and
  counts.
- **Faster than Racket on every solution:** yes, on all 15.
- **Median speed-up of at least 3×:** 3.0× against today's tree-walker,
  met since the `where` change of 2026-11-13 (2.9× before), and 4.2×
  against the plan's. The tree-walker shares every runtime improvement,
  so runtime work doesn't move this ratio; compiler work, such as item
  3, does.
- **All of `tests/` passing natively, with the same results and errors:**
  yes, with the leak check on in debug builds.

Over the whole corpus, native code takes 35.46 s against 86.56 s on the
tree-walker (2.44×); on 2026-11-12 it was 39.03 s against 85.63 s, and
at the end of phase 4 44.52 s against 92.50 s.

## Where the time goes now

From the profiles taken during phase 4, updated with those of
2026-11-12:

- **Compiled code itself: 1–4%** of the mid-range solutions.
- **Ordered maps and sets:** before the radix tree, about 45% of 2019
  day 25 (Intcode memory reads), about a third of 2018 day 24 (`gs[x]`,
  half of it the loop-invariant `gs[a]`), and about a third of 2018
  day 15 and a quarter of 2022 day 23 (`in` against sets of positions
  inside search functions). Sets and maps of ints and positions now use
  the radix tree; those keyed by strings or longer tuples still walk a
  B-tree.
- **Allocation, copying and freeing:** a fifth to a third everywhere:
  tuples, argument vectors for library calls, lists.
- **Hashing and comparing search states:** about a third of 2019 day 18
  (`dijkstra`'s table of states); half of its time is the solution's own
  `next` function.
- **List slicing:** most of 2022 day 20.
- **`where` records:** before 2026-11-13, a function with a `where` block
  made a record on every call and forced each constant through the
  runtime: over half of 2020 day 23, and about a third of 2017 day 25.
  About two thirds of the corpus's `where` blocks are now plain locals
  (item 3); the rest keep their records (below).

## Candidates, in order of expected value

### 1. Profile the solutions with the smallest speed-up: done

Done 2026-11-12 for 2018 day 15, 2022 day 23, 2019 day 18 and 2024
day 22. It found two cheap, general fixes:

- **Comparing positions:** two packed `(x, y)` pairs compare as one
  integer comparison (`Word::pair_order`) inline in `Value::cmp`, rather
  than an out-of-line call that decodes both words. 2018 day 15 13%
  faster.
- **`counts` and `tally`:** totals are kept in a hash table and sorted
  into the map once, rather than a lookup, a key check and an insert in
  the persistent map per item. 2024 day 22 31% faster.

The rest of 2018 day 15 and 2022 day 23 was B-tree lookups, which led to
item 2. 2019 day 18 had no cheap fix.

Profiling the slowest remaining solutions is still the cheapest way to
find the next fix: as of 2026-11-13, 2018 day 15 (3.97 s), 2022 day 23
(2.14 s), 2019 day 18 (1.47 s), 2018 day 24 (1.42 s) and 2022 day 20
(1.41 s). Converting a new year of puzzles is another: 2017 found items
3 and 4, and 2016 item 5.

### 2. An ordered radix trie for int and position keys (runtime): done

Done 2026-11-12. Sets and maps are now `ValueSet`/`ValueMap`
(`src/value/coll.rs`): a 16-way radix tree with compressed paths
(`src/value/radix.rs`) while every key is a fixnum, or every key a packed
pair, and `imbl` otherwise. Keys map to `u64`s that sort as the values do
(the sign bit flipped; for pairs, `Word::pair_order`). Leaves sit inline
in their parent's slots and keep the key's `Value`, so walks lend `&Value`
as `imbl` does.

A binary Patricia tree (as in Haskell's `IntMap`) was tried first: lookups
only 1.6–1.8× faster than `imbl`, since a lookup follows about 15 pointers
for 20,000 keys. The 16-way tree needs about 4: lookups 4.2–4.4× faster,
persistent inserts 1.9–2.3×, in-place builds 2.3–3.4×, walks level.

Whole solutions, release, native, best of two, against the build just
before it: the corpus 41.55 s → 39.24 s (−5.6%); 2020 day 11 −44%, 2021 day 25 −34%, 2019 day 25 −28%,
2024 day 20 −28%, 2023 day 16 −22%, 2018 day 15 −18%, 2022 day 23 −12%;
nothing slower.

### 3. `where` constants as plain locals (compiler): done

Done 2026-11-13. Each `where` block was lowered to a record: a cell per
constant, filled on first use by a thunk that captures the record. That
is what laziness needs in general, but it costs an allocation, a call
through the runtime and a state check per use, on every call: about 80
ns a call, which in 2017 day 25 (12 million calls) was a third of its
time.

`src/ir/wheres.rs`, a pass after lowering, turns a record into plain
locals when that can't be told apart: each constant is computed by
inlining its thunk at its first use, and later uses read the result. It
applies when nothing but the function and the cells' own thunks use the
record (no `where` function or lambda captures it), the cells don't
force each other in a cycle, and each cell has a use that comes before
all its others on every path and can't run twice for the same record
(not inside a loop that doesn't make a new one; missing that made 2022
day 23 take 82 s in the first version). A force inside another cell's
thunk counts as a use where that thunk is inlined, so cells are handled
before the cells they use. Laziness is kept exactly: a constant that
isn't reached still isn't computed, and errors come from the same
constant, at the same point.

A count over the corpus before starting: 190 `where` records, of which
75 qualified counting only direct uses and 121 (64%) with uses inside
other constants; 57 are captured by a function or lambda, and the rest
have a constant first used in separate branches. Tests:
`tests/features/where_constants.tin` (12 checks on all engines) and a
`dbg`-based test that a constant is computed once, even in a loop.

- **Gain:** the native total 38.60 s → 35.46 s with item 4 (−8%); 2020
  day 23 −55% (2.39 s → 1.08 s), 2018 day 14 −28%, 2024 day 22 −24%, 2022
  day 20 −21%; nothing slower.
- **Still open:** records captured by a `where` function or lambda, and
  constants first used in separate branches (a local per branch would
  work, computing it at most once on each path).

### 4. Fast comparisons of small values (runtime): done

Done 2026-11-13. A map keyed by `(state, 0)`, with a char in the tuple,
missed the fast path for pairs of small ints, and each comparison went
through the general one twice (for the tuple, then the char): about a
third of 2017 day 25. Now `none`, bools, small ints and chars compare by
a (type, value) key read from the word's bits, also as tuple elements.
A unit test checks the order against the general one on 576 pairs of
mixed values. 0.8% over the corpus, 3–5% for solutions with many tuple
keys, 9% for 2017 day 25.

The same profile pointed at the radix tree's `remove`, about 3.5 times
the cost of `insert` there. Three changes (scanning for a node's last
child only where one can be left, skipping the lookup before a removal,
and keeping one-child nodes to avoid freeing and rebuilding them as day
25 toggles cells) made no measurable difference and were reverted.

### 5. Faster `in xs.indices`, string constants and interpolation: done

Done 2026-11-14, from profiling 2016 day 5 (MD5 hashes of `"{door}{i}"`)
and day 25 (an assembunny interpreter), which took 3.44 s and 2.45 s
natively. Four fixes:

- **`x in xs.indices`:** an interpreter's `pc not in program.indices`
  built a range on every step only to test it: half of day 25. `simplify`
  now fuses a call to `indices` with an `in` or `not in` right after it,
  when nothing else uses the range, into `Op::InIndices`: a bounds check
  for a list or tuple, and the library's `indices` and `in` themselves
  otherwise, so results and errors are unchanged. IR only; the
  tree-walker still builds the range.
- **Matching against a string constant:** `EqConst` with a heap constant
  (as in `("cpy", x, y) => ...`) fell back to the IR interpreter through
  `rt_op`, 43 million times in day 25. It is now a direct call, `rt_eq`.
- **String interpolation:** each value was rendered into a string of its
  own and copied; `Value::write_display` appends it, with small ints
  written without `fmt`. Both engines.
- **`md5`'s hex digits:** `format!("{:x}")` on the digest took more time
  than the hashing (40% of day 5); the digits are now written directly.

Day 25 1.22 s (−50%), day 5 about 2.1 s (−35%), and 2016 day 12, another
assembunny program, 0.47 s → 0.06 s; nothing slower. Tests: `in indices`
and `not in indices` in `tests/features/collections.tin` (including two
errors), and interpolation of edge values in `basics.tin`.

Left over: day 5 is now 40% the MD5 compression itself (the crate's) and
about a quarter the lazy `map` and `filter` (each item passes through the
tree-walker's iterator, which calls the compiled lambdas); day 25 still
compares opcode strings (about a fifth), and its solution collects 40
outputs for every candidate rather than stopping at the first wrong one.

### 6. Loop-invariant hoisting (compiler)

CSE removes repeats within one pass of a loop body, but not work that is
the same on every pass, such as `gs[a]` in 2018 day 24's `choose`.
Hoisting can raise an error from a loop that runs no times, so it needs
either a proof that the body runs or the value cached on the first pass.

- **Expected gain:** mainly 2018 day 24; small elsewhere.

### 7. A per-call-site cache of compiled targets (compiler)

A call through a function value looks up the closure's compiled code in
the runtime every time (`rt_tail_target`). An inline cache of the last
closure and its code would skip that.

- **Expected gain:** a few percent on closure-heavy code.

### 8. Library calls without a heap-allocated argument vector (runtime)

Every library call that has no direct entry builds a `Vec` for its
arguments. A small inline array would avoid the allocation, but touches
the signature of every library function.

- **Expected gain:** 2–5%, broadly, on both engines.

### 9. Small lists without the final copy (runtime)

A comprehension's result is accumulated in a growable vector and then
copied into a small list's single allocation. Adopting the vector as it
is would change how lists are stored.

- **Expected gain:** a few percent on comprehension-heavy code.

### 10. Local type inference (compiler; the plan's original first item)

Knowing a variable is always an int would drop tag checks, reference
counting and error checks around it.

- **Expected gain:** 20–40% on int-heavy loops, which are already 3–6×
  faster than the tree-walker, so it barely moves the median.
- **Cost:** the largest of these.

### 11. Lazy library calls into compiled code (runtime, then compiler)

Over a generator or an infinite range, `map`, `filter`, `scan` and the
rest build a lazy generator, and each element passes through the
tree-walker's iterator, which calls the compiled lambda through four
layers (`ValIter::next`, `Interp::call`, `call_named`,
`ir::interp::call_value`) before reaching native code. Compiled loops
(`Op::Loopable`) only cover finite collections. Found in 2016 day 5,
`(0..).map(i => md5(...)).filter(...)`, where it is about a quarter of
the time (most of the rest is MD5 itself).

- **First step:** a direct path from the iterator to a closure that is
  already compiled, skipping the general dispatch. Nothing changes in
  what runs or in which order, so it is low risk.
- **Further:** compiling a chain over an infinite range that ends in
  `first`, `take`, `take_while`, `any` or `all` as one native loop. Its
  elements, errors and single-pass rules would have to match the
  tree-walker exactly, which is most of the work.
- **Expected gain:** small over the corpus: on 2026-11-14 only 13 calls
  in the 237 solutions start from an infinite range, plus a few chains on
  `iterate`, and nearly all are cheap. Perhaps 2 s of 2016 day 5's 9 s.
  Worth doing once a profile shows it more widely.

### 12. The right of `and` and `or` as a tail position (language, all engines)

SPEC 8.1 counts only the whole body of a clause and the branches of `if`
and `match` as tail positions. So `v == bit and f(...)` is not a tail
call: a recursive function written that way is memoized, hashing its
arguments on every call, and its recursion uses stack. Found in 2016 day
25, where `alternates` carries a growing set of seen states: the memo
warning fired, and the fix was rewriting it as
`if v == bit then f(...) else false`. The two forms read as the same
thing, so the difference is easy to trip over.

- **The catch:** `and` and `or` check that their right operand is a
  bool (`true and 5` is an error), and a plain tail call would skip
  that check. The check is idempotent, so a chain of tail calls needs
  only one: the trampoline (and the compiled tail-call loop) can carry a
  "must be a bool" flag, with the span of the `and` or `or` that set it,
  and check the final result once. The space stays constant. Which span
  the error reports, and what its trace shows, must be settled and be
  the same on all three engines.
- **What changes:** the SPEC 8.1 definition of a tail call (wording to
  agree with the user); the call-graph classification in the resolver,
  so these functions become loops and stop being memoized; tail calls in
  the tree-walker, the IR lowering and native code; and tests for the
  memoization change, deep recursion through `and`/`or`, and the bool
  check on a tail call's result.
- **Risk:** a function that is recursive only through `and`/`or` and
  really does repeat its arguments loses its memo, and could get much
  slower. It would need `@memo`. Before deciding, find the corpus's
  recursive functions that are memoized only because of an `and`/`or`
  call, and time them both ways.
- **Expected gain:** not speed in general, but fewer surprises: no
  memo warnings or hidden hashing in search-style recursion, and
  constant stack for loops written with `and`/`or`.

## Not candidates

- **Hashed maps and sets with sorted views** (branch `hash-proto`):
  about 4% across the corpus, with some programs up to 2× slower.
- **Compiling `bfs` and the other searches as loops:** their own
  bookkeeping is a few percent; the time is in the user's `next`
  function.
- **A priority-queue type:** the corpus barely uses sets as priority
  queues, and the searches already have their own.

## Known loose ends

- Leak reporting on the tree-walker has a known exception (a lambda
  escaping a frame whose `where` constants refer back to it), shown only
  with `TIN_LEAKS=1`.
- The IR interpreter is slow on some programs (3.4 s on 2019 day 25,
  against 1.9 s on the tree-walker); it exists to check the compiler,
  not for speed.
