# Solution timings

How long each solution in `tests/aocYYYY/` (2016 to 2025) takes to solve
both parts for the real input, with `tinsel run --no-examples`: compiled to
machine code (the default since 2026-11-11, `reference/native-plan.md`) and
on the tree-walking interpreter (`--tree`). The examples are left out because
they vary so much: some are a few lines, while others (such as 2020 day
23's part 2 example, a full 10-million-move game) cost as much as the real
input. Each time is the faster of two runs of the release build, on an
Apple M3 Max, measured on 2026-11-14, and includes compiling the program
(10–40 ms).

| Year | Solutions | Native | Tree-walker | Slowest (native) |
|---|---|---|---|---|
| 2025 | 12 | 1.02 s | 2.28 s | day 10, 0.36 s |
| 2024 | 25 | 2.54 s | 5.32 s | day 22, 0.73 s |
| 2023 | 25 | 3.87 s | 6.94 s | day 23, 1.08 s |
| 2022 | 25 | 5.61 s | 9.07 s | day 23, 2.19 s |
| 2021 | 25 | 3.58 s | 8.36 s | day 23, 1.08 s |
| 2020 | 25 | 4.74 s | 17.72 s | day 22, 1.29 s |
| 2019 | 25 | 4.65 s | 11.47 s | day 18, 1.59 s |
| 2018 | 25 | 11.42 s | 29.74 s | day 15, 4.01 s |
| 2017 | 25 | 10.13 s | 55.13 s | day 25, 2.12 s |
| 2016 | 25 | 19.95 s | 27.10 s | day 5, 10.27 s |
| **All** | **237** | **67.50 s** | **173.15 s** | |

219 of the 237 solutions finish in under a second natively, and 202 on the tree-walker.

The interpreter speedups of 2026-11-08 (reused call frames and argument
vectors, parameters bound without copying, and fast paths for small-int
arithmetic and comparisons) took the 2023 to 2025 solutions from 32.88 s
to 27.19 s (1.21× faster), measured then with their examples.

On 2026-11-09, calls to a library function or top-level function named
directly began skipping the general call dispatch. That took all 187
solutions from 162.65 s to 152.63 s (6% faster). Solutions that are mostly
calls gained most: 2018 day 9 by 20%, 2019 day 18 by 14%, and 2020 day 15
by 10%.

Later the same day, the interpreter's internal hash tables (memo tables,
search states, cycle detection) switched from SipHash to the faster FxHash,
and iterating a set or map stopped doing a range search for every element.
Together these took the total to 151.23 s. 2018 day 15, which searches
heavily, gained 6% (14.81 s to 13.86 s); the rest changed little.

Four further changes over 2026-11-09 and 2026-11-10 took the total to
120.83 s (20% less than 151.23 s), and three more solutions now finish
in under a second. Each was measured on the 15 slowest solutions:

- comparisons of small ints and of tuples and lists of them skip the
  general path: 0.4% to 4.5% faster each;
- the release build uses link-time optimisation and the mimalloc
  allocator: 90.68 s to 80.88 s (11%), with 2021 day 23 24% faster;
- the persistent collections switched from the unmaintained `im` crate to
  its maintained fork `imbl`: 1.4% faster overall, with heavy map and set
  users such as 2019 day 25 (30%) and 2018 day 15 (12%) gaining;
- lists longer than 32 elements are stored as a plain array until they
  are shared, so updating or appending to a large list at its last use is
  a single store: 80.44 s to 73.39 s (9%), with 2018 day 9 26%, 2020
  day 15 25% and 2020 day 23 17% faster.

Updating a grid in place at its last use also became O(1) in that
period, though no current solution updates a grid in a loop.

Later on 2026-11-10, values became a single tagged 8-byte word instead of
a 32-byte enum, with small ints held directly in the word (phase 2 of
`reference/native-plan.md`). On the 15 slowest solutions that took the
total from 75.95 s to 62.97 s (1.21× faster); every one of them gained,
from 10% (2023 day 23) to 47% (2022 day 20).

From 2026-11-10 to 2026-11-11, programs became compiled to machine code
with Cranelift (`reference/native-plan.md`, phases 3 and 4; now the
default). Natively, all 187 solutions take 44.52 s against 92.50 s on
the tree-walker (2.08× faster), and against 120.83 s for the
tree-walker before this work began. The tree-walker's own gain (to
92.50 s) comes from runtime work that both share: one-word values,
pairs of small ints held in the word, faster comparisons, indexing and
list building, and calling mimalloc directly (below).

Natively, the slowest solutions gain most: 2018 day 9 and 2020 day 15
run about 6× faster than on the tree-walker, 2018 day 11 about 5.5×.
The quickest ones run slightly slower, since compiling takes 10–40 ms.
Apart from that, every solution runs at least as fast natively as on the
tree-walker, except 2017 day 14 and 2016 day 16 (added later, below),
which are level. 2022
day 20 was also an exception, level at the time, since its
time is mostly list slicing in the runtime both share; natively it took
1.91 s every run, against 1.96 s (now and then about 1.6 s) on the
tree-walker, until the change of 2026-11-13 below. The first timing of
the whole corpus found four that
didn't, and the fixes (`native-plan.md`, section 12) helped others too:

- A generator comprehension passed to a library function, as in
  `sum(x * x for x in xs)`, is compiled as one loop: 2023 day 17 went
  from 1.02 s to 0.46 s, 2024 day 19 from 0.71 s to 0.33 s, 2025 day 8
  from 0.47 s to 0.25 s, and 2024 day 20 from 1.32 s to 0.89 s.
- mimalloc is called directly for ordinary allocations, since the way
  the `mimalloc` crate called it made memory pages churn: 2023 day 22
  went from 0.13 s to 0.07 s, and the tree-walker's total from 95.90 s to
  about 92.5 s.
- A `where` pattern such as `(mem, ip, rb) = m` lets go of `m` as soon
  as it is unpacked, so updating `mem` doesn't copy it, and
  `get(m, k, default)` has a direct runtime entry: 2019 day 25 went from
  0.84 s to 0.54 s, and the other Intcode solutions of 2019 (days 13,
  19 and 21) became about a third faster.
- A tuple written only to be matched, as in `match (a, d)`, isn't
  built: 2018 day 24 went from 1.89 s to 1.53 s.

On 2026-11-12, three changes to the shared runtime took the native total
from 44.52 s to 39.03 s (12% less) and the tree-walker's from 92.50 s to
85.63 s (7% less); no solution got slower:

- Two `(x, y)` positions are compared with a single integer comparison
  rather than a call that unpacks both: 2018 day 15, which looks up
  positions in sets throughout its searches, became 13% faster.
- `counts` and `tally` add up their totals in a hash table and build the
  sorted map once at the end, instead of updating the map for every item:
  2024 day 22 went from 1.35 s to 0.94 s.
- A set or map whose keys are all small ints, or all positions, is kept
  in a 16-way radix tree rather than a B-tree, so a lookup indexes a few
  arrays instead of comparing values (`reference/next-steps.md`). 2020
  day 11 went from 0.83 s to 0.37 s, 2024 day 20 from 0.89 s to 0.56 s,
  2021 day 25 from 0.44 s to 0.29 s, 2023 day 16 from 0.72 s to 0.49 s,
  2019 day 25 from 0.54 s to 0.40 s, 2018 day 15 to 3.99 s, and 2022
  day 23 from 2.43 s to 2.12 s.

On 2026-11-13, two changes took the native total for 2018 to 2025 from
38.60 s (measured again that day, before them) to 35.46 s (8% less); no
solution got slower. The tree-walker shares only the first, and is unchanged within
the noise of measuring on different days (85.63 s, then 86.56 s):

- `none`, bools, small ints and chars compare by their type and value,
  read straight from the word, including inside tuples, so a key such as
  `(state, 0)` with a char in it no longer takes the general path: 0.8%
  overall, and 3–5% for solutions with many tuple keys.
- When it is the same thing, a `where` block's constants are compiled as
  plain locals, each computed at its first use, rather than through a
  record made on every call (`src/ir/wheres.rs`). This applies when
  nothing captures the record, the constants don't depend on each other
  in a cycle, and each has a first use that comes before all its others
  and can't run twice per call. It covers about two thirds of the
  corpus's `where` blocks. 2020 day 23 went from 2.39 s to 1.08 s, 2018
  day 14 from 1.79 s to 1.29 s, 2024 day 22 from 0.94 s to 0.71 s, and
  2022 day 20 from 1.79 s to 1.41 s, now ahead of the tree-walker.

The 2017 solutions (`tests/aoc2017/`) were added on 2026-11-13, as a
practice run before December; the fixes it led to are the two changes
above, `where` constants and faster comparisons, and several additions to
the language and library (`reference/next-steps.md`, items 3 and 4).

The 2016 solutions (`tests/aoc2016/`) were added on 2026-11-14, the
second practice run. Profiling two of them led to faster `x in
xs.indices`, matching against string constants and string interpolation
(`reference/next-steps.md`, item 5); 2016 day 25 went from 2.45 s to
1.22 s natively before its solution changed.

## 2025

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Secret Entrance](tests/aoc2025/day01.tin) | 0.01 s | 0.01 s |
| 2 | [Gift Shop](tests/aoc2025/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Lobby](tests/aoc2025/day03.tin) | 0.01 s | 0.00 s |
| 4 | [Printing Department](tests/aoc2025/day04.tin) | 0.08 s | 0.17 s |
| 5 | [Cafeteria](tests/aoc2025/day05.tin) | 0.01 s | 0.00 s |
| 6 | [Trash Compactor](tests/aoc2025/day06.tin) | 0.01 s | 0.00 s |
| 7 | [Laboratories](tests/aoc2025/day07.tin) | 0.01 s | 0.01 s |
| 8 | [Playground](tests/aoc2025/day08.tin) | 0.16 s | 0.35 s |
| 9 | [Movie Theater](tests/aoc2025/day09.tin) | 0.34 s | 1.04 s |
| 10 | [Factory](tests/aoc2025/day10.tin) | 0.36 s | 0.69 s |
| 11 | [Reactor](tests/aoc2025/day11.tin) | 0.01 s | 0.00 s |
| 12 | [Christmas Tree Farm](tests/aoc2025/day12.tin) | 0.01 s | 0.00 s |

## 2024

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Historian Hysteria](tests/aoc2024/day01.tin) | 0.01 s | 0.00 s |
| 2 | [Red-Nosed Reports](tests/aoc2024/day02.tin) | 0.01 s | 0.01 s |
| 3 | [Mull It Over](tests/aoc2024/day03.tin) | 0.01 s | 0.00 s |
| 4 | [Ceres Search](tests/aoc2024/day04.tin) | 0.02 s | 0.03 s |
| 5 | [Print Queue](tests/aoc2024/day05.tin) | 0.02 s | 0.01 s |
| 6 | [Guard Gallivant](tests/aoc2024/day06.tin) | 0.36 s | 1.18 s |
| 7 | [Bridge Repair](tests/aoc2024/day07.tin) | 0.03 s | 0.03 s |
| 8 | [Resonant Collinearity](tests/aoc2024/day08.tin) | 0.01 s | 0.01 s |
| 9 | [Disk Fragmenter](tests/aoc2024/day09.tin) | 0.06 s | 0.07 s |
| 10 | [Hoof It](tests/aoc2024/day10.tin) | 0.01 s | 0.01 s |
| 11 | [Plutonian Pebbles](tests/aoc2024/day11.tin) | 0.04 s | 0.07 s |
| 12 | [Garden Groups](tests/aoc2024/day12.tin) | 0.04 s | 0.06 s |
| 13 | [Claw Contraption](tests/aoc2024/day13.tin) | 0.01 s | 0.00 s |
| 14 | [Restroom Redoubt](tests/aoc2024/day14.tin) | 0.03 s | 0.03 s |
| 15 | [Warehouse Woes](tests/aoc2024/day15.tin) | 0.04 s | 0.06 s |
| 16 | [Reindeer Maze](tests/aoc2024/day16.tin) | 0.03 s | 0.04 s |
| 17 | [Chronospatial Computer](tests/aoc2024/day17.tin) | 0.02 s | 0.01 s |
| 18 | [RAM Run](tests/aoc2024/day18.tin) | 0.03 s | 0.03 s |
| 19 | [Linen Layout](tests/aoc2024/day19.tin) | 0.34 s | 0.55 s |
| 20 | [Race Condition](tests/aoc2024/day20.tin) | 0.57 s | 1.23 s |
| 21 | [Keypad Conundrum](tests/aoc2024/day21.tin) | 0.02 s | 0.00 s |
| 22 | [Monkey Market](tests/aoc2024/day22.tin) | 0.73 s | 1.80 s |
| 23 | [LAN Party](tests/aoc2024/day23.tin) | 0.05 s | 0.05 s |
| 24 | [Crossed Wires](tests/aoc2024/day24.tin) | 0.02 s | 0.01 s |
| 25 | [Code Chronicle](tests/aoc2024/day25.tin) | 0.03 s | 0.03 s |

## 2023

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Trebuchet?!](tests/aoc2023/day01.tin) | 0.03 s | 0.04 s |
| 2 | [Cube Conundrum](tests/aoc2023/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Gear Ratios](tests/aoc2023/day03.tin) | 0.03 s | 0.03 s |
| 4 | [Scratchcards](tests/aoc2023/day04.tin) | 0.01 s | 0.00 s |
| 5 | [If You Give A Seed A Fertilizer](tests/aoc2023/day05.tin) | 0.01 s | 0.00 s |
| 6 | [Wait For It](tests/aoc2023/day06.tin) | 0.01 s | 0.00 s |
| 7 | [Camel Cards](tests/aoc2023/day07.tin) | 0.01 s | 0.01 s |
| 8 | [Haunted Wasteland](tests/aoc2023/day08.tin) | 0.03 s | 0.05 s |
| 9 | [Mirage Maintenance](tests/aoc2023/day09.tin) | 0.01 s | 0.01 s |
| 10 | [Pipe Maze](tests/aoc2023/day10.tin) | 0.01 s | 0.01 s |
| 11 | [Cosmic Expansion](tests/aoc2023/day11.tin) | 0.01 s | 0.00 s |
| 12 | [Hot Springs](tests/aoc2023/day12.tin) | 0.08 s | 0.20 s |
| 13 | [Point of Incidence](tests/aoc2023/day13.tin) | 0.02 s | 0.02 s |
| 14 | [Parabolic Reflector Dish](tests/aoc2023/day14.tin) | 0.67 s | 0.82 s |
| 15 | [Lens Library](tests/aoc2023/day15.tin) | 0.02 s | 0.01 s |
| 16 | [The Floor Will Be Lava](tests/aoc2023/day16.tin) | 0.48 s | 0.64 s |
| 17 | [Clumsy Crucible](tests/aoc2023/day17.tin) | 0.47 s | 0.92 s |
| 18 | [Lavaduct Lagoon](tests/aoc2023/day18.tin) | 0.01 s | 0.00 s |
| 19 | [Aplenty](tests/aoc2023/day19.tin) | 0.02 s | 0.01 s |
| 20 | [Pulse Propagation](tests/aoc2023/day20.tin) | 0.54 s | 0.90 s |
| 21 | [Step Counter](tests/aoc2023/day21.tin) | 0.18 s | 0.27 s |
| 22 | [Sand Slabs](tests/aoc2023/day22.tin) | 0.05 s | 0.08 s |
| 23 | [A Long Walk](tests/aoc2023/day23.tin) | 1.08 s | 2.86 s |
| 24 | [Never Tell Me The Odds](tests/aoc2023/day24.tin) | 0.05 s | 0.04 s |
| 25 | [Snowverload](tests/aoc2023/day25.tin) | 0.02 s | 0.02 s |

## 2022

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Calorie Counting](tests/aoc2022/day01.tin) | 0.00 s | 0.00 s |
| 2 | [Rock Paper Scissors](tests/aoc2022/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Rucksack Reorganization](tests/aoc2022/day03.tin) | 0.01 s | 0.00 s |
| 4 | [Camp Cleanup](tests/aoc2022/day04.tin) | 0.01 s | 0.00 s |
| 5 | [Supply Stacks](tests/aoc2022/day05.tin) | 0.01 s | 0.00 s |
| 6 | [Tuning Trouble](tests/aoc2022/day06.tin) | 0.01 s | 0.01 s |
| 7 | [No Space Left On Device](tests/aoc2022/day07.tin) | 0.01 s | 0.00 s |
| 8 | [Treetop Tree House](tests/aoc2022/day08.tin) | 0.04 s | 0.05 s |
| 9 | [Rope Bridge](tests/aoc2022/day09.tin) | 0.03 s | 0.05 s |
| 10 | [Cathode-Ray Tube](tests/aoc2022/day10.tin) | 0.02 s | 0.00 s |
| 11 | [Monkey in the Middle](tests/aoc2022/day11.tin) | 0.18 s | 0.37 s |
| 12 | [Hill Climbing Algorithm](tests/aoc2022/day12.tin) | 0.01 s | 0.01 s |
| 13 | [Distress Signal](tests/aoc2022/day13.tin) | 0.02 s | 0.01 s |
| 14 | [Regolith Reservoir](tests/aoc2022/day14.tin) | 0.03 s | 0.05 s |
| 15 | [Beacon Exclusion Zone](tests/aoc2022/day15.tin) | 0.02 s | 0.00 s |
| 16 | [Proboscidea Volcanium](tests/aoc2022/day16.tin) | 0.55 s | 1.08 s |
| 17 | [Pyroclastic Flow](tests/aoc2022/day17.tin) | 0.05 s | 0.06 s |
| 18 | [Boiling Boulders](tests/aoc2022/day18.tin) | 0.04 s | 0.05 s |
| 19 | [Not Enough Minerals](tests/aoc2022/day19.tin) | 0.11 s | 0.27 s |
| 20 | [Grove Positioning System](tests/aoc2022/day20.tin) | 1.77 s | 2.00 s |
| 21 | [Monkey Math](tests/aoc2022/day21.tin) | 0.01 s | 0.01 s |
| 22 | [Monkey Map](tests/aoc2022/day22.tin) | 0.05 s | 0.03 s |
| 23 | [Unstable Diffusion](tests/aoc2022/day23.tin) | 2.19 s | 4.12 s |
| 24 | [Blizzard Basin](tests/aoc2022/day24.tin) | 0.42 s | 0.88 s |
| 25 | [Full of Hot Air](tests/aoc2022/day25.tin) | 0.01 s | 0.00 s |

## 2021

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Sonar Sweep](tests/aoc2021/day01.tin) | 0.01 s | 0.00 s |
| 2 | [Dive!](tests/aoc2021/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Binary Diagnostic](tests/aoc2021/day03.tin) | 0.07 s | 0.19 s |
| 4 | [Giant Squid](tests/aoc2021/day04.tin) | 0.01 s | 0.00 s |
| 5 | [Hydrothermal Venture](tests/aoc2021/day05.tin) | 0.06 s | 0.08 s |
| 6 | [Lanternfish](tests/aoc2021/day06.tin) | 0.01 s | 0.00 s |
| 7 | [The Treachery of Whales](tests/aoc2021/day07.tin) | 0.01 s | 0.00 s |
| 8 | [Seven Segment Search](tests/aoc2021/day08.tin) | 0.01 s | 0.00 s |
| 9 | [Smoke Basin](tests/aoc2021/day09.tin) | 0.01 s | 0.01 s |
| 10 | [Syntax Scoring](tests/aoc2021/day10.tin) | 0.01 s | 0.01 s |
| 11 | [Dumbo Octopus](tests/aoc2021/day11.tin) | 0.03 s | 0.04 s |
| 12 | [Passage Pathing](tests/aoc2021/day12.tin) | 0.02 s | 0.02 s |
| 13 | [Transparent Origami](tests/aoc2021/day13.tin) | 0.02 s | 0.00 s |
| 14 | [Extended Polymerization](tests/aoc2021/day14.tin) | 0.01 s | 0.01 s |
| 15 | [Chiton](tests/aoc2021/day15.tin) | 0.22 s | 0.41 s |
| 16 | [Packet Decoder](tests/aoc2021/day16.tin) | 0.02 s | 0.00 s |
| 17 | [Trick Shot](tests/aoc2021/day17.tin) | 0.01 s | 0.01 s |
| 18 | [Snailfish](tests/aoc2021/day18.tin) | 0.55 s | 0.96 s |
| 19 | [Beacon Scanner](tests/aoc2021/day19.tin) | 0.15 s | 0.16 s |
| 20 | [Trench Map](tests/aoc2021/day20.tin) | 0.18 s | 0.70 s |
| 21 | [Dirac Dice](tests/aoc2021/day21.tin) | 0.04 s | 0.05 s |
| 22 | [Reactor Reboot](tests/aoc2021/day22.tin) | 0.71 s | 1.80 s |
| 23 | [Amphipod](tests/aoc2021/day23.tin) | 1.08 s | 3.24 s |
| 24 | [Arithmetic Logic Unit](tests/aoc2021/day24.tin) | 0.02 s | 0.00 s |
| 25 | [Sea Cucumber](tests/aoc2021/day25.tin) | 0.29 s | 0.65 s |

## 2020

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Report Repair](tests/aoc2020/day01.tin) | 0.01 s | 0.00 s |
| 2 | [Password Philosophy](tests/aoc2020/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Toboggan Trajectory](tests/aoc2020/day03.tin) | 0.01 s | 0.00 s |
| 4 | [Passport Processing](tests/aoc2020/day04.tin) | 0.01 s | 0.00 s |
| 5 | [Binary Boarding](tests/aoc2020/day05.tin) | 0.01 s | 0.00 s |
| 6 | [Custom Customs](tests/aoc2020/day06.tin) | 0.01 s | 0.00 s |
| 7 | [Handy Haversacks](tests/aoc2020/day07.tin) | 0.01 s | 0.00 s |
| 8 | [Handheld Halting](tests/aoc2020/day08.tin) | 0.03 s | 0.03 s |
| 9 | [Encoding Error](tests/aoc2020/day09.tin) | 0.01 s | 0.01 s |
| 10 | [Adapter Array](tests/aoc2020/day10.tin) | 0.01 s | 0.00 s |
| 11 | [Seating System](tests/aoc2020/day11.tin) | 0.37 s | 0.95 s |
| 12 | [Rain Risk](tests/aoc2020/day12.tin) | 0.01 s | 0.00 s |
| 13 | [Shuttle Search](tests/aoc2020/day13.tin) | 0.01 s | 0.00 s |
| 14 | [Docking Data](tests/aoc2020/day14.tin) | 0.04 s | 0.06 s |
| 15 | [Rambunctious Recitation](tests/aoc2020/day15.tin) | 1.24 s | 6.71 s |
| 16 | [Ticket Translation](tests/aoc2020/day16.tin) | 0.02 s | 0.02 s |
| 17 | [Conway Cubes](tests/aoc2020/day17.tin) | 0.09 s | 0.09 s |
| 18 | [Operation Order](tests/aoc2020/day18.tin) | 0.01 s | 0.01 s |
| 19 | [Monster Messages](tests/aoc2020/day19.tin) | 0.18 s | 0.50 s |
| 20 | [Jurassic Jigsaw](tests/aoc2020/day20.tin) | 0.04 s | 0.03 s |
| 21 | [Allergen Assessment](tests/aoc2020/day21.tin) | 0.01 s | 0.00 s |
| 22 | [Crab Combat](tests/aoc2020/day22.tin) | 1.29 s | 1.59 s |
| 23 | [Crab Cups](tests/aoc2020/day23.tin) | 1.13 s | 7.48 s |
| 24 | [Lobby Layout](tests/aoc2020/day24.tin) | 0.15 s | 0.19 s |
| 25 | [Combo Breaker](tests/aoc2020/day25.tin) | 0.02 s | 0.01 s |

Days 15 and 23 each run a loop of tens of millions of updates (30 million
turns of a memory game, and 10 million moves round a circle of a million
cups). Since 2026-11-08 the interpreter updates a map or list in place when
nothing else refers to it (SPEC 4.1), which took day 15 from 159 s to
22 s and day 23 from about 137 s to 26 s (times then included the
examples). Both then switched from a map to a list indexed by number,
updated with `xs.set(i, v)` (SPEC 9.2), which nearly halved their times
again. Day 15's list of 30 million entries takes about 1.6 GB, against
340 MB for the map.

## 2019

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [The Tyranny of the Rocket Equation](tests/aoc2019/day01.tin) | 0.01 s | 0.00 s |
| 2 | [1202 Program Alarm](tests/aoc2019/day02.tin) | 0.11 s | 0.35 s |
| 3 | [Crossed Wires](tests/aoc2019/day03.tin) | 0.09 s | 0.10 s |
| 4 | [Secure Container](tests/aoc2019/day04.tin) | 0.01 s | 0.01 s |
| 5 | [Sunny with a Chance of Asteroids](tests/aoc2019/day05.tin) | 0.02 s | 0.00 s |
| 6 | [Universal Orbit Map](tests/aoc2019/day06.tin) | 0.01 s | 0.01 s |
| 7 | [Amplification Circuit](tests/aoc2019/day07.tin) | 0.06 s | 0.07 s |
| 8 | [Space Image Format](tests/aoc2019/day08.tin) | 0.03 s | 0.00 s |
| 9 | [Sensor Boost](tests/aoc2019/day09.tin) | 0.09 s | 0.39 s |
| 10 | [Monitoring Station](tests/aoc2019/day10.tin) | 0.04 s | 0.07 s |
| 11 | [Space Police](tests/aoc2019/day11.tin) | 0.06 s | 0.14 s |
| 12 | [The N-Body Problem](tests/aoc2019/day12.tin) | 0.81 s | 1.37 s |
| 13 | [Care Package](tests/aoc2019/day13.tin) | 0.19 s | 0.87 s |
| 14 | [Space Stoichiometry](tests/aoc2019/day14.tin) | 0.01 s | 0.01 s |
| 15 | [Oxygen System](tests/aoc2019/day15.tin) | 0.04 s | 0.07 s |
| 16 | [Flawed Frequency Transmission](tests/aoc2019/day16.tin) | 0.05 s | 0.15 s |
| 17 | [Set and Forget](tests/aoc2019/day17.tin) | 0.06 s | 0.14 s |
| 18 | [Many-Worlds Interpretation](tests/aoc2019/day18.tin) | 1.59 s | 2.77 s |
| 19 | [Tractor Beam](tests/aoc2019/day19.tin) | 0.27 s | 1.40 s |
| 20 | [Donut Maze](tests/aoc2019/day20.tin) | 0.13 s | 0.21 s |
| 21 | [Springdroid Adventure](tests/aoc2019/day21.tin) | 0.17 s | 0.84 s |
| 22 | [Slam Shuffle](tests/aoc2019/day22.tin) | 0.01 s | 0.00 s |
| 23 | [Category Six](tests/aoc2019/day23.tin) | 0.08 s | 0.22 s |
| 24 | [Planet of Discord](tests/aoc2019/day24.tin) | 0.34 s | 0.57 s |
| 25 | [Cryostasis](tests/aoc2019/day25.tin) | 0.38 s | 1.70 s |

## 2018

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Chronal Calibration](tests/aoc2018/day01.tin) | 0.02 s | 0.03 s |
| 2 | [Inventory Management System](tests/aoc2018/day02.tin) | 0.09 s | 0.14 s |
| 3 | [No Matter How You Slice It](tests/aoc2018/day03.tin) | 0.07 s | 0.10 s |
| 4 | [Repose Record](tests/aoc2018/day04.tin) | 0.01 s | 0.00 s |
| 5 | [Alchemical Reduction](tests/aoc2018/day05.tin) | 0.05 s | 0.13 s |
| 6 | [Chronal Coordinates](tests/aoc2018/day06.tin) | 0.37 s | 0.58 s |
| 7 | [The Sum of Its Parts](tests/aoc2018/day07.tin) | 0.02 s | 0.00 s |
| 8 | [Memory Maneuver](tests/aoc2018/day08.tin) | 0.02 s | 0.01 s |
| 9 | [Marble Mania](tests/aoc2018/day09.tin) | 0.46 s | 2.75 s |
| 10 | [The Stars Align](tests/aoc2018/day10.tin) | 0.02 s | 0.01 s |
| 11 | [Chronal Charge](tests/aoc2018/day11.tin) | 0.57 s | 3.43 s |
| 12 | [Subterranean Sustainability](tests/aoc2018/day12.tin) | 0.02 s | 0.02 s |
| 13 | [Mine Cart Madness](tests/aoc2018/day13.tin) | 0.06 s | 0.12 s |
| 14 | [Chocolate Charts](tests/aoc2018/day14.tin) | 1.31 s | 7.17 s |
| 15 | [Beverage Bandits](tests/aoc2018/day15.tin) | 4.01 s | 6.02 s |
| 16 | [Chronal Classification](tests/aoc2018/day16.tin) | 0.03 s | 0.01 s |
| 17 | [Reservoir Research](tests/aoc2018/day17.tin) | 0.05 s | 0.06 s |
| 18 | [Settlers of The North Pole](tests/aoc2018/day18.tin) | 0.68 s | 1.42 s |
| 19 | [Go With The Flow](tests/aoc2018/day19.tin) | 0.02 s | 0.00 s |
| 20 | [A Regular Map](tests/aoc2018/day20.tin) | 0.04 s | 0.03 s |
| 21 | [Chronal Conversion](tests/aoc2018/day21.tin) | 0.03 s | 0.01 s |
| 22 | [Mode Maze](tests/aoc2018/day22.tin) | 1.19 s | 1.85 s |
| 23 | [Experimental Emergency Teleportation](tests/aoc2018/day23.tin) | 0.66 s | 1.06 s |
| 24 | [Immune System Simulator 20XX](tests/aoc2018/day24.tin) | 1.44 s | 4.48 s |
| 25 | [Four-Dimensional Adventure](tests/aoc2018/day25.tin) | 0.19 s | 0.30 s |

The slow ones:

- **Day 15** fights the battle once for part 1, then again for each elf
  attack power from 4 up to 40, stopping each battle when an elf dies.
- **Day 14** builds a scoreboard of 20 million recipes in one list, which
  takes about 590 MB.
- **Day 9** plays 7 million marbles on a circle kept as two lists of
  links, updated in place.
- **Day 11** checks every square of every size on a 300x300 grid with a
  summed-area table, about 9 million squares.

## 2017

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [Inverse Captcha](tests/aoc2017/day01.tin) | 0.01 s | 0.00 s |
| 2 | [Corruption Checksum](tests/aoc2017/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Spiral Memory](tests/aoc2017/day03.tin) | 0.02 s | 0.01 s |
| 4 | [High-Entropy Passphrases](tests/aoc2017/day04.tin) | 0.01 s | 0.00 s |
| 5 | [A Maze of Twisty Trampolines, All Alike](tests/aoc2017/day05.tin) | 2.11 s | 8.80 s |
| 6 | [Memory Reallocation](tests/aoc2017/day06.tin) | 0.02 s | 0.04 s |
| 7 | [Recursive Circus](tests/aoc2017/day07.tin) | 0.02 s | 0.01 s |
| 8 | [I Heard You Like Registers](tests/aoc2017/day08.tin) | 0.01 s | 0.00 s |
| 9 | [Stream Processing](tests/aoc2017/day09.tin) | 0.01 s | 0.01 s |
| 10 | [Knot Hash](tests/aoc2017/day10.tin) | 0.02 s | 0.02 s |
| 11 | [Hex Ed](tests/aoc2017/day11.tin) | 0.01 s | 0.01 s |
| 12 | [Digital Plumber](tests/aoc2017/day12.tin) | 0.01 s | 0.00 s |
| 13 | [Packet Scanners](tests/aoc2017/day13.tin) | 0.55 s | 1.90 s |
| 14 | [Disk Defragmentation](tests/aoc2017/day14.tin) | 0.49 s | 0.49 s |
| 15 | [Dueling Generators](tests/aoc2017/day15.tin) | 1.61 s | 18.14 s |
| 16 | [Permutation Promenade](tests/aoc2017/day16.tin) | 0.09 s | 0.12 s |
| 17 | [Spinlock](tests/aoc2017/day17.tin) | 0.50 s | 6.63 s |
| 18 | [Duet](tests/aoc2017/day18.tin) | 0.05 s | 0.09 s |
| 19 | [A Series of Tubes](tests/aoc2017/day19.tin) | 0.01 s | 0.01 s |
| 20 | [Particle Swarm](tests/aoc2017/day20.tin) | 0.27 s | 0.32 s |
| 21 | [Fractal Art](tests/aoc2017/day21.tin) | 0.02 s | 0.01 s |
| 22 | [Sporifica Virus](tests/aoc2017/day22.tin) | 1.41 s | 10.35 s |
| 23 | [Coprocessor Conflagration](tests/aoc2017/day23.tin) | 0.04 s | 0.06 s |
| 24 | [Electromagnetic Moat](tests/aoc2017/day24.tin) | 0.72 s | 1.71 s |
| 25 | [The Halting Problem](tests/aoc2017/day25.tin) | 2.12 s | 6.42 s |

The slow ones:

- **Day 5** follows 27.5 million jumps through a list of offsets, each
  updated in place.
- **Day 25** runs a Turing machine for 12.3 million steps, its tape a set
  of the positions holding a 1.
- **Day 15** makes 40 million pairs of generator values for part 1, and
  about 60 million values, kept or skipped, for part 2's 5 million pairs.
- **Day 22** runs 10 million bursts of the virus carrier over a map of
  node states.
- **Day 14** computes 128 knot hashes of 64 rounds each. It is the one
  solution that runs no faster natively than on the tree-walker: its
  time is reversing and joining lists in the runtime both share.

## 2016

| Day | Puzzle | Native | Tree-walker |
|---:|---|---:|---:|
| 1 | [No Time for a Taxicab](tests/aoc2016/day01.tin) | 0.01 s | 0.00 s |
| 2 | [Bathroom Security](tests/aoc2016/day02.tin) | 0.01 s | 0.00 s |
| 3 | [Squares With Three Sides](tests/aoc2016/day03.tin) | 0.01 s | 0.00 s |
| 4 | [Security Through Obscurity](tests/aoc2016/day04.tin) | 0.02 s | 0.01 s |
| 5 | [How About a Nice Game of Chess?](tests/aoc2016/day05.tin) | 10.27 s | 11.69 s |
| 6 | [Signals and Noise](tests/aoc2016/day06.tin) | 0.01 s | 0.00 s |
| 7 | [Internet Protocol Version 7](tests/aoc2016/day07.tin) | 0.03 s | 0.03 s |
| 8 | [Two-Factor Authentication](tests/aoc2016/day08.tin) | 0.02 s | 0.00 s |
| 9 | [Explosives in Cyberspace](tests/aoc2016/day09.tin) | 0.01 s | 0.01 s |
| 10 | [Balance Bots](tests/aoc2016/day10.tin) | 0.02 s | 0.01 s |
| 11 | [Radioisotope Thermoelectric Generators](tests/aoc2016/day11.tin) | 0.32 s | 0.85 s |
| 12 | [Leonardo's Monorail](tests/aoc2016/day12.tin) | 0.04 s | 0.00 s |
| 13 | [A Maze of Twisty Little Cubicles](tests/aoc2016/day13.tin) | 0.01 s | 0.00 s |
| 14 | [One-Time Pad](tests/aoc2016/day14.tin) | 7.68 s | 11.12 s |
| 15 | [Timing is Everything](tests/aoc2016/day15.tin) | 0.01 s | 0.00 s |
| 16 | [Dragon Checksum](tests/aoc2016/day16.tin) | 0.29 s | 0.27 s |
| 17 | [Two Steps Forward](tests/aoc2016/day17.tin) | 0.09 s | 0.10 s |
| 18 | [Like a Rogue](tests/aoc2016/day18.tin) | 0.19 s | 0.22 s |
| 19 | [An Elephant Named Joseph](tests/aoc2016/day19.tin) | 0.01 s | 0.00 s |
| 20 | [Firewall Rules](tests/aoc2016/day20.tin) | 0.01 s | 0.00 s |
| 21 | [Scrambled Letters and Hash](tests/aoc2016/day21.tin) | 0.01 s | 0.00 s |
| 22 | [Grid Computing](tests/aoc2016/day22.tin) | 0.16 s | 0.24 s |
| 23 | [Safe Cracking](tests/aoc2016/day23.tin) | 0.04 s | 0.00 s |
| 24 | [Air Duct Spelunking](tests/aoc2016/day24.tin) | 0.12 s | 0.19 s |
| 25 | [Clock Signal](tests/aoc2016/day25.tin) | 0.57 s | 2.31 s |

The slow ones:

- **Day 5** computes 32 million MD5 hashes: 7 million for part 1, and 25
  million for part 2, which starts again from index 0.
- **Day 14** computes 48 million: part 2 hashes each of about 23,700
  indices 2,017 times. On both MD5 days the time is mostly the hashing
  itself, about 100 ns a hash, which both engines share; Python takes
  15 s and 25 s.
- **Day 25** runs the program for each value of `a` from 1 to 196,
  stopping at its first wrong output.
- **Day 11** searches the states of 7 pairs of generators and chips for
  part 2, with the pairs sorted so that which element is which doesn't
  count.
- **Day 16** fills a disk of 35.6 million characters with string
  replacements and counts its zeros, all in the runtime both engines
  share, so it runs no faster natively.

## Measuring again

```sh
cargo build --release
for f in tests/aoc*/day*.tin; do
  /usr/bin/time -p ./target/release/tinsel run --no-examples "$f" 2>&1 >/dev/null | awk -v f="$f" '/^real/ {print f, $2}'
done
# Add --tree after --no-examples for the tree-walker.
```

A solution whose input file is missing (see "Where the input comes from" in
the [README](README.md)) stops with an error, so its time won't mean
anything.
