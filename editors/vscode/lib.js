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

// The library reference, from `tinsel doc --json` (SPEC 2.7)

/** The reference's entries indexed by name; a name can have several. */
function docIndex(entries) {
  const index = new Map();
  for (const entry of entries) {
    for (const name of entry.names) {
      if (!index.has(name)) {
        index.set(name, []);
      }
      index.get(name).push(entry);
    }
  }
  return index;
}

/** An entry as Markdown, for a hover or a completion's documentation. */
function entryMarkdown(entry) {
  return [
    '```tinsel', entry.signature, '```', '', entry.description, '',
    '```tinsel', ...entry.examples, '```',
  ].join('\n');
}

/** The whole reference as Markdown, grouped by section. */
function referenceMarkdown(entries) {
  const out = ['# Tinsel library reference', ''];
  let section = null;
  for (const entry of entries) {
    if (entry.section !== section) {
      section = entry.section;
      out.push(`## ${section}`, '');
    }
    out.push(`### ${entry.signature}`, '', entry.description, '',
      '```tinsel', ...entry.examples, '```', '');
  }
  return out.join('\n');
}

/** `text` split on `, ` outside brackets. */
function splitTop(text) {
  const parts = [];
  let depth = 0;
  let current = '';
  for (const c of text) {
    if ('([{'.includes(c)) {
      depth += 1;
    } else if (')]}'.includes(c)) {
      depth -= 1;
    }
    if (c === ',' && depth === 0) {
      parts.push(current.trim());
      current = '';
    } else {
      current += c;
    }
  }
  if (current.trim() !== '') {
    parts.push(current.trim());
  }
  return parts;
}

/**
 * The forms in an entry's signature, such as `s.split(sep)`: each with its
 * receiver (`s`, or null), name, parameters (null for no brackets) and
 * result.
 */
function signatureForms(signature) {
  const [forms, result] = signature.split(' → ');
  return splitTop(forms).map((form) => {
    const m = /^(?:([A-Za-z_]\w*)\.)?([A-Za-z_]\w*)(?:\((.*)\))?$/.exec(form);
    if (!m) {
      return null;
    }
    return {
      receiver: m[1] || null,
      name: m[2],
      params: m[3] === undefined ? null : splitTop(m[3]),
      result: result || '',
    };
  }).filter((f) => f !== null);
}

/** Whether an entry is a constant, such as `N` or `DIRS4`. */
function isConstant(entry) {
  return signatureForms(entry.signature).every((f) => f.receiver === null && f.params === null);
}

/**
 * The signatures to show while typing a call to one of `entries`: in
 * method form (`xs.split(sep)`) when `method`, otherwise as a function
 * call (`split(s, sep)`). Each has its label, the [start, end) of each
 * parameter in the label, and its entry.
 */
function signatures(entries, method) {
  const out = [];
  const seen = new Set();
  for (const entry of entries) {
    for (const form of signatureForms(entry.signature)) {
      if (form.receiver === null && form.params === null) {
        continue; // A constant, which isn't called.
      }
      const all = form.receiver === null ? form.params : [form.receiver, ...(form.params || [])];
      let label;
      let params;
      if (method) {
        // `xs.count` has no brackets in method form, so it isn't what
        // `xs.count(` is typing.
        if (form.params === null || all.length === 0 || all[0] === '...') {
          continue;
        }
        label = `${all[0]}.${form.name}(`;
        params = all.slice(1);
      } else {
        label = `${form.name}(`;
        params = all;
      }
      const ranges = params.map((p, i) => {
        if (i > 0) {
          label += ', ';
        }
        const start = label.length;
        label += p;
        return { label: p, start, end: label.length };
      });
      label += ')';
      if (form.result) {
        label += ` → ${form.result}`;
      }
      if (!seen.has(label)) {
        seen.add(label);
        out.push({ label, params: ranges, entry });
      }
    }
  }
  return out;
}

/**
 * Which of `sigs` fits a call whose current argument is number `arg`
 * (from 0): the first with enough parameters, or ending in `...`.
 */
function activeSignature(sigs, arg) {
  const i = sigs.findIndex((s) => s.params.length > arg ||
    (s.params.length > 0 && s.params[s.params.length - 1].label === '...'));
  return i < 0 ? 0 : i;
}

/**
 * Reads `text` as Tinsel, skipping strings, chars and comments. Gives the
 * brackets still open at its end, each with its position and how many
 * commas it has so far, and whether the end is inside a string or comment.
 */
function scan(text) {
  const stack = [];
  const triple = '"'.repeat(3);
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === '#') {
      const nl = text.indexOf('\n', i);
      if (nl < 0) {
        return { stack, inside: 'comment' };
      }
      i = nl + 1;
    } else if (text.startsWith(triple, i)) {
      const end = text.indexOf(triple, i + 3);
      if (end < 0) {
        return { stack, inside: 'string' };
      }
      i = end + 3;
    } else if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"' && text[j] !== '\n') {
        j += text[j] === '\\' ? 2 : 1;
      }
      if (j >= text.length || text[j] === '\n') {
        return { stack, inside: 'string' };
      }
      i = j + 1;
    } else if (c === "'" && /^'(\\.|[^'\\])'/.test(text.slice(i, i + 4))) {
      i += text[i + 1] === '\\' ? 4 : 3;
    } else {
      if ('([{'.includes(c)) {
        stack.push({ ch: c, pos: i, commas: 0 });
      } else if (')]}'.includes(c)) {
        stack.pop();
      } else if (c === ',' && stack.length > 0) {
        stack[stack.length - 1].commas += 1;
      }
      i += 1;
    }
  }
  return { stack, inside: null };
}

/** Whether the end of `text` is in code, not in a string or comment. */
function inCode(text) {
  return scan(text).inside === null;
}

/**
 * The call being typed at the end of `text`: the name before the
 * innermost open `(`, whether it is in method form (`xs.name(`), and
 * which argument, from 0, the end is in. Null if not in a named call.
 */
function callContext(text) {
  const { stack, inside } = scan(text);
  if (inside !== null) {
    return null;
  }
  for (let k = stack.length - 1; k >= 0; k -= 1) {
    const frame = stack[k];
    if (frame.ch !== '(') {
      continue;
    }
    const m = /([A-Za-z_]\w*)$/.exec(text.slice(0, frame.pos));
    if (!m) {
      return null;
    }
    const before = frame.pos - m[1].length - 1;
    return { name: m[1], method: before >= 0 && text[before] === '.', arg: frame.commas };
  }
  return null;
}

/**
 * Whether the program defines `name` itself, which hides a built-in of
 * that name (SPEC 9.7): a line starting `name =`, or `name(...)` followed
 * by `=`, as in `f(x) = ...` or `f(x) when x > 0 = ...`.
 */
function definesName(text, name) {
  const start = new RegExp(`^\\s*${name}\\s*([(=])`);
  for (const line of text.split('\n')) {
    const m = start.exec(line);
    if (!m) {
      continue;
    }
    const at = m.index + m[0].length - 1;
    if (m[1] === '=') {
      if (!/^=[=>]/.test(line.slice(at))) {
        return true;
      }
      continue;
    }
    let depth = 0;
    let close = -1;
    for (let j = at; j < line.length; j += 1) {
      if (line[j] === '(') {
        depth += 1;
      } else if (line[j] === ')') {
        depth -= 1;
        if (depth === 0) {
          close = j;
          break;
        }
      }
    }
    if (close >= 0 && /(^|[^=<>!.])=(?![=>])/.test(line.slice(close + 1))) {
      return true;
    }
  }
  return false;
}

// Definitions and uses, from `tinsel symbols --json` (SPEC 2.7)

/**
 * The output of `tinsel symbols --json`, indexed: definitions by id, and
 * for each, its uses. Paths are made absolute against `cwd`.
 */
function symbolIndex(objects, cwd) {
  const defs = new Map();
  const uses = new Map();
  const at = (o) => ({
    file: path.resolve(cwd || '', o.file),
    line: o.line, column: o.column, endLine: o.end_line, endColumn: o.end_column,
  });
  for (const o of objects) {
    if (o.kind === 'def') {
      defs.set(o.id, { id: o.id, name: o.name, what: o.what, container: o.container, ...at(o) });
      uses.set(o.id, []);
    }
  }
  for (const o of objects) {
    if (o.kind === 'ref' && uses.has(o.def)) {
      uses.get(o.def).push(at(o));
    }
  }
  return { defs, uses };
}

/** Whether 1-based (line, column) is within a located name. */
function covers(loc, file, line, column) {
  return loc.file === file && loc.line === line &&
    column >= loc.column && column <= loc.endColumn;
}

/**
 * The definition named at 1-based (line, column) of `file`: by a use of
 * it, or by the definition itself. Undefined for anything else, such as a
 * library function.
 */
function symbolAt(index, file, line, column) {
  for (const [id, list] of index.uses) {
    if (list.some((u) => covers(u, file, line, column))) {
      return index.defs.get(id);
    }
  }
  for (const d of index.defs.values()) {
    if (covers(d, file, line, column)) {
      return d;
    }
  }
  return undefined;
}

/** A definition and every use of it, as locations. */
function occurrences(index, def) {
  return [def, ...(index.uses.get(def.id) || [])];
}

/** The kinds of definition shown in an outline. */
const OUTLINED = new Set(['function', 'constant', 'where function', 'where constant']);

/**
 * The outline of `file`: its functions and constants in source order, each
 * with the `where` definitions inside it as children (parameters and
 * variables left out).
 */
function outline(index, file) {
  const shown = [...index.defs.values()].filter((d) => d.file === file && OUTLINED.has(d.what));
  const ids = new Set(shown.map((d) => d.id));
  // The nearest container that is shown, if any.
  const parent = (d) => {
    let c = d.container;
    while (c !== null && c !== undefined && !ids.has(c)) {
      c = index.defs.get(c) ? index.defs.get(c).container : null;
    }
    return c === undefined ? null : c;
  };
  const byPosition = (a, b) => a.line - b.line || a.column - b.column;
  const node = (d) => ({
    def: d,
    children: shown.filter((c) => parent(c) === d.id).sort(byPosition).map(node),
  });
  return shown.filter((d) => parent(d) === null).sort(byPosition).map(node);
}

module.exports = {
  symbolIndex, symbolAt, occurrences, outline,
  findBlocks, findChecks, definedParts, exampleCount, errorMessage, shorten, findBinary,
  parseJsonLines, errorRange, folderGroup,
  docIndex, entryMarkdown, referenceMarkdown, signatureForms, isConstant, signatures,
  activeSignature, inCode, callContext, definesName,
};
