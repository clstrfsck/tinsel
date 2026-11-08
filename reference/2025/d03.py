# Reference for 2025 day 3 (largest k-digit joltage per bank). Written for
# the Tinsel tests. Dynamic programming, unlike the greedy Tinsel solution.
import sys
from functools import lru_cache

def best(line, k):
    @lru_cache(None)
    def f(i, j):
        if j == 0:
            return 0
        if len(line) - i < j:
            return -1
        rest = f(i + 1, j - 1)
        take = -1 if rest < 0 else int(line[i]) * 10 ** (j - 1) + rest
        return max(take, f(i + 1, j))
    return f(0, k)

banks = open(sys.argv[1] if len(sys.argv) > 1 else "y25d03.txt").read().split()
print(sum(best(b, 2) for b in banks))
print(sum(best(b, 12) for b in banks))
