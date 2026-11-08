# Reference for 2025 day 2 (repeated-digit product IDs). Written for the
# Tinsel tests. Brute force over every ID, unlike the Tinsel solution, which
# generates the repeated numbers directly.
import sys

def invalid_sum(text, part2):
    total = 0
    for r in text.strip().split(","):
        a, b = map(int, r.split("-"))
        for n in range(a, b + 1):
            s = str(n)
            reps = range(2, len(s) + 1) if part2 else [2]
            if any(len(s) % k == 0 and s == s[: len(s) // k] * k for k in reps):
                total += n
    return total

text = open(sys.argv[1] if len(sys.argv) > 1 else "y25d02.txt").read()
print(invalid_sum(text, False))
print(invalid_sum(text, True))
