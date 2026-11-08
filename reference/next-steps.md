# Next steps for the native backend

Written 2026-11-11 at the end of phase 4 (`native-plan.md`, section 12),
and revised the same day after its last round of fixes. Native code is
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

Against the goals in `native-plan.md` section 1:

- **Faster than the tree-walker on every solution:** yes, on all 187 in
  `TIMINGS.md` apart from 10–20 ms of compiling on the quickest. 2022
  day 20 only just: its time is list slicing in the shared runtime.
- **Faster than Racket on every solution:** yes, on all 15.
- **Median speed-up of at least 3×:** 2.99× against today's tree-walker,
  3.7× against the plan's. The tree-walker shares every runtime
  improvement, so runtime work doesn't move this ratio.
- **All of `tests/` passing natively, with the same results and errors:**
  yes, with the leak check on in debug builds.

Over the whole corpus, native code takes 44.52 s against 92.50 s on the
tree-walker (2.08×).

## Where the time goes now

From the profiles taken during phase 4:

- **Compiled code itself: 1–4%** of the mid-range solutions.
- **Ordered maps and sets:** about 45% of 2019 day 25 (Intcode memory
  reads), about a third of 2018 day 24 (`gs[x]`, half of it the
  loop-invariant `gs[a]`), and most of 2018 day 15 and 2022 day 23 (`in`
  against ordered sets inside search functions). Lookups walk a B-tree,
  comparing values.
- **Allocation, copying and freeing:** a fifth to a third everywhere:
  tuples, argument vectors for library calls, lists.
- **Hashing and comparing search states:** about a third of 2019 day 18.
- **List slicing:** most of 2022 day 20.

## Candidates, in order of expected value

### 1. Profile the solutions with the smallest speed-up

2018 day 15 (5.56 s, the slowest), 2024 day 22, 2022 day 23 and 2019
day 18. The last four profiles each found a cheap, general fix that no
plan had predicted (an allocator interaction, a missing in-place update,
a missing direct entry, a tuple built only to be matched), so this is
the cheapest way to find the next one.

### 2. An ordered radix trie for int and position keys (runtime)

Ints, and packed `(x, y)` pairs, map to unsigned numbers that sort in the
same order as the keys. A persistent radix trie (a Patricia tree, as in
Haskell's `IntMap`) keyed by those numbers keeps sorted iteration
exactly, looks keys up with bit operations instead of value comparisons,
and has cheap updated copies. A map or set would use it while all its
keys are ints, or all packed pairs, and `imbl` otherwise.

- **Expected gain:** the lookup-heavy solutions above, on both engines;
  it closes gaps in absolute time rather than raising the speed-up over
  the tree-walker.
- **Cost:** the trie itself, and routing about 25 kinds of map and set
  operation through a two-form type.
- **First step:** a standalone measurement against `imbl::OrdMap` and
  `OrdSet` on int and position keys; integrate only if lookups are
  clearly (say 2×) faster.

### 3. Loop-invariant hoisting (compiler)

CSE removes repeats within one pass of a loop body, but not work that is
the same on every pass, such as `gs[a]` in 2018 day 24's `choose`.
Hoisting can raise an error from a loop that runs no times, so it needs
either a proof that the body runs or the value cached on the first pass.

- **Expected gain:** mainly 2018 day 24; small elsewhere.

### 4. A per-call-site cache of compiled targets (compiler)

A call through a function value looks up the closure's compiled code in
the runtime every time (`rt_tail_target`). An inline cache of the last
closure and its code would skip that.

- **Expected gain:** a few percent on closure-heavy code.

### 5. Library calls without a heap-allocated argument vector (runtime)

Every library call that has no direct entry builds a `Vec` for its
arguments. A small inline array would avoid the allocation, but touches
the signature of every library function.

- **Expected gain:** 2–5%, broadly, on both engines.

### 6. Small lists without the final copy (runtime)

A comprehension's result is accumulated in a growable vector and then
copied into a small list's single allocation. Adopting the vector as it
is would change how lists are stored.

- **Expected gain:** a few percent on comprehension-heavy code.

### 7. Local type inference (compiler; the plan's original first item)

Knowing a variable is always an int would drop tag checks, reference
counting and error checks around it.

- **Expected gain:** 20–40% on int-heavy loops, which are already 3–6×
  faster than the tree-walker, so it barely moves the median.
- **Cost:** the largest of these.

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
