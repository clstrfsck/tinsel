// Tinsel for VS Code: code lenses to run and test a program, tasks whose
// errors land in the Problems panel, and evaluating a selection with
// `tinsel eval`, its value shown at the end of the selection.
//
// Plain JavaScript, so the extension needs no build step.

'use strict';

const vscode = require('vscode');
const { execFile } = require('child_process');
const path = require('path');
const lib = require('./lib');

/** How long `tinsel eval` may run before it is stopped. */
const EVAL_TIMEOUT_MS = 60_000;

let output;
let resultDecoration;

function activate(context) {
  output = vscode.window.createOutputChannel('Tinsel');
  resultDecoration = vscode.window.createTextEditorDecorationType({
    after: {
      margin: '0 0 0 1em',
      color: new vscode.ThemeColor('editorCodeLens.foreground'),
      fontStyle: 'italic',
    },
  });

  const selector = { language: 'tinsel' };
  context.subscriptions.push(
    output,
    resultDecoration,
    vscode.languages.registerCodeLensProvider(selector, new LensProvider()),
    vscode.tasks.registerTaskProvider('tinsel', new TaskProvider()),
    vscode.commands.registerCommand('tinsel.run', (uri) => runTask('run', uri)),
    vscode.commands.registerCommand('tinsel.test', (uri) => runTask('test', uri)),
    vscode.commands.registerCommand('tinsel.runExample', runExample),
    vscode.commands.registerCommand('tinsel.evalSelection', () => evalSelection(false)),
    vscode.commands.registerCommand('tinsel.evalSelectionOnExample', () => evalSelection(true)),
    // An edit makes the values shown out of date.
    vscode.workspace.onDidChangeTextDocument((e) => {
      const editor = vscode.window.activeTextEditor;
      if (editor && e.document === editor.document && e.contentChanges.length > 0) {
        editor.setDecorations(resultDecoration, []);
      }
    }),
  );
}

function deactivate() {}

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

module.exports = { activate, deactivate };
