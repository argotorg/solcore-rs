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
   and the target for your machine. The current release, `0.1.0-alpha`, is an
   alpha prerelease. Only targets attached to that release are available.

   | Platform | Target |
   | --- | --- |
   | macOS, Apple Silicon | `aarch64-apple-darwin` |
   | macOS, Intel | `x86_64-apple-darwin` |
   | Linux, x86-64 | `x86_64-unknown-linux-gnu` |
   | Linux, ARM64 | `aarch64-unknown-linux-gnu` |

2. Download `SHA256SUMS` and the archive into a new temporary directory, then
   verify the archive. For example, on Apple Silicon macOS:

   ```sh
   version=0.1.0-alpha
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

Editor integrations add syntax highlighting and LSP features. Install the one
for your editor from the same release:

- [VS Code](editors/vscode-solcore/README.md#install)
- [Neovim and Vim](editors/vim-solcore/README.md#install)
- [Emacs](editors/emacs-solcore/README.md#install)

The integrations start `solcore lsp` automatically; you do not need to run it in
a terminal. [Editor integrations](editors/README.md) explains how they find the
compiler.

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

## Debug information

`--ethdebug` writes [ETHDebug](https://ethdebug.github.io/format/) debug
information next to the bytecode of each contract. It requires `--output-dir`:

```sh
cargo run -p solcore-driver -- tests/ethdebug/storage-fields/main.sol --ethdebug --output-dir out
```

For a contract `C` the directory receives:

- `C.bin` and `C.bin-runtime`: the creation and runtime bytecode as hex.
- `C_ethdebug.json` and `C_ethdebug-runtime.json`: the ETHDebug programs for the
  creation code and the runtime code, with one entry per instruction.
- `ethdebug_compilation.json`: the compiler, the source files with their ids,
  paths and contents, and a compilation id.
- `ethdebug_resources.json`: the compilation, and the type objects and pointer
  templates that the programs refer to by name.

The documents follow ETHDebug format 0.1.0-draft.3 (ethdebug/format commit
`952f1bd`), except for the type objects, which follow the draft type schema of
ethdebug/format pull request #424 (commit `e326f17`).
`compilation.settings.ethdebugTypes` in `ethdebug_compilation.json` names that
revision.

Debug information exists only for bytecode compiled with the Sonatina backend;
the Yul output carries none. `--ethdebug` requires optimization level 0 (the
default of `--opt-level`), because optimization can merge code that has
different source locations. `--bin` writes the bytecode alone, at the level that
`--opt-level 0|1|2|s` selects. With `--ethdebug` only level 0 is accepted.

What the programs describe:

- Source locations. Each instruction carries the range of source text it was
  compiled from, as byte offsets into the contents in `ethdebug_compilation.json`.
  Code that the compiler generated (entry code, storage helpers, derived
  methods, literal helpers) carries a remark `generated: <reason>` and no range.
  The condition of an `if` is located at the condition, and the jumps that leave
  its arms have no location. A `match` statement is not treated this way: its
  scrutinee and the jumps that leave its arms carry the range of the whole
  statement.
- Types. Types are written in the draft type schema: numbers, booleans,
  addresses, bytes, strings, arrays, mappings and functions, and user data types
  (one constructor is a struct, two are a variant). A type that has no
  description, such as a data type with three or more constructors, is left
  out, and a variable of that type is listed with its name only.
- Storage fields. Each runtime instruction lists the contract's storage fields
  with their types, except a field that a local of the same name hides at an
  instruction that lists locals (a jump lists none, so it lists every field). A
  field of type `word`, `uint256`, `bytes32` or `address` has a storage location
  when the slot is certain, which holds while every earlier field takes exactly
  one slot. A mapping from a one-word key to a one-word value has a pointer
  template that locates an entry from a key. Other fields have a type and no
  location.
- Parameters and locals. Each runtime instruction of user and library code lists
  the parameters and local variables in scope, with name, declaration range and
  type. Generated code and the opcodes that jump list none. Bindings that exist
  only at compile time and bindings that hold a function are not listed.
- Stack locations. A parameter or local of one word (`word`, `bool`, `uint8` to
  `uint128`, `uint256`, `address`, `bytes32`) has a pointer
  `{"location": "stack", "slot": n}` after an opcode when the compiler can
  verify, from its stack plan, which word of the stack holds its value after that
  opcode. Slot 0 is the top of the stack. The pointer is for the value the
  variable holds at that point, so a variable that is assigned again has a
  different word afterwards, and possibly a different slot. Variables that hold
  the same value are located at the same word, and so are the values that the
  backend merges because they are equal. The tests check `uint256` and `word`
  locals. A variable has no pointer, and is listed with its type only, in these
  cases:
  - its value is not one word (pairs, data types with payload, strings);
  - the value is a constant, or the backend folded it into one or removed it, or
    nothing from that instruction on reads it;
  - the instruction is in a block that is entered from more than one place, such
    as the head of a loop or the code after an `if`, and nothing before it in
    that block has read or assigned the variable. A variable has the value that
    the last read or assignment in its own block gave it, or, at the start of a
    block with a single predecessor, the value at the end of that predecessor.
    The values that several predecessors give are not joined. This is the cause
    of many of the missing locations inside loops;
  - the value is on the stack more than once, for example while a copy is a
    call's argument;
  - the opcode is a jump, which includes the jump of a call, or its instruction
    ends its block;
  - the stack plan of the instruction cannot be verified: its opcodes are not
    one contiguous run, the plan does not agree with the plan of the next
    instruction, or an opcode has a stack effect that is not known. A call is
    verified only if every word below the address that it pushes for its return
    is the same value at the bottom of the next instruction's plan, so a call
    with no word below that address has no location, at any of its opcodes.

Not described: values of variables that have no stack pointer, variables in
creation code, call structure (which function a call enters or returns from) and
optimized code.

The tests validate the emitted documents against the ethdebug/format schemas.
The schemas are fetched with `git` from the ethdebug/format repository at the
two pinned commits the first time a test needs them, and kept under the target
directory. Without network access the validation is skipped with a message;
`SOLCORE_REQUIRE_ETHDEBUG_SCHEMAS=1` makes missing schemas an error, as in CI.
`SOLCORE_ETHDEBUG_FORMAT_REPO` names another URL or a local path to fetch from.

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
