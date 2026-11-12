# Tinsel 🎄✨

## Deck the halls with algorithms 🎵

Tinsel is a small, pure programming language built especially for Advent of
Code. It combines functional programming, puzzle-friendly syntax, and native
compilation—so you can spend December solving riddles instead of wrestling
boilerplate.

## How did this happen? 🎅

Tinsel owes its existence to two powerful forces:

1. Two free Claude resets that needed using before December.
2. A complete lack of effective adult supervision.

Naturally, the only reasonable response was to build a new programming
language and native compiler for Advent of Code. 🧑‍💻🎄

## Why put Tinsel under your tree? 🎁

- **Free dynamic programming, courtesy of purity!** 🧠

  Pure functions always return the same result for the same arguments, so
  Tinsel can automatically memoize them.

- **Recursion without the stack anxiety** 🛷

  Tail calls run in constant stack space, while deep non-tail recursion is
  supported too.

- **Puzzle-shaped batteries included** 🔋

  Grids, vectors, ranges, persistent collections, comprehensions, BFS,
  Dijkstra, cycle detection, and more are built in.

- **Parsing without the naughty list** 📜

  String patterns, interpolation, and collection pipelines make transforming
  puzzle input concise.

- **Integers as large as your Christmas ambitions** ♾️

  Arbitrary-precision integers are standard, with a fast path for small
  values.

- **Pure values, fewer surprises** ❄️

  Values never change behind your back, making puzzle solutions easier to
  understand, test, and reuse.

- **Examples are executable ornaments** 🧪

  Put examples, expected answers, and regression tests directly beside the
  solution.

- **Compiled at sleigh speed** ⚡

  Tinsel compiles through its own intermediate representation to native code
  with Cranelift.

## Examples

If you remember the FFT and DAC problem from 2025 day 11, here is a minimal
Tinsel example that takes approximately 6ms to compile and run on a MacBook M3:

```tinsel
day 2025.11

graph = { name: outs.words for "{name}: {outs}" in input.lines }

paths(a, b) = if a == b then 1 else sum(paths(m, b) for m in graph.get(a, []))

part1 = paths("you", "out")
part2 = paths("svr", "fft") * paths("fft", "dac") * paths("dac", "out")
      + paths("svr", "dac") * paths("dac", "fft") * paths("fft", "out")
```

A more complicated ILP problem from 2025 day 10 involving joltage and taking
around 300ms on the same machine:

```tinsel
day 2025.10

parse("[{lights}] {buttons} {{{joltage}}}") =
  (tuple(int(c == '#') for c in lights), buttons.words.map(.ints), tuple(joltage.ints))

solve(lights, buttons, joltage) = (t[lights].map(.1).min, fewest(joltage))
  where
    t = subsets(buttons)
          .map(bs => (tuple(bs.count(i in _) for i in 0..<joltage.len), bs.len))
          .group_by((v, _) => v % 2)
    fewest(goal) when all(goal .== 0) = 0
    fewest(goal) = min(n + 2 * fewest((goal - v) / 2)
                       for (v, n) in t.get(goal % 2, []) if all(v .<= goal)) ?? inf

total = input.lines.map(parse).map(solve).sum

part1 = total.0
part2 = total.1
```

## The elevator pitch

> Tinsel is a pure, dynamically typed language for Advent of Code. It turns
> recursive definitions into memoized dynamic programs, makes grids and
> collections first-class, and compiles solutions to native code— all with
> syntax designed for solving puzzles before the cocoa gets cold. ☕🎄

## The stocking-sized version

> **Tinsel: less boilerplate, more baubles, free DP.** ✨
