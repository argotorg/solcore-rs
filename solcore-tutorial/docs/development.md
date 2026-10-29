# Developing and distributing the tutorial

These instructions are for contributors building the tutorial from a checkout.
Readers of a distributed copy need only a browser.

## Build and distribute this checkout

Install Rust through rustup, Node.js **22 or newer** with npm, `wasm-pack` **0.15.0**, and mdBook (CI pins **0.4.52**).
Rustup uses the repository's `rust-toolchain.toml` automatically.
From a fresh checkout at the repository root:

```sh
cargo install wasm-pack --version 0.15.0 --locked
cargo install mdbook --version 0.4.52 --locked
cd playground
npm ci
cd ../solcore-tutorial
npm run build
npm run preview
```

The build generates `solcore-tutorial/book/`, containing the book, project sources in `projects/`, and the bundled editor in `playground/`.
Visit [http://127.0.0.1:4177/](http://127.0.0.1:4177/), open a chapter, and click **Try here**.
Keep the preview server running while you read.

Copy the entire `book/` output directory to a static HTTP(S) server to distribute it.
It contains the editor, compiler, language server, pinned solc build, projects, and supporting assets; it does not depend on a separately deployed Playground.
Serve `.wasm` files as `application/wasm`.
Opening `index.html` through a `file://` URL does not support the module workers.
The output also works below a hosting subdirectory when you copy all its files together.

`npm ci` links the local WASM package directories before they are built.
The full tutorial build generates both WASM packages and downloads the pinned solc build if needed.
See the [playground development guide](https://github.com/argotorg/solcore-rs/blob/HEAD/playground/README.md#development) for the shared editor and runtime.

For chapter, example, or frontend changes, run `npm run build:book` from `solcore-tutorial/` to reuse prepared WASM and solc assets.
Run `npm run build` after compiler or language-server changes, and `npm run preview` to serve the result.
Preview serves built files; rebuild and reload after editing.
If port 4177 is already occupied, stop the earlier preview or choose another port with `PORT=4180 npm run preview`.

## Read the book without building WASM

From the repository root:

```sh
mdbook serve solcore-tutorial --open
```

This previews the prose and source listings.
It does not generate project JSON or bundle the executable editor, so project links are disabled.
Use the complete build above for a tutorial you can distribute and run independently.

## Check a source file locally

Run these commands from the repository root:

```sh
cargo run -p solcore-driver --locked -- --std-root std \
  solcore-tutorial/examples/tutorial-basics/Basics.sol

cargo run -p solcore-driver --locked -- --std-root std --emit-yul \
  solcore-tutorial/examples/tutorial-basics/Basics.sol

cargo run -p solcore-fmt --locked -- --check \
  solcore-tutorial/examples/tutorial-basics/Basics.sol
```

The first command checks the source.
The second also emits Yul.
Neither command executes the test comments; use the editor's test action to run them.
The third checks formatting; use `--write` instead of `--check` to format the file in place.

## Troubleshoot the local build

| Symptom | What to check |
| --- | --- |
| The inline editor does not appear | Use the complete build over HTTP and copy both `playground/` and `projects/` with the book. Plain `mdbook serve` only previews the prose. |
| A project cannot load | Check that the complete `projects/` folder is served with the book. Rebuild after changing its catalog or sources. |
| Local WASM changes are not reflected in the tutorial | Run `npm run build` from `solcore-tutorial/` and reload. When developing the shared editor directly, `npm run dev:force` in `playground/` clears Vite's dependency cache. |

For execution and saved-workspace questions, see [Using the editor](../src/using-the-editor.md).
For the teaching approach and integration choices, see [design notes](design.md).
