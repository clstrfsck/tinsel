# Reference for 2025 day 9 (largest rectangle of red and green tiles).
# Written for the Tinsel tests. Part 2 flood-fills the outside of the loop
# on compressed coordinates, unlike the Tinsel solution, which tests edge
# crossings.
import itertools
import sys

pts = [tuple(map(int, l.split(","))) for l in open(sys.argv[1] if len(sys.argv) > 1 else "y25d09.txt").read().split()]

def area(a, b):
    return (abs(a[0] - b[0]) + 1) * (abs(a[1] - b[1]) + 1)

print(max(area(a, b) for a, b in itertools.combinations(pts, 2)))

# Compressed grid: real coordinates at odd indices, gaps between them at even ones.
xi = {x: 2 * i + 1 for i, x in enumerate(sorted({p[0] for p in pts}))}
yi = {y: 2 * i + 1 for i, y in enumerate(sorted({p[1] for p in pts}))}
W, H = 2 * len(xi) + 2, 2 * len(yi) + 2
wall = [[False] * W for _ in range(H)]
for a, b in zip(pts, pts[1:] + pts[:1]):
    x1, x2 = sorted((xi[a[0]], xi[b[0]]))
    y1, y2 = sorted((yi[a[1]], yi[b[1]]))
    for x in range(x1, x2 + 1):
        for y in range(y1, y2 + 1):
            wall[y][x] = True

outside = [[False] * W for _ in range(H)]
outside[0][0] = True
stack = [(0, 0)]
while stack:
    x, y = stack.pop()
    for u, v in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
        if 0 <= u < W and 0 <= v < H and not outside[v][u] and not wall[v][u]:
            outside[v][u] = True
            stack.append((u, v))

# Prefix sums, to count outside cells in a rectangle in O(1).
P = [[0] * (W + 1) for _ in range(H + 1)]
for y in range(H):
    for x in range(W):
        P[y + 1][x + 1] = P[y][x + 1] + P[y + 1][x] - P[y][x] + outside[y][x]

best = 0
for a, b in itertools.combinations(pts, 2):
    x1, x2 = sorted((xi[a[0]], xi[b[0]]))
    y1, y2 = sorted((yi[a[1]], yi[b[1]]))
    if area(a, b) > best and P[y2 + 1][x2 + 1] - P[y1][x2 + 1] - P[y2 + 1][x1] + P[y1][x1] == 0:
        best = area(a, b)
print(best)
