# Reference for 2022 day 12 (hill climbing). Written for the Tinsel tests.
# Part 2 runs one BFS from every height-a square at once.
import sys
from collections import deque

grid = open(sys.argv[1] if len(sys.argv) > 1 else "y22d12.txt").read().strip().split("\n")
cells = {(x, y): c for y, row in enumerate(grid) for x, c in enumerate(row)}
height = {p: ord({"S": "a", "E": "z"}.get(c, c)) for p, c in cells.items()}
start = next(p for p, c in cells.items() if c == "S")
end = next(p for p, c in cells.items() if c == "E")

def steps(starts):
    dist = {p: 0 for p in starts}
    queue = deque(starts)
    while queue:
        p = queue.popleft()
        for dx, dy in ((0, -1), (1, 0), (0, 1), (-1, 0)):
            n = (p[0] + dx, p[1] + dy)
            if n in height and n not in dist and height[n] <= height[p] + 1:
                dist[n] = dist[p] + 1
                queue.append(n)
    return dist.get(end)

print(steps([start]))
print(steps([p for p, h in height.items() if h == ord("a")]))
