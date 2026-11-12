# Tinsel language specification

Tinsel is a small language for solving Advent of Code puzzles. You write the
answer as definitions (equations), as in Haskell, and process data in
pipelines, as in Kotlin. Recursive functions are memoized automatically, so
dynamic programming needs no extra code.

This document is the source of truth for the interpreter. Every feature here
should have at least one test under `tests/`. Section numbers are cited from
the code, so they stay fixed.

---

## 1. Goals and non-goals

**Goals**

- Short, readable solutions. The puzzle logic should be most of the code.
- Parsing, memoization and checking against examples are built in.
- Fast enough that a reasonable algorithm finishes in seconds.
- Clear error messages that point at the source line.

**Non-goals (for now)**

- General-purpose programming: files, networking, concurrency.
- Static typing. Tinsel is dynamically typed.
- Floating point. Integers are arbitrary precision; see open question Q3.

---

## 2. Programs

A program is one `.tin` file containing a sequence of **top-level items**:

```tinsel
day 2025.1                 # optional header: year.day

example """                # zero or more worked examples
L50
R200
"""
  part1 = 1
  part2 = 3

answers                    # optional answers for the real input
  part1 = 1177
  part2 = 6768

moves = input.lines.map(...)    # definitions, in any order
part1 = ...
part2 = ...

test "name" = expr         # zero or more unit tests
```

### 2.1 Definitions are order-independent

Top-level definitions can appear in any order and can refer to each other
freely. Each name can be defined only once. A top-level constant is
evaluated the first time it is used, and only once. If a constant depends
on itself (for example `a = b + 1`, `b = a`), evaluation stops with an
error naming the cycle.

### 2.2 `input`

`input` is a built-in constant holding the puzzle input as a `str`.

- When running a program normally, it is the real input file (see 2.5).
- Inside an `example` check, it is that example's text.
- When there is no `day` header and no `--input` flag, using `input` is an
  error.

### 2.3 `example` blocks

```tinsel
example """
...text...
"""
  part1 = 3
  part2 = 10
```

The indented lines after the string list the expected values. Either part
may be left out. A program can have several `example` blocks.

Puzzles sometimes use a different parameter for the example than for the
real input, such as "connect the 10 closest pairs" instead of 1000. A
`with` line overrides a top-level constant for that example only:

```tinsel
example """
...
"""
  with pairs = 10
  part1 = 24

pairs = 1000
```

`tinsel run` checks every example first. If any check fails, it reports
expected and actual values and does **not** run the real input.

### 2.4 `answers` block

Lists the known answers for the real input, used as a regression check.
`tinsel test` checks them when the input file exists and skips them, with a
note, when it doesn't. That way the tests still pass on machines without
your inputs.

### 2.5 Finding the input

For `day 2025.1`, the interpreter looks for `input/2025/y25d01.txt` in the
program's directory and then in each parent directory. The first match
wins. `--input PATH` overrides the search.

### 2.6 `test` declarations

```tinsel
test "floor division" = -7 / 2 == -4
test_error "length mismatch" = (1, 2) + (1, 2, 3)
```

- `test` passes when its expression evaluates to `true`. When the top-level
  expression is an `==` comparison, a failure shows both sides.
- `test_error` passes when evaluating its expression raises a runtime
  error.
- A `test` or `test_error` that needs the real input is skipped, with a
  note, when the input file is missing (as `answers` is, 2.4).

### 2.7 Command line

```
tinsel run FILE.tin [--input PATH] [--no-examples] [--tree | --ir | --native]
tinsel test [--tree | --ir | --native] [FILE_OR_DIR ...]   # runs tests, examples and answers
tinsel repl [FILE.tin]             # planned, not yet implemented
```

`run` prints `part1: <value>` and `part2: <value>` for whichever parts are
defined. If neither is defined, it says so and exits with an error status.

By default, `run` and `test` compile the program to machine code before
running it. `--tree` runs it on the tree-walking interpreter instead,
which is the reference implementation: slower, but simpler. `--ir` runs it
on the IR interpreter, which interprets the form the native compiler works
from; it exists to check that compiler. `--native` asks for the default.
The results and errors are the same whichever is used; only the speed
differs. Giving two different ones is an error.

### 2.8 `include`

Some years reuse one piece of code across several days, such as 2019's
Intcode computer. `include` shares it without copying:

```tinsel
day 2019.9
include "intcode.tin"
```

- The path is a plain string (no interpolation), relative to the directory
  of the file containing the `include`.
- The included file's top-level definitions join the program, exactly as
  if they had been written in it. They are order-independent like any
  others (2.1), and a name defined in both files is a compile error, as any
  duplicate definition is. There are no namespaces or export lists.
- An included file can include other files. A file included more than
  once, directly or not, is included only once. An include cycle is a
  compile error.
- An included file can't have a `day` header, `example` blocks or an
  `answers` block. Its `test` and `test_error` declarations are ignored
  when it is included; they run when the file is tested on its own, so a
  shared library carries its own unit tests. Since `tinsel test` runs every
  `.tin` file it finds, a library must compile on its own, without
  definitions from the files that include it.
- Error messages and stack traces name the file and line where the code
  is written, including inside included files.
- `include` is not a keyword: it is recognised only at the start of a
  top-level item and followed by a string, so it is still usable as a
  name.

---

## 3. Lexical structure

- **Comments** run from `#` to the end of the line.
- **Identifiers** match `[A-Za-z_][A-Za-z0-9_]*`. Convention: `snake_case`,
  with `UPPER_CASE` for library constants such as `N` and `DIRS4`.
- **Keywords:** `where match when if then else for let in and or not xor
  true false none inf`. The item words `day`, `example`, `answers`, `test`
  and `test_error` are keywords only at the start of a top-level item: in
  column 1, at the start of a line, and followed by what the item needs (a
  number after `day`, a string after `example`, `test` and `test_error`,
  and nothing else on the line after `answers`). Anywhere else they are
  ordinary names, so `day(n) = ...` defines a function.
- **Integers:** `0`, `42`, `1_000_000`. There is no floating-point syntax,
  so `5.double` lexes as `5` `.` `double`.
- **Strings:** `"..."` with the escapes `\n \t \\ \"` and no others.
  `{expr}` interpolates; the expression may contain nested strings, such as
  `"{xs.join(",")}"`. `{{` and `}}` are literal braces. Any other `{` or `}`
  that doesn't open or close an interpolation is a compile error. Test
  names and `example` strings are not interpolated.
- **Characters:** `'a'`, `'\n'`, `'\''`: the string escapes, plus `\'`.
- **Triple-quoted strings:** `"""..."""`. If the opening quotes are followed
  directly by a newline, that newline is dropped. Everything else is kept
  verbatim, with no escapes and no interpolation.

### 3.1 Layout

Tinsel uses indentation in a similar way to Haskell, with simpler rules:

1. Each top-level item starts in column 1.
2. A line indented further than the start of the current item or block
   entry *continues* it.
3. `where`, `match`, `example` and `answers` open a **block**. The block's
   entries are the following lines indented deeper than the line that
   opened it. All entries must start at the same column as the first one,
   and lines indented deeper than that continue an entry.
4. Inside `(...)`, `[...]` and `{...}`, newlines and indentation are
   ignored.

---

## 4. Values

All values are **immutable**.

| Type | Examples | Notes |
|---|---|---|
| `int` | `42`, `2 ** 100`, `inf` | Arbitrary precision (implementations should use an `i64` fast path). `inf` and `-inf` are ints greater (or less) than every other. They absorb arithmetic, `inf + 1 == inf`, and undefined combinations such as `inf - inf` and `inf * 0` are errors. |
| `bool` | `true`, `false` | **Not a number.** Arithmetic on a bool (`true + 1`, `[true].sum`) is an error; convert with `int(b)`. **There is no truthiness either:** `if`, `when`, `and`, `or`, `not`, comprehension `if`, and the predicates passed to `filter`, `count`, `any`, `all` and `first` must produce a `bool`. Anything else is an error. |
| `char` | `'a'` | One Unicode scalar value. |
| `str` | `"abc"` | Indexing gives `char`. |
| `none` | `none` | "No value". Returned by lookups that can fail. |
| `tuple` | `(1, 2)`, `(x,)` | Fixed length. Also serves as a **vector**; see 5.3. |
| `list` | `[1, 2, 3]` | |
| `set` | `{1, 2}` | `set()` is the empty set. |
| `map` | `{"a": 1}` | `{}` is the empty map. |
| `range` | `1..5`, `0..<n`, `1..` | Lazy. `..` includes the end and `..<` excludes it. `a..` never ends, and neither does a range ending at `inf`. `..<` leaves out its end even when that is `inf` or `-inf`, so `inf..<inf` is empty and `inf in (1..<inf)` is `false`. A non-empty range starting at `-inf` or `inf` has no first element, so iterating over it is an error. |
| `function` | `x => x + 1`, `fact` | |
| `grid` | `input.grid` | A rectangle of cells addressed by `(x, y)` positions (9.3). |
| `rangeset` | `rangeset([1..5, 9])` | A set of ints stored as merged ranges (9.8). |
| `generator` | `(x for x in xs)` | Lazy and **single-pass**: only one operation (including `len`) can read it, and reading it again is an error. Collect it with `list()`, `tuple()` or `set()` to use it more than once. |

### 4.1 Equality, ordering and hashing

- `==` and `!=` compare structurally and **never fail**. Values of different
  types are simply unequal: `1 == "1"`, `true == 1` and `none == 0` are all
  `false`. This keeps checks like `x == none` and literal patterns safe on
  any value. It also means `1` and `true` are different map keys.
- **Ordering (`<`, `<=`, `>`, `>=`, `sort`, `min`, `max`) is defined only
  between values of the same type. Comparing values of different types is a
  runtime error** that names both types. This is deliberate: silently
  ordering `"5"` against `3` hides bugs. Within a type:
  - `int`: numeric, with `-inf` and `inf` at the ends. `inf` counts as an
    `int` for this purpose.
  - `bool`: `false < true`.
  - `char`, `str`: by Unicode scalar value, so strings compare
    lexicographically.
  - `tuple`, `list`: lexicographic. Comparing two elements of different
    types is an error, as is comparing a tuple with a list.
  - `set`, `map`: lexicographic over their sorted elements (or key/value
    pairs).
  - `none`, `range`, `grid`, `rangeset`, functions and generators: not
    ordered.
- Every value except functions and generators is hashable, so it can be a
  map key or a set element. Memoization handles functions and generators
  specially (8.1).
- **Maps and sets iterate in sorted key order**, so iteration is
  deterministic and doesn't depend on insertion order. Updated copies share
  structure with the original, so `visit(p, seen | {p})` costs O(log n)
  rather than O(n). (Why sorted rather than hashed: Q5.)
- **Updates in place.** An implementation may update a map, set, list or
  grid in place when nothing else can refer to it, such as at a local's
  last use. This changes only speed, never a result. It applies to `set`,
  `add` and `remove`, to `|` and `-` on sets, `|` on maps, and `++` on
  lists, so loops like `step(seen | {p}, ...)`, `step(g.set(p, '#'), ...)`
  or `xs.fold([], (acc, x) => acc ++ [x])` don't copy. A grid update, and
  `set` or `++` of a short list on an unshared list, costs O(1); a list
  indexed by small ints is often faster than a map with the same keys.
- Keys in one map, and elements of one set, must have the same type.
  Adding a key of a different type, as in `{1: "a", "b": 2}`, is a runtime
  error. For types that `<` can't order (`none`, ranges, grids), and for
  tuples and lists whose elements have different types, sets and maps use
  a fixed internal order. So a set of grids works, for example to record
  states already seen.

---

## 5. Expressions

### 5.1 Operators, highest precedence first

| Level | Operators | Associativity |
|---|---|---|
| 1 | `f(x)`  `x.name`  `x.name(args)`  `x[i]`  `x.0` | left |
| 2 | `**` | right |
| 3 | unary `-`, `not`, `~` | prefix |
| 4 | `*`  `/`  `%` | left |
| 5 | `+`  `-`  `++` | left |
| 6 | `<<`  `>>` | left |
| 7 | `&` | left |
| 8 | `xor` | left |
| 9 | `\|` | left |
| 10 | `..`  `..<` | none |
| 11 | `.==` `.!=` `.<` `.<=` `.>` `.>=` | left |
| 12 | `==` `!=` `<` `<=` `>` `>=` `in` `not in` | chained, as in Python |
| 13 | `and` | left |
| 14 | `or` | left |
| 15 | `??` | right |
| 16 | `if c then a else b`, `x => e`, `match` | |

There is no function-composition or pipe operator. Pipelines are written
with method-call syntax (5.2), and composition with leading-dot chains
(5.4).

- **Integer division** `/` rounds toward negative infinity: `-7 / 2 == -4`.
- `~n` is bitwise not on an int, `-n - 1`, as in two's complement: `~5 == -6`.
- **Modulo** `%` takes the sign of the divisor: `-7 % 3 == 2`, so
  `a == (a / b) * b + a % b`.
- **Chained comparisons:** `0 <= x < w` means `0 <= x and x < w`.
- `a ?? b` gives `a` unless `a` is `none`, in which case it gives `b`.
- `++` concatenates strings and lists. `+` on lists is an error, because
  `+` on tuples is element-wise.
- **Sets:** `a | b` is union, `a & b` intersection, `a - b` difference and
  `a xor b` symmetric difference.
- **Maps:** `a | b` merges, with values from `b` winning when a key is in
  both.
- A trailing comma is allowed in any bracketed list.

### 5.2 Calls and method syntax

- `x.f(a, b)` means `f(x, a, b)`, and `x.f` with no parentheses means
  `f(x)`. This works for both built-in and user-defined functions. This
  "uniform function call syntax" is how Tinsel writes **pipelines**:
  `input.lines.map(parse).filter(valid).len` reads left to right, in the
  order the steps happen.
- In `x.f`, a constant or variable named `f` doesn't hide a library
  function `f`: only a function definition does. So a program can define
  `width = 101` and still call `g.width`. When no library function has
  that name, `x.f` calls whatever `f` is, such as a lambda parameter.
- `x.0`, `x.1`, ... index tuples and lists.
- **Tuple spreading:** if a function taking *n* ≥ 2 parameters is called
  with exactly one argument, and that argument is an *n*-tuple, the tuple is
  spread across the parameters. This is what lets
  `pairs.map((a, b) => a + b)` and `machines.map(solve)` work. It also
  applies to library functions that always take exactly *n* ≥ 2 arguments
  (not counting named ones), such as `gcd`, `manhattan` and `fold`, so
  `pairs.map(gcd)` works. It doesn't apply where the number of arguments
  changes the meaning: `min((3, 1))` is the minimum of the tuple.

### 5.3 Tuples as vectors

The arithmetic operators `+ - * / %` work on tuples element-wise:

- tuple ∘ tuple requires equal lengths, otherwise it's an error;
- tuple ∘ int and int ∘ tuple apply the int to every element.

Unary `-` negates every element. Elements that are themselves tuples are
handled the same way, recursively: `((1, 2), 3) + ((10, 20), 30) ==
((11, 22), 33)`.

```tinsel
(1, 2) + (10, 20) == (11, 22)
(1, 2) * 3        == (3, 6)
goal % 2                       # parity vector
```

The dotted comparisons `.== .!= .< .<= .> .>=` compare element-wise and
return a tuple of bools: `all(v .<= goal)`. The ordinary comparisons
`< <=` etc. on tuples are lexicographic, so tuples sort sensibly.

### 5.4 Lambdas and shorthands

```tinsel
x => x + 1
(a, b) => a * b
((x, y), d) => ...              # parameters are patterns (section 6)
"{dir:char}{n}" => ...          # including string patterns
```

A lambda's body extends as far to the right as possible.

**Placeholder `_`.** Wherever an expression is expected, an expression
containing `_` is turned into a one-parameter lambda. The lambda covers the
nearest enclosing call argument, or the whole right-hand side of a
definition. **Every `_` in that expression refers to the same argument**,
so `_ * _` squares. (Scala works differently.)

A call argument that is just `_` by itself doesn't count as the enclosing
argument: the lambda covers the next argument out, or the right-hand side.
So `f(_, k)` is a partial application, and `x => x` must be written out.

```tinsel
xs.map(_ * 2)          # x => x * 2
xs.filter(_ > 4)
bs.count(i in _)       # b => i in b
xs.map(f(_, k))        # x => f(x, k)
xs.count(ways(_) > 0)  # x => ways(x) > 0
inc = add(_, 1)        # x => add(x, 1)
```

A `_` that is not inside any call argument or right-hand side, such as one
in a guard, is a compile error.

**Leading dot.** An expression that starts with `.` is a one-parameter
lambda applied to that parameter. The lambda covers only the chain of
postfix operations that follows (`.a.b(1)[0]`). The chain can also start
with an index: `.[1]` is `x => x[1]`. With method-call syntax,
a chain like this also serves as function composition: `.ints.tuple` is
`x => tuple(ints(x))`.

```tinsel
xs.map(.len)           # x => x.len
pairs.map(.1)          # p => p.1
lines.map(.words.len)  # x => len(words(x))
lines.map(.ints.tuple) # x => tuple(ints(x))
rows.map(.[0])         # r => r[0]
```

Because the lambda covers only the chain, `.len == 2` is a function compared
with `2`. For a predicate, write `_.len == 2`.

To compose two lambdas, write `x => g(f(x))`.

**Numbered placeholders.** `_1`, `_2`, ... work like `_`, with the same
rules for how far the lambda extends, but each number is a separate
parameter, in order. The lambda takes as many parameters as the highest
number used, so it spreads a tuple argument (5.2):

```tinsel
pairs.map(_1 * 10 + _2)    # (a, b) => a * 10 + b
xs.fold(0, _1 + _2 * 2)    # (acc, x) => acc + x * 2
xs.map(add(_1, k))         # x => add(x, k), as with `_`
```

One lambda can't mix `_` with numbered placeholders, and `_1`, `_2`, ...
can't be used as names in patterns.

In a parameter pattern, `_` is a wildcard, not a placeholder.

### 5.5 Conditionals and `match`

```tinsel
if c then a else b

match x
  0 => "zero"
  n when n < 0 => "negative"
  (a, b) => "pair"
  [first, ..rest] => "list"
  _ => "other"
```

The arms are tried in order. If none matches, it's a runtime error. A
guard (`when expr`) ends at the arm's `=>`, so a guard can end with a name:
`n when n < limit => ...`. A lambda inside brackets in a guard, as in
`when xs.any(x => x > n)`, is unaffected.

### 5.6 Comprehensions and generators

```tinsel
[e for p in xs if cond]                 # list
{e for p in xs}                         # set
{k: v for p in xs}                      # map
(e for p in xs)                         # generator (lazy)
f(e for p in xs)                        # a generator as the only argument needs no extra parentheses
[(a, b) for a in xs for b in ys]        # nested loops, left to right
[d for (a, b) in ps let d = b - a if d > 0]   # let binds a name for the later clauses
```

`let p = e` is the same as `for p in [e]`: it binds `p` for the clauses
after it and the result expression, and if `p` doesn't match `e`, the item
is skipped.

If the loop pattern `p` doesn't match an item, the item is **skipped** (as
in Haskell). This makes `for "{k}: {v}" in lines` act as a filter as well
as a parser.

When a map comprehension or map literal produces the same key more than
once, the last value wins: `{x % 2: x for x in 1..4} == {0: 4, 1: 3}`.

---

## 6. Patterns

Patterns appear in function parameters, lambda parameters, `match` arms and
comprehension loops.

| Pattern | Matches |
|---|---|
| `x` | anything, and binds it to `x` |
| `_` | anything, without binding |
| `x: int`, `_: str`, ... | a value of that type, and binds it (or not, with `_`). The type names are `int` (including `inf` and `-inf`), `bool`, `char`, `str`, `none`, `tuple`, `list`, `set`, `map`, `range`, `rangeset`, `grid` and `function`. |
| `0`, `'a'`, `"out"`, `true`, `none` | that exact value |
| `(p1, p2)` | a tuple of that length |
| `[p1, p2]` | a list of that length |
| `[p1, ..rest]`, `[.., a, b]`, `[a, .., z]` | a list of at least that length. At most one `..` is allowed, and `..name` binds the middle as a list. |
| `"text {cap} more"` | a string (6.1) |

A name can be bound only once in a pattern, and only once across a
function's or lambda's parameters: `f(x, x)` and `(a, a) => ...` are
compile errors. To require two values to be equal, use a guard.

### 6.1 String patterns

A string literal in pattern position matches the **whole** string. Its
`{...}` parts are captures:

| Capture | Matches | Result |
|---|---|---|
| `{name}` | the shortest text that lets the rest of the pattern match (or everything that's left, if it is last) | always a `str` |
| `{name:str}` | same as `{name}` (an explicit spelling of the default) | `str` |
| `{name:int}` | `[-+]?[0-9]+` (greedy, with no backtracking, so `"{a:int}5"` never matches `"125"`) | `int` |
| `{name:char}` | exactly one character | `char` |
| `{name:word}` | `[A-Za-z0-9_]+` (greedy) | `str` |
| `{_}` | like `{name}` | not bound |

`{{` and `}}` match literal braces. Two plain `{name}` captures next to
each other are a compile-time error, because there's no way to know where
one ends and the next starts; give one of them a type.

A capture's type depends only on the pattern, never on the text it
matches. An untyped capture is always a `str`; anything else needs a type.

```tinsel
move("{dir:char}{n:int}") = ...
parse("[{lights}] {buttons} {{{joltage}}}") = ...
graph = { k: v.words for "{k}: {v}" in input.lines }
```

---

## 7. Definitions

### 7.1 Constants and functions

```tinsel
width = 101                         # constant (lazy, evaluated once)

fact(0) = 1                         # function made of several clauses
fact(n) = n * fact(n - 1)

classify(n) when n < 0 = "neg"      # guard
classify(0) = "zero"
classify(_) = "pos"
```

- A constant's left-hand side can be a pattern, at top level or in a
  `where` block: `[top, ..rows] = input.lines`, `(q, r) = divmod(n, 7)`. If
  the pattern doesn't match, it's a runtime error. The left-hand side counts
  as a pattern when it starts with `(` or `[`; when it starts with an
  identifier followed by `(`, it's a function definition.
- All clauses of a function must be **next to each other**, with no other
  item (not even a `test` or `example`) between them, and have the same
  number of parameters. They are tried in order. If none matches, it's a
  runtime error that shows the arguments.
- A guard (`when expr`) is checked after the parameter patterns match.

### 7.2 `where`

Any definition can be followed by a `where` block of local definitions. The
local definitions can see the enclosing definition's parameters and each
other, in any order. Local constants are lazy, evaluated at most once for
each call of the enclosing function. In a function with several clauses, a
`where` block belongs to the clause it follows, and that clause's guard can
see it. As at top level, each name can be defined only once in a `where`
block.

When a definition's whole body is a `match`, its `where` block can come
after the arms, at the same indentation as the arms:

```tinsel
describe(x) = match x
  0 => "none"
  n when n > lots => "lots"
  _ => "some"
  where
    lots = 100
```

A `where` among the arms must be the last entry, and only one is allowed.

```tinsel
solve(lights, buttons, joltage) = (fewest_toggles, fewest_presses(joltage))
  where
    t = table(buttons, joltage.len)
    fewest_toggles = t[lights].map(.1).min
    fewest_presses(goal) when all(goal .== 0) = 0
    fewest_presses(goal) = ...
```

### 7.3 Annotations

`@memo` and `@nomemo` go on the line before a function's first clause and
override automatic memoization (section 8).

---

## 8. Evaluation and memoization

Expressions are evaluated **strictly** (arguments before the call).
Laziness applies only to top-level and `where` constants (2.1, 7.2), and to
ranges and generators.

### 8.1 Automatic memoization

A named function is **memoized automatically when it is recursive and at
least one of its recursive calls is not a tail call**.

- *Recursive* means the function can call itself, directly or through other
  named functions. The interpreter works this out from the static call
  graph: a function is recursive when it belongs to a strongly connected
  component that contains a cycle.
- A call is a *tail call* when its result is the function's result: the
  whole body of a clause, or a branch of an `if` or `match` arm in tail
  position.
- A function whose recursive calls are **all** tail calls is a **loop**. It
  is not memoized, because a loop rarely revisits a state and its arguments
  are often large (a whole simulation state), so hashing them would cost
  more than it saves. Tail calls must run in constant stack space, so a
  loop can run for any number of iterations.

So `fib(n) = fib(n - 1) + fib(n - 2)` is memoized, while
`fall(state) = if done then state else fall(next)` is a loop.

The remaining rules:

- The memo key is the tuple of argument values. A function argument is
  matched by identity: the same function value, not merely one that
  behaves the same. So a memoized function can take a lambda and pass it
  along to its recursive calls. A lambda that uses no local names from
  around it, such as `_ * 2`, is the same value every time it is
  evaluated, so passing it from a loop still hits the cache.
- A call whose arguments hold a generator anywhere inside them isn't
  cached, because a cache hit would hide the error from reading the
  generator a second time.
- A function defined in a `where` block gets **a separate cache for each
  call of the enclosing function**. Its key covers only its own parameters,
  and the cache is discarded when that call's results are no longer
  reachable. This is the same as defining the memo table inside a function
  in Haskell.
- Non-recursive helpers are not memoized, so they cost nothing extra.
- `@memo` forces memoization, and `@nomemo` turns it off (for example, for
  a recursive function whose arguments are expensive to hash and never
  repeat).
- A memo key is hashed on every call, so memoizing a function whose
  arguments hold a lot of data, such as a growing simulation state passed
  from call to call, can cost far more than it saves. When a function
  memoized automatically (not with `@memo`) is called with arguments
  holding more than **1,000 values**, counting every value inside
  collections, the interpreter prints a warning on stderr naming the
  function. It warns once per function for each run of a program, and the
  program carries on. `@nomemo` turns the memoization off; `@memo` keeps
  it and silences the warning.

Because values are immutable and functions are pure, memoization never
changes a program's result, only its speed.

### 8.2 Limits

- **Recursion depth.** Implementations must support a call depth of at
  least **1,000,000** Tinsel calls, since deep memoized recursion is common
  in AoC. In the Rust implementation, this means growing the stack
  (`stacker`) or using an explicit stack.
- **Size.** A value built from a number in the program may take at most
  **4 GiB**: the result of `a << n` or `a ** n` (2³⁵ bits), a `str` width
  (9.1) or a `repeat` result (9.2). Past that, or when the memory can't be
  allocated, it's an error rather than an attempt to allocate it.

### 8.3 Errors

A runtime error stops the program and reports a message, the source span,
and a Tinsel stack trace (the most recent calls first, limited to 20
entries). Repeated identical calls are shown once, with a count. Tail calls
reuse their caller's place in the trace, so a chain of them appears as the
call that started it, with the number of tail calls that followed.

---

## 9. Standard library

Functions are listed in method form, `x.f(args)`, which is the same as
`f(x, args)`.

### 9.1 Strings

| Function | Result |
|---|---|
| `s.lines` | Split on `\n`. A single trailing newline doesn't produce an empty final line. |
| `s.blocks` | Split on blank lines, with each block's trailing newline removed. |
| `s.words` | Split on runs of whitespace, ignoring whitespace at the ends. |
| `s.chars` | `list` of `char`. |
| `s.ints` | Every integer in `s`. A `-` counts as a sign only if it isn't directly after a letter or digit, so `"11-22"` gives `[11, 22]` and `"x=-3"` gives `[-3]`. |
| `s.split(sep)`, `xs.join(sep)` | |
| `s.trim`, `s.starts_with(t)`, `s.ends_with(t)` | |
| `s.replace(old, new)` | Every occurrence of `old` replaced by `new`, scanning left to right without overlaps. `old` and `new` can be a `str` or a `char`; an empty `old` is an error. |
| `int(s)`, `int(s, base)` | Parse an int (a sign and surrounding whitespace are allowed). With a base from 2 to 36, letters stand for 10 and up, in either case: `int("ff", 16) == 255`. `int(c)` parses a digit char, and `int(b)` converts a bool to 0 or 1. |
| `str(x)`, `str(n, base)`, `str(n, base, width)` | Display form. With a base from 2 to 36, formats an int using lowercase letters: `str(255, 16) == "ff"`, `str(-5, 2) == "-101"`. With a width, pads with zeros (after any sign) to at least that many digits: `str(5, 2, 8) == "00000101"`. A result past the size limit (8.2) is an error. |
| `ord(c)`, `chr(n)` | A `char`'s Unicode code point as an `int`, and the `char` for a code point: `ord('a') == 97`. `chr` of an invalid code point is an error. |
| `c.is_digit`, `c.is_lower`, `c.is_upper` | Whether a char is an ASCII digit, lowercase letter or uppercase letter. On a `str`, whether it is non-empty and every char is: `"2024".is_digit`, `not "".is_digit`. |

### 9.2 Collections

These work on lists, tuples, strings, ranges, sets, maps (which iterate
their keys) and generators, unless stated otherwise. Functions that return
sequences return a **list**, unless a `tuple`, `set` or `map` result makes
more sense.

`map`, `filter`, `enumerate`, `zip`, `drop` and `drop_while` read a
generator or an infinite range lazily: given one, they return a **generator**, computing each element only
when it is read. (`zip` does this when any argument is a generator, or when
none of its arguments ends.) So `iterate(x, f).enumerate.first(...)` and
`(1..).map(f).take(3)` work. Over finite collections they still return
lists.

| Function | Notes |
|---|---|
| `len` | `len` of an infinite range is an error. |
| `map(f)`, `filter(f)`, `count(f)`, `any(f)`, `all(f)` | `count`, `any` and `all` with no argument expect the elements to be bools. `count(x)` where `x` isn't a function counts elements equal to `x`. |
| `index(x)`, `index(f)` | Position of the first element equal to `x` (or the first substring match, for strings), or `none`. Given a function, the position of the first element for which it is true, as with `count(f)`. |
| `indices` | `0..<len` |
| `sum`, `product` | `sum` of an empty collection is `0`, `product` is `1`. Tuples add element-wise (5.3), so `[(1, 2), (3, 4)].sum == (4, 6)`. |
| `min`, `max` | Return `none` for an empty collection. `min(a, b, ...)` with two or more arguments compares those arguments instead. On a range they take constant time, so `(0..10 ** 12).max` is fine; `max` of a range that never ends is an error, as is `min` of one starting at `-inf`. |
| `min_by(f)`, `max_by(f)` | |
| `sort`, `sort_by(f)`, `reverse` | `sort(desc: true)` and `sort_by(f, desc: true)` sort from largest to smallest; `sort_by` keeps elements with equal keys in their original order either way. `reverse` of a `str` gives a `str`, and of a tuple gives a tuple. |
| `first`, `first(f)`, `last` | `none` if there's no such element. `first` works on ranges that never end; `last` of one is an error. |
| `take(n)`, `take_while(f)` | The first `n` elements, or the leading elements for which `f` is true, as a list. They read only as far as they need, so they work on infinite ranges and generators: `iterate(1, _ * 2).take(4) == [1, 2, 4, 8]`. |
| `drop(n)`, `drop_while(f)` | The elements after the first `n`, or after the leading elements for which `f` is true. Like `map` and `filter`, they give a list for a finite collection, and stay lazy on a generator or an infinite range: `iterate(1, _ * 2).drop(3).take(2) == [8, 16]`. |
| `r.bisect(f)` | For a range `r` only: the first int in `r` for which `f` is true, found by binary search, or `none` if there is none. `f` must be false up to some point and true from then on. It calls `f` O(log n) times, and on an infinite range `a..` it searches upwards by doubling: `(0..).bisect(n => n * n >= 50) == 8`. |
| `fold(init, f)` | `f(acc, x)` |
| `scan(init, f)` | Includes `init`, so the result has `len + 1` elements |
| `enumerate`, `zip(a, b, ...)` | Give tuples |
| `pairs` | Neighbouring pairs as tuples: `[1,2,3]` → `[(1,2),(2,3)]` |
| `windows(n)`, `chunks(n)` | Give lists. The last chunk may be shorter. |
| `uniq` | Remove duplicates, keeping the first occurrence |
| `repeat(n)` | For a `str` or list only: `n` copies joined together, `"ab".repeat(3) == "ababab"`. `n` must be 0 or more. Any number of copies of an empty `str` or list is empty. A result past the size limit (8.2) is an error. |
| `flatten` | One level |
| `group_by(f)`, `group_by(f, g)` | `map` from key `f(x)` to a list of the elements with that key, in their original order. With `g`, the list holds `g(x)` instead of `x`, so `edges.group_by(.0, .1)` maps each node to its neighbours. |
| `counts` | `map` from element to how often it occurs |
| `tally` | For `(key, n)` pairs: a `map` from key to the sum of its `n`s. `counts` is `tally` with every `n` equal to 1. |
| `subsets` | All subsets as lists: by size, then in index order |
| `combos(k)`, `perms` | Give **tuples** (fixed length, so they spread into functions), in index order |
| `transpose` | For a list of equal-length rows. Rows that are strings give string columns; lists and tuples give lists. |
| `list(x)`, `tuple(x)`, `set(x)` | Convert |
| `join`, `join(sep)` | Concatenate the elements' display forms. The default separator is `""`. |
| `keys`, `values`, `items` | Maps only; return lists in sorted key order |
| `m.set(k, v)`, `m.remove(k)`, `s.add(x)`, `s.remove(x)`, `xs.set(i, v)` | Return an updated copy. The original is unchanged. For a list, tuple or string, `xs.set(i, v)` replaces element `i` (a `char`, for a string); a negative `i` counts from the end, and an `i` outside it is an error. |
| `get(k)`, `get(k, default)` | Maps, lists, tuples, strings. Return `none`/`default` when missing. `m[k]` is an error when the key or index is missing. |
| `x in coll` | For maps, checks keys. For ranges, it's O(1). |
| `a.subset(b)` | For sets: whether every element of `a` is in `b`. (`a <= b` orders sets instead; see 4.1.) |
| `sets.union`, `sets.intersection` | For a collection whose members are all sets, or all rangesets: everything in any of them, or in all of them. The union of none is `set()`; the intersection of none is an error. |
| `points.bounds` | For tuples of the same length, the `(min, max)` of each coordinate: `[(1, 5), (3, 2)].bounds == ((1, 3), (2, 5))`. For other values, `(min, max)`. `none` for an empty collection. |
| indexing and slicing | `xs[i]` with negative `i` counts from the end. `xs[a..b]`, `xs[a..<b]`, `xs[a..]`, `xs[..<b]`. Slice bounds may also be negative, and are clamped to the sequence, so `[1, 2, 3][5..] == []`. |

### 9.3 Grids and positions

Positions are `(x, y)` tuples, with `x` increasing to the right and `y`
increasing **downwards**, matching how the input is laid out.

| Function | Notes |
|---|---|
| `s.grid`, `s.grid(f)` | A `Grid` built from `s.lines`, with each cell a `char` or `f(char)`. Every row must be the same length. |
| `g.width`, `g.height` | |
| `g[p]`, `g.get(p)` | `g[p]` is an error outside the grid; `g.get(p)` gives `none` |
| `p in g` | `true` when `p` is inside the grid |
| `g.positions` | All positions in row order (top row first, left to right) |
| `g.where(v)`, `g.where(f)` | `set` of positions whose cell equals `v`, or for which `f(cell)` is true |
| `g.find(v)` | First matching position in row order, or `none` |
| `g.nbrs4(p)`, `g.nbrs8(p)` | Neighbours that are inside the grid |
| `nbrs4(p)`, `nbrs8(p)` | Neighbours of any position, with no bounds. Order: `nbrs4` gives N, E, S, W; `nbrs8` goes row by row, skipping `p`. |
| `g.set(p, v)` | Updated copy; the original is unchanged. An update at a grid's last use costs O(1) (4.1). A position outside the grid is an error. |
| `g.rows` | List of rows. A char grid gives strings. |
| `g.cols` | List of columns, left to right, each read top to bottom. A char grid gives strings. |
| `g.transpose` | A new grid flipped across its main diagonal: the cell at `(x, y)` moves to `(y, x)`, and width and height swap. |
| `g.rot`, `g.rot(n)` | A new grid turned 90° clockwise, as `cw` turns directions, or `n` quarter turns, where a negative `n` turns anticlockwise and `inf` is an error. `g.rot(4) == g`. With `transpose`, this gives all eight orientations: mirroring left to right is `g.transpose.rot`. |
| `g.show` | Display as text, for `dbg` |

### 9.4 Iteration and cycles

| Function | Notes |
|---|---|
| `iterate(x, f)` | Lazy infinite generator: `x, f(x), f(f(x)), ...` |
| `iterate(x, f, times: n)` | `f` applied `n` times. Every state is hashed; if one repeats, the cycle is skipped arithmetically, so `times: 1_000_000_000` is fine when the states cycle. |
| `fixpoint(x, f)` | Apply `f` until the result equals its input |
| `xs.find_cycle(key: k)` | Reads the sequence until `k(item)` repeats. Returns `(start, period, prefix)`, where `prefix` holds items `0` through `start + period`, inclusive. Item `start + period` has the same key as item `start`. Use this when the state as a whole never repeats (for example, a height that keeps growing) but a normalised key does. You then extrapolate yourself. `key` is optional and defaults to the item itself. |

Only library functions take named arguments, such as `times:` and `key:`.

### 9.5 Numbers

`abs`, `sign`, `gcd`, `lcm` and `divmod(a, b)` work as usual (with `min`
and `max` from 9.2, and `int` from 9.1), and:

| Function | Result |
|---|---|
| `isqrt(n)` | The largest `r` with `r * r <= n`, for any int `n` of 0 or more: `isqrt(17) == 4`. |
| `n.bits` | How many binary digits an int of 0 or more has: `255.bits == 8`, `0.bits == 0`. |
| `n.digits`, `digits(n, base)` | The digits of an int of 0 or more, most significant first: `1234.digits == [1, 2, 3, 4]`, `digits(5, 2) == [1, 0, 1]`, `0.digits == [0]`. A negative int is an error. The base is from 2 to 36. |
| `modinv(a, m)` | `x` in `0..<m` with `a * x % m == 1`, or `none` if there is none |
| `modpow(b, e, m)` | `b ** e % m`, without computing `b ** e`. `e` must be 0 or more, and `m` positive. |
| `crt(pairs)` | For `(remainder, modulus)` pairs: `(x, m)`, where `m` is the lcm of the moduli and `x` in `0..<m` satisfies every pair. The moduli don't have to be coprime, but must be positive. `none` if no `x` exists. |
| `shoelace(points)` | The area of the polygon through the `(x, y)` points, closing back to the first. An error if the area isn't a whole number (Tinsel has no fractions, Q3). With Pick's theorem, the number of grid points strictly inside is `area - boundary / 2 + 1`. |
| `solve(rows, rhs)` | The integer solution `xs` of the linear system `rows · xs == rhs`, found with exact arithmetic. `rows` is a list of equal-length rows of ints. `none` if there isn't exactly one solution, or if it isn't all integers. |

### 9.6 Debugging

`dbg(x)` prints `x` to stderr and returns it unchanged. `dbg(label, x)`
prints a label first.

### 9.7 Directions and search

Directions use the same `(x, y)` positions as grids (9.3), with `y`
increasing downwards.

| Name | Notes |
|---|---|
| `N`, `E`, `S`, `W` | `(0, -1)`, `(1, 0)`, `(0, 1)`, `(-1, 0)` |
| `DIRS4` | `[N, E, S, W]`, in the same order as `nbrs4` |
| `DIRS8` | The 8 unit steps, in the same order as `nbrs8` |
| `cw(d)`, `ccw(d)` | `d` turned 90° clockwise or anticlockwise, so `cw(N) == E`. Works on any `(x, y)`, not only unit steps. |
| `manhattan(a, b)` | The sum of `abs(a_i - b_i)`, for two tuples of the same length |

As with any library name, a program's own definition (such as `N = 100`)
hides these.

In the search functions, `next(state)` gives the states one step away from
`state`, as any iterable. States can be any hashable values. `goal:` takes a
predicate, which must return a bool, or a plain value: `goal: end` means
`goal: _ == end`.

| Function | Result |
|---|---|
| `bfs(start, next)` | A map from every state reachable from `start` to its number of steps from `start` |
| `bfs(start, next, goal: g)` | `(dist, path)` for the nearest state matching `g`: the number of steps, and a shortest path as a list of states from `start` to that state, inclusive. `none` if no reachable state matches. |
| `dijkstra(start, next)`, `dijkstra(start, next, goal: g)` | The same, where `next(state)` gives `(state, cost)` pairs and distances are total costs. Costs are ints of 0 or more; a negative cost is an error. |
| `bfs_from(starts, next)`, `dijkstra_from(starts, next)` | Like `bfs` and `dijkstra` (with or without `goal:`), but searching from every state in `starts` at once, each at distance 0. With `goal:`, the path begins at whichever start it came from. |
| `components(nodes, next)` | The connected components of `nodes`, as a list of sets. `next` is followed only between members of `nodes`, and should be symmetric. Each set holds the nodes reachable from its smallest member, and the list is ordered by those smallest members. |

When there are several shortest paths, `path` is the one found first:
neighbours are explored in the order that `next` gives them, and starts in
the order that `starts` gives them. `starts` can be any iterable, and
repeats don't matter. With no starts, the result is an empty map, or `none`
with `goal:`.

With `all_paths: true`, which needs `goal:`, these four functions return
`(dist, states)` instead, where `states` is the set of every state on at
least one shortest path from a start to a state matching the goal. It
includes the starts and every goal state reached at that shortest distance.
The result is still `none` if no goal is reachable. (There can be
exponentially many shortest paths, so they aren't listed one by one.)

### 9.8 Intervals

A `rangeset` is a set of ints stored as sorted, disjoint ranges, merged so
that no two touch. It stays small however many ints it covers.

| Operation | Result |
|---|---|
| `rangeset(xs)`, `rangeset()` | A rangeset of the ranges and ints in `xs`: `rangeset([1..3, 2..5, 7]) == rangeset([1..5, 7])`. Empty ranges are ignored; infinite ranges are an error. |
| `a \| b`, `a & b`, `a - b` | Union, intersection and difference |
| `x in r` | Whether `r` contains `x`, in O(log n) time |
| `r.len` | How many ints `r` contains |
| `r.ranges` | The disjoint ranges, in order |
| `min`, `max`, `first`, `last`, iteration | As for a set of ints. Iteration is lazy, so `first` is fine on a huge rangeset. |

---

## 10. Display forms

`str(x)`, string interpolation, `join` and the `part1:` and `part2:` lines
of `tinsel run` show a value's **display form**:

| Value | Display |
|---|---|
| `int`, `bool`, `none` | As written in source: `-12`, `inf`, `true`, `none` |
| `str`, `char` | The text itself, without quotes |
| `tuple`, `list` | `(1, 2)`, `(1,)` for one element, `[1, 2]` |
| `set`, `map` | `{1, 2}` and `{"a": 1}`, in sorted order; empty, `set()` and `{}` |
| `range` | As written: `1..5`, `0..<5`, `1..`. `indices` gives a `0..<n` range. Ranges with the same elements are equal however they were written, so `0..<5 == 0..4` and `1..inf == 1..`. |
| `rangeset` | `rangeset([1..5, 7..7])` |
| `grid` | `<grid 3x2>` (width x height); `g.show` gives its cells |
| `function`, `generator` | `<function f>`, `<function <lambda>>`, `<generator>` |

Inside a collection, strings and chars are shown as literals, quoted and
escaped (3): `str(["a", 'b']) == "[\"a\", 'b']"`. `dbg` and the values that a
failed check reports use the same forms, except that a string or char on
its own is quoted too.

---

## 11. Design decisions

Settled decisions, with their reasoning, and the one question still open
(Q3).

| # | Question | Decision |
|---|---|---|
| Q1 | ~~Should untyped string captures become ints automatically?~~ | **Resolved: no.** Untyped captures are always `str`, and other types are written out (`{n:int}`), so parsing is predictable from the pattern alone. Automatic conversion meant a field like `"5"` became an int on some lines and stayed a string on others. |
| Q2 | ~~**Simulation state.**~~ | **Resolved: no mutable state or `loop` form for now.** Five state-heavy puzzles (2025 days 4, 7 and 8, 2023 day 14, 2022 day 17) were ported using only `fixpoint`, `scan`/`fold`, `iterate(times:)`, `find_cycle` and tail-recursive loops. The ports led to two rule changes: tail-recursive functions are loops and are not memoized (8.1), and there's now a `with` override for example parameters (2.3). Union-find (2025 day 8) works as a fold over two persistent maps, which is adequate though not elegant. If a mutable-style form is still wanted later, it should be sugar for a fold. |
| Q3 | Floats or exact rationals? | **Open: deferred** until a puzzle needs it. The likely direction is a Scheme-style tower of exact numbers (int → rational) and inexact ones (float). So that this stays an addition rather than a breaking change, `int / int` remains floor division permanently. Exact results would come from a rational operand (`rat(7) / 2 == 7/2`) or a new operator, never from changing `/` on ints. |
| Q4 | ~~Should `bool` count as 0/1 in arithmetic?~~ | **Resolved: no** (4). Bools are not numbers, and nothing else acts as a bool. This follows the same reasoning as Q8: no silent coercion between types. To count, use `xs.count(pred)` or `int(b)`. |
| Q5 | ~~Map and set iteration order: sorted, or insertion order?~~ | **Resolved: sorted** (see 4.1). Insertion order would match Kotlin and Python, but Rust has no persistent insertion-ordered map. It would also need order-independent equality and hashing, plus a canonical order for sorting sets. For the rare puzzle where insertion order matters, use a list of pairs. Maps and sets are persistent sorted trees (`imbl::OrdMap` and `OrdSet`), so equality, hashing and ordering don't depend on how they were built, at the cost of O(log n) lookups. If that becomes a bottleneck (very large BFS visited sets, say), add a separate hash-based set type rather than changing what `{}` means. |
| Q6 | ~~`>>` for both shift and composition.~~ | **Resolved: there is no composition operator**, and `>>` is only a shift. Method-call syntax gives pipelines (`x.f.g`), and leading-dot chains give composition (`.ints.tuple`). `|>` was considered, but in F#, OCaml and Elixir it means *pipe*, not composition. If it's ever added, it should be a pipe. |
| Q7 | ~~Modules or a shared prelude between days.~~ | **Resolved: textual `include` only** (2.8). 2019 uses its Intcode computer on twelve days, and copying it into each would leave twelve versions to keep in step. `include` merges another file's definitions into the program, with no namespaces, export lists or module values: name clashes are compile errors, as duplicates always are. A real module system can be revisited if clashes become a problem. |
| Q8 | ~~Should comparing values of different types be an error?~~ | **Resolved: yes** (4.1). Ordering values of different types is a runtime error. Equality never fails; values of different types are just unequal, and that now includes `true == 1`. Type patterns (`n: int`, 6) let a `match` arm check the type before comparing. The reasoning: JavaScript shows that silent coercion between types causes real correctness bugs. |
