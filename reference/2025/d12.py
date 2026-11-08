# Reference for 2025 day 12 (presents under trees). Written for the Tinsel
# tests. Rather than search for packings, it classifies each region and
# stops if any region would need a real search.
import sys

parts = open(sys.argv[1] if len(sys.argv) > 1 else "y25d12.txt").read().strip().split("\n\n")
cells = [p.count("#") for p in parts[:-1]]
fits = 0
for line in parts[-1].split("\n"):
    size, counts = line.split(": ")
    w, h = map(int, size.split("x"))
    counts = list(map(int, counts.split()))
    if (w // 3) * (h // 3) >= sum(counts):
        fits += 1  # each present gets its own 3x3 slot
    elif sum(n * c for n, c in zip(counts, cells)) <= w * h:
        sys.exit(f"region {line!r} needs a real packing search")
print(fits)
