# Solcore for VS Code

Syntax highlighting and LSP support for Solcore `.sol` files. The extension
starts the language server bundled in the compiler with `solcore lsp`.

A `.sol` file is Solcore only when it starts with `pragma solcore;` (comments may
precede it). Other `.sol` files are switched to the `solidity` language when a
Solidity extension is installed, or to plain text otherwise, so the Solcore
server never runs on Solidity code. Saving a file after adding the pragma
switches it to Solcore.

## Install

Install the [native compiler release](https://github.com/argotorg/solcore-rs#installation) first,
keeping `std/` beside `solcore`. Then download the extension from the same
release, verify it, and install it:

```sh
version=0.1.0-alpha
vsix="solcore-language-${version}.vsix"
url="https://github.com/argotorg/solcore-rs/releases/download/v${version}"

cd "$(mktemp -d)"
curl -fLO "$url/SHA256SUMS"
curl -fLO "$url/$vsix"
awk -v file="$vsix" '$2 == file' SHA256SUMS | shasum -a 256 --check
code --install-extension "$vsix"
```

On Linux, use `sha256sum --check`. Without the `code` command, download the
`.vsix` file from the release page and run **Extensions: Install from VSIX** in
VS Code.

This distribution does not require a Marketplace publisher account. Upgrading
the compiler updates the server independently of the extension; restart VS Code
after replacing the compiler. Install a newer VSIX to update the extension.

For Remote SSH, WSL, or containers, install the compiler on the remote host.
The extension is a workspace extension and resolves paths on that host.

## Language server

Set `solcore.path` to the compiler executable if it is not discovered:

```json
{
  "solcore.path": "/absolute/path/to/solcore"
}
```

Discovery uses `solcore.path`, then the legacy `solcore.lsp.serverPath`, then
`SOLCORE_PATH`, then legacy `SOLCORE_LSP_SERVER`, then `$CARGO_HOME/bin` (or
`~/.cargo/bin`), `~/.local/bin`, and absolute directories on `PATH`.
`solcore.lsp.serverPath` and `SOLCORE_LSP_SERVER` start a standalone server with
no arguments; all compiler paths start with `lsp`. Explicit settings take
precedence even if invalid, so a configuration mistake is reported clearly.
Do not append `lsp` to a path setting.

Changing either path setting restarts the server without reloading the window.
If startup fails, the extension offers to open settings. In untrusted workspaces,
syntax highlighting remains available and the native server starts only after
you trust the workspace.

## Build a release VSIX locally

Node.js 22 or newer is required by the pinned packaging tool:

```sh
npm ci
npm run test:unit
npm run package -- --out /absolute/path/to/solcore-language-0.1.0-alpha.vsix
```

The lockfile pins the packaging tool and language client dependencies. Packaging
includes runtime dependencies and the license, excludes tests and packaging
files, and does not publish anything. The repository release script runs these
steps together with the native and playground builds.

The extension uses the official
[VSIX packaging workflow](https://code.visualstudio.com/api/working-with-extensions/publishing-extension#packaging-extensions).

For development, launch this directory in an Extension Development Host after
`npm ci`. The reusable grammar lives in `syntaxes/solcore.tmLanguage.json`.
