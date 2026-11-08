# Reference for 2024 day 16 (reindeer maze). Written for the Tinsel tests.
# Dijkstra over (position, facing). Part 2 searches backwards from the end
# too: a state is on a best path when the two costs add up to the best.
import heapq
import sys

grid = open(sys.argv[1] if len(sys.argv) > 1 else "y24d16.txt").read().strip().split("\n")
cells = {(x, y): c for y, row in enumerate(grid) for x, c in enumerate(row)}
start = next(p for p, c in cells.items() if c == "S")
end = next(p for p, c in cells.items() if c == "E")
DIRS = [(1, 0), (0, 1), (-1, 0), (0, -1)]

def search(starts, backwards):
    dist = {}
    heap = [(0, s) for s in starts]
    while heap:
        d, (p, f) = heapq.heappop(heap)
        if (p, f) in dist:
            continue
        dist[(p, f)] = d
        dx, dy = DIRS[f]
        step = (p[0] - dx, p[1] - dy) if backwards else (p[0] + dx, p[1] + dy)
        moves = [((p, (f + 1) % 4), 1000), ((p, (f - 1) % 4), 1000)]
        if cells.get(step, "#") != "#":
            moves.append(((step, f), 1))
        for state, cost in moves:
            if state not in dist:
                heapq.heappush(heap, (d + cost, state))
    return dist

forward = search([(start, 0)], False)
best = min(d for (p, _), d in forward.items() if p == end)
backward = search([(end, f) for f in range(4)], True)
print(best)
print(len({p for s, d in forward.items() if s in backward and d + backward[s] == best for p in [s[0]]}))
