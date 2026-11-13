# Tinsel library reference

Every built-in function, with examples. [`SPEC.md`](../SPEC.md) section 9
is the full definition; this is the quick reference.

Functions are shown in method form, `x.f(args)`, which is the same as
`f(x, args)`. A function with no other arguments can drop the brackets:
`s.lines` is `lines(s)`. `input`, the puzzle input as a `str`, is in SPEC
2.2.

Every line of every example below is a check that must be `true`, run on
all three engines by `cargo test --test docs`. A line ending in `# error`
must raise an error instead.

## Strings

### s.lines → list
Splits on `\n`. A single trailing newline doesn't give an empty last line.

```tinsel
"a\nb\n".lines == ["a", "b"]
"a\n\nb".lines == ["a", "", "b"]
"".lines == []
```

### s.blocks → list
Splits on blank lines, as between the sections of many puzzle inputs. Each
block's trailing newline is removed.

```tinsel
"1\n2\n\n3\n".blocks == ["1\n2", "3"]
"1\n2\n\n3\n".blocks.map(.ints) == [[1, 2], [3]]
```

### s.words → list
Splits on runs of whitespace, ignoring whitespace at the ends.

```tinsel
"  move 3   from 1 ".words == ["move", "3", "from", "1"]
```

### s.chars → list
The string's chars, in order.

```tinsel
"abc".chars == ['a', 'b', 'c']
```

### s.ints → list
Every integer in `s`, in order. A `-` is a sign only when it isn't directly
after a letter or digit, so ranges such as `11-22` give positive numbers.

```tinsel
"p=<3,-4> v=<-1,2>".ints == [3, -4, -1, 2]
"11-22".ints == [11, 22]
"x=-3".ints == [-3]
"no numbers".ints == []
```

### s.split(sep) → list
Splits on every occurrence of `sep`, a `str` or `char`. An empty `sep` is
an error.

```tinsel
"a,b,,c".split(",") == ["a", "b", "", "c"]
"1-3".split('-') == ["1", "3"]
"abc".split("") # error
```

### xs.join(sep) → str
The items' display forms, separated by `sep`. Without `sep`, nothing goes
between them.

```tinsel
["a", "b", "c"].join(", ") == "a, b, c"
[1, 2, 3].join("-") == "1-2-3"
['a', 'b'].join == "ab"
```

### s.trim → str
`s` without whitespace at either end.

```tinsel
"  hi there \n".trim == "hi there"
```

### s.starts_with(t) → bool
Whether `s` begins with `t`, a `str` or `char`.

```tinsel
"Game 12".starts_with("Game")
"#..#".starts_with('#')
not "abc".starts_with("b")
```

### s.ends_with(t) → bool
Whether `s` ends with `t`, a `str` or `char`.

```tinsel
"file.tin".ends_with(".tin")
not "abc".ends_with('b')
```

### s.replace(old, new) → str
Every occurrence of `old` replaced by `new`, scanning from the left without
overlaps. `old` and `new` can each be a `str` or `char`. An empty `old` is
an error.

```tinsel
"a.b.c".replace(".", "") == "abc"
"aaa".replace("aa", "b") == "ba"
"#.#".replace('#', '1') == "1.1"
"abc".replace("", "x") # error
```

### int(s), int(s, base) → int
Parses an int. A sign and whitespace around it are allowed. With a base
from 2 to 36, letters stand for 10 and up, in either case. Also turns a
digit char into its value, and a bool into 0 or 1.

```tinsel
int(" -42 ") == -42
int("ff", 16) == 255
int("FF", 16) == 255
int("1011", 2) == 11
int('7') == 7
int(true) == 1
int("12x") # error
```

### str(x), str(n, base), str(n, base, width) → str
The display form of any value. With a base from 2 to 36, formats an int
with lowercase letters. With a width, pads with zeros, after any sign, to
at least that many digits.

```tinsel
str(42) == "42"
str((1, 2)) == "(1, 2)"
str(255, 16) == "ff"
str(-5, 2) == "-101"
str(5, 2, 8) == "00000101"
```

### ord(c) → int
A char's Unicode code point.

```tinsel
ord('a') == 97
ord('z') - ord('a') == 25
```

### chr(n) → char
The char for a Unicode code point. An invalid code point is an error.

```tinsel
chr(97) == 'a'
chr(ord('a') + 2) == 'c'
chr(-1) # error
```

### c.is_digit → bool
Whether a char is an ASCII digit. On a `str`, whether it is non-empty and
every char is a digit.

```tinsel
'7'.is_digit
not 'x'.is_digit
"2024".is_digit
not "".is_digit
"a1b2".chars.filter(.is_digit) == ['1', '2']
```

### c.is_lower → bool
Whether a char is an ASCII lowercase letter. On a `str`, whether it is
non-empty and every char is.

```tinsel
'a'.is_lower
not 'A'.is_lower
"start".is_lower
not "Start".is_lower
```

### c.is_upper → bool
Whether a char is an ASCII uppercase letter. On a `str`, whether it is
non-empty and every char is.

```tinsel
'A'.is_upper
"XMAS".is_upper
not "Xmas".is_upper
```

## Collections

These work on lists, tuples, strings, ranges, sets, maps (which give their
keys) and generators, unless an entry says otherwise. A sequence comes back
as a list, unless the entry says otherwise.

### xs.len → int
How many elements `xs` has. An infinite range is an error.

```tinsel
[1, 2, 3].len == 3
"hello".len == 5
{"a": 1, "b": 2}.len == 2
(1..10).len == 10
(1..).len # error
```

### xs.map(f) → list
`f` applied to each element. On a generator or an infinite range, the
result is a lazy generator.

```tinsel
[1, 2, 3].map(_ * 10) == [10, 20, 30]
["1", "2"].map(int) == [1, 2]
(1..).map(_ * 2).take(3) == [2, 4, 6]
```

### xs.filter(f) → list
The elements for which `f` is true. On a generator or an infinite range,
the result is a lazy generator.

```tinsel
[1, 2, 3, 4].filter(_ % 2 == 0) == [2, 4]
(1..).filter(_ % 7 == 0).take(2) == [7, 14]
```

### xs.count(f), xs.count(x), xs.count → int
How many elements `f` is true for, or how many equal `x`. With no
argument, the elements must be bools, and the true ones are counted.

```tinsel
[1, 5, 7].count(_ > 4) == 2
"banana".count('a') == 3
[true, false, true].count == 2
```

### xs.any(f), xs.any → bool
Whether `f` is true for some element. With no argument, the elements must
be bools.

```tinsel
[1, 5, 7].any(_ > 6)
not [].any(_ > 6)
[false, true].any
```

### xs.all(f), xs.all → bool
Whether `f` is true for every element. With no argument, the elements must
be bools.

```tinsel
[2, 4].all(_ % 2 == 0)
[].all(_ > 6)
not [true, false].all
```

### xs.index(x), xs.index(f) → int
The position of the first element equal to `x`, or of the first for which
`f` is true. On a string, `x` can be a substring. `none` if there isn't
one.

```tinsel
[5, 6, 7].index(6) == 1
[1, 4, 9].index(_ > 3) == 1
"hello".index("ll") == 2
[1, 2].index(9) == none
```

### xs.indices → range
The valid positions of `xs`, `0..<xs.len`.

```tinsel
"abc".indices == 0..<3
list([5, 6].indices) == [0, 1]
```

### xs.sum → int
The total. An empty collection gives `0`. Tuples add element by element.

```tinsel
[1, 2, 3].sum == 6
[].sum == 0
[(1, 2), (3, 4)].sum == (4, 6)
```

### xs.product → int
All the elements multiplied together. An empty collection gives `1`.

```tinsel
[2, 3, 4].product == 24
[].product == 1
```

### xs.min, min(a, b, ...) → value
The smallest element, or `none` for an empty collection. With two or more
arguments, the smallest argument. On a range it takes constant time.

```tinsel
[3, 1, 2].min == 1
min(3, 1, 2) == 1
[].min == none
(5..).min == 5
```

### xs.max, max(a, b, ...) → value
The largest element, or `none` for an empty collection. With two or more
arguments, the largest argument. On a range it takes constant time; a
range that never ends is an error.

```tinsel
["b", "c", "a"].max == "c"
max(3, 1, 2) == 3
(0..10 ** 12).max == 10 ** 12
(1..).max # error
```

### xs.min_by(f) → value
The element with the smallest `f(x)`, or `none` for an empty collection.

```tinsel
["bb", "a", "ccc"].min_by(.len) == "a"
[(1, 9), (2, 3)].min_by(.1) == (2, 3)
```

### xs.max_by(f) → value
The element with the largest `f(x)`, or `none` for an empty collection.

```tinsel
["bb", "a", "ccc"].max_by(.len) == "ccc"
```

### xs.sort, xs.sort(desc: true) → list
The elements in increasing order, or decreasing with `desc: true`.

```tinsel
[3, 1, 2].sort == [1, 2, 3]
[3, 1, 2].sort(desc: true) == [3, 2, 1]
"cab".sort == ['a', 'b', 'c']
```

### xs.sort_by(f), xs.sort_by(f, desc: true) → list
The elements ordered by `f(x)`. Elements with equal keys keep their
order, either way.

```tinsel
["bb", "a", "cc"].sort_by(.len) == ["a", "bb", "cc"]
["bb", "a", "cc"].sort_by(.len, desc: true) == ["bb", "cc", "a"]
```

### xs.reverse → list
The elements in reverse order. A string gives a string, and a tuple a
tuple.

```tinsel
[1, 2, 3].reverse == [3, 2, 1]
"abc".reverse == "cba"
(1, 2).reverse == (2, 1)
```

### xs.first, xs.first(f) → value
The first element, or the first for which `f` is true. `none` if there
isn't one. Works on ranges and generators that never end.

```tinsel
[4, 5, 6].first == 4
[4, 5, 6].first(_ > 4) == 5
[].first == none
(1..).first(n => n * n > 50) == 8
```

### xs.last → value
The last element, or `none` for an empty collection. A range that never
ends is an error.

```tinsel
[4, 5, 6].last == 6
"abc".last == 'c'
[].last == none
(1..).last # error
```

### r.bisect(f) → int
For a range only: the first int in `r` for which `f` is true, found by
binary search, or `none`. `f` must be false up to some point and true from
then on. On a range that never ends, it searches upwards by doubling.

```tinsel
(0..100).bisect(n => n * n >= 50) == 8
(0..).bisect(n => n * n >= 50) == 8
(0..5).bisect(_ > 10) == none
```

### xs.take(n) → list
The first `n` elements. Reads only as far as it needs, so it works on
ranges and generators that never end.

```tinsel
[1, 2, 3].take(2) == [1, 2]
[1, 2].take(5) == [1, 2]
iterate(1, _ * 2).take(4) == [1, 2, 4, 8]
```

### xs.take_while(f) → list
The leading elements for which `f` is true.

```tinsel
[1, 2, 5, 1].take_while(_ < 3) == [1, 2]
(1..).take_while(_ < 4) == [1, 2, 3]
```

### xs.drop(n) → list
The elements after the first `n`. On a generator or an infinite range, the
result is a lazy generator.

```tinsel
[1, 2, 3, 4].drop(1) == [2, 3, 4]
iterate(1, _ * 2).drop(3).take(2) == [8, 16]
```

### xs.drop_while(f) → list
The elements from the first for which `f` is false. On a generator or an
infinite range, the result is a lazy generator.

```tinsel
[1, 2, 5, 1].drop_while(_ < 3) == [5, 1]
"   x ".drop_while(_ == ' ') == ['x', ' ']
```

### xs.fold(init, f) → value
Combines the elements from the left, starting from `init`: `f(acc, x)`
for each element in turn.

```tinsel
[1, 2, 3].fold(0, (acc, x) => acc + x) == 6
"abc".fold("", (acc, c) => "{c}{acc}") == "cba"
```

### xs.scan(init, f) → list
Every intermediate value of `fold`, starting with `init`, so the result
has one more element than `xs`.

```tinsel
[1, 2, 3].scan(0, (acc, x) => acc + x) == [0, 1, 3, 6]
```

### xs.enumerate → list
`(index, element)` tuples. On a generator or an infinite range, the result
is a lazy generator.

```tinsel
"ab".enumerate == [(0, 'a'), (1, 'b')]
["x", "y"].enumerate.map((i, s) => "{i}{s}") == ["0x", "1y"]
```

### zip(a, b, ...) → list
Tuples of the elements at the same position in each argument. It stops at
the end of the shortest one, and is a lazy generator when an argument is a
generator, or when none of them ends.

```tinsel
zip([1, 2, 3], "ab") == [(1, 'a'), (2, 'b')]
zip([1, 2], [3, 4], [5, 6]) == [(1, 3, 5), (2, 4, 6)]
zip(1.., "ab") == [(1, 'a'), (2, 'b')]
```

### xs.pairs → list
Each element with the next, as tuples.

```tinsel
[1, 2, 3].pairs == [(1, 2), (2, 3)]
[1, 4, 9].pairs.map((a, b) => b - a) == [3, 5]
```

### xs.windows(n) → list
Every run of `n` neighbouring elements, as lists.

```tinsel
[1, 2, 3, 4].windows(3) == [[1, 2, 3], [2, 3, 4]]
```

### xs.chunks(n) → list
The elements split into lists of `n`. The last may be shorter.

```tinsel
[1, 2, 3, 4, 5].chunks(2) == [[1, 2], [3, 4], [5]]
```

### xs.uniq → list
The elements without repeats, keeping each one's first occurrence.

```tinsel
[3, 1, 3, 2, 1].uniq == [3, 1, 2]
```

### xs.repeat(n) → str or list
For a string or list only: `n` copies joined together. `n` must be 0 or
more.

```tinsel
"ab".repeat(3) == "ababab"
[0].repeat(3) == [0, 0, 0]
"ab".repeat(0) == ""
"ab".repeat(-1) # error
```

### xss.flatten → list
Joins a collection of collections, one level deep.

```tinsel
[[1, 2], [3], []].flatten == [1, 2, 3]
[[1, [2]], [3]].flatten == [1, [2], 3]
```

### xs.group_by(f), xs.group_by(f, g) → map
A map from each key `f(x)` to the elements with that key, in their
original order. With `g`, the lists hold `g(x)` in place of `x`.

```tinsel
[1, 2, 3, 4].group_by(_ % 2) == {0: [2, 4], 1: [1, 3]}
[("a", "b"), ("a", "c"), ("b", "c")].group_by(.0, .1) == {"a": ["b", "c"], "b": ["c"]}
```

### xs.counts → map
A map from each element to how often it occurs.

```tinsel
"abca".counts == {'a': 2, 'b': 1, 'c': 1}
[3, 3, 3].counts == {3: 3}
```

### pairs.tally → map
For `(key, n)` pairs: a map from each key to the sum of its `n`s.

```tinsel
[("a", 2), ("b", 1), ("a", 3)].tally == {"a": 5, "b": 1}
```

### xs.subsets → list
Every subset, as lists: by size, and in index order within a size.

```tinsel
[1, 2, 3].subsets == [[], [1], [2], [3], [1, 2], [1, 3], [2, 3], [1, 2, 3]]
```

### xs.combos(k) → list
Every choice of `k` elements, as tuples, in index order.

```tinsel
[1, 2, 3].combos(2) == [(1, 2), (1, 3), (2, 3)]
[1, 2, 3, 4].combos(3).len == 4
```

### xs.perms → list
Every ordering of the elements, as tuples, in index order.

```tinsel
[1, 2, 3].perms == [(1, 2, 3), (1, 3, 2), (2, 1, 3), (2, 3, 1), (3, 1, 2), (3, 2, 1)]
```

### rows.transpose → list
For a list of equal-length rows: the columns. String rows give string
columns, and lists or tuples give lists. On a grid, see `g.transpose`.

```tinsel
[[1, 2, 3], [4, 5, 6]].transpose == [[1, 4], [2, 5], [3, 6]]
["ab", "cd"].transpose == ["ac", "bd"]
```

### list(xs) → list
The elements of any collection, range or generator, as a list.

```tinsel
list(1..3) == [1, 2, 3]
list("ab") == ['a', 'b']
list({2, 1}) == [1, 2]
```

### tuple(xs) → tuple
The elements as a tuple.

```tinsel
tuple([1, 2]) == (1, 2)
tuple("ab") == ('a', 'b')
```

### set(xs), set() → set
The distinct elements as a set, or an empty set.

```tinsel
set([1, 1, 2]) == {1, 2}
set().len == 0
set("hello").len == 4
```

### m.set(k, v), xs.set(i, v) → map or list
An updated copy: a map with `k` set to `v`, or a list, tuple or string
with element `i` replaced (a char, for a string). A negative `i` counts
from the end, and an `i` outside the sequence is an error. The original is
unchanged. On a grid, see `g.set`.

```tinsel
{"a": 1}.set("b", 2) == {"a": 1, "b": 2}
{"a": 1}.set("a", 5) == {"a": 5}
[1, 2, 3].set(0, 9) == [9, 2, 3]
"abc".set(-1, 'x') == "abx"
[1, 2].set(5, 0) # error
```

### m.keys → list
A map's keys, in sorted order.

```tinsel
{"b": 1, "a": 2}.keys == ["a", "b"]
```

### m.values → list
A map's values, in the sorted order of their keys.

```tinsel
{"b": 1, "a": 2}.values == [2, 1]
```

### m.items → list
A map's `(key, value)` pairs, in sorted key order.

```tinsel
{"b": 1, "a": 2}.items == [("a", 2), ("b", 1)]
```

### s.add(x) → set
A copy of the set with `x` added.

```tinsel
{1, 2}.add(3) == {1, 2, 3}
{1, 2}.add(1) == {1, 2}
```

### s.remove(x), m.remove(k) → set or map
A copy of the set without `x`, or of the map without the key `k`. Removing
something that isn't there gives an unchanged copy.

```tinsel
{1, 2}.remove(1) == {2}
{"a": 1, "b": 2}.remove("a") == {"b": 2}
{1, 2}.remove(5) == {1, 2}
```

### a.subset(b) → bool
For sets: whether every element of `a` is in `b`. (`a <= b` orders sets
instead.)

```tinsel
{1, 2}.subset({1, 2, 3})
not {1, 4}.subset({1, 2, 3})
```

### sets.union → set
For a collection of sets, or of rangesets: everything in any of them. The
union of none is `set()`.

```tinsel
[{1, 2}, {2, 3}].union == {1, 2, 3}
[].union == set()
```

### sets.intersection → set
For a collection of sets, or of rangesets: everything in all of them. The
intersection of none is an error.

```tinsel
[{1, 2}, {2, 3}].intersection == {2}
[].intersection # error
```

### points.bounds → tuple
For tuples of the same length: the `(min, max)` of each coordinate. For
other values, `(min, max)`. `none` for an empty collection.

```tinsel
[(1, 5), (3, 2)].bounds == ((1, 3), (2, 5))
[4, 1, 7].bounds == (1, 7)
[].bounds == none
```

### xs.get(k), xs.get(k, default) → value
For maps, lists, tuples and strings: the value at key or index `k`, or
`none` (or `default`) when it's missing. `xs[k]` is an error when it's
missing. On a grid, see `g.get`.

```tinsel
{"a": 1}.get("a") == 1
{"a": 1}.get("b") == none
{"a": 1}.get("b", 0) == 0
[1, 2].get(5) == none
[1, 2].get(-1) == 2
```

## Grids and positions

Positions are `(x, y)` tuples, with `x` increasing to the right and `y`
increasing **downwards**, as the input is laid out. `g[p]` reads a cell,
and `p in g` says whether `p` is inside the grid.

### s.grid, s.grid(f) → grid
A grid built from the lines of `s`, with each cell a char, or `f(char)`.
Every row must be the same length.

```tinsel
"#.\n.#".grid[(1, 1)] == '#'
"12\n34".grid(int)[(1, 0)] == 2
(2, 0) in "abc\ndef".grid
(3, 0) not in "abc\ndef".grid
"ab\nc".grid # error
```

### g.width → int
The number of columns.

```tinsel
"abc\ndef".grid.width == 3
```

### g.height → int
The number of rows.

```tinsel
"abc\ndef".grid.height == 2
```

### g.get(p) → value
The cell at `p`, or `none` outside the grid. `g[p]` is an error there.

```tinsel
"ab\ncd".grid.get((1, 0)) == 'b'
"ab\ncd".grid.get((5, 5)) == none
"ab\ncd".grid[(5, 5)] # error
```

### g.positions → list
Every position, in row order: the top row first, left to right.

```tinsel
"ab\ncd".grid.positions == [(0, 0), (1, 0), (0, 1), (1, 1)]
```

### g.where(v), g.where(f) → set
The positions whose cell equals `v`, or for which `f(cell)` is true.

```tinsel
"#.\n.#".grid.where('#') == {(0, 0), (1, 1)}
"19\n55".grid(int).where(_ > 4) == {(1, 0), (0, 1), (1, 1)}
```

### g.find(v) → tuple
The first position, in row order, whose cell equals `v`, or `none`.

```tinsel
"..\n.S".grid.find('S') == (1, 1)
"..\n..".grid.find('S') == none
```

### g.nbrs4(p), nbrs4(p) → list
The four neighbours of `p`, in the order N, E, S, W. On a grid, only those
inside it.

```tinsel
nbrs4((0, 0)) == [(0, -1), (1, 0), (0, 1), (-1, 0)]
"...\n...".grid.nbrs4((0, 0)) == [(1, 0), (0, 1)]
```

### g.nbrs8(p), nbrs8(p) → list
The eight neighbours of `p`, row by row. On a grid, only those inside it.

```tinsel
nbrs8((0, 0)).len == 8
nbrs8((0, 0))[0] == (-1, -1)
"...\n...".grid.nbrs8((0, 0)) == [(1, 0), (0, 1), (1, 1)]
```

### g.set(p, v) → grid
An updated copy of the grid with the cell at `p` set to `v`. The original
is unchanged. A position outside the grid is an error.

```tinsel
"ab".grid.set((0, 0), 'x').rows == ["xb"]
"ab".grid.set((5, 0), 'x') # error
```

### g.rows → list
The rows, top to bottom. A grid of chars gives strings.

```tinsel
"ab\ncd".grid.rows == ["ab", "cd"]
"12\n34".grid(int).rows == [[1, 2], [3, 4]]
```

### g.cols → list
The columns, left to right, each read top to bottom. A grid of chars gives
strings.

```tinsel
"ab\ncd".grid.cols == ["ac", "bd"]
```

### g.transpose → grid
A new grid flipped across its main diagonal: the cell at `(x, y)` moves to
`(y, x)`.

```tinsel
"ab\ncd".grid.transpose.rows == ["ac", "bd"]
"abc".grid.transpose.height == 3
```

### g.rot, g.rot(n) → grid
A new grid turned 90° clockwise, or `n` quarter turns, where a negative
`n` turns anticlockwise. Mirroring left to right is `g.transpose.rot`.

```tinsel
"ab\ncd".grid.rot.rows == ["ca", "db"]
"ab\ncd".grid.rot(-1).rows == ["bd", "ac"]
"ab\ncd".grid.rot(4) == "ab\ncd".grid
"ab\ncd".grid.transpose.rot.rows == ["ba", "dc"]
```

### g.show → str
The grid's cells as text, each row ending in a newline, for `dbg`.

```tinsel
"#.\n.#".grid.show == "#.\n.#\n"
```

## Iteration and cycles

### iterate(x, f) → generator
The lazy, never-ending sequence `x`, `f(x)`, `f(f(x))`, and so on.

```tinsel
iterate(1, _ * 2).take(4) == [1, 2, 4, 8]
iterate(1, _ * 3).first(_ > 100) == 243
```

### iterate(x, f, times: n) → value
`f` applied `n` times. Every state is remembered, and if one repeats, the
cycle is skipped arithmetically, so a huge `n` is fine when the states
cycle.

```tinsel
iterate(1, _ * 2, times: 10) == 1024
iterate(0, x => (x + 1) % 3, times: 1_000_000_000) == 1
```

### fixpoint(x, f) → value
Applies `f` until the result equals its input.

```tinsel
fixpoint(100, n => n / 2) == 0
fixpoint({1}, s => s | set(x * 2 for x in s if x < 10)) == {1, 2, 4, 8, 16}
```

### xs.find_cycle, xs.find_cycle(key: k) → tuple
Reads the sequence until `k(item)` repeats, and gives `(start, period,
prefix)`, where `prefix` holds items `0` to `start + period`. Item `start +
period` has the same key as item `start`. `key` defaults to the item
itself.

```tinsel
[1, 2, 3, 2, 3, 2].find_cycle == (1, 2, [1, 2, 3, 2])
iterate(0, _ + 1).find_cycle(key: _ % 3) == (0, 3, [0, 1, 2, 3])
```

## Numbers

### abs(n) → int
The absolute value.

```tinsel
abs(-3) == 3
abs(-inf) == inf
```

### sign(n) → int
`-1`, `0` or `1`.

```tinsel
sign(-5) == -1
sign(0) == 0
sign(7) == 1
```

### gcd(a, b) → int
The greatest common divisor.

```tinsel
gcd(12, 18) == 6
gcd(0, 5) == 5
```

### lcm(a, b) → int
The least common multiple.

```tinsel
lcm(4, 6) == 12
[2, 3, 4].fold(1, lcm) == 12
```

### divmod(a, b) → tuple
`(a / b, a % b)`. Division rounds towards negative infinity, and the
remainder takes the sign of `b`.

```tinsel
divmod(7, 2) == (3, 1)
divmod(-7, 2) == (-4, 1)
divmod(1, 0) # error
```

### isqrt(n) → int
The largest `r` with `r * r <= n`, for an int of 0 or more.

```tinsel
isqrt(17) == 4
isqrt(10 ** 40) == 10 ** 20
isqrt(-1) # error
```

### n.bits → int
How many binary digits an int of 0 or more has.

```tinsel
255.bits == 8
256.bits == 9
0.bits == 0
```

### n.digits, digits(n, base) → list
The digits of an int of 0 or more, most significant first. The base is
from 2 to 36. A negative int is an error.

```tinsel
1234.digits == [1, 2, 3, 4]
digits(5, 2) == [1, 0, 1]
0.digits == [0]
(-5).digits # error
```

### modinv(a, m) → int
The `x` in `0..<m` with `a * x % m == 1`, or `none` if there isn't one.

```tinsel
modinv(3, 7) == 5
modinv(2, 4) == none
```

### modpow(b, e, m) → int
`b ** e % m`, without computing `b ** e`. `e` must be 0 or more, and `m`
positive.

```tinsel
modpow(2, 10, 1000) == 24
modpow(3, 10 ** 18, 7) == 3 ** (10 ** 18 % 6) % 7
```

### crt(pairs) → tuple
For `(remainder, modulus)` pairs: `(x, m)`, where `m` is the lcm of the
moduli and `x`, in `0..<m`, leaves each remainder. The moduli don't have
to be coprime, but must be positive. `none` if there's no such `x`.

```tinsel
crt([(2, 3), (3, 5), (2, 7)]) == (23, 105)
crt([(1, 4), (3, 6)]) == (9, 12)
crt([(1, 2), (0, 4)]) == none
```

### shoelace(points) → int
The area of the polygon through the `(x, y)` points, back to the first.
An area that isn't a whole number is an error. By Pick's theorem, the grid
points strictly inside number `area - boundary / 2 + 1`.

```tinsel
shoelace([(0, 0), (4, 0), (4, 3), (0, 3)]) == 12
shoelace([(0, 0), (1, 0), (0, 1)]) # error
```

### solve(rows, rhs) → list
The integer solution `xs` of the linear system `rows · xs == rhs`, using
exact arithmetic. `none` if there isn't exactly one solution, or it isn't
all integers.

```tinsel
solve([[1, 1], [1, -1]], [10, 2]) == [6, 4]
solve([[1, 1], [2, 2]], [1, 2]) == none
solve([[2]], [3]) == none
```

## Directions and search

Directions are `(x, y)` steps, with `y` increasing downwards, as in grids.
A program's own definition of one of these names, such as `N = 100`, hides
it.

### N, E, S, W → tuple
The unit steps north, east, south and west.

```tinsel
N == (0, -1)
E == (1, 0)
S == (0, 1)
W == (-1, 0)
(3, 3) + N == (3, 2)
```

### DIRS4 → list
`[N, E, S, W]`, in the same order as `nbrs4`.

```tinsel
DIRS4 == [N, E, S, W]
DIRS4.map(d => (5, 5) + d) == nbrs4((5, 5))
```

### DIRS8 → list
The eight unit steps, in the same order as `nbrs8`.

```tinsel
DIRS8.len == 8
DIRS8.map(d => (5, 5) + d) == nbrs8((5, 5))
```

### cw(d) → tuple
`d` turned 90° clockwise. Works on any `(x, y)`, not only unit steps.

```tinsel
cw(N) == E
cw(W) == N
cw((2, 1)) == (-1, 2)
```

### ccw(d) → tuple
`d` turned 90° anticlockwise.

```tinsel
ccw(N) == W
ccw(cw((2, 1))) == (2, 1)
```

### manhattan(a, b) → int
The sum of the absolute differences of two tuples' coordinates.

```tinsel
manhattan((1, 2), (4, 0)) == 5
manhattan((0, 0, 0), (1, -1, 1)) == 3
```

### bfs(start, next), bfs(start, next, goal: g) → map or tuple
Breadth-first search. `next(state)` gives the states one step away.
Without `goal:`, a map from every reachable state to its number of steps.
With it, `(steps, path)` for the nearest state matching `g`, which is a
predicate or a plain value, or `none` if none is reachable. With
`all_paths: true` as well, `(steps, states)`, where `states` is the set of
every state on some shortest path.

```tinsel
bfs(0, n => [n + 1, n + 2].filter(_ <= 4)) == {0: 0, 1: 1, 2: 1, 3: 2, 4: 2}
bfs(1, n => [n + 1, n * 2], goal: 10) == (4, [1, 2, 4, 5, 10])
bfs(1, n => [n + 1, n * 2], goal: _ >= 16) == (4, [1, 2, 4, 8, 16])
bfs(0, n => [n + 1].filter(_ < 3), goal: 9) == none
bfs(0, n => [n + 1, n + 2].filter(_ <= 2), goal: 2, all_paths: true) == (1, {0, 2})
```

### dijkstra(start, next), dijkstra(start, next, goal: g) → map or tuple
As `bfs`, where `next(state)` gives `(state, cost)` pairs and distances
are total costs. Costs must be 0 or more.

```tinsel
dijkstra("a", s => {"a": [("b", 5), ("c", 1)], "c": [("b", 1)]}.get(s, [])) == {"a": 0, "b": 2, "c": 1}
dijkstra("a", s => {"a": [("b", 5), ("c", 1)], "c": [("b", 1)]}.get(s, []), goal: "b") == (2, ["a", "c", "b"])
dijkstra(0, n => [(n + 1, -1)], goal: 5) # error
```

### bfs_from(starts, next) → map or tuple
As `bfs`, searching from every state in `starts` at once, each at
distance 0. With `goal:`, the path begins at whichever start it came from.

```tinsel
bfs_from([0, 10], n => [n - 1, n + 1].filter(_ in 0..10))[7] == 3
bfs_from([0, 10], n => [n - 1, n + 1].filter(_ in 0..10), goal: 8) == (2, [10, 9, 8])
```

### dijkstra_from(starts, next) → map or tuple
As `dijkstra`, searching from every state in `starts` at once.

```tinsel
dijkstra_from(["a", "b"], s => {"a": [("c", 5)], "b": [("c", 2)]}.get(s, []), goal: "c") == (2, ["b", "c"])
```

### components(nodes, next) → list
The connected components of `nodes`, as a list of sets. `next` is followed
only between members of `nodes`, and should be symmetric. The sets are in
the order of their smallest members.

```tinsel
components([1, 2, 3, 10, 11], n => [n - 1, n + 1]) == [{1, 2, 3}, {10, 11}]
components("#.#\n#..".grid.where('#'), nbrs4).len == 2
```

## Intervals

A rangeset is a set of ints kept as sorted, separate ranges, so it stays
small however many ints it covers. `a | b`, `a & b` and `a - b` give the
union, intersection and difference, `x in r` is fast, and `min`, `max`,
`first`, `last` and `len` work as for a set.

### rangeset(xs), rangeset() → rangeset
A rangeset of the ranges and ints in `xs`, or an empty one. Empty ranges
are ignored, and a range that never ends is an error.

```tinsel
rangeset([1..3, 2..5, 7]) == rangeset([1..5, 7])
rangeset([1..10]).len == 10
5 in rangeset([1..3, 5..9])
(rangeset([1..10]) - rangeset([3..4])).ranges == [1..2, 5..10]
(rangeset([1..5]) & rangeset([4..9])).ranges == [4..5]
rangeset([0..<10 ** 15]).max == 10 ** 15 - 1
rangeset([1..]) # error
```

### r.ranges → list
The separate ranges of a rangeset, in order.

```tinsel
rangeset([5..6, 1..2, 3]).ranges == [1..3, 5..6]
rangeset().ranges == []
```

## Debugging

### dbg(x), dbg(label, x) → value
Prints `x` to stderr, with where it was called and the source text of `x`
(or `label`), and returns `x` unchanged, so it can go in the middle of a
chain. Large values are cut short. `tinsel eval` shows any expression's
value without changing the program.

```tinsel
[1, 2, 3].dbg.sum == 6
dbg("total", 2 + 3) == 5
```
