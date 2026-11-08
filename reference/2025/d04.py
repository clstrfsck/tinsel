# Python port of d04.rkt (Racket isn't installed here). Same algorithm.
import sys

def neighbourhood(p):
    x, y = p
    return [(x + dx, y + dy) for dy in (-1, 0, 1) for dx in (-1, 0, 1)]

def count_neighbours(p, points):
    return sum(q in points for q in neighbourhood(p))   # includes p itself

def main(path="y25d04.txt"):
    lines = open(path).read().splitlines()
    points = {(x, y) for y, line in enumerate(lines) for x, c in enumerate(line) if c == '@'}
    print("Result1:", sum(count_neighbours(p, points) < 5 for p in points))
    total, current = 0, set(points)
    while True:
        remove = {p for p in current if count_neighbours(p, current) < 5}
        if not remove:
            break
        total += len(remove)
        current -= remove
    print("Result2:", total)

main(*sys.argv[1:])
