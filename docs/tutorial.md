# A Tinsel tutorial

Tinsel is a small language for solving [Advent of Code](https://adventofcode.com)
puzzles, and puzzles like them. A Tinsel program reads like a list of
definitions: what the input is, what the answer is, and the functions in
between. Tinsel takes care of the rest: parsing helpers, grids, searches,
numbers of any size, and remembering the results of recursive functions so
that you don't have to.

This tutorial takes about half an hour. It covers:

1. [Getting started](#1-getting-started): building Tinsel and running a program.
2. [The shape of a program](#2-the-shape-of-a-program): examples, parts and input.
3. [The language in brief](#3-the-language-in-brief): values, functions,
   patterns, pipelines and comprehensions.
4. [A worked example](#4-a-worked-example-the-hedge-garden): a puzzle solved
   step by step, from reading a grid to counting 260 sextillion routes.
5. [When things go wrong](#5-when-things-go-wrong): `dbg`, `tinsel eval` and
   stack traces.
6. [Where next](#6-where-next).

Every example here is checked by the test suite, so what you see is what
Tinsel does.

## 1. Getting started

You need a stable Rust toolchain ([rustup.rs](https://rustup.rs)). From the
repository root, build Tinsel and put `tinsel` on your `PATH`:

```sh
cargo install --path .
```

(Or build it with `cargo build --release` and use `./target/release/tinsel`
in place of `tinsel` below.)

Then run the finished program from this tutorial:

```console
$ tinsel run --input docs/tutorial/garden.txt docs/tutorial/garden.tin
part1: 108
part2: 260799980021648409650880
```

If you use VS Code or Cursor, install the extension in `editors/vscode/`
(see the [README](../README.md#vs-code-and-cursor)). It colours Tinsel,
shows each built-in's documentation as you type, runs programs and tests
from the editor, and shows the value of any expression you select.

## 2. The shape of a program

Here is a complete program for a small made-up puzzle. The input is rows of
numbers. Part 1 is the sum of all of them; part 2 is the sum of each row's
largest number.

<!-- file: docs/tutorial/first.tin -->
```tinsel
example """
3 4 5
10 -2
"""
  part1 = 20
  part2 = 15

rows = input.lines.map(.ints)

part1 = rows.flatten.sum
part2 = rows.map(.max).sum
```

- **`example`** gives a worked example and the answers it should produce.
  Inside an example, `input` is the example's text. Tinsel checks every
  example before it runs your real input, so a wrong answer on the example
  stops you before you submit one.
- **`input`** is the puzzle input as a string. `input.lines` splits it into
  lines, and `.map(.ints)` turns each line into the list of numbers in it.
- **`part1`** and **`part2`** are the answers. They're ordinary
  definitions: Tinsel prints whichever ones the program defines.
- Definitions can come in **any order**, and each is worked out only when
  it's first needed.

`tinsel test` checks the examples (and any `test` lines) without needing
any input:

```console
$ tinsel test docs/tutorial/first.tin
1 passed, 0 failed, 0 skipped
```

For a real Advent of Code puzzle, add a `day` line at the top, such as
`day 2024.1`. Tinsel then finds your input by itself, in
`input/2024/y24d01.txt` next to the program or in any folder above it, and
`tinsel run day01.tin` runs it. Once the site has accepted your answers,
record them in an `answers` block, so that a later change can't break the
solution without you noticing:

```text
answers
  part1 = 2769675
  part2 = 24643097
```

## 3. The language in brief

The examples in this section are `test` lines: each one names a check and
gives an expression that must be `true`. `tinsel test` runs them, so a file
of tests is a good way to try things out.

### Values

All values are immutable: an "update" gives a new value and leaves the old
one as it was.

```tinsel
test "integers never overflow" = 2 ** 100 == 1267650600228229401496703205376
test "division rounds down" = -7 / 2 == -4
test "indexing a string gives a char" = "abc"[0] == 'a'
test "interpolation" = "{1 + 2} wise men" == "3 wise men"
test "lists" = [1, 2] ++ [3] == [1, 2, 3]
test "sets" = {3, 1, 3} == {1, 3}
test "maps" = {"a": 1, "b": 2}["b"] == 2
test "ranges" = list(1..<4) == [1, 2, 3] and list(1..3) == [1, 2, 3]
test "none for something missing" = {"a": 1}.get("z") == none
```

Tuples, such as `(3, 4)`, have a fixed length, and they double as
**vectors**: arithmetic works on each element. That makes positions and
directions easy.

```tinsel
test "adding positions" = (1, 2) + (10, 20) == (11, 22)
test "scaling" = (1, 2) * 3 == (3, 6)
test "wrapping around" = (12, -1) % (10, 10) == (2, 9)
```

There is no "truthiness": `if`, `and`, `filter` and the rest need a real
`true` or `false`, and booleans aren't numbers.

### Functions

A function can be written as several **clauses**, tried in order. Each
parameter is a pattern, and `when` adds a condition. A `where` block holds
local definitions.

```tinsel
square(x) = x * x

fact(0) = 1
fact(n) = n * fact(n - 1)

describe(n) when n < 0 = "negative"
describe(0) = "zero"
describe(_) = "positive"

hypotenuse_squared(a, b) = aa + bb
  where
    aa = square(a)
    bb = square(b)

test "a function" = square(7) == 49
test "clauses" = fact(20) == 2432902008176640000
test "guards" = [-5, 0, 5].map(describe) == ["negative", "zero", "positive"]
test "where" = hypotenuse_squared(3, 4) == 25
```

`if` always has an `else`, and `match` picks the first arm whose pattern
fits:

```tinsel
collatz(n) = if n % 2 == 0 then n / 2 else 3 * n + 1

shape(x) = match x
  0 => "zero"
  n: int when n < 0 => "negative"
  (a, b) => "a pair adding up to {a + b}"
  [first, ..rest] => "a list starting with {first}"
  _ => "something else"

test "if" = collatz(6) == 3 and collatz(3) == 10
test "match" = shape((2, 3)) == "a pair adding up to 5"
test "match on a list" = shape([7, 8, 9]) == "a list starting with 7"
```

### Pipelines

`x.f(y)` means the same as `f(x, y)`, and `x.f` means `f(x)`. This works
for every function, built-in or your own, so a computation reads from left
to right, in the order it happens.

Lambdas are written `x => ...`. Two shorthands cover the common cases:
`_ * 10` is `x => x * 10`, and `.len` is `x => x.len`.

```tinsel
double(x) = x * 2

test "method syntax" = len("abc") == "abc".len
test "your own functions too" = 21.double == 42
test "a pipeline" = "3 1 2".ints.sort.map(double) == [2, 4, 6]
test "a lambda" = [1, 2, 3].map(x => x * 10) == [10, 20, 30]
test "the _ placeholder" = [1, 2, 3].map(_ * 10) == [10, 20, 30]
test "a leading dot" = ["ab", "cde"].map(.len) == [2, 3]
test "tuples spread over parameters" = [(1, 2), (3, 4)].map((a, b) => a * b) == [2, 12]
```

The library has about 120 functions for this kind of work: `lines`,
`ints`, `counts`, `group_by`, `windows`, `bfs`, `dijkstra`, `find_cycle`
and many more. The [library reference](library.md) lists them all with
examples, and `tinsel doc NAME` shows one in the terminal.

### Comprehensions

Comprehensions build lists, sets and maps from loops. A comprehension
passed straight to a function needs no extra brackets.

```tinsel
test "a list" = [x * x for x in 1..6 if x % 2 == 1] == [1, 9, 25]
test "a map" = {w: w.len for w in ["hi", "there"]} == {"hi": 2, "there": 5}
test "into a function" = sum(x * x for x in 1..3) == 14
test "nested loops" = [(a, b) for a in 1..2 for b in "xy"] == [(1, 'x'), (1, 'y'), (2, 'x'), (2, 'y')]
```

### String patterns

A string with `{...}` in a pattern parses text. `{name}` captures a string,
and `{n:int}` an integer. In a comprehension, lines that don't match are
skipped.

```tinsel
parse("{name} is {age:int} years old") = (name, age)

test "parsing a line" = parse("Rudolph is 7 years old") == ("Rudolph", 7)
test "patterns filter" = [n for "x={n:int}" in ["x=1", "y=2", "x=3"]] == [1, 3]
```

For a record that takes several lines, write the pattern in triple
quotes, laid out like the text. Each line of the pattern matches one line
of the text, so a missing or extra line means no match.

```tinsel
reindeer("""
Name: {name}
Speed: {speed:int}""") = (name, speed)

test "a record over two lines" = reindeer("Name: Dasher\nSpeed: 14") == ("Dasher", 14)
```

## 4. A worked example: the hedge garden

### The puzzle

A palace garden is laid out as a grid of squares. Some squares are hedge
(`#`), and the rest are paths (`.`). You start at `S` and want to reach
`E`, moving one square at a time up, down, left or right, never into a
hedge.

```text
###########
#S..#.....#
#.#.#.#.#.#
#.#...#...#
#.#####.#.#
#.......#E#
###########
```

- **Part 1:** how many steps does the shortest route take? Here, 16.
- **Part 2:** how many different shortest routes are there? Here, 3.

The real garden, in `docs/tutorial/garden.txt`, is 61 squares wide and 31
high, with long hedges and wide open lawns.

### Reading the garden

Start the program with the example and its answers. Tinsel will check them
for us at every step.

<!-- start: garden -->
```tinsel
example """
###########
#S..#.....#
#.#.#.#.#.#
#.#...#...#
#.#####.#.#
#.......#E#
###########
"""
  part1 = 16
  part2 = 3

garden = input.grid
start = garden.find('S')
finish = garden.find('E')
```

`input.grid` turns the text into a grid. Positions are `(x, y)` tuples:
`x` counts across from 0 on the left, and `y` counts **down** from 0 at the
top, the way the text is laid out. `garden[(1, 1)]` is the cell at a
position, here `'S'`, and `garden.find('S')` is the position of the first
`'S'`.

`tinsel eval` shows the value of any expression in a program. With
`--example 1`, `input` is the first example's text:

```console
$ tinsel eval --example 1 docs/tutorial/garden.tin start
(1, 1)
```

### Taking a step

From a square, we can step to each neighbour that isn't hedge:

<!-- continues: garden -->
```tinsel
steps(p) = garden.nbrs4(p).filter(q => garden[q] != '#')
```

`garden.nbrs4(p)` gives the four neighbours of `p` that are inside the
grid. From the start, we can go right or down:

```console
$ tinsel eval --example 1 docs/tutorial/garden.tin 'steps(start)'
[(2, 1), (1, 2)]
```

### Part 1: the shortest route

Finding shortest routes is breadth-first search, and Tinsel has it built
in. `bfs(start, steps)` explores outwards from `start`, calling `steps` for
the squares one step from each square, and gives a map from every square
it reaches to the number of steps to it.

<!-- continues: garden -->
```tinsel
dist = bfs(start, steps)

part1 = dist[finish]
```

```console
$ tinsel eval --example 1 docs/tutorial/garden.tin 'dist[finish]'
16
```

### Seeing the route

With `goal:`, `bfs` stops at the goal and gives `(steps, route)`, where the
route lists every square from the start to the goal. To draw it, mark each
path square of the route with `o`. `g.set(p, 'o')` gives a copy of the
grid with one square changed, `fold` does that for each square in turn,
and `show` turns the grid back into text.

<!-- continues: garden -->
```tinsel
route = bfs(start, steps, goal: finish).1
picture = route.filter(p => garden[p] == '.').fold(garden, (g, p) => g.set(p, 'o')).show
```

`tinsel eval` shows a string in quotes, the way you would write it in a
program. `--raw` prints it as it is:

```console
$ tinsel eval --raw --example 1 docs/tutorial/garden.tin picture
###########
#Soo#ooooo#
#.#o#o#.#o#
#.#ooo#..o#
#.#####.#o#
#.......#E#
###########
```

### Part 2: counting the routes

How many shortest routes reach a square `p`? Think about the last step of
a shortest route to `p`. It comes from a neighbour `q` that is one step
closer to the start: `dist[q] == dist[p] - 1`. Coming from any other
neighbour would make the route longer. Everything before that last step is
a shortest route to `q`.

So to count the shortest routes to `p`, add up the counts for those
neighbours. If 2 shortest routes reach the square above `p` and 3 reach the
square to its left, then 5 reach `p`.

The counting ends at the start, which has exactly one route to itself: the
route that hasn't taken a step yet.

<!-- continues: garden -->
```tinsel
routes(p) =
  if p == start then 1
  else sum(routes(q) for q in steps(p) if dist[q] == dist[p] - 1)

part2 = routes(finish)
```

The example agrees: there are three shortest routes.

```console
$ tinsel test docs/tutorial/garden.tin
1 passed, 0 failed, 0 skipped
```

Now the real garden:

```console
$ tinsel run --input docs/tutorial/garden.txt docs/tutorial/garden.tin
part1: 108
part2: 260799980021648409650880
```

Two things happened here that Tinsel took care of:

- **The answer is huge**, about 2.6 × 10²³, far beyond a 64-bit integer.
  Tinsel's integers grow as needed, so nothing overflows.
- **`routes` would take forever if written this way in most languages.**
  Each square's count is used by the squares after it, so the recursion
  asks for the same squares again and again: the number of calls grows as
  fast as the answer. Tinsel sees that `routes` calls itself, other than
  as its very last step, and so it remembers each result the first time
  it's worked out (memoization). Each square is then counted once, and the
  program finishes in milliseconds. You don't need to write a cache, or
  even think about one: any recursive function like this is remembered.
  (A function that only calls itself as its very last step is a loop, and
  isn't remembered. `@nomemo` and `@memo` override the choice; see
  [SPEC 8.1](../SPEC.md#81-automatic-memoization).)

### The whole program

<!-- file: docs/tutorial/garden.tin -->
```tinsel
# The hedge garden: the worked example in docs/tutorial.md.
# Part 1: how many steps is the shortest route from S to E?
# Part 2: how many different shortest routes are there?

example """
###########
#S..#.....#
#.#.#.#.#.#
#.#...#...#
#.#####.#.#
#.......#E#
###########
"""
  part1 = 16
  part2 = 3

garden = input.grid
start = garden.find('S')
finish = garden.find('E')

# The squares one step from p that aren't hedge.
steps(p) = garden.nbrs4(p).filter(q => garden[q] != '#')

# How many steps every reachable square is from the start.
dist = bfs(start, steps)

part1 = dist[finish]

# The number of shortest routes from the start to p. Each one arrives
# from a neighbour that is one step closer to the start.
routes(p) =
  if p == start then 1
  else sum(routes(q) for q in steps(p) if dist[q] == dist[p] - 1)

part2 = routes(finish)

# The first shortest route, drawn on the garden.
route = bfs(start, steps, goal: finish).1
picture = route.filter(p => garden[p] == '.').fold(garden, (g, p) => g.set(p, 'o')).show
```

## 5. When things go wrong

**`dbg`** prints a value, with where it is and the code that produced it,
and returns the value unchanged. So it can go in the middle of anything:
`garden.nbrs4(p).dbg.filter(...)` prints each list of neighbours as it's
found, without changing what the program does.

**`tinsel eval`** shows the value of any expression, as above. Only what
the expression needs is worked out, so it's a quick way to check what
parsing gives, or what one function returns for one case.

**Errors** show where they happened and the calls that led there, with
their arguments. Asking for the routes to a hedge square, which `bfs`
never reached:

```console
$ tinsel eval --example 1 docs/tutorial/garden.tin 'routes((2, 2))'
Error: key (2, 2) is not in the map
    ╭─[ docs/tutorial/garden.tin:33:54 ]
    │
 33 │   else sum(routes(q) for q in steps(p) if dist[q] == dist[p] - 1)
    │
    ├─[ <expression>:1:1 ]
    │
  1 │ routes((2, 2))
    │ ───────┬──────
    │        ╰──────── `routes((2, 2))` called here
────╯
```

The error is in `dist[p]`, at line 33, column 54: `(2, 2)` has no
distance because it's a hedge.

**`tinsel doc NAME`** shows a built-in's description and examples:

```console
$ tinsel doc nbrs4
g.nbrs4(p), nbrs4(p) → list    (Grids and positions)

The four neighbours of `p`, in the order N, E, S, W. On a grid, only those
inside it.

    nbrs4((0, 0)) == [(0, -1), (1, 0), (0, 1), (-1, 0)]
    "...\n...".grid.nbrs4((0, 0)) == [(1, 0), (0, 1)]
```

**`tinsel check`** reports syntax errors and unknown names without running
anything. The editor extension runs it every time you save.

## 6. Where next

- **[The library reference](library.md)**: every built-in function, with
  examples.
- **[The language specification](../SPEC.md)**: the full definition, for
  when you need the exact rules.
- **The solutions in `tests/aoc2018` to `tests/aoc2025`**: 187 Advent of
  Code puzzles solved in Tinsel. For more grids and searches, try
  2024 day 16 (`tests/aoc2024/day16.tin`); for more memoization, 2023 day
  12 (`tests/aoc2023/day12.tin`).
- **The [README](../README.md)**: running, testing and timing solutions,
  and the editor extension.
