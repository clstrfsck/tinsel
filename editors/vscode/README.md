# Tinsel for VS Code

Syntax highlighting for Tinsel (`.tin` files): comments, strings with
`{...}` interpolation and string-pattern captures such as `{n:int}`,
triple-quoted strings, chars, numbers, keywords, top-level items (`day`,
`example`, `answers`, `test`, `test_error`, `with`, `include`), function definitions,
`@memo`/`@nomemo`, typed patterns (`n: int`) and operators. It also sets up
`#` comments, bracket matching, auto-closing quotes, and indentation after
`where`, `match` and `=`.

## Installing

The same extension works in VS Code and in Cursor. For both, see
"Syntax highlighting in VS Code and Cursor" in the
[top-level README](../../README.md). In short, from the repository root:

```bash
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/local.tinsel-0.1.0   # VS Code
ln -s "$PWD/editors/vscode" ~/.cursor/extensions/local.tinsel-0.1.0   # Cursor
```

Then run "Developer: Reload Window". Or package a `.vsix` with
`npx @vscode/vsce package --allow-missing-repository --skip-license`, and
install it with `code --install-extension` or `cursor --install-extension`.

## Changing the grammar

The grammar is `syntaxes/tinsel.tmLanguage.json`. TextMate rules are tried
in order, and a wrong rule doesn't fail: it just highlights wrongly, often for
every line after it (an unclosed string, for example). After a change, check
it against real `.tin` files, for example with the `vscode-textmate` and
`vscode-oniguruma` npm packages, which are the tokenizer VS Code itself uses.
