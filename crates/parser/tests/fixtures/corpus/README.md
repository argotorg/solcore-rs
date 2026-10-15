# Solcore 2f372bde frontend corpus

This corpus ports every source under `test/examples/` from
[`argotorg/solcore@2f372bde`](https://github.com/argotorg/solcore/tree/2f372bde2801612814015a22319d0bc51486cbf0/test/examples).
The 499 examples keep the snapshot's module layout and semantics while using
the canonical `.sol` syntax. Parseable sources also receive local whitespace-only
formatting with `solcore-fmt`; intentional syntax-error fixtures remain unchanged.
Sources accepted by the reference frontend live under `ok/test/examples/`;
reference failures and timeouts live under `fail/test/examples/`.

The standard-library sources in `ok/std/` track the current `a7b7afb0`
snapshot with local whitespace-only formatting, independently of these historical
examples/verdicts. They are byte-identical to [`std/`](../../../../../std/); see
[`std/README.md`](../../../../../std/README.md) for the synchronization
policy. The `test/imports/` and `known-diagnostic-gaps/` trees are Rust-specific
regressions and are not part of the reference example snapshot.

The current compatibility target and focused latest-upstream regressions are
recorded in [`SEMANTIC_DIFFERENCES.md`](../../../../../SEMANTIC_DIFFERENCES.md).
Do not relabel this historical verdict ledger as a run of the latest compiler.

## Reference verdicts

[`reference-frontend.tsv`](reference-frontend.tsv) records a fresh run of the
2f372bde compiler built from the same checkout. Every source was run
independently with a 60-second limit and these options:

```text
sol-core --file <2f372bde>/test/examples/<path> \
  --root <2f372bde> --include <2f372bde>/std \
  --no-specialise --no-gen-dispatch \
  --type-class-resolution legacy \
  --color never --unicode never --diagnostic-format short
```

The original snapshot contains 337 passes, 160 failures, and two timeouts.
Ledger paths map to the migrated `.sol` files by module stem. `code` is the
first structured `SCnnnn` diagnostic emitted for a failure; `-` means that no
structured code applies. The two timeout rows are
`cases/tabled-cycle-fail.sol` and `cases/tabled-left-recursive-fail.sol`.
These verdicts describe the legacy frontend with specialization and generated
dispatch disabled, not the full compiler or the tabled resolver.

The two Rust allowance manifests make the comparison mode explicit:

- [`rust-accepted-reference-failures.tsv`](rust-accepted-reference-failures.tsv)
  lists reference failures accepted by the corresponding Rust frontend gate.
- [`rust-rejected-reference-passes.tsv`](rust-rejected-reference-passes.tsv)
  lists diagnostics from Rust's full-frontend gate for reference passes. It
  includes deliberate Rust-only negative import fixtures and phase differences
  caused by enabling generated dispatch.

Do not add a local regression to the reference example trees. Put it in a
dedicated Rust test/fixture tree instead. Parser snapshots are retained only
for reference-failed sources that also exercise a Rust parser diagnostic.
