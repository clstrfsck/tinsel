# Reference solutions (2025)

Copied from https://clstrfsck.github.io/aoc-2025/. These are the source of
truth for the expected answers in `tests/aoc2025/`.

Small changes from the published versions, so they also run on the made-up
examples:

- `d01.nim` calls `.strip()` before `splitLines()`, so a trailing newline
  doesn't produce an empty move.
- `d10.kts` filters out blank lines.
- `d08.go` skips blank lines, and reads the number of part 1 pairs from the
  `PAIRS` environment variable (default 1000). The example uses `PAIRS=10`.
- `d04.py` is a Python port of `d04.rkt` using the same algorithm, because
  Racket isn't installed here. Both are kept.

Days 2, 3, 5, 6, 9 and 12 have no published solutions, so `d02.py`,
`d03.py`, `d05.py`, `d06.py`, `d09.py` and `d12.py` were written in Python
for the Tinsel tests. Where practical they use a different algorithm from
the Tinsel solution, so the two check each other: brute force for day 2,
dynamic programming for day 3, and a flood fill for day 9. Each also
reproduces the answers to the puzzle's own worked example. `d12.py` doesn't
search for packings: it stops with an error if any region is neither
trivially packable nor too small, which never happens on this input. Each
takes an optional input path, so it can also be run on an example file.

Run them from `input/2025/`, since each reads `y25dNN.txt` from the current directory:

```
cd input/2025
nim r --hints:off ../../reference/2025/d01.nim
python3 ../../reference/2025/d02.py        # likewise d03, d05, d06, d09, d12
python3 ../../reference/2025/d04.py        # or: racket ../../reference/2025/d04.rkt
sbcl --script ../../reference/2025/d07.lsp
go run ../../reference/2025/d08.go
kotlin ../../reference/2025/d10.kts
runhaskell ../../reference/2025/d11.hs
```

| Day | Part 1 | Part 2 |
|---|---|---|
| 1 | 1177 | 6768 |
| 2 | 24747430309 | 30962646823 |
| 3 | 17408 | 172740584266849 |
| 4 | 1389 | 9000 |
| 5 | 744 | 347468726696961 |
| 6 | 6605396225322 | 11052310600986 |
| 7 | 1550 | 9897897326778 |
| 8 | 112230 | 2573952864 |
| 9 | 4749672288 | 1479665889 |
| 10 | 469 | 19293 |
| 11 | 539 | 413167078187872 |
| 12 | 505 | (no part 2) |
