# Reference for 2025 day 6 (cephalopod worksheet). Written for the Tinsel
# tests. Finds the problems by scanning for all-space columns.
import math
import sys

rows = open(sys.argv[1] if len(sys.argv) > 1 else "y25d06.txt").read().rstrip("\n").split("\n")
width = max(map(len, rows))
rows = [r.ljust(width) for r in rows]
blank = [all(r[c] == " " for r in rows) for c in range(width)]

part1 = part2 = 0
c = 0
while c < width:
    if blank[c]:
        c += 1
        continue
    e = c
    while e < width and not blank[e]:
        e += 1
    calc = sum if rows[-1][c:e].strip() == "+" else math.prod
    part1 += calc(int(r[c:e]) for r in rows[:-1] if r[c:e].strip())
    part2 += calc(int("".join(r[x] for r in rows[:-1])) for x in range(c, e))
    c = e
print(part1)
print(part2)
