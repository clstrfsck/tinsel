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

# `check`, plus every .tin file on the IR interpreter (about 7 minutes more).
check-all: check test-ir

# Build the debug interpreter.
build:
    cargo build

# Build the release interpreter, which the editor and timings use.
release:
    cargo build --release

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

# Every .tin file natively, with the leak check (debug build).
test-native:
    TIN_ENGINE=native cargo test --test run_tin

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
    node --test editors/vscode/test

# Run a solution with the release build, e.g. `just run tests/aoc2024/day01.tin`.
run file *args: release
    ./target/release/tinsel run {{args}} {{file}}

# Time a solution on its real input only, with the release build.
time file: release
    time ./target/release/tinsel run --no-examples {{file}}

# Show a built-in's entry in the library reference, e.g. `just doc find_cycle`.
doc name="": release
    ./target/release/tinsel doc {{name}}

# Package the VS Code extension as a .vsix (needs Node.js; downloads vsce).
vsix:
    cd editors/vscode && npx @vscode/vsce package --allow-missing-repository --skip-license
