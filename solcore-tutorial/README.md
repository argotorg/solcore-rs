# Solcore Tutorial

An English introduction to Solcore, with examples you can edit and run in the page.
Start with [Welcome](src/introduction.md).

## Build and preview

Complete the [first-time setup](docs/development.md#build-and-distribute-this-checkout), then run from the repository root:

```sh
cd solcore-tutorial
npm run build
npm run preview
```

Open [http://127.0.0.1:4177/](http://127.0.0.1:4177/).

For chapter, example, or editor changes, use `npm run build:book` to reuse prepared runtime assets.
Use `npm run build` after compiler or language-server changes.
Preview serves the built files; rebuild and reload after editing.

## Distribute

Copy the entire generated `book/` directory to a static HTTP(S) server.
It includes the editor, compiler, and example projects; no separate Playground deployment is needed.
Serve `.wasm` files as `application/wasm`.

## Edit and check

Chapters live in `src/`.
The tutorial owns its sources in `examples/`, listed in [catalog.json](examples/catalog.json).
Include those files in chapters instead of copying code.

After building, run from this directory:

```sh
npm test
npm run test:wasm
npm run test:bundle
```

See [development guide](docs/development.md) for setup details and [design notes](docs/design.md) for the teaching approach.

[Apache-2.0 license](../LICENSE).
