# Reference solutions and answers (2018 to 2024)

Where the expected answers in `tests/aoc2018/` to `tests/aoc2024/` come
from. For 2025, see [`2025/README.md`](2025/README.md): the user's own
published solutions, in several languages.

## Python reference scripts (2022, 2023, 2024)

Written in Python for the Tinsel tests: there were no published solutions to
copy. Each takes an optional input path and otherwise reads `yYYdDD.txt`
from the current directory.

| Puzzle | File | Part 1 | Part 2 |
|---|---|---|---|
| 2023 day 14 | `2023/d14.py` | 106378 | 90795 |
| 2022 day 17 | `2022/d17.py` | 3124 | 1561176470569 |
| 2022 day 12 | `2022/d12.py` | 339 | 332 |
| 2024 day 16 | `2024/d16.py` | 79404 | 451 |

`2022/d17.py` was also checked against the puzzle's public example
(3068 / 1514285714288). That example isn't stored in the repo.

The answers for 2022 day 12 and 2024 day 16 also match the ones the puzzle
site accepted. Both scripts reproduce the puzzles' worked examples, which are
stored in the `.tin` files (31 / 29; 7036 / 45 and 11048 / 64).

## Complete years from the puzzle pages (2023, 2024)

The other 2023 and 2024 solutions have no reference scripts. Their `answers`
blocks hold the answers the puzzle site accepted, as shown on each puzzle
page ("Your puzzle answer was ..."). Their `example` blocks hold the
puzzles' own worked examples and the results the puzzle text gives for
them.

## 2022

The `answers` blocks hold the answers the puzzle site accepted, as shown on
each puzzle page. The `example` blocks hold the puzzles' own worked
examples and the results the puzzle text gives for them.

## 2019

Days 1 to 15 hold the answers the puzzle site accepted, as shown on each
puzzle page. Days 16 to 25 were solved before their answers were submitted,
and were checked against separately written Python scripts that are not
kept in the repository. The day 25 password needs no check: the game prints
it only when the droid carries the right weight. The `example` blocks hold
the puzzles' own worked examples; for days 16 to 25, any part 2 example
values were written in before the part 2 text was unlocked, and still need
checking against it.

The Intcode computer is shared through `include` (SPEC 2.8):
`tests/aoc2019/intcode.tin` holds it, with the small example programs from
days 2, 5 and 9 as its tests. `tests/lib/ocr.tin` reads the capital letters
drawn by 2019 days 8 and 11 and 2022 day 10.

## 2020

The `answers` blocks hold the answers the puzzle site accepted. Part 2 of
days 20 and 22, and days 23 to 25, were solved before those answers were
submitted, and were checked first against separately written Python
scripts that are not kept in the repository. The `example` blocks hold the
puzzles' own worked examples and the results the puzzle text gives for
them.

## 2021

The `answers` blocks hold the answers the puzzle site accepted. Days 19 to
25 were solved before those answers were submitted, and were checked first
against separately written Python scripts that are not kept in the
repository; day 24's answers are also checked by running the puzzle's own
program in `tests/aoc2021/day24.tin`. The `example` blocks hold the
puzzles' own worked examples and the results the puzzle text gives for
them.

## 2018

The `answers` blocks hold the answers the puzzle site accepted. None of
2018 had been solved before these solutions were written, so every answer,
both parts, was first checked against separately written Python scripts
that are not kept in the repository. The `example` blocks hold the
puzzles' own worked examples and the results the puzzle text gives for
them; the example for day 10 checks only the time, since its letters are
smaller than the ones `tests/lib/ocr.tin` reads.

The wrist device's instruction set is shared through `include`:
`tests/aoc2018/device.tin` holds the opcodes and a program runner, used by
days 16, 19 and 21. Days 19 and 21 also rely on reading their programs:
day 19's sums the divisors of a number it builds, and day 21's hashes a
value in a loop; each file explains this and checks its shortcut against
the device running the program. `tests/lib/ocr.tin` reads day 10's taller
letters, but knows only the letters seen so far.
