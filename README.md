# solcore-rs

Experimental Core Solidity Compiler
This is a prototype implementation of Solidity's new type system. It is functional and can produce executable EVM code.

> [!WARNING]
> This project is a work in progress and is not ready for production use.

Try it in the [online playground](https://playground.solcore.soliditylang.org/).

Learn with the English [Solcore Tutorial](solcore-tutorial/README.md): a guided
path from your first program to traits, compile-time computation, and a tested
contract. The tutorial owns and tests its source examples.
The book bundles the Playground editor and compiler to run those projects in
the page and can be distributed as a standalone static site.

## Installation

Release archives include the compiler, formatter and matching language server;
Rust is not required to use them. If an archive is not yet available for your
platform, [build from source](#build-and-test).

### macOS and Linux

1. Choose a [release](https://github.com/argotorg/solcore-rs/releases) version
   and the target for your machine. Only targets attached to that release are
   available.

   | Platform | Target |
   | --- | --- |
   | macOS, Apple Silicon | `aarch64-apple-darwin` |
   | macOS, Intel | `x86_64-apple-darwin` |
   | Linux, x86-64 | `x86_64-unknown-linux-gnu` |
   | Linux, ARM64 | `aarch64-unknown-linux-gnu` |

2. Download `SHA256SUMS` and the archive into a new temporary directory, then
   verify the archive. For example, on Apple Silicon macOS:

   ```sh
   version=0.1.0
   target=aarch64-apple-darwin
   release="solcore-v${version}-${target}"
   archive="${release}.tar.gz"
   url="https://github.com/argotorg/solcore-rs/releases/download/v${version}"

   cd "$(mktemp -d)"
   curl -fLO "$url/SHA256SUMS"
   curl -fLO "$url/$archive"
   awk -v file="$archive" '$2 == file' SHA256SUMS | shasum -a 256 --check
   ```

   On Linux, use `sha256sum --check` instead of `shasum -a 256 --check`.
   Continue only if verification reports `OK` for the downloaded archive, and
   run the next step in the same terminal. Use `curl` rather than a web browser:
   macOS quarantines browser downloads, and Gatekeeper blocks the extracted
   executables because they are not yet notarized.

3. Extract the entire archive and add its directory to `PATH`:

   ```sh
   mkdir -p "$HOME/.local/opt/solcore"
   tar -xzf "$archive" -C "$HOME/.local/opt/solcore"
   export SOLCORE_PATH="$HOME/.local/opt/solcore/$release/solcore"
   export PATH="${SOLCORE_PATH%/*}:$PATH"

   solcore --version
   solcore lsp --version
   solcore-fmt --version
   ```

   Keep `std/` beside the executables. To make the settings persist, add the
   `SOLCORE_PATH` and `PATH` exports to `~/.zshrc` or `~/.bashrc`, replacing
   `$release` with the actual extracted directory name. Restart your terminal
   and editor after changing these settings.

Compile a source file with `solcore main.sol --emit-yul`. The native Windows
release is not yet qualified; WSL can use a matching Linux archive.

### Editor support

Install the separate [VS Code, Neovim/Vim, or Emacs integration](editors/README.md)
for syntax highlighting and LSP features. For VS Code, download the release's
`.vsix` file and use **Extensions: Install from VSIX...**. The integrations start
`solcore lsp` automatically; you do not need to run it in a terminal.

`SOLCORE_PATH` selects the installed compiler explicitly. If a GUI editor does
not inherit your shell environment, configure the absolute executable path in
the editor (for example, `solcore.path` in VS Code). Updating the compiler also
updates its LSP; editor packages need updating only when their integration changes.

Maintainers: [local release workflow](docs/releasing.md) covers native archives,
editor packages, GitHub Releases and playground deployment without hosted CI.

## Language and standard library

The Rust compiler and its regression tests define the supported Solcore
language. Sources use the canonical `.sol` syntax, and every source file must
begin with `pragma solcore;` (comments may precede it). The pragma marks the file
as Solcore rather than Solidity, which shares the extension; the compiler rejects
files without it (`SC0002`). The ten library sources
in [`std/`](std/) are maintained with this compiler; see the standard library's
[README](std/README.md) for its maintenance and verification policy.

The compiler reports structured diagnostics and validates ABI support before
backend execution. The 499-source parser corpus is a fixed historical
regression set, as documented in its
[README](crates/parser/tests/fixtures/corpus/README.md).

## Build and test

[Rust](https://www.rust-lang.org/tools/install) is required. The checked-in
`rust-toolchain.toml` selects Rust 1.97.0 automatically when using rustup.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

## Run the compiler

From the repository root, compile an example and print the generated Yul:

```sh
cargo run -p solcore-driver -- tests/e2e/00answer/main.sol --emit-yul
```

Replace the example path with your own `.sol` file. For all available options:

```sh
cargo run -p solcore-driver -- --help
```

## Formatting Solcore

The standalone `solcore-fmt` command formats UTF-8 Solcore source files:

```sh
# Print formatted source without changing the file.
cargo run -p solcore-fmt -- main.sol

# Read stdin and print the result.
cargo run -p solcore-fmt -- < main.sol

# Format one or more files in place.
cargo run -p solcore-fmt -- --write main.sol library.sol

# Check formatting in CI without changing files.
cargo run -p solcore-fmt -- --check main.sol

# Install the standalone command locally.
cargo install --path crates/fmt
```

Use `solcore-fmt --help` for all options. Indentation defaults to four spaces;
`--indent-width N` changes it (1–16 spaces). A `-` input reads stdin. Multiple
inputs are printed in argument order unless `--write` or `--check` is selected.
Inputs are individual files; directories are not searched recursively. `--check` exits
with status 1 when formatting differs; invalid syntax, arguments, or I/O errors
exit with status 2.

## License

Licensed under the [Apache License 2.0](LICENSE).
