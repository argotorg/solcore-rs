# Solcore editor integrations

The compiler release contains the native language server: `solcore lsp` speaks
LSP over stdio. Editor packages contain highlighting and client integration;
they do not bundle or download another copy of the compiler. Updating the
installed compiler updates its language server at the same time. Restart the
editor's language server after replacing a compiler release. Editor packages
only need updating for client features, grammar changes, or protocol changes.

## Install the compiler

Download the archive for your operating system and architecture from the
project's GitHub Releases, verify its SHA-256 checksum, and extract the **whole
archive**. Keep the `std/` directory beside `solcore` and `solcore-fmt`. Add the
extracted directory to `PATH`, or configure the absolute path to `solcore`.
The compiler does not run a background updater.

From a source checkout, the native compiler can also be built with:

```sh
cargo build --release --locked -p solcore-driver --bin solcore
```

Point your editor at `target/release/solcore` explicitly for development.
The compiler CLI loads its adjacent `std/`; the language server embeds the
matching standard library at build time. Preserve the complete release layout
so compiler commands and editor analysis use the same version.

## Executable discovery

All integrations prefer explicit editor configuration. Otherwise they use:

1. `SOLCORE_PATH`: compiler executable, launched with the argument `lsp`.
2. `SOLCORE_LSP_SERVER`: legacy standalone LSP executable, launched without arguments.
3. `$CARGO_HOME/bin/solcore`, or `~/.cargo/bin/solcore` when `CARGO_HOME` is unset.
4. `~/.local/bin/solcore`.
5. `solcore` in absolute directories on `PATH` (Emacs uses `exec-path`).

Windows searches for `solcore.exe`. Empty or relative path entries are ignored
in automatic discovery. Repository build directories are not probed. Explicit
configuration is authoritative: a broken override reports an error rather than
silently switching compiler versions. Environment variables contain executable
paths, not shell command lines; paths with spaces need no additional quoting in
the variable value.

The standalone `solcore-lsp` binary remains supported for existing installations:

```sh
cargo install --path crates/lsp --features native --locked
export SOLCORE_LSP_SERVER=/absolute/path/to/solcore-lsp
```

## Editor packages

| Editor | Distribution | Setup |
| --- | --- | --- |
| VS Code | Platform-independent `.vsix` in GitHub Releases | Install from VSIX; use `solcore.path` for an explicit compiler path. |
| Neovim | `vim-solcore` release archive | Install as a runtime package, then `require("solcore").setup()`. |
| Vim | Same `vim-solcore` archive | Syntax support works directly; use `solcore#lsp_command()` with a separate LSP client. |
| Emacs | `emacs-solcore` release archive | Add it to `load-path`; enable Eglot or lsp-mode. |

See each package's README for installation commands. The release scripts build
VSIX locally and archive Vim/Neovim and Emacs packages. Marketplace, Open VSX,
MELPA, and plugin-manager registry publication are separate distribution
channels and are not required for the first release. With VS Code Remote SSH,
WSL, or containers, install the compiler in the remote extension host environment.

The TextMate grammar in `vscode-solcore/syntaxes/solcore.tmLanguage.json` is
also used by the playground. The playground embeds its own WASM compiler and
LSP from the same release source; it does not search for a native executable.

## Solcore and Solidity files

Solcore and Solidity both use the `.sol` extension. A `.sol` file is Solcore
when its first declaration is `pragma solcore;` (comments may precede it); the
compiler requires this pragma in every source file. The editor integrations open
only such files as Solcore and leave other `.sol` files to the editor's Solidity
support, so the Solcore language server never runs on Solidity code.

## Language features

On initialization, the native server indexes the `.sol` files that declare
`pragma solcore;` below every workspace folder. Each root has an isolated
compiler namespace, and dynamic workspace-folder and watched-file changes keep
unopened import targets and workspace symbols up to date.

The top-level directory names `__solcore_workspace__` and
`__solcore_detached__` are reserved for the server's virtual filesystem and
should not be used inside a Solcore workspace.

The server supports diagnostics, scope and qualified-member completion,
signature/type hover with source documentation, go-to-definition for
values/types/predicates/constructors/import modules, references, document
highlights, rename with prepare-rename, signature help, document/workspace
symbols, diagnostic quick fixes, whole-document formatting, folding ranges,
smart selection ranges, semantic tokens, and inlay hints.
