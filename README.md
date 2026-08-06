# solcore-rs

`solcore-rs` is a Rust implementation of [Solcore](https://github.com/argotorg/solcore).

> [!WARNING]
> This project is a work in progress and is not ready for production use.

Try it in the [online playground](https://solcore-rs-preview.solcore-rs-team.workers.dev/).

## Build and test

[Rust](https://www.rust-lang.org/tools/install) is required. The checked-in
`rust-toolchain.toml` selects Rust 1.97.0 automatically when using rustup.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

## License

Licensed under the [Apache License 2.0](LICENSE).
