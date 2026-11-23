# Tasks for working on Tinsel. Run `just` to list them.
#
# `just check` is the pre-commit check: everything in CONTRIBUTING.md's
# "Run the Required Checks", plus every solution on the native engine.

# List the recipes.
default:
    @just --list --unsorted

# The pre-commit check: fmt, clippy, all tests, native corpus, extension tests.
check: fmt-check clippy test test-native ext-test
    @echo "All checks passed."

# `check`, plus every .tin file on the IR interpreter.
check-all: check test-ir

# Build the debug interpreter.
build:
    cargo build

# Build the release interpreter, which the editor and timings use.
release:
    cargo build --release

# Install a release `tinsel` in ~/.cargo/bin, replacing any earlier copy.
install:
    cargo install --path . --locked

# Format the code.
fmt:
    cargo fmt

# Check the formatting without changing anything.
fmt-check:
    cargo fmt -- --check

# Lint everything, tests included, with warnings as errors.
clippy:
    cargo clippy --all-targets -- -D warnings

# Rust, CLI, library-reference and .tin tests (features on all engines).
test:
    cargo test

# Every non-feature .tin file natively, with the leak check (debug build).
# `cargo test` already runs feature files natively.
test-native:
    TIN_ENGINE=native cargo test --test run_tin -- tests/aoc20 tests/ec20 tests/lib

# Every .tin file on the IR interpreter, with the leak check (slow).
test-ir:
    TIN_ENGINE=ir cargo test --test run_tin

# The .tin files whose path contains a filter, e.g. `just tin aoc2024 memo`.
tin +filters:
    TIN_VERBOSE=1 cargo test --test run_tin -- {{filters}}

# The library reference's checks (`docs/library.md`).
test-docs:
    cargo test --test docs

# The VS Code extension's tests, against a fresh release build.
ext-test: release
    TINSEL_BIN=target/release/tinsel node --test editors/vscode/test/*.test.js

# Run a solution with the release build, e.g. `just run tests/aoc2024/day01.tin`.
run file *args: release
    ./target/release/tinsel run {{args}} {{file}}

# Time a solution on its real input only, with the release build: each
# step (loading, compiling, each part) from `--time`, then the whole process.
time file: release
    time ./target/release/tinsel run --no-examples --time {{file}}

# Start a day's solution from a template, e.g. `just new 2026 1`.
new year day:
    #!/usr/bin/env bash
    # Creates tests/aoc2026/day01.tin; an existing file is never overwritten.
    set -euo pipefail
    if [[ ! "{{year}}" =~ ^[0-9]{4}$ || ! "{{day}}" =~ ^[0-9]{1,2}$ ]]; then
        echo "usage: just new YEAR DAY" >&2; exit 1
    fi
    day=$((10#{{day}}))
    file=$(printf 'tests/aoc%s/day%02d.tin' {{year}} $day)
    if [[ -e $file ]]; then echo "$file already exists"; exit 0; fi
    mkdir -p "$(dirname "$file")"
    cat > "$file" <<EOF
    day {{year}}.$day

    # Part 1:
    # Part 2:

    example """
    """
      part1 = 0

    answers

    lines = input.lines

    part1 = lines.len
    EOF
    echo "created $file"

# Start a day: `input`, then `new` only if that worked, e.g. `just go 2026 1`.
go year day: (input year day) (new year day)

# Download a day's puzzle input with curl, e.g. `just input 2026 1`.
input year day:
    #!/usr/bin/env bash
    # Saves input/2026/y26d01.txt. The session cookie comes from $AOC_SESSION
    # or the gitignored file .aoc-session (the cookie's value, copied from the
    # browser while logged in). An input already saved isn't downloaded
    # again, and nothing is requested before the puzzle unlocks.
    # $AOC_USER_AGENT replaces the User-Agent, for example to add contact
    # details as the site asks of automated tools.
    set -euo pipefail
    if [[ ! "{{year}}" =~ ^[0-9]{4}$ || ! "{{day}}" =~ ^[0-9]{1,2}$ ]]; then
        echo "usage: just input YEAR DAY" >&2; exit 1
    fi
    day=$((10#{{day}}))
    out=$(printf 'input/%s/y%02dd%02d.txt' {{year}} $(({{year}} % 100)) $day)
    if [[ -s $out ]]; then echo "$out already exists"; exit 0; fi
    # Puzzles unlock at midnight US Eastern time, 05:00 UTC.
    if [[ $(date -u +%Y%m%d%H) < $(printf '%s12%02d05' {{year}} $day) ]]; then
        printf '%s day %d unlocks at 05:00 UTC on %s-12-%02d\n' {{year}} $day {{year}} $day >&2; exit 1
    fi
    session=${AOC_SESSION:-}
    if [[ -z $session && -f .aoc-session ]]; then session=$(tr -d ' \t\r\n' < .aoc-session); fi
    session=${session#session=}
    if [[ -z $session ]]; then
        echo "no session cookie: put it in .aoc-session or set AOC_SESSION" >&2; exit 1
    fi
    mkdir -p "$(dirname "$out")"
    tmp=$(mktemp "$out.XXXXXX")
    trap 'rm -f "$tmp"' EXIT
    # The cookie goes through a config on stdin, so it never appears in `ps`.
    code=$(printf 'header = "Cookie: session=%s"\n' "$session" | curl -sS -K - \
        -A "${AOC_USER_AGENT:-tinsel just input (curl)}" \
        -o "$tmp" -w '%{http_code}' "https://adventofcode.com/{{year}}/day/$day/input")
    if [[ $code != 200 ]]; then
        echo "download failed (HTTP $code): $(head -c 200 "$tmp")" >&2; exit 1
    fi
    mv "$tmp" "$out"
    trap - EXIT
    echo "saved $out ($(wc -l < "$out" | tr -d ' ') lines)"

# Show a built-in's entry in the library reference, e.g. `just doc find_cycle`.
doc name="": release
    ./target/release/tinsel doc {{name}}

# Package the VS Code extension as a .vsix (needs Node.js; downloads vsce).
vsix:
    cd editors/vscode && npx @vscode/vsce package --allow-missing-repository --skip-license
