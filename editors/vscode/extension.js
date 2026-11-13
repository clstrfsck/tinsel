// Tinsel for VS Code: code lenses to run and test a program, tasks whose
// errors land in the Problems panel, evaluating a selection with
// `tinsel eval` (its value shown at the end of the selection), errors on
// save from `tinsel check`, and every test, example and answers block in
// the Test Explorer, run with `tinsel test --json`.
//
// Plain JavaScript, so the extension needs no build step.

'use strict';

const vscode = require('vscode');
const { execFile, spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const lib = require('./lib');

/** How long `tinsel eval` may run before it is stopped. */
const EVAL_TIMEOUT_MS = 60_000;

let output;
let resultDecoration;
let diagnostics;

function activate(context) {
  output = vscode.window.createOutputChannel('Tinsel');
  resultDecoration = vscode.window.createTextEditorDecorationType({
    after: {
      margin: '0 0 0 1em',
      color: new vscode.ThemeColor('editorCodeLens.foreground'),
      fontStyle: 'italic',
    },
  });

  diagnostics = vscode.languages.createDiagnosticCollection('tinsel');
  const selector = { language: 'tinsel' };
  context.subscriptions.push(
    diagnostics,
    vscode.workspace.onDidSaveTextDocument(checkDocument),
    vscode.workspace.onDidOpenTextDocument(checkDocument),
    vscode.workspace.onDidCloseTextDocument((doc) => diagnostics.delete(doc.uri)),
    output,
    resultDecoration,
    vscode.languages.registerCodeLensProvider(selector, new LensProvider()),
    vscode.tasks.registerTaskProvider('tinsel', new TaskProvider()),
    vscode.commands.registerCommand('tinsel.run', (uri) => runTask('run', uri)),
    vscode.commands.registerCommand('tinsel.test', (uri) => runTask('test', uri)),
    vscode.commands.registerCommand('tinsel.runExample', runExample),
    vscode.commands.registerCommand('tinsel.evalSelection', () => evalSelection(false)),
    vscode.commands.registerCommand('tinsel.evalSelectionOnExample', () => evalSelection(true)),
    vscode.commands.registerCommand('tinsel.openReference', openReference),
    vscode.languages.registerHoverProvider(selector, new DocHoverProvider()),
    vscode.languages.registerCompletionItemProvider(selector, new DocCompletionProvider(), '.'),
    vscode.languages.registerSignatureHelpProvider(selector, new DocSignatureProvider(),
      { triggerCharacters: ['(', ','], retriggerCharacters: [')'] }),
    // A different binary may have a different reference.
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('tinsel.path')) {
        docs = null;
      }
    }),
    // An edit makes the values shown out of date.
    vscode.workspace.onDidChangeTextDocument((e) => {
      const editor = vscode.window.activeTextEditor;
      if (editor && e.document === editor.document && e.contentChanges.length > 0) {
        editor.setDecorations(resultDecoration, []);
      }
    }),
  );
  for (const editor of vscode.window.visibleTextEditors) {
    checkDocument(editor.document);
  }
  context.subscriptions.push(...testController());
}

function deactivate() {}

// Errors on save: `tinsel check --json`

/** Files whose diagnostics came from checking each main file. */
const reportedFor = new Map();

async function checkDocument(doc) {
  if (doc.languageId !== 'tinsel' || doc.uri.scheme !== 'file') {
    return;
  }
  if (!vscode.workspace.getConfiguration('tinsel').get('checkOnSave', true)) {
    return;
  }
  const main = doc.uri.fsPath;
  const r = await tinsel(['check', '--json', main], cwdFor(doc));
  const parsed = lib.parseJsonLines(r.stdout)[0];
  if (!parsed) {
    // The binary is missing or too old to have `check`: say nothing here;
    // running or testing reports it.
    return;
  }
  // Clear what this file reported last time (it may have been in an
  // included file), then report the new errors, each in its own file.
  for (const uri of reportedFor.get(main) || []) {
    diagnostics.delete(uri);
  }
  const byFile = new Map();
  for (const e of parsed.errors || []) {
    const file = path.resolve(cwdFor(doc), e.file);
    const { line, col, endLine, endCol } = lib.errorRange(e);
    const d = new vscode.Diagnostic(new vscode.Range(line, col, endLine, endCol), e.message,
      vscode.DiagnosticSeverity.Error);
    d.source = 'tinsel';
    byFile.set(file, [...(byFile.get(file) || []), d]);
  }
  diagnostics.set(doc.uri, byFile.get(main) || []);
  const uris = [doc.uri];
  for (const [file, ds] of byFile) {
    if (file !== main) {
      const uri = vscode.Uri.file(file);
      diagnostics.set(uri, ds);
      uris.push(uri);
    }
  }
  reportedFor.set(main, uris);
}

// The Test Explorer: `tinsel test --json`

/**
 * A test controller listing every `.tin` file in the workspace, grouped
 * by folder, with its tests, examples and answers block as children.
 * Gives the disposables to keep.
 */
function testController() {
  const ctrl = vscode.tests.createTestController('tinsel', 'Tinsel');
  const exclude = '{**/target/**,**/node_modules/**,**/.git/**}';

  /**
   * The item for a file's folder (see `lib.folderGroup`), made on first
   * use; none for a file listed at the top level, such as one outside the
   * workspace.
   */
  const folderItem = (uri) => {
    const ws = vscode.workspace.getWorkspaceFolder(uri);
    if (!ws) {
      return undefined;
    }
    const rel = path.relative(ws.uri.fsPath, path.dirname(uri.fsPath));
    const roots = (vscode.workspace.workspaceFolders || []).length;
    const group = lib.folderGroup(ws.uri.toString(), ws.name, rel, roots);
    if (!group) {
      return undefined;
    }
    let item = ctrl.items.get(group.id);
    if (!item) {
      item = ctrl.createTestItem(group.id, group.label);
      ctrl.items.add(item);
    }
    return item;
  };

  /** The item for a file, made on first use. */
  const fileItem = (uri) => {
    const parent = folderItem(uri);
    const siblings = parent ? parent.children : ctrl.items;
    let item = siblings.get(uri.toString());
    if (!item) {
      item = ctrl.createTestItem(uri.toString(), path.basename(uri.fsPath), uri);
      item.canResolveChildren = true;
      siblings.add(item);
    }
    return item;
  };

  /** Reads a file's checks into its item's children. */
  const resolveFile = (item, text) => {
    if (text === undefined) {
      const open = vscode.workspace.textDocuments.find((d) => d.uri.toString() === item.uri.toString());
      try {
        text = open ? open.getText() : fs.readFileSync(item.uri.fsPath, 'utf8');
      } catch (_) {
        return;
      }
    }
    const children = lib.findChecks(text).map((c) => {
      const child = ctrl.createTestItem(`${item.uri.toString()}#${c.line}`, c.name, item.uri);
      child.range = new vscode.Range(c.line, 0, c.line, 0);
      child.description = c.kind === 'test_error' ? 'test_error' : undefined;
      return child;
    });
    item.children.replace(children);
  };

  const removeFile = (uri) => {
    const parent = folderItem(uri);
    (parent ? parent.children : ctrl.items).delete(uri.toString());
    if (parent && parent.children.size === 0) {
      ctrl.items.delete(parent.id);
    }
  };

  ctrl.resolveHandler = async (item) => {
    if (item) {
      resolveFile(item);
      return;
    }
    const uris = await vscode.workspace.findFiles('**/*.tin', exclude);
    uris.sort((a, b) => a.fsPath.localeCompare(b.fsPath));
    for (const uri of uris) {
      fileItem(uri);
    }
  };

  const watcher = vscode.workspace.createFileSystemWatcher('**/*.tin');
  watcher.onDidCreate((uri) => fileItem(uri));
  watcher.onDidDelete(removeFile);
  watcher.onDidChange((uri) => resolveFile(fileItem(uri)));

  const run = async (request, token) => {
    const testRun = ctrl.createTestRun(request);
    // Which items to report, by file: a folder or file means all its
    // checks; a check means just that one.
    const wanted = new Map();
    const fileItems = new Map();
    const want = (fileIt, checks) => {
      fileItems.set(fileIt.uri.fsPath, fileIt);
      if (fileIt.children.size === 0) {
        resolveFile(fileIt);
      }
      const all = [];
      fileIt.children.forEach((c) => all.push(c));
      const set = wanted.get(fileIt.uri.fsPath) || new Set();
      for (const c of checks || all) {
        if (!(request.exclude || []).includes(c)) {
          set.add(c);
        }
      }
      wanted.set(fileIt.uri.fsPath, set);
    };
    const visit = (item) => {
      if (item.id.startsWith('dir:')) {
        item.children.forEach(visit);
      } else if (item.id.includes('#')) {
        want(item.parent, [item]);
      } else {
        want(item);
      }
    };
    if (request.include) {
      request.include.forEach(visit);
    } else {
      ctrl.items.forEach(visit);
    }
    for (const set of wanted.values()) {
      set.forEach((c) => testRun.enqueued(c));
    }
    const files = [...wanted.keys()].sort();
    if (files.length === 0) {
      testRun.end();
      return;
    }
    // Save open files first: the CLI reads them from disk.
    for (const doc of vscode.workspace.textDocuments) {
      if (doc.isDirty && wanted.has(doc.uri.fsPath)) {
        await doc.save();
      }
    }

    const byLine = new Map();
    for (const [file, set] of wanted) {
      set.forEach((c) => byLine.set(`${file}:${c.range.start.line + 1}`, c));
    }
    const reported = new Set();
    const report = (o) => {
      if (o.kind === 'load') {
        const where = new vscode.Location(vscode.Uri.file(o.file), new vscode.Position(
          Math.max(((o.error && o.error.line) || 1) - 1, 0), 0));
        const checks = [...(wanted.get(o.file) || [])];
        // A file with no checks of its own is marked itself, so that the
        // error still shows.
        const items = checks.length > 0 ? checks : [fileItems.get(o.file)].filter(Boolean);
        for (const c of items) {
          const msg = new vscode.TestMessage(o.message);
          msg.location = where;
          testRun.errored(c, msg);
          reported.add(c);
        }
        return;
      }
      const item = byLine.get(`${o.file}:${o.line}`);
      if (!item) {
        return;
      }
      reported.add(item);
      if (o.outcome === 'pass') {
        testRun.passed(item);
      } else if (o.outcome === 'skip') {
        testRun.skipped(item);
        testRun.appendOutput(`${item.label}: skipped (${o.message})\r\n`, undefined, item);
      } else {
        const msg = new vscode.TestMessage(o.message);
        msg.location = new vscode.Location(item.uri, item.range);
        testRun.failed(item, msg);
      }
    };

    await new Promise((resolve) => {
      const args = ['test', '--json', ...engineArgs(), ...files];
      const cwd = (vscode.workspace.workspaceFolders || [])[0]?.uri.fsPath || path.dirname(files[0]);
      wanted.forEach((set) => set.forEach((c) => testRun.started(c)));
      let child;
      try {
        child = spawn(binary(), args, { cwd });
      } catch (e) {
        testRun.appendOutput(`cannot run tinsel: ${e.message}\r\n`);
        resolve();
        return;
      }
      token.onCancellationRequested(() => child.kill());
      let pending = '';
      child.stdout.on('data', (chunk) => {
        pending += chunk.toString();
        const lines = pending.split('\n');
        pending = lines.pop();
        lib.parseJsonLines(lines.join('\n')).forEach(report);
      });
      child.stderr.on('data', (chunk) => testRun.appendOutput(chunk.toString().replace(/\n/g, '\r\n')));
      child.on('error', (e) => {
        testRun.appendOutput(`cannot run ${binary()}: ${e.message}. ` +
          'Build it with `cargo build --release`, or set `tinsel.path`.\r\n');
        resolve();
      });
      child.on('close', () => {
        lib.parseJsonLines(pending).forEach(report);
        resolve();
      });
    });
    // Anything not reported didn't run (the run was stopped, say).
    wanted.forEach((set) => set.forEach((c) => {
      if (!reported.has(c)) {
        testRun.skipped(c);
      }
    }));
    testRun.end();
  };

  ctrl.createRunProfile('Run', vscode.TestRunProfileKind.Run, run, true);

  // Keep open files' checks up to date as they are edited.
  const onEdit = vscode.workspace.onDidChangeTextDocument((e) => {
    if (e.document.languageId === 'tinsel' && e.document.uri.scheme === 'file') {
      resolveFile(fileItem(e.document.uri), e.document.getText());
    }
  });
  return [ctrl, watcher, onEdit];
}

// Running the CLI

function binary() {
  const setting = vscode.workspace.getConfiguration('tinsel').get('path');
  const roots = (vscode.workspace.workspaceFolders || []).map((f) => f.uri.fsPath);
  return lib.findBinary(setting, roots);
}

/** The engine option from the settings, if not the default. */
function engineArgs() {
  const engine = vscode.workspace.getConfiguration('tinsel').get('engine');
  return engine && engine !== 'native' ? [`--${engine}`] : [];
}

/** Runs the CLI with `args`; resolves to { ok, stdout, stderr }. */
function tinsel(args, cwd) {
  return new Promise((resolve) => {
    execFile(binary(), args, { cwd, timeout: EVAL_TIMEOUT_MS, maxBuffer: 64 << 20 },
      (err, stdout, stderr) => {
        if (err && err.code === 'ENOENT') {
          resolve({
            ok: false,
            stdout: '',
            stderr: `Error: cannot find the tinsel binary (${binary()}). ` +
              'Build it with `cargo build --release`, or set `tinsel.path`.',
          });
          return;
        }
        if (err && err.killed) {
          resolve({ ok: false, stdout, stderr: `Error: stopped after ${EVAL_TIMEOUT_MS / 1000} s` });
          return;
        }
        resolve({ ok: !err, stdout, stderr });
      });
  });
}

/** The Tinsel document to act on: `uri`, or the active editor's. */
async function tinselDocument(uri) {
  let doc;
  if (uri instanceof vscode.Uri) {
    doc = await vscode.workspace.openTextDocument(uri);
  } else {
    doc = vscode.window.activeTextEditor && vscode.window.activeTextEditor.document;
  }
  if (!doc || doc.languageId !== 'tinsel') {
    vscode.window.showWarningMessage('Open a Tinsel (.tin) file first.');
    return undefined;
  }
  // The CLI reads the file from disk.
  if (doc.isDirty) {
    await doc.save();
  }
  return doc;
}

function cwdFor(doc) {
  const folder = vscode.workspace.getWorkspaceFolder(doc.uri);
  return folder ? folder.uri.fsPath : path.dirname(doc.uri.fsPath);
}

// Tasks: `tinsel run` and `tinsel test`, with problem matchers

function makeTask(command, file, scope) {
  const definition = { type: 'tinsel', command, file };
  const name = `${command} ${path.basename(file)}`;
  const execution = new vscode.ProcessExecution(binary(), [command, ...engineArgs(), file]);
  const matchers = ['$tinsel', '$tinsel-checks', '$tinsel-examples'];
  const task = new vscode.Task(definition, scope || vscode.TaskScope.Workspace, name, 'tinsel',
    execution, matchers);
  task.presentationOptions = { reveal: vscode.TaskRevealKind.Always, clear: true };
  return task;
}

class TaskProvider {
  /** `run` and `test` for the file in the active editor. */
  provideTasks() {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.languageId !== 'tinsel') {
      return [];
    }
    const file = editor.document.uri.fsPath;
    const scope = vscode.workspace.getWorkspaceFolder(editor.document.uri);
    return ['run', 'test'].map((c) => makeTask(c, file, scope));
  }

  /** A task written in tasks.json: { "type": "tinsel", "command", "file" }. */
  resolveTask(task) {
    const { command, file } = task.definition;
    if (!command || !file) {
      return undefined;
    }
    return makeTask(command, file, task.scope);
  }
}

async function runTask(command, uri) {
  const doc = await tinselDocument(uri);
  if (doc) {
    const scope = vscode.workspace.getWorkspaceFolder(doc.uri);
    await vscode.tasks.executeTask(makeTask(command, doc.uri.fsPath, scope));
  }
}

// Code lenses

class LensProvider {
  provideCodeLenses(doc) {
    const text = doc.getText();
    const parts = lib.definedParts(text);
    const lenses = [];
    const at = (line) => new vscode.Range(line, 0, line, 0);
    const blocks = lib.findBlocks(text);
    const day = blocks.find((b) => b.kind === 'day');
    // Without a `day` header, the lenses go on the first line.
    const top = day ? day.line : 0;
    if (parts.length > 0) {
      lenses.push(new vscode.CodeLens(at(top), {
        title: '▶ Run', command: 'tinsel.run', arguments: [doc.uri],
        tooltip: 'tinsel run: the examples, then the real input',
      }));
    }
    lenses.push(new vscode.CodeLens(at(top), {
      title: '✓ Test', command: 'tinsel.test', arguments: [doc.uri],
      tooltip: 'tinsel test: every test, example and answers check in this file',
    }));
    if (parts.length > 0) {
      for (const b of blocks.filter((b) => b.kind === 'example')) {
        lenses.push(new vscode.CodeLens(at(b.line), {
          title: `▶ Run example ${b.n}`, command: 'tinsel.runExample', arguments: [doc.uri, b.n],
          tooltip: `${parts.join(' and ')} on this example's input`,
        }));
      }
    }
    return lenses;
  }
}

/** Shows `part1` and `part2` for example `n` in the output panel. */
async function runExample(uri, n) {
  const doc = await tinselDocument(uri);
  if (!doc) {
    return;
  }
  const file = doc.uri.fsPath;
  output.show(true);
  output.appendLine(`${path.basename(file)}, example ${n}:`);
  for (const part of lib.definedParts(doc.getText())) {
    const r = await tinsel(['eval', ...engineArgs(), file, '--example', String(n), part], cwdFor(doc));
    output.appendLine(r.ok ? `  ${part}: ${r.stdout.trim()}` : `  ${part}: ${r.stderr.trim()}`);
  }
}

// Evaluating a selection

/**
 * The expression to evaluate: the selection, or with none, the word under
 * the cursor.
 */
function selectedExpression(editor) {
  const sel = editor.selection;
  if (!sel.isEmpty) {
    return { text: editor.document.getText(sel), range: sel };
  }
  const word = editor.document.getWordRangeAtPosition(sel.active);
  return word ? { text: editor.document.getText(word), range: word } : undefined;
}

async function evalSelection(onExample) {
  const editor = vscode.window.activeTextEditor;
  const doc = await tinselDocument();
  if (!editor || !doc) {
    return;
  }
  const expr = selectedExpression(editor);
  if (!expr || expr.text.trim() === '') {
    vscode.window.showWarningMessage('Select an expression to evaluate.');
    return;
  }
  const args = ['eval', ...engineArgs(), doc.uri.fsPath];
  let where = 'real input';
  if (onExample) {
    const count = lib.exampleCount(doc.getText());
    if (count === 0) {
      vscode.window.showWarningMessage('This program has no examples.');
      return;
    }
    let n = 1;
    if (count > 1) {
      const items = Array.from({ length: count }, (_, i) => `Example ${i + 1}`);
      const pick = await vscode.window.showQuickPick(items, { placeHolder: 'Evaluate on which example?' });
      if (!pick) {
        return;
      }
      n = items.indexOf(pick) + 1;
    }
    args.push('--example', String(n));
    where = `example ${n}`;
  }
  args.push(expr.text);

  const r = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Window, title: 'Tinsel: evaluating…' },
    () => tinsel(args, cwdFor(doc)),
  );
  const shown = r.ok ? `⇒ ${r.stdout.trim()}` : `⇒ error: ${lib.errorMessage(r.stderr)}`;
  editor.setDecorations(resultDecoration, [{
    range: new vscode.Range(expr.range.end, expr.range.end),
    renderOptions: { after: { contentText: lib.shorten(shown, 120) } },
    hoverMessage: new vscode.MarkdownString().appendCodeblock(r.ok ? r.stdout : r.stderr, 'text'),
  }]);
  output.appendLine(`${lib.shorten(expr.text, 80)}  (${where})`);
  output.appendLine(r.ok ? `  ${r.stdout.trim()}` : r.stderr.trim().replace(/^/gm, '  '));
}

// The library reference: `tinsel doc --json`

/** How long to wait before asking a binary without `doc` again. */
const DOCS_RETRY_MS = 30_000;

/** The reference, once read: { entries, index, at }. */
let docs = null;

/**
 * The library reference from the binary, read once. If it can't be read
 * (no build yet, or one from before `tinsel doc`), the reference is empty
 * and is asked for again a little later.
 */
async function libraryDocs() {
  if (docs && (docs.entries.length > 0 || Date.now() - docs.at < DOCS_RETRY_MS)) {
    return docs.promise;
  }
  const loading = { entries: [], at: Date.now(), promise: null };
  loading.promise = tinsel(['doc', '--json']).then((r) => {
    const entries = lib.parseJsonLines(r.stdout).filter((e) => Array.isArray(e.names));
    if (entries.length === 0) {
      output.appendLine(`Library reference unavailable: ${lib.errorMessage(r.stderr)}`);
    }
    loading.entries = entries;
    return { entries, index: lib.docIndex(entries) };
  });
  docs = loading;
  return loading.promise;
}

/**
 * The built-in entries for `name` at `offset` in `doc`, or none: not in a
 * string or comment, and not hidden by the program's own definition.
 */
async function entriesAt(doc, name, offset) {
  const text = doc.getText();
  if (!lib.inCode(text.slice(0, offset)) || lib.definesName(text, name)) {
    return undefined;
  }
  const { index } = await libraryDocs();
  return index.get(name);
}

function docMarkdown(entries) {
  return new vscode.MarkdownString(entries.map(lib.entryMarkdown).join('\n\n---\n\n'));
}

const NAME = /[A-Za-z_][A-Za-z0-9_]*/;

/** Hovering over a built-in shows its entries. */
class DocHoverProvider {
  async provideHover(doc, position) {
    const range = doc.getWordRangeAtPosition(position, NAME);
    if (!range) {
      return undefined;
    }
    const entries = await entriesAt(doc, doc.getText(range), doc.offsetAt(range.start));
    return entries ? new vscode.Hover(docMarkdown(entries), range) : undefined;
  }
}

/**
 * Completes the names of built-ins, with their entries as documentation.
 * After a `.`, constants such as `N` are left out.
 */
class DocCompletionProvider {
  async provideCompletionItems(doc, position) {
    const offset = doc.offsetAt(position);
    const text = doc.getText();
    const before = text.slice(0, offset);
    if (!lib.inCode(before)) {
      return undefined;
    }
    const method = /\.[A-Za-z_0-9]*$/.test(before) && !/\d\.$/.test(before);
    const range = doc.getWordRangeAtPosition(position, NAME);
    const { index } = await libraryDocs();
    const items = [];
    for (const [name, entries] of index) {
      const constant = entries.every(lib.isConstant);
      if ((method && constant) || lib.definesName(text, name)) {
        continue;
      }
      const kind = constant ? vscode.CompletionItemKind.Constant
        : method ? vscode.CompletionItemKind.Method : vscode.CompletionItemKind.Function;
      const item = new vscode.CompletionItem(name, kind);
      item.detail = entries.map((e) => e.signature).join('  |  ');
      item.documentation = docMarkdown(entries);
      if (range) {
        item.range = range;
      }
      items.push(item);
    }
    return items;
  }
}

/** While typing a call to a built-in, shows its parameters. */
class DocSignatureProvider {
  async provideSignatureHelp(doc, position) {
    const before = doc.getText(new vscode.Range(new vscode.Position(0, 0), position));
    const call = lib.callContext(before);
    if (!call) {
      return undefined;
    }
    const entries = await entriesAt(doc, call.name, before.length);
    if (!entries) {
      return undefined;
    }
    const sigs = lib.signatures(entries, call.method);
    if (sigs.length === 0) {
      return undefined;
    }
    const help = new vscode.SignatureHelp();
    help.signatures = sigs.map((s) => {
      const info = new vscode.SignatureInformation(s.label,
        new vscode.MarkdownString(s.entry.description));
      info.parameters = s.params.map((p) => new vscode.ParameterInformation([p.start, p.end]));
      return info;
    });
    help.activeSignature = lib.activeSignature(sigs, call.arg);
    const params = sigs[help.activeSignature].params.length;
    help.activeParameter = Math.min(call.arg, Math.max(params - 1, 0));
    return help;
  }
}

/** "Tinsel: Open Library Reference": the whole reference, previewed. */
async function openReference() {
  const { entries } = await libraryDocs();
  if (entries.length === 0) {
    vscode.window.showErrorMessage('Tinsel: the library reference is unavailable. ' +
      'Build the interpreter with `cargo build --release`, or set `tinsel.path`.');
    return;
  }
  const doc = await vscode.workspace.openTextDocument({
    content: lib.referenceMarkdown(entries),
    language: 'markdown',
  });
  try {
    await vscode.commands.executeCommand('markdown.showPreview', doc.uri);
  } catch (_) {
    await vscode.window.showTextDocument(doc);
  }
}

module.exports = { activate, deactivate };
