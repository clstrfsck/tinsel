// The parts of the extension that don't need VS Code, so that they can be
// tested with plain Node (`node --test editors/vscode/test`).

'use strict';

const fs = require('fs');
const path = require('path');

// The item keywords start an item only in column 1 and when followed by
// what the item needs (SPEC 3): `day` by a number, `example`, `test` and
// `test_error` by a string, and `answers` by the end of its line.
// Otherwise they are ordinary names, as in `example = 5`.
const DAY = /^day\s+\d+\.\d+/;
const EXAMPLE = /^example\s*"/;
const ANSWERS = /^answers\s*(#.*)?$/;
const TEST = /^(test_error|test)\s*"((?:[^"\\]|\\.)*)"/;

/**
 * The top-level lines of a program that get code lenses: the `day` header
 * and each `example` block, numbered from 1. Lines inside triple-quoted
 * strings (the examples' own text) are skipped, so an input line such as
 * "day 3" can't be taken for a header.
 */
function findBlocks(text) {
  const blocks = [];
  let inString = false;
  let examples = 0;
  text.split('\n').forEach((line, i) => {
    const wasInString = inString;
    // Each `"""` opens or closes a triple-quoted string.
    const quotes = line.split('"""').length - 1;
    if (quotes % 2 === 1) {
      inString = !inString;
    }
    if (wasInString) {
      return;
    }
    if (DAY.test(line)) {
      blocks.push({ line: i, kind: 'day' });
    } else if (EXAMPLE.test(line)) {
      examples += 1;
      blocks.push({ line: i, kind: 'example', n: examples });
    }
  });
  return blocks;
}

/**
 * The checks a program declares, at the top level, as `tinsel test`
 * reports them: each `test` and `test_error` with its name, each `example`
 * (numbered from 1) and the `answers` block. `line` is 0-based. Lines
 * inside triple-quoted strings are skipped.
 */
function findChecks(text) {
  const checks = [];
  let inString = false;
  let examples = 0;
  text.split('\n').forEach((line, i) => {
    const wasInString = inString;
    if ((line.split('"""').length - 1) % 2 === 1) {
      inString = !inString;
    }
    if (wasInString) {
      return;
    }
    const t = TEST.exec(line);
    if (t) {
      checks.push({ line: i, kind: t[1], name: unescape(t[2]) });
    } else if (EXAMPLE.test(line)) {
      examples += 1;
      checks.push({ line: i, kind: 'example', name: `example ${examples}` });
    } else if (ANSWERS.test(line)) {
      checks.push({ line: i, kind: 'answers', name: 'answers' });
    }
  });
  return checks;
}

/** A Tinsel string literal's escapes undone: `\"`, `\\`, `\n`, `\t`. */
function unescape(s) {
  return s.replace(/\\(.)/g, (_, c) => ({ n: '\n', t: '\t' })[c] || c);
}

/**
 * The objects in JSON-lines output, such as `tinsel test --json`. Lines
 * that aren't JSON objects are ignored.
 */
function parseJsonLines(text) {
  const out = [];
  for (const line of text.split('\n')) {
    const t = line.trim();
    if (t.startsWith('{')) {
      try {
        out.push(JSON.parse(t));
      } catch (_) {
        // Not a complete object: ignore it.
      }
    }
  }
  return out;
}

/**
 * The 0-based range of an error object from `--json` output: from its
 * start to its end, or one char when the span is empty.
 */
function errorRange(e) {
  const line = Math.max((e.line || 1) - 1, 0);
  const col = Math.max((e.column || 1) - 1, 0);
  let endLine = e.end_line ? e.end_line - 1 : line;
  let endCol = e.end_column ? e.end_column - 1 : col;
  if (endLine < line || (endLine === line && endCol <= col)) {
    endLine = line;
    endCol = col + 1;
  }
  return { line, col, endLine, endCol };
}

/**
 * The Test Explorer group for a file in folder `rel` (relative to its
 * workspace folder, `''` at the top), or none to list it at the top level.
 * The id includes the workspace folder, so that two roots with the same
 * folder (say both have `tests`) don't share an item. With several roots,
 * groups are labelled with the root's name, and files at the top of a root
 * are grouped under it.
 */
function folderGroup(workspaceUri, workspaceName, rel, rootCount) {
  if (rel === '' && rootCount <= 1) {
    return undefined;
  }
  const shown = rel.split(path.sep).join('/');
  let label = shown;
  if (rootCount > 1) {
    label = shown === '' ? workspaceName : `${workspaceName}/${shown}`;
  }
  return { id: `dir:${workspaceUri}|${shown}`, label };
}

/** Which of `part1` and `part2` the program defines at the top level. */
function definedParts(text) {
  return ['part1', 'part2'].filter((p) => new RegExp(`^${p}\\s*=`, 'm').test(text));
}

/** How many `example` blocks the program has. */
function exampleCount(text) {
  return findBlocks(text).filter((b) => b.kind === 'example').length;
}

/**
 * The first line of an error report from the CLI, such as
 * "Error: unknown name `x`", without the "Error: " prefix.
 */
function errorMessage(stderr) {
  const first = stderr.split('\n').find((l) => l.trim() !== '') || 'failed';
  return first.replace(/^Error:\s*/, '').trim();
}

/** `text` on one line and at most `max` chars, for showing inline. */
function shorten(text, max) {
  const one = text.replace(/\s+/g, ' ').trim();
  return one.length <= max ? one : `${one.slice(0, max - 1)}…`;
}

/**
 * The `tinsel` binary to run: the `tinsel.path` setting if given, else a
 * release or debug build in the workspace (this repository), else
 * `tinsel` on the PATH.
 */
function findBinary(setting, workspaceRoots) {
  if (setting && setting.trim() !== '') {
    return setting.trim();
  }
  const exe = process.platform === 'win32' ? 'tinsel.exe' : 'tinsel';
  for (const root of workspaceRoots) {
    for (const build of ['release', 'debug']) {
      const candidate = path.join(root, 'target', build, exe);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return 'tinsel';
}

module.exports = {
  findBlocks, findChecks, definedParts, exampleCount, errorMessage, shorten, findBinary,
  parseJsonLines, errorRange, folderGroup,
};
