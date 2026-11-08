# Reference for 2022 day 17 (falling rocks). Written for the Tinsel tests.
import sys

ROCKS = [
    [(0, 0), (1, 0), (2, 0), (3, 0)],
    [(1, 0), (0, 1), (1, 1), (2, 1), (1, 2)],
    [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2)],
    [(0, 0), (0, 1), (0, 2), (0, 3)],
    [(0, 0), (1, 0), (0, 1), (1, 1)],
]

def simulate(jets, n):
    filled = set()
    height = 0
    jet = 0
    seen = {}
    heights = [0]
    for i in range(n):
        rock = ROCKS[i % 5]
        x, y = 2, height + 3
        while True:
            dx = 1 if jets[jet] == '>' else -1
            jet = (jet + 1) % len(jets)
            if all(0 <= x + dx + rx < 7 and (x + dx + rx, y + ry) not in filled for rx, ry in rock):
                x += dx
            if all(y - 1 + ry >= 0 and (x + rx, y - 1 + ry) not in filled for rx, ry in rock):
                y -= 1
            else:
                break
        for rx, ry in rock:
            filled.add((x + rx, y + ry))
            height = max(height, y + ry + 1)
        heights.append(height)
        # skyline: depth of the top cell in each column, relative to height
        sky = tuple(height - max((yy for yy in range(height - 1, max(-1, height - 60), -1) if (c, yy) in filled), default=-1) for c in range(7))
        key = ((i + 1) % 5, jet, sky)
        if key in seen:
            start = seen[key]
            period = (i + 1) - start
            gain = heights[i + 1] - heights[start]
            q, r = divmod(n - start, period)
            return heights[start + r] + q * gain
        seen[key] = i + 1
    return height

def main(path="y22d17.txt"):
    jets = open(path).read().strip()
    print("Result1:", simulate(jets, 2022))
    print("Result2:", simulate(jets, 1_000_000_000_000))

main(*sys.argv[1:])
