# Solcore editor grammar

This directory contains a VS Code extension for Solcore `.solc` files. It ships
the reusable TextMate grammar used by the playground and starts the native
`solcore-lsp` stdio server when a Solcore file opens.

The package is shaped like a small VS Code extension:

- `syntaxes/solcore.tmLanguage.json` provides TextMate scopes.
- `language-configuration.json` provides comments, brackets, auto-close pairs,
  indentation, folding markers, and the Solcore word pattern.
- `extension.js` starts `solcore-lsp` through `vscode-languageclient`.
- `package.json` wires the `.solc` extension to the grammar, configuration, and
  language client.

## Language server

Build or install the native server first:

```sh
cargo install --path ../../crates/lsp --features native --locked
```

The extension resolves the server command in this order:

1. `solcore.lsp.serverPath` VS Code setting.
2. `SOLCORE_LSP_SERVER` environment variable.
3. `solcore-lsp` on `PATH`.

## Development

Run `npm install` in this directory before launching an Extension Development
Host. Other editors that consume TextMate grammars can use
`syntaxes/solcore.tmLanguage.json` directly.
