# Next steps for the native backend

Written 2026-11-11 at the end of phase 4 (`native-plan.md`, section 12),
revised the same day after its last round of fixes, and again on
2026-11-12 after a round of profiling and the radix tree (items 1 and 2). Native code is
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
| 2019 day 25 | 2.51 s | 1.61 s | 0.40 s | 4.0× | 0.76 s |
| 2024 day 6 | 1.67 s | 1.13 s | 0.38 s | 3.0× | 0.53 s |
| 2018 day 24 | 5.72 s | 4.33 s | 1.57 s | 2.8× | 1.91 s |
| 2020 day 23 | 10.18 s | 7.05 s | 2.30 s | 3.1× | 3.46 s |
| 2020 day 15 | 8.53 s | 6.14 s | 1.06 s | 5.8× | 3.65 s |
| 2023 day 23 | 3.32 s | 2.68 s | 1.08 s | 2.5× | 1.57 s |
| 2018 day 9 | 3.51 s | 2.60 s | 0.44 s | 5.9× | 1.69 s |
| 2021 day 23 | 3.51 s | 2.99 s | 1.05 s | 2.9× | 1.84 s |
| 2018 day 11 | 3.44 s | 3.15 s | 0.56 s | 5.6× | 1.93 s |
| 2019 day 18 | 2.86 s | 2.51 s | 1.43 s | 1.8× | 1.76 s |
| 2022 day 23 | 5.33 s | 3.91 s | 2.12 s | 1.8× | 3.60 s |
| 2024 day 22 | 2.96 s | 1.73 s | 0.94 s | 1.8× | 2.31 s |
| 2018 day 14 | 8.79 s | 6.84 s | 1.77 s | 3.9× | 9.67 s |
| 2018 day 15 | 10.24 s | 5.65 s | 3.99 s | 1.4× | 11.70 s |
| 2022 day 20 | 3.03 s | 1.96 s | 1.91 s | 1.0× | 4.74 s |

2022 day 20 on the tree-walker usually takes 1.96 s, but some runs take
about 1.6 s (`TIMINGS.md` has the best of two, 1.67 s).

Against the goals in `native-plan.md` section 1:

- **Faster than the tree-walker on every solution:** yes, on all 187 in
  `TIMINGS.md` apart from 10–20 ms of compiling on the quickest, and
  2022 day 20, which is level: its time is list slicing in the shared
  runtime.
- **Faster than Racket on every solution:** yes, on all 15.
- **Median speed-up of at least 3×:** 2.9× against today's tree-walker,
  3.7× against the plan's. The tree-walker shares every runtime
  improvement, so runtime work doesn't move this ratio.
- **All of `tests/` passing natively, with the same results and errors:**
  yes, with the leak check on in debug builds.

Over the whole corpus, native code takes 39.03 s against 85.63 s on the
tree-walker (2.19×); at the end of phase 4 it was 44.52 s against
92.50 s.

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
find the next fix: now 2018 day 15 (3.99 s), 2020 day 23 (2.30 s), 2022
day 23 (2.12 s), 2018 day 14 (1.77 s) and 2018 day 24 (1.57 s).

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
