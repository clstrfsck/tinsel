// The parts of the extension that don't need VS Code, so that they can be
// tested with plain Node (`node --test editors/vscode/test`).

'use strict';

const fs = require('fs');
const path = require('path');

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
    if (/^day\s+\d+\.\d+/.test(line)) {
      blocks.push({ line: i, kind: 'day' });
    } else if (/^example\b/.test(line)) {
      examples += 1;
      blocks.push({ line: i, kind: 'example', n: examples });
    }
  });
  return blocks;
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

module.exports = { findBlocks, definedParts, exampleCount, errorMessage, shorten, findBinary };
