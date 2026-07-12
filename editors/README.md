# Solcore editor integrations

These packages share the current Solcore language implementation:

- `crates/lsp` provides the native stdio language server binary.
- `editors/vscode-solcore/syntaxes/solcore.tmLanguage.json` provides the
  canonical TextMate grammar used by the playground and VS Code.

## Language server binary

Install a local `solcore-lsp` binary:

```sh
cargo install --path crates/lsp --features native --locked
```

Or build it in-place:

```sh
cargo build -p solcore-lsp --features native --bin solcore-lsp
```

Every editor integration can start the same stdio server. By default they use
`SOLCORE_LSP_SERVER` when it is set and non-empty, then fall back to
`solcore-lsp` on `PATH`. VS Code and Neovim also expose editor-specific command
overrides for local development.

On initialization, the native server indexes `.solc` files below the workspace
root. Real editor file URIs are mapped into the compiler VFS, and watched file
changes keep unopened import targets and workspace symbols up to date.

The server currently supports diagnostics, scope and qualified-member
completion, signature/type hover with source documentation, go-to-definition
for values/types/predicates/constructors/import modules, references, document
highlights, rename with prepare-rename, signature help, document/workspace
symbols, semantic tokens, and inlay hints.

## Packages

- `vscode-solcore`: VS Code extension with TextMate highlighting and LSP client.
- `vim-solcore`: Vim/Neovim package with `.solc` highlighting and LSP setup.
- `emacs-solcore`: Emacs major mode plus eglot/lsp-mode setup.
