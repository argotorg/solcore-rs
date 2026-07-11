# Compiler wall-time benchmark

`scripts/bench-compile.sh` measures cold compiler sessions through the native
`solcore-driver` process. It builds the driver once, then uses `hyperfine` with
shell execution disabled. Every timed invocation creates a fresh compiler
database; filesystem pages and the executable are warmed by the configured
warmup runs.

The fixed cases cover distinct compiler workloads:

| Case | Fixture | Purpose |
| --- | --- | --- |
| `std-free` | `SingleFun.solc` | Small frontend run without reachable std/runtime |
| `dispatch-small` | `tests/e2e/022add` | Small contract with compiler-owned dispatch |
| `erc20-large` | `tests/e2e/128minierc20` | Larger std- and storage-heavy contract |
| `multi-file` | `tests/e2e/ltimp` | Main module plus a local import |

Each case is measured at five end-to-end boundaries:

- `frontend`: parse, resolve, diagnostics, and type checking
- `hull`: frontend, specialization, Hull emission/checking, and Hull rendering
- `yul`: frontend and the shared Hull pipeline followed by Yul rendering
- `sonatina`: frontend and the shared Hull pipeline followed by Sonatina IR rendering
- `all`: the shared pipeline followed by all three textual outputs

The backend stages intentionally write their artifacts to a temporary directory
so terminal rendering is outside the measurement. The driver currently does not
expose specialization as a standalone CLI boundary; `hull` is the first fixed
measurement that includes it.

Install `hyperfine`, quiet other CPU-heavy work, and run from anywhere inside
the checkout:

```sh
./scripts/bench-compile.sh
```

Defaults are two warmups and ten measured runs per command. Environment
variables make focused development runs possible without changing the suite:

```sh
BENCH_RUNS=30 BENCH_WARMUP=5 \
BENCH_EXPORT_DIR=target/bench-results \
./scripts/bench-compile.sh

BENCH_STAGE=frontend BENCH_CASE=dispatch-small \
./scripts/bench-compile.sh
```

`BENCH_STAGE` accepts `frontend`, `hull`, `yul`, `sonatina`, or `all`;
`BENCH_CASE` accepts one of the four case names above. Set
`BENCH_SKIP_BUILD=1` to reuse an already-built binary. `BENCH_PROFILE` defaults
to the native speed-oriented `release` profile. If Cargo is configured with a
target triple or a nonstandard output layout, point `SOLCORE_DRIVER` at the
binary explicitly.

For comparable before/after results, record `rustc --version`, the git commit,
machine/power state, and the JSON files produced through `BENCH_EXPORT_DIR`.
