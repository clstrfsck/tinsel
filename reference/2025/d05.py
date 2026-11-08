# Reference for 2025 day 5 (fresh ingredient ID ranges). Written for the
# Tinsel tests.
import sys

text = open(sys.argv[1] if len(sys.argv) > 1 else "y25d05.txt").read()
range_text, id_text = text.strip().split("\n\n")
ranges = [tuple(map(int, line.split("-"))) for line in range_text.split()]
print(sum(any(lo <= int(x) <= hi for lo, hi in ranges) for x in id_text.split()))

merged = []
for lo, hi in sorted(ranges):
    if merged and lo <= merged[-1][1] + 1:
        merged[-1][1] = max(merged[-1][1], hi)
    else:
        merged.append([lo, hi])
print(sum(hi - lo + 1 for lo, hi in merged))
