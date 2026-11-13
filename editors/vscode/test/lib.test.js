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

test('findBlocks ignores item words used as names', () => {
  assert.deepStrictEqual(lib.findBlocks('day(n) = n\nexample = 5\nexample "x"\n  part1 = 1\n'),
    [{ line: 2, kind: 'example', n: 1 }]);
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

test('findChecks finds tests, examples and answers', () => {
  const text = [
    'test "plain" = 1 == 1',
    'test_error "with \\"quotes\\"" = 1 / 0',
    'example """',
    'test "not a test" = 1',
    'answers',
    '"""',
    '  part1 = 1',
    '',
    'answers',
    '  part1 = 2',
    'tests = 3',
    // Item words used as names (SPEC 3).
    'example = 5',
    'answers = 7',
    'test(x) = x * 2',
    'day(n) = n + 1',
  ].join('\n');
  assert.deepStrictEqual(lib.findChecks(text), [
    { line: 0, kind: 'test', name: 'plain' },
    { line: 1, kind: 'test_error', name: 'with "quotes"' },
    { line: 2, kind: 'example', name: 'example 1' },
    { line: 8, kind: 'answers', name: 'answers' },
  ]);
});

test('parseJsonLines and errorRange', () => {
  assert.deepStrictEqual(lib.parseJsonLines('{"a":1}\nnot json\n{"b":\n\n{"c":[2]}\n'), [{ a: 1 }, { c: [2] }]);
  assert.deepStrictEqual(lib.errorRange({ line: 2, column: 3, end_line: 2, end_column: 7 }),
    { line: 1, col: 2, endLine: 1, endCol: 6 });
  // An empty span still covers one char.
  assert.deepStrictEqual(lib.errorRange({ line: 1, column: 5, end_line: 1, end_column: 5 }),
    { line: 0, col: 4, endLine: 0, endCol: 5 });
  assert.deepStrictEqual(lib.errorRange({ message: 'no position' }),
    { line: 0, col: 0, endLine: 0, endCol: 1 });
});

test('tinsel check --json gives errors with ranges', { skip: !binary && 'no tinsel build' }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'tinsel-'));
  const good = path.join(dir, 'good.tin');
  const bad = path.join(dir, 'bad.tin');
  fs.writeFileSync(good, 'part1 = 1\n');
  fs.writeFileSync(bad, 'x = 1\npart1 = x + nope\n');
  assert.deepStrictEqual(JSON.parse(cli(['check', '--json', good])), { errors: [] });
  const errors = JSON.parse(cli(['check', '--json', bad])).errors;
  assert.deepStrictEqual(errors, [{
    file: bad, line: 2, column: 13, end_line: 2, end_column: 17, message: 'unknown name `nope`',
  }]);
});

test('every check tinsel test --json reports is one findChecks finds', { skip: !binary && 'no tinsel build' }, () => {
  // The Test Explorer matches results to its items by file and line, so
  // both must agree on where each check is, what kind it is and its name.
  const dirs = ['tests/features', 'tests/aoc2025', 'tests/aoc2018'].map((d) => path.join(root, d));
  const results = lib.parseJsonLines(cli(['test', '--json', ...dirs]));
  const summary = results.pop();
  assert.ok('passed' in summary, 'ends with a summary');
  assert.strictEqual(summary.failed, 0);
  const found = new Map();
  for (const r of results) {
    assert.notStrictEqual(r.kind, 'load', `${r.file} loads`);
    if (!found.has(r.file)) {
      found.set(r.file, lib.findChecks(fs.readFileSync(r.file, 'utf8')));
    }
    const match = found.get(r.file).find((c) => c.line + 1 === r.line);
    assert.ok(match, `${r.file}:${r.line} ${r.kind} ${r.name}`);
    assert.strictEqual(match.kind, r.kind, `${r.file}:${r.line}`);
    assert.strictEqual(match.name, r.name, `${r.file}:${r.line}`);
  }
  // And every check found was run, apart from those of included files.
  const counted = [...found.values()].reduce((n, cs) => n + cs.length, 0);
  assert.strictEqual(counted, results.length);
  assert.ok(results.length > 400, `${results.length} results`);
});

test('folderGroup keeps folders of different roots apart', () => {
  const sep = path.sep;
  // One root: grouped by relative folder; top-level files aren't grouped.
  assert.deepStrictEqual(lib.folderGroup('file:///a', 'a', `tests${sep}aoc2024`, 1),
    { id: 'dir:file:///a|tests/aoc2024', label: 'tests/aoc2024' });
  assert.strictEqual(lib.folderGroup('file:///a', 'a', '', 1), undefined);
  // Two roots with the same folder: different ids, labelled by root.
  const one = lib.folderGroup('file:///one', 'one', 'tests', 2);
  const two = lib.folderGroup('file:///two', 'two', 'tests', 2);
  assert.notStrictEqual(one.id, two.id);
  assert.deepStrictEqual([one.label, two.label], ['one/tests', 'two/tests']);
  // With several roots, a root's top-level files are grouped under it.
  assert.deepStrictEqual(lib.folderGroup('file:///one', 'one', '', 2),
    { id: 'dir:file:///one|', label: 'one' });
});

// The library reference

const splitEntry = {
  section: 'Strings',
  signature: 's.split(sep) → list',
  names: ['split'],
  description: 'Splits on every occurrence of `sep`.',
  examples: ['"a,b".split(",") == ["a", "b"]', '"abc".split("") # error'],
};

test('callContext finds the call being typed and its argument', () => {
  assert.deepStrictEqual(lib.callContext('x = grid.where(_ > 4, '),
    { name: 'where', method: true, arg: 1 });
  assert.deepStrictEqual(lib.callContext('x = zip(a, [1, 2'), { name: 'zip', method: false, arg: 1 });
  assert.deepStrictEqual(lib.callContext('x = bfs(start, n => f(n), '),
    { name: 'bfs', method: false, arg: 2 });
  // Brackets and commas in strings, chars and comments don't count.
  assert.deepStrictEqual(lib.callContext("x = f(')', \"a,(b\", "), { name: 'f', method: false, arg: 2 });
  assert.deepStrictEqual(lib.callContext('x = f(1, # (a, b\n  '), { name: 'f', method: false, arg: 1 });
  // Not in a named call: a tuple, a string, or a closed call.
  assert.strictEqual(lib.callContext('x = (1, '), null);
  assert.strictEqual(lib.callContext('x = "abc('), null);
  assert.strictEqual(lib.callContext('x = f(1) + '), null);
});

test('inCode is false in strings and comments', () => {
  assert.strictEqual(lib.inCode('x = lines'), true);
  assert.strictEqual(lib.inCode('x = "lines'), false);
  assert.strictEqual(lib.inCode('x = 1  # lines'), false);
  assert.strictEqual(lib.inCode('example """\nday 3 lines'), false);
  assert.strictEqual(lib.inCode('example """\n1\n"""\npart1 = lines'), true);
});

test('definesName finds definitions that hide a built-in', () => {
  assert.strictEqual(lib.definesName('N = 100\n', 'N'), true);
  assert.strictEqual(lib.definesName('f(x) = x\n', 'f'), true);
  assert.strictEqual(lib.definesName('  f((x, y)) when x > 0 = y\n', 'f'), true);
  assert.strictEqual(lib.definesName('fewest(goal) when all(goal .== 0) = 0\n', 'fewest'), true);
  // Uses, not definitions.
  assert.strictEqual(lib.definesName('N == 1\n', 'N'), false);
  assert.strictEqual(lib.definesName('  bfs(start, next)\n', 'bfs'), false);
  assert.strictEqual(lib.definesName('  bfs(start, next).len >= 3\n', 'bfs'), false);
  assert.strictEqual(lib.definesName('sets = [1]\n', 'set'), false);
});

test('signatures are given in method or function form', () => {
  const labels = (sig, method) => lib.signatures([{ signature: sig }], method).map((s) => s.label);
  assert.deepStrictEqual(labels('s.split(sep) → list', true), ['s.split(sep) → list']);
  assert.deepStrictEqual(labels('s.split(sep) → list', false), ['split(s, sep) → list']);
  assert.deepStrictEqual(labels('zip(a, b, ...) → list', true), ['a.zip(b, ...) → list']);
  assert.deepStrictEqual(labels('xs.count(f), xs.count(x), xs.count → int', true),
    ['xs.count(f) → int', 'xs.count(x) → int']);
  assert.deepStrictEqual(labels('xs.count(f), xs.count(x), xs.count → int', false),
    ['count(xs, f) → int', 'count(xs, x) → int', 'count(xs) → int']);
  assert.deepStrictEqual(labels('N, E, S, W → tuple', false), []);
  // Each parameter's range is its text in the label.
  const [sig] = lib.signatures([{ signature: 'bfs(start, next), bfs(start, next, goal: g) → map' }], false);
  assert.deepStrictEqual(sig.params.map((p) => sig.label.slice(p.start, p.end)), ['start', 'next']);
});

test('activeSignature picks one with enough parameters', () => {
  const sigs = lib.signatures([{ signature: 'int(s), int(s, base) → int' }], false);
  assert.strictEqual(lib.activeSignature(sigs, 0), 0);
  assert.strictEqual(lib.activeSignature(sigs, 1), 1);
  const zip = lib.signatures([{ signature: 'zip(a, b, ...) → list' }], false);
  assert.strictEqual(lib.activeSignature(zip, 5), 0);
});

test('entries become Markdown and are indexed by name', () => {
  const md = lib.entryMarkdown(splitEntry);
  assert.match(md, /^```tinsel\ns\.split\(sep\) → list\n```\n\nSplits on/);
  assert.match(md, /"abc"\.split\(""\) # error\n```$/);
  const index = lib.docIndex([splitEntry, { ...splitEntry, names: ['split', 'other'] }]);
  assert.strictEqual(index.get('split').length, 2);
  assert.strictEqual(index.get('other').length, 1);
  assert.match(lib.referenceMarkdown([splitEntry]), /^# Tinsel library reference\n\n## Strings\n\n### s\.split/);
  assert.strictEqual(lib.isConstant({ signature: 'N, E, S, W → tuple' }), true);
  assert.strictEqual(lib.isConstant({ signature: 's.lines → list' }), false);
});

test('every entry from tinsel doc --json can be shown', { skip: !binary && 'no tinsel build' }, () => {
  const entries = lib.parseJsonLines(cli(['doc', '--json']));
  assert.ok(entries.length > 100, `only ${entries.length} entries`);
  for (const entry of entries) {
    const forms = lib.signatureForms(entry.signature);
    assert.ok(forms.length > 0, `can't read ${entry.signature}`);
    // The extension and the CLI agree on which names an entry describes.
    const names = [...new Set(forms.map((f) => f.name))];
    assert.deepStrictEqual(names, entry.names, entry.signature);
    if (!lib.isConstant(entry)) {
      assert.ok(lib.signatures([entry], false).length > 0, `no signature for ${entry.signature}`);
    }
  }
});
