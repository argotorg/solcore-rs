# Teaching design and references

This tutorial uses the current repository's language implementation as its baseline and owns the projects used by its curriculum.
Other language tutorials inform the sequence of explanation, execution, and practice.
Their syntax and language semantics are not specifications for Solcore.

## Reference tutorials

| Reference | Useful pattern | Application here |
| --- | --- | --- |
| [The Rust Programming Language](https://doc.rust-lang.org/book/) | A book progressing from basics toward types and abstraction | Chapter sequence, prerequisites, and learning objectives |
| [Rust by Example](https://doc.rust-lang.org/rust-by-example/) and [Hello World](https://doc.rust-lang.org/rust-by-example/hello.html) | Modifiable executable examples followed by activities | Tutorial `.sol` projects, small changes, and expected results |
| [A Tour of Go](https://go.dev/tour/welcome/1) and its [official introduction](https://go.dev/blog/tour) | Playground-backed editing, compilation, execution, and exercises | A browser-first path before installing a local toolchain |
| [Svelte Tutorial](https://svelte.dev/tutorial/svelte/welcome-to-svelte) | Explicit prerequisites and sequence, exercises, and solutions | Concrete steps and collapsible hints and solutions |

References consulted on October 22, 2026.
The prose and exercises were written for the current Solcore examples.

## Why use the existing Playground?

The Playground already supports compilation, browser-side EVM execution, expected-value tests, and manual calls that retain state.
Readers can also inspect multiple files, diagnostics, and ABI output.
It lets them check a claim immediately using the same compiler and execution tools as the development Playground, using tutorial-owned project sources.

| Integration | Benefit | Decision |
| --- | --- | --- |
| Links to a project, file, and view | Gives the IDE a full window and leaves the explanation available | Retained as a fallback and for additional examples |
| Always-loaded iframe | Explanation and editor appear together immediately | Avoided to keep reading lightweight and limit active compilers |
| Bundled iframe loaded on request | Keeps practice in the page and uses the exact compiler and examples shipped with the text | Primary learning path through **Try here** |
| Dedicated tutorial editor | Could add exercise-specific feedback and progress tracking | Deferred; the shared Playground already provides editing, diagnostics, tests, and calls |

The complete distribution includes the Playground under the book's own `playground/` directory, with project JSON generated from the curriculum catalog into `projects/`.
Each iframe has its own page and worker lifecycle, while the editor source remains shared with the ordinary Playground.
This keeps the tutorial independent of a public Playground deployment without maintaining a second compiler integration.
Closing an editor releases its runtime, and opening another exercise on the page closes the previous one.
Full-tab links remain available for more room and advanced controls.

Same-origin iframes share browser storage.
Tutorial sessions therefore use separate workspace keys for each project, isolated from the ordinary Playground.
The inline and full-tab versions of a project share saved source.
Opening the full-tab view closes its inline editor to reduce the chance of two editors overwriting the same saved work.

## Reproducible practice

Links identify bundled projects.
They do not share edited source, manual arguments, or contract state.
Each project preserves its edits independently, including when moving between chapters.
Chapters using the same project share a workspace.
**Reset project** restores that project's original files.
Readers should export important code before clearing browser storage, and keep one editor open for a given project at a time.

Tests and manual calls are explained separately.
Tests start from a new deployment for each contract and replay preceding sends.
Manual calls retain their session state.
Understanding that these states are independent helps explain why a manual call may differ from a test result.

## Keeping the tutorial aligned

Complete listings use mdBook includes to read the tutorial's `.sol` files under `examples/`.
The same files become editable projects through `examples/catalog.json`; complete source is not duplicated in chapter prose.
When a curriculum project changes, its expected results and instructions must change too.
Tutorial checks validate the catalog, source includes, project links, and generated distribution, while compiler and browser-runtime tests check the curriculum sources and their expected values.

The ordinary Playground maintains its own examples independently.
It accepts a project URL through a general loading interface; it has no tutorial catalog or book-generation step.
The tutorial owns its build commands and uses the shared editor as one dependency of its output.
A plain mdBook preview only includes the prose and listings.
The complete build includes matching projects and runtime, ready for distribution without a separate Playground deployment.
