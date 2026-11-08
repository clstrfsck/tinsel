# Reference for 2023 day 14 (tilting platform). Written for the Tinsel tests.
import sys

def tilt_north(rows):
    cols = [''.join(c) for c in zip(*rows)]
    # within each column, roll O towards index 0 between '#' stops
    cols = ['#'.join(''.join(sorted(seg, key=lambda ch: ch != 'O')) for seg in col.split('#')) for col in cols]
    return tuple(''.join(r) for r in zip(*cols))

def rotate_cw(rows):
    return tuple(''.join(r) for r in zip(*rows[::-1]))

def spin(rows):
    for _ in range(4):           # N, W, S, E: tilt north then rotate clockwise
        rows = rotate_cw(tilt_north(rows))
    return rows

def load(rows):
    h = len(rows)
    return sum(row.count('O') * (h - y) for y, row in enumerate(rows))

def main(path="y23d14.txt"):
    rows = tuple(open(path).read().split())
    print("Result1:", load(tilt_north(rows)))
    seen, history, n = {}, [], 1_000_000_000
    while rows not in seen:
        seen[rows] = len(history)
        history.append(rows)
        rows = spin(rows)
    start = seen[rows]
    period = len(history) - start
    print("Result2:", load(history[start + (n - start) % period]))

main(*sys.argv[1:])
