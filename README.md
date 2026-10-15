# solcore-rs

`solcore-rs` is a Rust implementation of [Solcore](https://github.com/argotorg/solcore).

> [!WARNING]
> This project is a work in progress and is not ready for production use.

Try it in the [online playground](https://playground.solcore.soliditylang.org/).

## Language and compatibility targets

The source-language and standard-library target is the Haskell implementation
at [`argotorg/solcore@a7b7afb0`](https://github.com/argotorg/solcore/tree/a7b7afb0638e50895008d778a167afd0e50a2c86).
Both implementations now use the canonical `.sol`
[source syntax](https://github.com/argotorg/solcore/blob/a7b7afb0638e50895008d778a167afd0e50a2c86/doc/src/sail/syntax.md).
The eight library sources in `std/` come from that revision, with local
whitespace-only formatting applied by `solcore-fmt`.

See [`SEMANTIC_DIFFERENCES.md`](SEMANTIC_DIFFERENCES.md) for intentional
corrections to upstream defects, ABI limitations, and validation scope. Rust
retains structured diagnostics and safety checks; compatibility does not imply
identical compiler output or production readiness. The 499-source parser
corpus remains a historical `2f372bde` regression set, as documented in its
[README](crates/parser/tests/fixtures/corpus/README.md).

## Build and test

[Rust](https://www.rust-lang.org/tools/install) is required. The checked-in
`rust-toolchain.toml` selects Rust 1.97.0 automatically when using rustup.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
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

Formatting reuses the existing lexer and parser through
`solcore_parser::format::format_source`. It changes whitespace while retaining
the original token, literal, and comment text, and verifies that the token and
comment sequence is unchanged. Syntax errors prevent formatting. The initial
style preserves explicit continuation line breaks and existing CRLF line
endings; automatic line wrapping and a configurable maximum line width are not
implemented yet.

The native and browser language servers use the same formatter for document
formatting, respecting the editor's indentation and final-newline preferences.
Documents with syntax errors are left unchanged in the editor.

The existing CI tests also require Solcore test fixtures, E2E sources, fuzz
seeds, playground examples, and the standard library to be formatted. Only
unparseable fixtures in the designated syntax-error test directories are exempt.
Run this check locally with `cargo test -p solcore-parser --test formatting --locked`.

Before `--write` replaces any file, every input is read and formatted, and all
changed results are staged in the destination directories. Individual files
are replaced atomically with their permissions preserved. Symlinks and
read-only files are rejected. A later filesystem failure during replacement
can still leave earlier files formatted; this is not a multi-file transaction.

## License

Licensed under the [Apache License 2.0](LICENSE).
