// Tests for the parts of the extension that don't need VS Code. Run from
// the repository root, after `cargo build --release`:
//
//   node --test editors/vscode/test

'use strict';

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');
const lib = require('../lib');

const root = path.resolve(__dirname, '..', '..', '..');

test('findBlocks finds the day header and numbers the examples', () => {
  const text = fs.readFileSync(path.join(root, 'tests/aoc2018/day15.tin'), 'utf8');
  const blocks = lib.findBlocks(text);
  assert.deepStrictEqual(blocks[0], { line: 0, kind: 'day' });
  const examples = blocks.filter((b) => b.kind === 'example');
  assert.strictEqual(examples.length, 6);
  assert.deepStrictEqual(examples.map((b) => b.n), [1, 2, 3, 4, 5, 6]);
  const lines = text.split('\n');
  for (const b of examples) {
    assert.match(lines[b.line], /^example\b/);
  }
});

test('findBlocks skips lines inside triple-quoted strings', () => {
  const text = [
    'day 2020.1',
    '',
    'example """',
    'day 3.4',
    'example text',
    '"""',
    '  part1 = 1',
    '',
    'example """x"""',
    '  part1 = 2',
    'notes = """',
    'example',
    '"""',
    'example """',
    '1',
    '"""',
  ].join('\n');
  assert.deepStrictEqual(lib.findBlocks(text), [
    { line: 0, kind: 'day' },
    { line: 2, kind: 'example', n: 1 },
    { line: 8, kind: 'example', n: 2 },
    { line: 13, kind: 'example', n: 3 },
  ]);
  assert.strictEqual(lib.exampleCount(text), 3);
});

test('definedParts finds top-level part definitions only', () => {
  assert.deepStrictEqual(lib.definedParts('part1 = 1\npart2=2\n'), ['part1', 'part2']);
  assert.deepStrictEqual(lib.definedParts('example """\n"""\n  part1 = 6\npart2 = 1\n'), ['part2']);
  assert.deepStrictEqual(lib.definedParts('test "t" = 1 == 1\n'), []);
});

test('errorMessage and shorten', () => {
  assert.strictEqual(lib.errorMessage('Error: unknown name `x`\n   ╭─[ <expression>:1:1 ]\n'), 'unknown name `x`');
  assert.strictEqual(lib.errorMessage('\n'), 'failed');
  assert.strictEqual(lib.shorten('[1,\n  2]', 20), '[1, 2]');
  assert.strictEqual(lib.shorten('abcdefghij', 5), 'abcd…');
});

test('findBinary prefers the setting, then a workspace build', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tinsel-'));
  assert.strictEqual(lib.findBinary(' /x/tinsel ', [dir]), '/x/tinsel');
  assert.strictEqual(lib.findBinary('', [dir]), 'tinsel');
  fs.mkdirSync(path.join(dir, 'target', 'debug'), { recursive: true });
  const exe = process.platform === 'win32' ? 'tinsel.exe' : 'tinsel';
  fs.writeFileSync(path.join(dir, 'target', 'debug', exe), '');
  assert.strictEqual(lib.findBinary('', [dir]), path.join(dir, 'target', 'debug', exe));
});

// The problem matchers, applied as VS Code applies a multi-line pattern:
// consecutive lines, each matching the next regexp in turn.

const manifest = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'package.json'), 'utf8'));

function problems(matcherName, outputText) {
  const matcher = manifest.contributes.problemMatchers.find((m) => m.name === matcherName);
  const patterns = matcher.pattern.map((p) => ({ ...p, re: new RegExp(p.regexp) }));
  const lines = outputText.split('\n');
  const found = [];
  for (let i = 0; i + patterns.length <= lines.length; i++) {
    const ms = patterns.map((p, k) => p.re.exec(lines[i + k]));
    if (ms.every(Boolean)) {
      const item = {};
      patterns.forEach((p, k) => {
        for (const key of ['file', 'line', 'column', 'message', 'code']) {
          if (p[key] !== undefined) {
            item[key] = ms[k][p[key]];
          }
        }
      });
      found.push(item);
    }
  }
  return found;
}

const binary = ['release', 'debug']
  .map((b) => path.join(root, 'target', b, process.platform === 'win32' ? 'tinsel.exe' : 'tinsel'))
  .find((p) => fs.existsSync(p));

/** Runs the CLI, which may fail; gives stdout and stderr together. */
function cli(args) {
  try {
    return execFileSync(binary, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  } catch (e) {
    return `${e.stdout}${e.stderr}`;
  }
}

test('problem matchers read the CLI output', { skip: !binary && 'no tinsel build' }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tinsel-'));
  const file = path.join(dir, 'bad.tin');
  fs.writeFileSync(file, [
    'example """',
    '1 2',
    '"""',
    '  part1 = 4',
    '',
    'test "two" = 1 + 1 == 3',
    'part1 = input.ints.sum',
    'part2 = [1][5]',
  ].join('\n') + '\n');

  // `tinsel test`: a failed test and a failed example.
  const tested = problems('tinsel-checks', cli(['test', file]));
  assert.deepStrictEqual(tested.map((p) => [p.file, p.line, p.code]), [
    [file, '6', 'test "two"'],
    [file, '1', 'example "example 1"'],
  ]);
  assert.match(tested[0].message, /false/);

  // `tinsel run`: the failed example stops the run.
  const ran = problems('tinsel-examples', cli(['run', file]));
  assert.deepStrictEqual(ran.map((p) => [p.file, p.line, p.code]), [[file, '1', 'example 1']]);
  assert.match(ran[0].message, /part1/);

  // Runtime errors, with their positions: part1 reads an input the
  // program has none of, and part2 indexes past the end of a list.
  const errors = problems('tinsel', cli(['run', '--no-examples', file]));
  assert.deepStrictEqual(errors.map((p) => [p.file, p.line, p.column]), [
    [file, '7', '9'],
    [file, '8', '9'],
  ]);
  assert.match(errors[0].message, /input/);
  assert.match(errors[1].message, /out of range/);
});
