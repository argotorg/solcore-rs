# AFL++ fuzz targets

This directory is an independent Cargo workspace so normal compiler builds do
not depend on `afl` or require an AFL-instrumented Rust toolchain.

The three targets share one raw UTF-8 `main.solc` input and deliberately treat
ordinary diagnostics as successful executions:

- `parser`: parse and lower one source file.
- `frontend`: parse, resolve imports against the embedded standard library, and
  collect type diagnostics.
- `backend`: when frontend diagnostics are error-free, specialize, emit Hull,
  and validate the Hull program.

Build and run one target locally with cargo-afl:

```sh
cargo afl build --release --manifest-path fuzz/Cargo.toml --bin backend
cargo afl fuzz -i fuzz/corpus/backend -o fuzz/findings/backend \
  fuzz/target/release/backend
```

For campaigns on `fuzz-01`, use tofu's `solcore-rs-fuzz` helper.  It reuses the
host's Slurm artifact/metadata handling and gives every target a separate
shared corpus at `/data/fuzz-corpora/solcore-rs/<target>/`.

The initial byte-stream targets intentionally have no local-import bundle
format.  Add imports, incremental edits, and differential outcomes as separate
structured targets; mixing them into the raw target would turn most executions
into missing-file diagnostics and dilute coverage.
